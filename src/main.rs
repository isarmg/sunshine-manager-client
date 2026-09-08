use std::{path::PathBuf, process::ExitCode};
use sunshine_client::{provisioning, storage};
#[cfg(target_os = "windows")]
mod windows_service;

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "--version" {
        println!(
            "sunshine-client {} (git {}; {})",
            env!("CARGO_PKG_VERSION"),
            env!("SUNSHINE_CLIENT_BUILD_SHA"),
            sunshine_client_protocol::PROTOCOL
        );
        return ExitCode::SUCCESS;
    }
    #[cfg(target_os = "windows")]
    if args.first().is_some_and(|a| a == "service") {
        return windows_service::dispatch().map_or(ExitCode::FAILURE, |()| ExitCode::SUCCESS);
    }
    let result = execute(&args);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::from(78)
        }
    }
}
fn execute(args: &[std::ffi::OsString]) -> Result<(), String> {
    if args.len() < 3 || args[1] != "--state" {
        return Err("Usage: sunshine-client init --state ABSOLUTE_PATH --bootstrap PROTECTED_FILE | pair --state ABSOLUTE_PATH | run --state ABSOLUTE_PATH | --version".into());
    }
    let path = PathBuf::from(&args[2]);
    if !path.is_absolute() {
        return Err("Client state path must be absolute".into());
    }
    if args[0] == "init" && args.len() == 5 && args[3] == "--bootstrap" {
        let _root = storage::prepare_root(&path).map_err(|e| e.to_string())?;
        let store =
            storage::ProtectedState::open(&path.join("provisioning")).map_err(|e| e.to_string())?;
        store
            .import_bootstrap(&PathBuf::from(&args[4]))
            .map_err(|e| e.to_string())?;
        println!(
            "Protected bootstrap imported; registration occurs when Client starts. Remove the original bootstrap after successful registration."
        );
        return Ok(());
    }
    if args[0] == "pair" && args.len() == 3 {
        return tokio::runtime::Runtime::new()
            .map_err(|_| "Client runtime unavailable")?
            .block_on(provisioning::pair(&path))
            .map_err(|e| e.to_string());
    }
    if args[0] != "run" || args.len() != 3 {
        return Err("Invalid Client command".into());
    }
    tokio::runtime::Runtime::new()
        .map_err(|_| "Client runtime unavailable")?
        .block_on(async {
            let (tx, rx) = tokio::sync::watch::channel(false);
            let stop = async move {
                #[cfg(unix)]
                {
                    let mut term =
                        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                            .map_err(|_| "Client signal handler unavailable")?;
                    tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}
                }
                #[cfg(windows)]
                tokio::signal::ctrl_c()
                    .await
                    .map_err(|_| "Client signal handler unavailable")?;
                let _ = tx.send(true);
                Ok::<(), String>(())
            };
            tokio::select! {
                result=provisioning::run(&path,rx)=>result.map_err(|e|e.to_string()),
                result=stop=>result,
            }
        })
}
