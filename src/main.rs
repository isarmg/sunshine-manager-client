use std::process::ExitCode;
#[cfg(windows)]
mod windows_service;
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(windows)]
    if args
        .first()
        .is_some_and(|s| s == "--windows-service" || s == "service")
        && args.get(1).is_some_and(|s| s == "--state")
    {
        return windows_service::dispatch().map_or(ExitCode::FAILURE, |_| ExitCode::SUCCESS);
    }
    ExitCode::from(sunshine_client::cli::entry(args))
}
