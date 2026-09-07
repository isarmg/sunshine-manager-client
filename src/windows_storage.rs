//! Native Windows ACL/handle protection. SQLite FULL transactions provide durable facts;
//! no portable directory-fsync fallback or permission emulation is used.
use super::{StorageError, storage_error};
use rusqlite::OptionalExtension;
use std::{
    ffi::{OsStr, c_void},
    fs::{File, OpenOptions},
    io::Read,
    os::windows::{
        ffi::OsStrExt,
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle},
    },
    path::{Component, Path, PathBuf, Prefix},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
    System::Threading::*,
};

pub struct DirectoryGuard {
    _ancestors: Vec<File>,
    path: PathBuf,
}
pub struct ProtectedState {
    connection: rusqlite::Connection,
    _database: File,
    _lock: File,
    _directory: DirectoryGuard,
}
fn wide(value: &OsStr) -> Result<Vec<u16>, StorageError> {
    let mut bytes: Vec<u16> = value.encode_wide().collect();
    if bytes.contains(&0) {
        return Err(StorageError);
    }
    bytes.push(0);
    Ok(bytes)
}
struct LocalAllocation(*mut c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
fn sid_text(sid: PSID) -> Result<String, StorageError> {
    unsafe {
        let mut output = std::ptr::null_mut();
        if ConvertSidToStringSidW(sid, &mut output) == 0 {
            return Err(StorageError);
        }
        let _owned = LocalAllocation(output.cast());
        let mut len = 0;
        while *output.add(len) != 0 {
            len += 1;
            if len > 256 {
                return Err(StorageError);
            }
        }
        String::from_utf16(std::slice::from_raw_parts(output, len)).map_err(storage_error)
    }
}
fn current_sid() -> Result<String, StorageError> {
    unsafe {
        let mut raw = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) == 0 {
            return Err(StorageError);
        }
        let token = File::from_raw_handle(raw);
        let mut needed = 0;
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            std::ptr::null_mut(),
            0,
            &mut needed,
        );
        if needed == 0 || needed > 16384 {
            return Err(StorageError);
        }
        // u64 storage supplies native alignment for TOKEN_USER.
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
        if GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        ) == 0
        {
            return Err(StorageError);
        }
        sid_text((*(buffer.as_ptr().cast::<TOKEN_USER>())).User.Sid)
    }
}
fn trusted(sid: &str, current: &str) -> bool {
    sid == current || sid == "S-1-5-18" || sid == "S-1-5-32-544"
}
fn verify_private(file: &File) -> Result<(), StorageError> {
    let current = current_sid()?;
    unsafe {
        let mut owner = std::ptr::null_mut();
        let mut acl = std::ptr::null_mut();
        let mut descriptor = std::ptr::null_mut();
        if GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            &mut acl,
            std::ptr::null_mut(),
            &mut descriptor,
        ) != 0
        {
            return Err(StorageError);
        }
        let _owned = LocalAllocation(descriptor);
        if owner.is_null() || acl.is_null() || !trusted(&sid_text(owner)?, &current) {
            return Err(StorageError);
        }
        if (*acl).AceCount == 0 {
            return Err(StorageError);
        }
        for index in 0..(*acl).AceCount {
            let mut ace = std::ptr::null_mut();
            if GetAce(acl, index.into(), &mut ace) == 0 {
                return Err(StorageError);
            }
            let header = &*ace.cast::<ACE_HEADER>();
            // Only simple allow ACEs for the service identity, SYSTEM and Administrators.
            // Unknown/object/callback ACEs fail closed; inheritance-only ACEs are checked too.
            if header.AceType != 0 {
                return Err(StorageError);
            }
            let allow = &*ace.cast::<ACCESS_ALLOWED_ACE>();
            if !trusted(
                &sid_text(std::ptr::addr_of!(allow.SidStart).cast_mut().cast())?,
                &current,
            ) {
                return Err(StorageError);
            }
        }
    }
    Ok(())
}
fn verify_kind(file: &File, directory: bool) -> Result<(), StorageError> {
    unsafe {
        let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
        if GetFileInformationByHandle(file.as_raw_handle(), &mut info) == 0
            || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
            || (!directory && info.nNumberOfLinks != 1)
        {
            return Err(StorageError);
        }
    }
    Ok(())
}
fn directory_handle(path: &Path) -> Result<File, StorageError> {
    let file = OpenOptions::new()
        .access_mode(READ_CONTROL | FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(storage_error)?;
    verify_kind(&file, true)?;
    Ok(file)
}
pub fn prepare_root(path: &Path) -> Result<DirectoryGuard, StorageError> {
    // Local absolute DOS drive paths only: no UNC shares, device namespaces, ADS or traversal.
    let mut components = path.components();
    if !matches!(components.next(),Some(Component::Prefix(p)) if matches!(p.kind(),Prefix::Disk(_)))
        || !matches!(components.next(), Some(Component::RootDir))
    {
        return Err(StorageError);
    }
    let parts: Vec<_> = components.collect();
    if parts.is_empty()||parts.iter().any(|c|!matches!(c,Component::Normal(n) if !n.to_string_lossy().contains([':', '*', '?'])&&!n.to_string_lossy().ends_with(['.',' ']))) {return Err(StorageError);}
    let parent = path.parent().ok_or(StorageError)?;
    let mut ancestors = Vec::new();
    let mut cursor = PathBuf::new();
    for part in parent.components() {
        cursor.push(part.as_os_str());
        if matches!(part, Component::Prefix(_)) {
            continue;
        }
        ancestors.push(directory_handle(&cursor)?);
    }
    let current = current_sid()?;
    let sddl = wide(OsStr::new(&format!(
        "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;{current})"
    )))?;
    unsafe {
        let mut descriptor = std::ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(StorageError);
        }
        let _owned = LocalAllocation(descriptor);
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        if CreateDirectoryW(wide(path.as_os_str())?.as_ptr(), &attributes) == 0
            && GetLastError() != ERROR_ALREADY_EXISTS
        {
            return Err(StorageError);
        }
    }
    let leaf = directory_handle(path)?;
    verify_private(&leaf)?;
    ancestors.push(leaf);
    Ok(DirectoryGuard {
        _ancestors: ancestors,
        path: path.to_owned(),
    })
}
fn private_file(path: &Path, create: bool, exclusive: bool) -> Result<File, StorageError> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(create)
        .create(create)
        .share_mode(if exclusive {
            0
        } else {
            FILE_SHARE_READ | FILE_SHARE_WRITE
        })
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let file = options.open(path).map_err(storage_error)?;
    verify_kind(&file, false)?;
    verify_private(&file)?;
    Ok(file)
}
impl ProtectedState {
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        let directory = prepare_root(path)?;
        let lock = private_file(&directory.path.join("state.lock"), true, true)?;
        // Refuse unexpected or aliased SQLite sidecars before the database opens them.
        for entry in std::fs::read_dir(path).map_err(storage_error)? {
            let entry = entry.map_err(storage_error)?;
            let name = entry.file_name();
            if name == "state.lock" {
                continue;
            }
            if ![
                "state.sqlite3",
                "state.sqlite3-journal",
                "state.sqlite3-wal",
                "state.sqlite3-shm",
            ]
            .iter()
            .any(|n| name == *n)
            {
                return Err(StorageError);
            }
            private_file(&entry.path(), false, false)?;
        }
        let database = private_file(&directory.path.join("state.sqlite3"), true, false)?;
        let connection = rusqlite::Connection::open(directory.path.join("state.sqlite3"))
            .map_err(storage_error)?;
        connection.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA secure_delete=ON; CREATE TABLE IF NOT EXISTS facts(name TEXT PRIMARY KEY,value BLOB NOT NULL) STRICT;").map_err(storage_error)?;
        let integrity: String = connection
            .query_row("PRAGMA quick_check", [], |r| r.get(0))
            .map_err(storage_error)?;
        if integrity != "ok" {
            return Err(StorageError);
        }
        Ok(Self {
            connection,
            _database: database,
            _lock: lock,
            _directory: directory,
        })
    }
    pub fn read(&self, name: &str) -> Result<Option<Vec<u8>>, StorageError> {
        validate_name(name)?;
        let length: Option<i64> = self
            .connection
            .query_row(
                "SELECT length(value) FROM facts WHERE name=?",
                [name],
                |r| r.get(0),
            )
            .optional()
            .map_err(storage_error)?;
        if length.is_some_and(|n| n > 2 * 1024 * 1024) {
            return Err(StorageError);
        }
        self.connection
            .query_row("SELECT value FROM facts WHERE name=?", [name], |r| r.get(0))
            .optional()
            .map_err(storage_error)
    }
    pub fn names(&self) -> Result<Vec<String>, StorageError> {
        let mut statement = self
            .connection
            .prepare("SELECT name FROM facts ORDER BY name LIMIT 4098")
            .map_err(storage_error)?;
        statement
            .query_map([], |r| r.get(0))
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }
    pub fn put(&self, name: &str, bytes: &[u8]) -> Result<(), StorageError> {
        validate_name(name)?;
        if bytes.len() > 2 * 1024 * 1024 {
            return Err(StorageError);
        }
        self.connection.execute("INSERT INTO facts(name,value) VALUES(?,?) ON CONFLICT(name) DO UPDATE SET value=excluded.value",rusqlite::params![name,bytes]).map_err(storage_error)?;
        Ok(())
    }
    pub fn import_bootstrap(&self, path: &Path) -> Result<(), StorageError> {
        // Hold every ancestor and verify the existing input directory (never create missing input).
        if !path.parent().ok_or(StorageError)?.is_dir() {
            return Err(StorageError);
        }
        let _directory = prepare_root(path.parent().ok_or(StorageError)?)?;
        let file = private_file(path, false, false)?;
        let mut bytes = zeroize::Zeroizing::new(Vec::new());
        file.take(2 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(storage_error)?;
        if bytes.len() > 2 * 1024 * 1024
            || self.read("bootstrap.json")?.is_some()
            || self.read("identity.json")?.is_some()
        {
            return Err(StorageError);
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
        Err(StorageError)
    } else {
        Ok(())
    }
}
