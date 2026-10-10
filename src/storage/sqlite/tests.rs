use super::*;
fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("facts.sqlite");
    std::fs::File::create(&path).unwrap();
    (directory, path)
}
#[test]
fn current_table_rows_and_durability_survive_connection_reopen() {
    let (_directory, path) = fixture();
    // Construct the preexisting current schema independently of Facts::open.
    xcsc::runtime::block_on_worker_future(async {
        let mut connection =
            SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path))
                .await
                .unwrap();
        sqlx::raw_sql("CREATE TABLE facts(name TEXT PRIMARY KEY,value BLOB NOT NULL) STRICT;")
            .execute(&mut connection)
            .await
            .unwrap();
        sqlx::query("INSERT INTO facts VALUES(?,?)")
            .bind("identity.json")
            .bind(&[0u8, 255, 10][..])
            .execute(&mut connection)
            .await
            .unwrap();
        connection.close().await.unwrap();
    });
    let store = Facts::open(&path, true).unwrap();
    assert_eq!(store.read("identity.json").unwrap(), Some(vec![0, 255, 10]));
    store.put("pending.json", b"pending operation").unwrap();
    drop(store);
    let store = Facts::open(&path, false).unwrap();
    assert_eq!(
        store.read("pending.json").unwrap().unwrap(),
        b"pending operation"
    );
    assert!(store.put("new", b"value").is_err());
    assert!(store.archive("identity.json", "history.json").is_err());
    assert_eq!(store.names().unwrap(), ["identity.json", "pending.json"]);
}
#[test]
fn archive_conflict_or_missing_source_preserves_all_original_facts() {
    let (_directory, path) = fixture();
    let store = Facts::open(&path, true).unwrap();
    store.put("source", b"source bytes").unwrap();
    store.put("archive", b"existing archive").unwrap();
    assert!(store.archive("source", "archive").is_err());
    assert!(store.archive("missing", "new").is_err());
    assert_eq!(store.read("source").unwrap().unwrap(), b"source bytes");
    assert_eq!(store.read("archive").unwrap().unwrap(), b"existing archive");
    assert!(store.read("new").unwrap().is_none());
    store.archive("source", "completed").unwrap();
    drop(store);
    let reopened = Facts::open(&path, false).unwrap();
    assert!(reopened.read("source").unwrap().is_none());
    assert_eq!(
        reopened.read("completed").unwrap().unwrap(),
        b"source bytes"
    );
}
#[test]
fn fact_budget_and_existing_only_open_are_enforced() {
    let (directory, path) = fixture();
    assert!(Facts::open(&directory.path().join("missing"), true).is_err());
    let store = Facts::open(&path, true).unwrap();
    assert!(store.put("huge", &vec![0; MAX_FACT_BYTES + 1]).is_err());
    assert!(store.read("huge").unwrap().is_none());
    // Simulate a database writer outside the bounded facts API.
    let mut connection = store.connection.lock().unwrap();
    xcsc::runtime::block_on_worker_future(
        sqlx::query("INSERT INTO facts VALUES('huge', zeroblob(?))")
            .bind(MAX_FACT_BYTES as i64 + 1)
            .execute(connection.as_mut().unwrap()),
    )
    .unwrap();
    drop(connection);
    assert!(store.read("huge").is_err());
}
#[tokio::test]
async fn synchronous_facts_api_operates_inside_existing_tokio_runtime() {
    let (_directory, path) = fixture();
    let store = Facts::open(&path, true).unwrap();
    store.put("inside", b"no nested runtime").unwrap();
    assert_eq!(store.read("inside").unwrap().unwrap(), b"no nested runtime");
    drop(store);
}

#[test]
fn unknown_schema_is_rejected_without_repair_or_mutable_pragmas() {
    let (_directory, path) = fixture();
    xcsc::runtime::block_on_worker_future(async {
        let mut connection =
            SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path))
                .await
                .unwrap();
        sqlx::raw_sql("CREATE TABLE facts(name TEXT PRIMARY KEY,value BLOB); INSERT INTO facts VALUES('original',X'00FF');")
            .execute(&mut connection).await.unwrap();
        connection.close().await.unwrap();
    });
    let original = std::fs::read(&path).unwrap();
    assert!(Facts::open(&path, true).is_err());
    assert!(Facts::open(&path, false).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[test]
fn names_reject_oversized_database_keys_without_decoding_them() {
    let (_directory, path) = fixture();
    let store = Facts::open(&path, true).unwrap();
    let mut connection = store.connection.lock().unwrap();
    xcsc::runtime::block_on_worker_future(
        sqlx::query("INSERT INTO facts VALUES(?,X'01')")
            .bind("x".repeat(256 * 1024))
            .execute(connection.as_mut().unwrap()),
    )
    .unwrap();
    drop(connection);
    assert!(store.names().is_err());
    store.put("still-usable", b"value").unwrap();
    assert_eq!(store.read("still-usable").unwrap().unwrap(), b"value");
}
