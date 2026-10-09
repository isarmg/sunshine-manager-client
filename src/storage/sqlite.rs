//! Windows facts keep the existing STRICT table and FULL durability settings.
//! SQLx's SQLite worker provides async I/O; a direct connection preserves the
//! synchronous provisioning API without entering or nesting a Tokio runtime.
use super::{StorageError, storage_error};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use std::{path::Path, sync::Mutex, time::Duration};

const MAX_FACT_BYTES: usize = 2 * 1024 * 1024;
const SCHEMA: &str = include_str!("../../schema/windows-facts.sql");

pub(super) struct Facts {
    connection: Mutex<Option<SqliteConnection>>,
    writable: bool,
}
impl Facts {
    pub(super) fn open(path: &Path, writable: bool) -> Result<Self, StorageError> {
        xcsc_runtime::block_on_worker_future(async {
            let options = SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(false)
                .read_only(!writable)
                .busy_timeout(Duration::from_secs(5));
            let mut connection = SqliteConnection::connect_with(&options)
                .await
                .map_err(storage_error)?;
            let validation = async {
                // Inspect the current authoritative schema before mutable pragmas.
                // Even corrupted private data cannot make an unknown schema look
                // like an empty facts store or allocate unbounded DDL text.
                let definitions: Vec<Option<String>> = sqlx::query_scalar(
                    "SELECT CASE WHEN type='table' AND name='facts' AND length(sql)<=1024 THEN sql ELSE NULL END FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' LIMIT 2",
                )
                .fetch_all(&mut connection).await.map_err(storage_error)?;
                let expected = SCHEMA.trim().trim_end_matches(';').replace(" IF NOT EXISTS", "");
                match definitions.as_slice() {
                    [] if writable => {
                        sqlx::raw_sql(SCHEMA).execute(&mut connection).await.map_err(storage_error)?;
                    }
                    [Some(actual)] if actual == &expected => {}
                    _ => return Err(StorageError::Unsafe),
                }
                if writable {
                    sqlx::raw_sql("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA secure_delete=ON;")
                        .execute(&mut connection).await.map_err(storage_error)?;
                }
                let integrity: String = sqlx::query_scalar("PRAGMA quick_check")
                    .fetch_one(&mut connection)
                    .await
                    .map_err(storage_error)?;
                if integrity != "ok" {
                    return Err(StorageError::Unsafe);
                }
                Ok(())
            }
            .await;
            if let Err(error) = validation {
                let _ = connection.close().await;
                return Err(error);
            }
            Ok(Self {
                connection: Mutex::new(Some(connection)),
                writable,
            })
        })
    }
    pub(super) fn read(&self, name: &str) -> Result<Option<Vec<u8>>, StorageError> {
        let mut connection = self.connection.lock().map_err(|_| StorageError::Unsafe)?;
        let connection = connection.as_mut().ok_or(StorageError::Unsafe)?;
        xcsc_runtime::block_on_worker_future(async {
            let length: Option<i64> =
                sqlx::query_scalar("SELECT length(value) FROM facts WHERE name=?")
                    .bind(name)
                    .fetch_optional(&mut *connection)
                    .await
                    .map_err(storage_error)?;
            if length.is_some_and(|n| n < 0 || n > MAX_FACT_BYTES as i64) {
                return Err(StorageError::Unsafe);
            }
            sqlx::query_scalar("SELECT value FROM facts WHERE name=?")
                .bind(name)
                .fetch_optional(&mut *connection)
                .await
                .map_err(storage_error)
        })
    }
    pub(super) fn names(&self) -> Result<Vec<String>, StorageError> {
        let mut connection = self.connection.lock().map_err(|_| StorageError::Unsafe)?;
        let names: Vec<Option<String>> = xcsc_runtime::block_on_worker_future(
            sqlx::query_scalar("SELECT CASE WHEN length(name) BETWEEN 1 AND 128 THEN name ELSE NULL END FROM facts ORDER BY name LIMIT 4098")
                .fetch_all(connection.as_mut().ok_or(StorageError::Unsafe)?),
        )
        .map_err(storage_error)?;
        names
            .into_iter()
            .map(|name| name.ok_or(StorageError::Unsafe))
            .collect()
    }

    pub(super) fn put(&self, name: &str, bytes: &[u8]) -> Result<(), StorageError> {
        if !self.writable || bytes.len() > MAX_FACT_BYTES {
            return Err(StorageError::Unsafe);
        }
        let mut connection = self.connection.lock().map_err(|_| StorageError::Unsafe)?;
        xcsc_runtime::block_on_worker_future(sqlx::query("INSERT INTO facts(name,value) VALUES(?,?) ON CONFLICT(name) DO UPDATE SET value=excluded.value")
            .bind(name).bind(bytes).execute(connection.as_mut().ok_or(StorageError::Unsafe)?)).map_err(storage_error)?;
        Ok(())
    }
    pub(super) fn archive(&self, name: &str, destination: &str) -> Result<(), StorageError> {
        if !self.writable {
            return Err(StorageError::Unsafe);
        }
        let mut connection = self.connection.lock().map_err(|_| StorageError::Unsafe)?;
        xcsc_runtime::block_on_worker_future(async {
            let mut transaction = connection
                .as_mut()
                .ok_or(StorageError::Unsafe)?
                .begin()
                .await
                .map_err(storage_error)?;
            let result = async {
                let copied = sqlx::query(
                    "INSERT INTO facts(name,value) SELECT ?,value FROM facts WHERE name=?",
                )
                .bind(destination)
                .bind(name)
                .execute(&mut *transaction)
                .await
                .map_err(storage_error)?
                .rows_affected();
                if copied != 1 {
                    return Err(StorageError::Unsafe);
                }
                let deleted = sqlx::query("DELETE FROM facts WHERE name=?")
                    .bind(name)
                    .execute(&mut *transaction)
                    .await
                    .map_err(storage_error)?
                    .rows_affected();
                if deleted != 1 {
                    return Err(StorageError::Unsafe);
                }
                Ok(())
            }
            .await;
            match result {
                Ok(()) => transaction.commit().await.map_err(storage_error),
                Err(error) => {
                    transaction.rollback().await.map_err(storage_error)?;
                    Err(error)
                }
            }
        })
    }
}
impl Drop for Facts {
    fn drop(&mut self) {
        // SQLx Drop requests asynchronous worker shutdown. Await close here so
        // the protected database/ancestor handles outlive every SQLite access.
        let connection = self
            .connection
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(connection) = connection {
            let _ = xcsc_runtime::block_on_worker_future(connection.close());
        }
    }
}

#[cfg(test)]
#[path = "sqlite/tests.rs"]
mod tests;
