//! Sunshine-specific local management executor. Does not run inside Sunshine.
pub mod adapter;
pub mod engine;
#[cfg(target_os = "linux")]
pub mod journal;
#[cfg(target_os = "windows")]
#[path = "windows_journal.rs"]
pub mod journal;
pub mod provisioning;
pub mod storage;
pub mod transport;
