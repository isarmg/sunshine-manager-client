//! Linux durable execution facts, using Foundation's held-directory and atomic-file primitives.
//! Windows must not use the Foundation 0.6.0 portable fallback (known ACL/handle release blocker).
use crate::engine::{ExecutionRecord, Journal, JournalError};
use sarmg_client_fs_safety::{
    AdvisoryLock, AtomicFile, EntryName, InventoryLimits, PrivateDirectory,
};
use std::{collections::BTreeSet, path::Path};
use sunshine_client_protocol::validate_revision;

const MAX_RECORDS: usize = 4096;
const MAX_RECORD_BYTES: usize = 16 * 1024;
const LOCK_NAME: &str = "execution.lock";

pub struct FileJournal {
    directory: PrivateDirectory,
    _lock: AdvisoryLock,
    ids: BTreeSet<String>,
    /// A failed fsync can have installed the new name. Do not trust cached inventory afterward.
    poisoned: bool,
}

impl FileJournal {
    pub fn open(path: &Path) -> Result<Self, JournalError> {
        let directory = PrivateDirectory::create(path).map_err(storage)?;
        let lock_name = EntryName::new(LOCK_NAME).map_err(storage)?;
        let lock = AdvisoryLock::acquire(&directory, &lock_name.as_relative()).map_err(storage)?;
        let files = directory
            .files(InventoryLimits {
                max_entries: MAX_RECORDS + 64,
                max_total_bytes: (MAX_RECORDS * MAX_RECORD_BYTES) as u64,
            })
            .map_err(storage)?;
        let mut ids = BTreeSet::new();
        for file in files {
            let name = file
                .name
                .as_os_str()
                .to_str()
                .ok_or(JournalError::Storage)?;
            if name == LOCK_NAME || AtomicFile::is_temporary_name(&file.name) {
                continue;
            }
            let id = name.strip_suffix(".json").ok_or(JournalError::Storage)?;
            record_name(id)?;
            let bytes = directory
                .read_private_bounded(&file.name, MAX_RECORD_BYTES)
                .map_err(storage)?;
            let record: ExecutionRecord = serde_json::from_slice(&bytes).map_err(storage)?;
            validate_record(&record)?;
            ids.insert(id.to_owned());
        }
        if ids.len() > MAX_RECORDS {
            return Err(JournalError::Full);
        }
        Ok(Self {
            directory,
            _lock: lock,
            ids,
            poisoned: false,
        })
    }
}

impl Journal for FileJournal {
    fn load(&mut self, id: &str) -> Result<Option<ExecutionRecord>, JournalError> {
        if self.poisoned {
            return Err(JournalError::Storage);
        }
        let name = record_name(id)?;
        if !self.ids.contains(id) {
            return Ok(None);
        }
        let bytes = self
            .directory
            .read_private_bounded(&name, MAX_RECORD_BYTES)
            .map_err(storage)?;
        let record: ExecutionRecord = serde_json::from_slice(&bytes).map_err(storage)?;
        validate_record(&record)?;
        Ok(Some(record))
    }

    fn create(&mut self, id: &str, record: &ExecutionRecord) -> Result<(), JournalError> {
        if self.poisoned || self.ids.contains(id) {
            return Err(JournalError::Storage);
        }
        if self.ids.len() >= MAX_RECORDS {
            return Err(JournalError::Full);
        }
        let name = record_name(id)?;
        let bytes = encode(record)?;
        if AtomicFile::create(&self.directory, &name, &bytes).is_err() {
            self.poisoned = true;
            return Err(JournalError::Storage);
        }
        self.ids.insert(id.to_owned());
        Ok(())
    }

    fn replace(&mut self, id: &str, record: &ExecutionRecord) -> Result<(), JournalError> {
        if self.poisoned {
            return Err(JournalError::Storage);
        }
        let existing = self.load(id)?.ok_or(JournalError::Storage)?;
        // Never erase an intent, change an operation fingerprint or rewrite a final observation.
        if existing.fingerprint != record.fingerprint
            || (existing.effect.is_some() && existing.effect != record.effect)
            || existing.report.as_ref().is_some_and(|report| {
                !matches!(report, sunshine_client_protocol::Report::Unknown { .. })
                    && Some(report) != record.report.as_ref()
            })
        {
            return Err(JournalError::Storage);
        }
        let bytes = encode(record)?;
        if AtomicFile::replace(&self.directory, &record_name(id)?.as_relative(), &bytes).is_err() {
            self.poisoned = true;
            return Err(JournalError::Storage);
        }
        Ok(())
    }
}

fn record_name(id: &str) -> Result<EntryName, JournalError> {
    // No caller-controlled path fragments. Matches protocol operation IDs exactly.
    let uuid = id.strip_prefix("op_").ok_or(JournalError::Storage)?;
    if uuid.len() != 36
        || uuid.bytes().enumerate().any(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte != b'-'
            } else {
                !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte)
            }
        })
    {
        return Err(JournalError::Storage);
    }
    EntryName::new(format!("{id}.json")).map_err(storage)
}

fn validate_record(record: &ExecutionRecord) -> Result<(), JournalError> {
    validate_revision(&record.fingerprint).map_err(storage)?;
    if let Some(crate::engine::EffectIntent::Save { target_revision }) = &record.effect {
        validate_revision(target_revision).map_err(storage)?;
    }
    Ok(())
}

fn encode(record: &ExecutionRecord) -> Result<Vec<u8>, JournalError> {
    validate_record(record)?;
    let bytes = serde_json::to_vec(record).map_err(storage)?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(JournalError::Full);
    }
    Ok(bytes)
}

fn storage(_: impl std::fmt::Debug) -> JournalError {
    JournalError::Storage
}

/// Read local execution facts without opening the writer or acquiring a lock.
pub fn inspect(path: &Path, operation: Option<&str>) -> Result<serde_json::Value, JournalError> {
    let directory = PrivateDirectory::open_for_administration(path).map_err(storage)?;
    if let Some(id) = operation {
        let bytes = directory
            .read_private_bounded(&record_name(id)?, MAX_RECORD_BYTES)
            .map_err(storage)?;
        let record: ExecutionRecord = serde_json::from_slice(&bytes).map_err(storage)?;
        validate_record(&record)?;
        return Ok(
            serde_json::json!({"scope":"local_execution_observation","operation_id":id,"record":record}),
        );
    }
    let mut records = Vec::new();
    for file in directory
        .files(InventoryLimits {
            max_entries: MAX_RECORDS + 64,
            max_total_bytes: (MAX_RECORDS * MAX_RECORD_BYTES) as u64,
        })
        .map_err(storage)?
    {
        let key = file
            .name
            .as_os_str()
            .to_str()
            .ok_or(JournalError::Storage)?;
        if key == LOCK_NAME || AtomicFile::is_temporary_name(&file.name) {
            continue;
        }
        let id = key.strip_suffix(".json").ok_or(JournalError::Storage)?;
        record_name(id)?;
        let bytes = directory
            .read_private_bounded(&file.name, MAX_RECORD_BYTES)
            .map_err(storage)?;
        let record: ExecutionRecord = serde_json::from_slice(&bytes).map_err(storage)?;
        validate_record(&record)?;
        records.push(
            serde_json::json!({"operation_id":id,"effect":record.effect,"report":record.report}),
        );
    }
    Ok(serde_json::json!({"scope":"local_execution_observation","records":records}))
}
