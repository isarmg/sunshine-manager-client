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

fn report_failure(message: &str) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::EventLog::{
        DeregisterEventSource, EVENTLOG_ERROR_TYPE, RegisterEventSourceW, ReportEventW,
    };
    let wide = |text: &str| -> Vec<u16> {
        std::ffi::OsStr::new(text)
            .encode_wide()
            .chain(Some(0))
            .collect()
    };
    let name = wide(NAME);
    let message_units = wide(message);
    // Event Log is independent of configuration, SQLite stores and the runtime
    // status pipe, so failures can be recorded before protected state opens.
    unsafe {
        let source = RegisterEventSourceW(std::ptr::null(), name.as_ptr());
        if !source.is_null() {
            let strings = [message_units.as_ptr()];
            let _ = ReportEventW(
                source,
                EVENTLOG_ERROR_TYPE,
                0,
                1000,
                std::ptr::null_mut(),
                1,
                0,
                strings.as_ptr(),
                std::ptr::null(),
            );
            let _ = DeregisterEventSource(source);
        }
    }
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "{message}");
}

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
        ProvisionError::Startup { source, .. } => service_failure(source),
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
    // Panic payloads can contain arbitrary data. Persist only the source
    // location, then preserve Rust's normal console panic diagnostics.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        let location = panic.location().map(|location| {
            format!(
                "{}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            )
        });
        report_failure(&format!(
            "sunshine-client service panic: code=1099; location={}",
            location.as_deref().unwrap_or("unknown")
        ));
        previous(panic);
    }));
    service_dispatcher::start(NAME, entry).inspect_err(|error| {
        report_failure(&format!(
            "sunshine-client service dispatcher failed: {error}"
        ));
    })
}
fn service_main(_: Vec<OsString>) {
    if let Err(error) = run() {
        report_failure(&format!(
            "sunshine-client service bootstrap failed: {error}"
        ));
    }
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
                    let failure = service_failure(&error);
                    report_failure(&format!(
                        "sunshine-client service stopped: code={}; {error}",
                        failure as u32
                    ));
                    failure
                }),
            Err(error) => {
                report_failure(&format!(
                    "sunshine-client service runtime initialization failed: code=1099; {error}"
                ));
                Err(ServiceFailure::RuntimeFailure)
            }
        }
    } else {
        report_failure("sunshine-client service arguments invalid: code=1002");
        Err(ServiceFailure::Configuration)
    };
    let exit_code = match result {
        Ok(()) => ServiceExitCode::Win32(0),
        Err(failure) => ServiceExitCode::ServiceSpecific(failure as u32),
    };
    handle.set_service_status(status(ServiceState::Stopped, exit_code))?;
    Ok(())
}
