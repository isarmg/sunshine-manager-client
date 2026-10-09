use std::{ffi::OsString, path::PathBuf, time::Duration};
use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
};
const NAME: &str = "Xscc";

#[repr(u32)]
#[derive(Clone, Copy)]
enum ServiceFailure {
    ProtectedState = 1001,
    Configuration = 1002,
    LocalSunshineUnavailable = 1003,
    LocalSunshineCredentialsRejected = 1004,
    UnsupportedSunshineVersion = 1005,
    ManagerCredentialRejected = 1006,
    ManagerUnavailable = 1007,
    RuntimeFailure = 1099,
}

fn service_failure(error: &xscc::provisioning::ProvisionError) -> ServiceFailure {
    use xscc::provisioning::ProvisionError;
    match error {
        ProvisionError::Storage(_)
        | ProvisionError::StateDocumentCorrupt { .. }
        | ProvisionError::Journal => ServiceFailure::ProtectedState,
        ProvisionError::EntropyUnavailable => ServiceFailure::RuntimeFailure,
        ProvisionError::Configuration
        | ProvisionError::InvalidAuthorizationCode
        | ProvisionError::Unpaired => ServiceFailure::Configuration,
        ProvisionError::SunshineApiUnavailable => ServiceFailure::LocalSunshineUnavailable,
        ProvisionError::SunshineCredentialsRejected => {
            ServiceFailure::LocalSunshineCredentialsRejected
        }
        ProvisionError::SunshineVersionUnsupported { .. } => {
            ServiceFailure::UnsupportedSunshineVersion
        }
        ProvisionError::Rejected
        | ProvisionError::Transport(xscc::transport::TransportError::Revoked) => {
            ServiceFailure::ManagerCredentialRejected
        }
        ProvisionError::Unavailable
        | ProvisionError::RateLimited
        | ProvisionError::Transport(xscc::transport::TransportError::Disconnected) => {
            ServiceFailure::ManagerUnavailable
        }
        _ => ServiceFailure::RuntimeFailure,
    }
}
define_windows_service!(entry, service_main);
pub fn dispatch() -> windows_service::Result<()> {
    service_dispatcher::start(NAME, entry)
}
fn service_main(_: Vec<OsString>) {
    let _ = run();
}
fn run() -> windows_service::Result<()> {
    let (tx, rx) = tokio::sync::watch::channel(false);
    let handle = service_control_handler::register(NAME, move |event| match event {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = tx.send(true);
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;
    let status = |state, exit_code| ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: if state == ServiceState::Running {
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
        } else {
            ServiceControlAccept::empty()
        },
        exit_code,
        checkpoint: 0,
        wait_hint: Duration::ZERO,
        process_id: None,
    };
    handle.set_service_status(status(
        ServiceState::StartPending,
        ServiceExitCode::Win32(0),
    ))?;
    let args: Vec<_> = std::env::args_os().collect();
    let result = if args.len() == 4 && args[2] == "--state" {
        let state_path = PathBuf::from(&args[3]);
        let protected = xscc::storage::prepare_root(&state_path);
        let prepared = protected
            .as_ref()
            .map_err(|_| ServiceFailure::ProtectedState)
            .and_then(|_| {
                xcss_log::RotatingLogFile::create_private(
                    state_path.join("logs"),
                    "xscc",
                    xcss_log::LogRetention::default(),
                )
                .and_then(xcss_log::install_rotating_file)
                .map_err(|_| ServiceFailure::RuntimeFailure)
            });
        match prepared.and_then(|_| {
            tokio::runtime::Runtime::new().map_err(|_| ServiceFailure::RuntimeFailure)
        }) {
            Ok(runtime) => runtime
                .block_on(async {
                    xcss_log::LogRecord::server(
                        "xscc",
                        "windows-service",
                        "xscc.windows.started",
                        "Windows service runtime started.",
                        xcss_log::Level::Info,
                    )
                    .and_then(|record| record.emit())
                    .map_err(|_| {
                        xscc::provisioning::ProvisionError::Transport(
                            xscc::transport::TransportError::Diagnostics,
                        )
                    })?;
                    handle
                        .set_service_status(status(
                            ServiceState::Running,
                            ServiceExitCode::Win32(0),
                        ))
                        .map_err(|_| {
                            xscc::provisioning::ProvisionError::Transport(
                                xscc::transport::TransportError::Diagnostics,
                            )
                        })?;
                    xscc::provisioning::run(&state_path, rx).await
                })
                .map_err(|error| {
                    let failure = service_failure(&error);
                    if service_diagnostic("xscc.windows.runtime_failed", failure).is_err() {
                        ServiceFailure::RuntimeFailure
                    } else {
                        failure
                    }
                }),
            Err(failure) => {
                let _ = service_diagnostic(
                    "xscc.windows.initialization_failed",
                    ServiceFailure::RuntimeFailure,
                );
                Err(failure)
            }
        }
    } else {
        Err(ServiceFailure::Configuration)
    };
    let exit_code = match result {
        Ok(()) => ServiceExitCode::Win32(0),
        Err(failure) => ServiceExitCode::ServiceSpecific(failure as u32),
    };
    handle.set_service_status(status(ServiceState::Stopped, exit_code))?;
    Ok(())
}

fn service_diagnostic(event: &str, failure: ServiceFailure) -> Result<(), xcss_log::LogError> {
    let code = match failure {
        ServiceFailure::ProtectedState => "state_invalid",
        ServiceFailure::Configuration => "configuration_invalid",
        ServiceFailure::LocalSunshineUnavailable => "sunshine_unavailable",
        ServiceFailure::LocalSunshineCredentialsRejected => "sunshine_credential_rejected",
        ServiceFailure::UnsupportedSunshineVersion => "sunshine_version_unsupported",
        ServiceFailure::ManagerCredentialRejected => "manager_credential_rejected",
        ServiceFailure::ManagerUnavailable => "manager_unavailable",
        ServiceFailure::RuntimeFailure => "windows_runtime_failed",
    };
    xcss_log::LogRecord::server(
        "xscc",
        "windows-service",
        event,
        "Windows service runtime failed.",
        xcss_log::Level::Error,
    )?
    .with_error_code(code)?
    .with_attribute("service_exit_code", failure as u32)?
    .emit()
}
