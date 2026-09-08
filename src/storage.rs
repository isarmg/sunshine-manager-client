//! Local provisioning storage, never an Client command surface.
#[cfg(target_os = "linux")]
use std::path::Path;

#[derive(Debug, thiserror::Error)]
#[error("protected Client state unavailable; check local ownership, permissions and integrity")]
pub struct StorageError;
pub(crate) fn storage_error(_: impl std::fmt::Debug) -> StorageError {
    StorageError
}

#[cfg(target_os = "linux")]
pub fn prepare_root(path: &Path) -> Result<sarmg_client_fs_safety::PrivateDirectory, StorageError> {
    sarmg_client_fs_safety::PrivateDirectory::create(path).map_err(storage_error)
}
#[cfg(target_os = "windows")]
pub use platform::prepare_root;

#[cfg(target_os = "windows")]
#[path = "windows_storage.rs"]
mod platform;
#[cfg(target_os = "windows")]
pub use platform::ProtectedState;

#[cfg(target_os = "linux")]
pub struct ProtectedState {
    directory: sarmg_client_fs_safety::PrivateDirectory,
    _lock: sarmg_client_fs_safety::AdvisoryLock,
}
#[cfg(target_os = "linux")]
impl ProtectedState {
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        use sarmg_client_fs_safety::{AdvisoryLock, EntryName, PrivateDirectory};
        let directory = PrivateDirectory::create(path).map_err(storage_error)?;
        let lock = AdvisoryLock::acquire(
            &directory,
            &EntryName::new("provisioning.lock")
                .map_err(storage_error)?
                .as_relative(),
        )
        .map_err(storage_error)?;
        Ok(Self {
            directory,
            _lock: lock,
        })
    }
    pub fn read(&self, name: &str) -> Result<Option<Vec<u8>>, StorageError> {
        use sarmg_client_fs_safety::{EntryName, InventoryLimits};
        let name = EntryName::new(name).map_err(storage_error)?;
        let files = self
            .directory
            .files(InventoryLimits {
                max_entries: 32,
                max_total_bytes: 16 * 1024 * 1024,
            })
            .map_err(storage_error)?;
        if !files.iter().any(|f| f.name == name) {
            return Ok(None);
        }
        self.directory
            .read_private_bounded(&name, 2 * 1024 * 1024)
            .map(Some)
            .map_err(storage_error)
    }
    pub fn put(&self, name: &str, bytes: &[u8]) -> Result<(), StorageError> {
        use sarmg_client_fs_safety::{AtomicFile, EntryName};
        if bytes.len() > 2 * 1024 * 1024 {
            return Err(StorageError);
        }
        let entry = EntryName::new(name).map_err(storage_error)?;
        if self.read(name)?.is_some() {
            AtomicFile::replace(&self.directory, &entry.as_relative(), bytes).map_err(storage_error)
        } else {
            AtomicFile::create(&self.directory, &entry, bytes).map_err(storage_error)
        }
    }
    pub fn import_bootstrap(&self, path: &Path) -> Result<(), StorageError> {
        use sarmg_client_fs_safety::{EntryName, PrivateDirectory};
        let parent = PrivateDirectory::open_existing(path.parent().ok_or(StorageError)?)
            .map_err(storage_error)?;
        let name = EntryName::new(path.file_name().ok_or(StorageError)?).map_err(storage_error)?;
        let bytes = zeroize::Zeroizing::new(
            parent
                .read_private_bounded(&name, 2 * 1024 * 1024)
                .map_err(storage_error)?,
        );
        if let Some(identity) = self.read("identity.json")? {
            return if crate::provisioning::pending_retry(&bytes, &zeroize::Zeroizing::new(identity))
            {
                Ok(())
            } else {
                Err(StorageError)
            };
        }
        crate::provisioning::validate_bootstrap(&bytes).map_err(storage_error)?;
        self.put("bootstrap.json", &bytes)
    }
}
