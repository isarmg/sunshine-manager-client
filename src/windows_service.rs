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
    let status = |state, code| ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: if state == ServiceState::Running {
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
        } else {
            ServiceControlAccept::empty()
        },
        exit_code: ServiceExitCode::Win32(code),
        checkpoint: 0,
        wait_hint: Duration::ZERO,
        process_id: None,
    };
    handle.set_service_status(status(ServiceState::Running, 0))?;
    let args: Vec<_> = std::env::args_os().collect();
    let result = if args.len() == 4 && args[2] == "--state" {
        tokio::runtime::Runtime::new()
            .ok()
            .map(|runtime| {
                runtime
                    .block_on(sunshine_client::provisioning::run(
                        &PathBuf::from(&args[3]),
                        rx,
                    ))
                    .is_ok()
            })
            .unwrap_or(false)
    } else {
        false
    };
    handle.set_service_status(status(ServiceState::Stopped, if result { 0 } else { 1 }))?;
    Ok(())
}
