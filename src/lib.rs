//! Sunshine-specific local management executor. Does not run inside Sunshine.
pub mod adapter;
pub mod engine;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod journal;
#[cfg(target_os = "windows")]
#[path = "windows_journal.rs"]
pub mod journal;
pub mod provisioning;
pub mod storage;
pub mod transport;

pub mod cli;
mod cli_common;

pub mod runtime_status;
