//! Windows facts use the shared private directory policy and held-file guard.
//! SQLx closes its worker before any database or ancestor pin is released.
use super::sqlite::Facts;
use super::{StorageError, storage_error};
use std::path::Path;
use xcsc::fs_safety::{
    AdvisoryLock, AtomicFile, EntryName, InventoryLimits, PrivateDirectory, PrivateFileAccess,
    WindowsPrivateAccess, WindowsPrivateFile,
};

const DATABASE: &str = "state.sqlite3";
const MAX_DATABASE_BYTES: u64 = 256 * 1024 * 1024;

pub struct ProtectedState {
    // Field order matters: Facts::drop explicitly closes SQLx before its held file.
    facts: Facts,
    _database: WindowsPrivateFile,
    _lock: Option<AdvisoryLock>,
    _directory: PrivateDirectory,
}

fn service_access() -> Result<WindowsPrivateAccess, StorageError> {
    // The product's existing SCM definition runs as LocalSystem. Only SYSTEM
    // and Administrators may access these provisioning/outbox credentials.
    WindowsPrivateAccess::for_service_account("S-1-5-18").map_err(storage_error)
}

pub fn prepare_root(path: &Path) -> Result<PrivateDirectory, StorageError> {
    PrivateDirectory::create_with_windows_access(path, service_access()?).map_err(storage_error)
}

fn open_root(path: &Path) -> Result<PrivateDirectory, StorageError> {
    PrivateDirectory::open_with_windows_access(path, service_access()?).map_err(storage_error)
}

fn validate_files(directory: &PrivateDirectory) -> Result<bool, StorageError> {
    let files = directory
        .files(InventoryLimits {
            max_entries: 5,
            max_total_bytes: MAX_DATABASE_BYTES,
        })
        .map_err(storage_error)?;
    let database = EntryName::new(DATABASE).map_err(storage_error)?;
    let mut found_database = false;
    for file in files {
        let name = file.name.as_os_str();
        if ![
            "state.lock",
            DATABASE,
            "state.sqlite3-journal",
            "state.sqlite3-wal",
            "state.sqlite3-shm",
        ]
        .iter()
        .any(|allowed| name == *allowed)
        {
            return Err(StorageError::Unsafe);
        }
        found_database |= file.name == database;
        // Inventory verifies every file's ACL, type and single link. SQLite's
        // DELETE journal must remain deletable, so only its main file is held
        // for the full connection lifetime below. The protected parent denies
        // untrusted creation or replacement of inspected sidecars.
    }
    Ok(found_database)
}

impl ProtectedState {
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        let directory = prepare_root(path)?;
        let lock = AdvisoryLock::acquire(
            &directory,
            &EntryName::new("state.lock")
                .map_err(storage_error)?
                .as_relative(),
        )
        .map_err(storage_error)?;
        if !validate_files(&directory)? {
            AtomicFile::create(
                &directory,
                &EntryName::new(DATABASE).map_err(storage_error)?,
                &[],
            )
            .map_err(storage_error)?;
        }
        let database = directory
            .open_private_file(
                &EntryName::new(DATABASE).map_err(storage_error)?,
                PrivateFileAccess::ReadWrite,
            )
            .map_err(storage_error)?;
        let facts = Facts::open(&directory.path().join(DATABASE), true)?;
        database.verify().map_err(storage_error)?;
        Ok(Self {
            facts,
            _database: database,
            _lock: Some(lock),
            _directory: directory,
        })
    }

    /// Diagnostics never create files, repair ACLs, acquire a writer lock or
    /// configure mutable SQLite pragmas.
    pub fn open_readonly(path: &Path) -> Result<Self, StorageError> {
        let directory = open_root(path)?;
        if !validate_files(&directory)? {
            return Err(StorageError::Unsafe);
        }
        let database = directory
            .open_private_file(
                &EntryName::new(DATABASE).map_err(storage_error)?,
                PrivateFileAccess::ReadOnly,
            )
            .map_err(storage_error)?;
        let facts = Facts::open(&directory.path().join(DATABASE), false)?;
        database.verify().map_err(storage_error)?;
        Ok(Self {
            facts,
            _database: database,
            _lock: None,
            _directory: directory,
        })
    }

    pub fn read(&self, name: &str) -> Result<Option<Vec<u8>>, StorageError> {
        validate_name(name)?;
        self.facts.read(name)
    }
    pub fn names(&self) -> Result<Vec<String>, StorageError> {
        self.facts.names()
    }
    pub fn put(&self, name: &str, bytes: &[u8]) -> Result<(), StorageError> {
        validate_name(name)?;
        self.facts.put(name, bytes)
    }
    pub fn archive(&self, name: &str, archive_name: &str) -> Result<(), StorageError> {
        validate_name(name)?;
        validate_name(archive_name)?;
        self.facts.archive(name, archive_name)
    }
    pub fn import_bootstrap(&self, path: &Path) -> Result<(), StorageError> {
        let bytes = read_private_input(path, 2 * 1024 * 1024)?;
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

fn validate_name(name: &str) -> Result<(), StorageError> {
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        Err(StorageError::Unsafe)
    } else {
        Ok(())
    }
}

fn read_private_input(
    path: &Path,
    max_bytes: usize,
) -> Result<zeroize::Zeroizing<Vec<u8>>, StorageError> {
    let directory = PrivateDirectory::open_existing(path.parent().ok_or(StorageError::Unsafe)?)
        .map_err(storage_error)?;
    let name =
        EntryName::new(path.file_name().ok_or(StorageError::Unsafe)?).map_err(storage_error)?;
    directory
        .read_private_bounded(&name, max_bytes)
        .map(zeroize::Zeroizing::new)
        .map_err(storage_error)
}

pub fn read_input(path: &Path) -> Result<zeroize::Zeroizing<Vec<u8>>, StorageError> {
    read_private_input(path, 65536)
}
