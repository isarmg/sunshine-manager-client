use sunshine_client::storage::{ProtectedState, prepare_root};
#[test]
fn protected_state_persists_and_excludes_other_processes() {
    let temporary = tempfile::tempdir().unwrap();
    let root = physical_parent(&temporary).join("client");
    let _root = prepare_root(&root).unwrap();
    let path = root.join("state");
    let store = ProtectedState::open(&path).unwrap();
    assert!(ProtectedState::open(&path).is_err());
    assert!(store.read("identity.json").unwrap().is_none());
    store.put("identity.json", b"first").unwrap();
    store.put("identity.json", b"second").unwrap();
    assert!(store.put("../escape", b"x").is_err());
    drop(store);
    let reopened = ProtectedState::open(&path).unwrap();
    assert_eq!(reopened.read("identity.json").unwrap().unwrap(), b"second");
}
#[cfg(windows)]
#[test]
fn windows_journal_facts_survive_reopen_and_cannot_erase_intent() {
    use sunshine_client::{
        engine::{EffectIntent, ExecutionRecord, Journal},
        journal::FileJournal,
    };
    let temporary = tempfile::tempdir().unwrap();
    let path = physical_parent(&temporary).join("journal");
    let mut journal = FileJournal::open(&path).unwrap();
    assert!(FileJournal::open(&path).is_err());
    let id = "op_00000000-0000-4000-8000-000000000001";
    let mut record = ExecutionRecord {
        fingerprint: "a".repeat(64),
        effect: None,
        report: None,
    };
    journal.create(id, &record).unwrap();
    record.effect = Some(EffectIntent::Restart);
    journal.replace(id, &record).unwrap();
    drop(journal);
    let mut reopened = FileJournal::open(&path).unwrap();
    assert_eq!(reopened.load(id).unwrap().unwrap(), record);
    record.effect = None;
    assert!(reopened.replace(id, &record).is_err());
    assert!(reopened.create(id, &record).is_err());
}
#[cfg(windows)]
#[test]
fn windows_rejects_world_readable_acl_and_hardlinked_database() {
    let temporary = tempfile::tempdir().unwrap();
    let root = physical_parent(&temporary).join("state");
    drop(ProtectedState::open(&root).unwrap());
    std::fs::hard_link(
        root.join("state.sqlite3"),
        physical_parent(&temporary).join("alias.db"),
    )
    .unwrap();
    assert!(ProtectedState::open(&root).is_err());
    let other = physical_parent(&temporary).join("public");
    drop(ProtectedState::open(&other).unwrap());
    let result = std::process::Command::new("icacls.exe")
        .arg(&other)
        .args(["/grant", "*S-1-1-0:(OI)(CI)R"])
        .output()
        .unwrap();
    assert!(result.status.success());
    assert!(ProtectedState::open(&other).is_err());
}

fn physical_parent(temporary: &tempfile::TempDir) -> std::path::PathBuf {
    #[cfg(unix)]
    {
        temporary.path().canonicalize().unwrap()
    }
    #[cfg(windows)]
    {
        temporary.path().to_owned()
    }
}
