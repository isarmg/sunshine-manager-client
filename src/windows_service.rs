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
const NAME: &str = "SunshineClient";

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

fn service_failure(error: &sunshine_client::provisioning::ProvisionError) -> ServiceFailure {
    use sunshine_client::provisioning::ProvisionError;
    match error {
        ProvisionError::Storage(_)
        | ProvisionError::StateDocumentCorrupt { .. }
        | ProvisionError::StateSchemaUnsupported { .. }
        | ProvisionError::Journal => ServiceFailure::ProtectedState,
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
        | ProvisionError::Transport(sunshine_client::transport::TransportError::Revoked) => {
            ServiceFailure::ManagerCredentialRejected
        }
        ProvisionError::Unavailable
        | ProvisionError::RateLimited
        | ProvisionError::Transport(sunshine_client::transport::TransportError::Disconnected) => {
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
    handle.set_service_status(status(ServiceState::Running, ServiceExitCode::Win32(0)))?;
    let args: Vec<_> = std::env::args_os().collect();
    let result = if args.len() == 4 && args[2] == "--state" {
        match tokio::runtime::Runtime::new() {
            Ok(runtime) => runtime
                .block_on(sunshine_client::provisioning::run(
                    &PathBuf::from(&args[3]),
                    rx,
                ))
                .map_err(|error| {
                    eprintln!("sunshine-client service stopped: {error}");
                    service_failure(&error)
                }),
            Err(error) => {
                eprintln!("sunshine-client service runtime initialization failed: {error}");
                Err(ServiceFailure::RuntimeFailure)
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
