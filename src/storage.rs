//! Local provisioning storage, never an Client command surface.
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::path::Path;

#[derive(Debug, Clone, Copy, thiserror::Error)]
pub enum StorageError {
    #[error("protected Client state failed ownership or integrity validation")]
    Unsafe,
    #[error("permission denied accessing protected Client state")]
    PermissionDenied,
    #[error("protected Client state is in use")]
    Busy,
    #[error("state publication committed but durability was not confirmed")]
    Published,
}
pub(crate) fn storage_error(error: impl std::fmt::Debug + std::any::Any) -> StorageError {
    let error = &error as &dyn std::any::Any;
    if let Some(error) = error.downcast_ref::<StorageError>() {
        return *error;
    }
    if let Some(error) = error.downcast_ref::<std::io::Error>() {
        return match error.kind() {
            std::io::ErrorKind::PermissionDenied => StorageError::PermissionDenied,
            std::io::ErrorKind::WouldBlock => StorageError::Busy,
            _ if matches!(error.raw_os_error(), Some(32 | 33)) && cfg!(windows) => {
                StorageError::Busy
            }
            _ => StorageError::Unsafe,
        };
    }
    #[cfg(unix)]
    if let Some(error) = error.downcast_ref::<sarmg_client_fs_safety::Error>() {
        return match error {
            sarmg_client_fs_safety::Error::AlreadyLocked(_) => StorageError::Busy,
            sarmg_client_fs_safety::Error::Io(io)
                if io.kind() == std::io::ErrorKind::PermissionDenied =>
            {
                StorageError::PermissionDenied
            }
            sarmg_client_fs_safety::Error::PublishedDurabilityUnknown(_) => StorageError::Published,
            _ => StorageError::Unsafe,
        };
    }
    StorageError::Unsafe
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn prepare_root(path: &Path) -> Result<sarmg_client_fs_safety::PrivateDirectory, StorageError> {
    sarmg_client_fs_safety::PrivateDirectory::create_for_administration(path).map_err(storage_error)
}
#[cfg(target_os = "windows")]
pub use platform::prepare_root;

#[cfg(target_os = "windows")]
#[path = "windows_storage.rs"]
mod platform;
#[cfg(target_os = "windows")]
pub use platform::ProtectedState;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub struct ProtectedState {
    directory: sarmg_client_fs_safety::PrivateDirectory,
    _lock: Option<sarmg_client_fs_safety::AdvisoryLock>,
}
#[cfg(any(target_os = "linux", target_os = "macos"))]
impl ProtectedState {
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        use sarmg_client_fs_safety::{AdvisoryLock, EntryName, PrivateDirectory};
        let directory = PrivateDirectory::create_for_administration(path).map_err(storage_error)?;
        let lock = AdvisoryLock::acquire(
            &directory,
            &EntryName::new("provisioning.lock")
                .map_err(storage_error)?
                .as_relative(),
        )
        .map_err(storage_error)?;
        Ok(Self {
            directory,
            _lock: Some(lock),
        })
    }
    /// Strictly read-only: does not create the directory, locks or records.
    pub fn open_readonly(path: &Path) -> Result<Self, StorageError> {
        let directory = sarmg_client_fs_safety::PrivateDirectory::open_for_administration(path)
            .map_err(storage_error)?;
        Ok(Self {
            directory,
            _lock: None,
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
        if self._lock.is_none() {
            return Err(StorageError::Unsafe);
        }
        use sarmg_client_fs_safety::{AtomicFile, EntryName};
        if bytes.len() > 2 * 1024 * 1024 {
            return Err(StorageError::Unsafe);
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
        let parent = PrivateDirectory::open_existing(path.parent().ok_or(StorageError::Unsafe)?)
            .map_err(storage_error)?;
        let name =
            EntryName::new(path.file_name().ok_or(StorageError::Unsafe)?).map_err(storage_error)?;
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
                Err(StorageError::Unsafe)
            };
        }
        crate::provisioning::validate_bootstrap(&bytes).map_err(storage_error)?;
        self.put("bootstrap.json", &bytes)
    }
}

/// Outer maintenance gate is always acquired before provisioning/journal locks.
/// An exclusive runtime lease also excludes all writers and competing runs.
#[cfg(unix)]
pub struct MaintenanceGuard {
    _directory: sarmg_client_fs_safety::PrivateDirectory,
    _lock: sarmg_client_fs_safety::AdvisoryLock,
}
#[cfg(unix)]
impl MaintenanceGuard {
    pub fn acquire(path: &std::path::Path) -> Result<Self, StorageError> {
        use sarmg_client_fs_safety::{AdvisoryLock, EntryName};
        let directory = prepare_root(path)?;
        let lock = AdvisoryLock::acquire(
            &directory,
            &EntryName::new("maintenance.lock")
                .map_err(storage_error)?
                .as_relative(),
        )
        .map_err(storage_error)?;
        Ok(Self {
            _directory: directory,
            _lock: lock,
        })
    }
}
#[cfg(windows)]
pub struct MaintenanceGuard {
    _store: ProtectedState,
}
#[cfg(windows)]
impl MaintenanceGuard {
    pub fn acquire(path: &std::path::Path) -> Result<Self, StorageError> {
        let _root = prepare_root(path)?;
        Ok(Self {
            _store: ProtectedState::open(&path.join("maintenance"))?,
        })
    }
}

#[cfg(unix)]
pub fn read_input(path: &std::path::Path) -> Result<zeroize::Zeroizing<Vec<u8>>, StorageError> {
    use sarmg_client_fs_safety::{ConfigurationDirectory, EntryName, InputVisibility};
    let dir = ConfigurationDirectory::open(path.parent().ok_or(StorageError::Unsafe)?)
        .map_err(storage_error)?;
    dir.read_input_bounded(
        &EntryName::new(path.file_name().ok_or(StorageError::Unsafe)?).map_err(storage_error)?,
        65536,
        InputVisibility::Confidential,
    )
    .map(zeroize::Zeroizing::new)
    .map_err(storage_error)
}
#[cfg(windows)]
pub use platform::read_input;
