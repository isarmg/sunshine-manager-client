#![cfg(target_os = "linux")]
use std::os::unix::fs::{PermissionsExt, symlink};
use sunshine_client::{
    engine::{EffectIntent, ExecutionRecord, Journal},
    journal::FileJournal,
};

const ID: &str = "op_00000000-0000-4000-8000-000000000001";
fn record() -> ExecutionRecord {
    ExecutionRecord {
        fingerprint: "a".repeat(64),
        effect: None,
        report: None,
    }
}

#[test]
fn facts_survive_reopen_and_hold_an_exclusive_process_lock() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("journal");
    let mut journal = FileJournal::open(&path).unwrap();
    assert!(FileJournal::open(&path).is_err());
    journal.create(ID, &record()).unwrap();
    let mut intent = record();
    intent.effect = Some(EffectIntent::Restart);
    journal.replace(ID, &intent).unwrap();
    assert!(journal.replace(ID, &record()).is_err());
    drop(journal);
    let mut reopened = FileJournal::open(&path).unwrap();
    assert_eq!(reopened.load(ID).unwrap(), Some(intent));
    assert!(reopened.create(ID, &record()).is_err());
    assert_eq!(
        std::fs::metadata(path.join(format!("{ID}.json")))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[test]
fn corrupt_record_and_symlink_fail_closed() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("journal");
    let mut journal = FileJournal::open(&path).unwrap();
    journal.create(ID, &record()).unwrap();
    drop(journal);
    std::fs::write(path.join(format!("{ID}.json")), b"broken").unwrap();
    assert!(FileJournal::open(&path).is_err());
    let linked = temporary.path().join("linked");
    symlink(&path, &linked).unwrap();
    assert!(FileJournal::open(&linked).is_err());
}

#[test]
fn permissive_directory_and_path_traversal_are_rejected() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("journal");
    let mut journal = FileJournal::open(&path).unwrap();
    assert!(journal.create("../escape", &record()).is_err());
    assert!(journal.load("../escape").is_err());
    drop(journal);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(FileJournal::open(&path).is_err());
}
