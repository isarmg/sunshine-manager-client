//! Windows execution facts use protected SQLite FULL commits, not a second task lifecycle.
use crate::{
    engine::{ExecutionRecord, Journal, JournalError},
    storage::ProtectedState,
};
use std::{collections::BTreeSet, path::Path};
pub struct FileJournal {
    store: ProtectedState,
    ids: BTreeSet<String>,
    poisoned: bool,
}
fn fail(_: impl std::fmt::Debug) -> JournalError {
    JournalError::Storage
}
fn name(id: &str) -> Result<String, JournalError> {
    let raw = id.strip_prefix("op_").ok_or(JournalError::Storage)?;
    let uuid = uuid::Uuid::parse_str(raw).map_err(fail)?;
    if uuid.is_nil() || uuid.to_string() != raw {
        return Err(JournalError::Storage);
    }
    Ok(format!("{id}.json"))
}
fn encode(record: &ExecutionRecord) -> Result<Vec<u8>, JournalError> {
    sunshine_client_protocol::validate_revision(&record.fingerprint).map_err(fail)?;
    if let Some(crate::engine::EffectIntent::Save { target_revision }) = &record.effect {
        sunshine_client_protocol::validate_revision(target_revision).map_err(fail)?;
    }
    let bytes = serde_json::to_vec(record).map_err(fail)?;
    if bytes.len() > 16384 {
        return Err(JournalError::Full);
    }
    Ok(bytes)
}
impl FileJournal {
    pub fn open(path: &Path) -> Result<Self, JournalError> {
        let store = ProtectedState::open(path).map_err(fail)?;
        let mut ids = BTreeSet::new();
        for key in store.names().map_err(fail)? {
            let id = key.strip_suffix(".json").ok_or(JournalError::Storage)?;
            name(id)?;
            let bytes = store
                .read(&key)
                .map_err(fail)?
                .ok_or(JournalError::Storage)?;
            let record: ExecutionRecord = serde_json::from_slice(&bytes).map_err(fail)?;
            encode(&record)?;
            ids.insert(id.to_owned());
        }
        if ids.len() > 4096 {
            return Err(JournalError::Full);
        }
        Ok(Self {
            store,
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
        self.store
            .read(&name(id)?)
            .map_err(fail)?
            .map(|b| serde_json::from_slice(&b).map_err(fail))
            .transpose()
    }
    fn create(&mut self, id: &str, record: &ExecutionRecord) -> Result<(), JournalError> {
        if self.poisoned || self.ids.contains(id) {
            return Err(JournalError::Storage);
        }
        if self.ids.len() >= 4096 {
            return Err(JournalError::Full);
        }
        if self.store.put(&name(id)?, &encode(record)?).is_err() {
            self.poisoned = true;
            return Err(JournalError::Storage);
        }
        self.ids.insert(id.into());
        Ok(())
    }
    fn replace(&mut self, id: &str, record: &ExecutionRecord) -> Result<(), JournalError> {
        let prior = self.load(id)?.ok_or(JournalError::Storage)?;
        if prior.fingerprint != record.fingerprint
            || (prior.effect.is_some() && prior.effect != record.effect)
            || prior.report.as_ref().is_some_and(|r| {
                !matches!(r, sunshine_client_protocol::Report::Unknown { .. })
                    && Some(r) != record.report.as_ref()
            })
        {
            return Err(JournalError::Storage);
        }
        if self.store.put(&name(id)?, &encode(record)?).is_err() {
            self.poisoned = true;
            return Err(JournalError::Storage);
        }
        Ok(())
    }
}
