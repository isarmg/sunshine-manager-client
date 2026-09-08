#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Stdio},
};
use sunshine_client::storage::{MaintenanceGuard, ProtectedState};
fn call(path: &Path, args: &[&str], input: Option<&str>) -> (i32, Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_sunshine-client"))
        .args(args)
        .arg("--state")
        .arg(path)
        .args(["--format", "json", "--non-interactive"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(input) = input {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    } else {
        drop(child.stdin.take());
    }
    let output = child.wait_with_output().unwrap();
    let json = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.code().unwrap(), json)
}
fn private_file(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}
#[test]
fn readonly_missing_state_never_creates_it_and_invalid_input_is_bounded() {
    let temp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let state = temp.path().join("missing");
    for command in [["status"], ["doctor"]] {
        assert_eq!(call(&state, &command, None).0, 0);
        assert!(!state.exists());
    }
    assert_eq!(
        call(
            &state,
            &["pair", "--input-stdin"],
            Some("{\"password\":\"do-not-print\"}")
        )
        .0,
        2
    );
    assert!(!state.exists());
    assert_eq!(
        call(&state, &["pair", "--input-stdin"], Some(&"x".repeat(65537))).0,
        2
    );
    assert!(!state.exists());
}
#[test]
fn credentials_update_preserves_binding_credential_and_execution_journal() {
    let temp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let state = temp.path().join("state");
    let _root = sunshine_client::storage::prepare_root(&state).unwrap();
    let store = ProtectedState::open(&state.join("provisioning")).unwrap();
    let binding = json!({"manager_id":uuid::Uuid::new_v4(),"device_id":uuid::Uuid::new_v4(),"installation_id":uuid::Uuid::new_v4()});
    let identity = json!({"binding":binding,"credential":"b".repeat(64),"enrolled":true,"config":{"manager_endpoint":"wss://manager.example/sunshine-client/v1/connect","enrollment_token":"","sunshine_endpoint":"https://127.0.0.1:47990/","sunshine_username":"old","sunshine_password":"old-secret","restart_allowed":false}});
    store
        .put("identity.json", &serde_json::to_vec(&identity).unwrap())
        .unwrap();
    drop(store);
    fs::create_dir(state.join("journal")).unwrap();
    fs::set_permissions(state.join("journal"), fs::Permissions::from_mode(0o700)).unwrap();
    private_file(
        &state.join("journal/evidence"),
        b"unchanged execution facts",
    );
    let (exit, result) = call(
        &state,
        &["credentials", "update", "--input-stdin"],
        Some(r#"{"sunshine_username":"new","sunshine_password":"new-secret"}"#),
    );
    assert_eq!(exit, 0, "{result}");
    assert!(!result.to_string().contains("new-secret"));
    let store = ProtectedState::open_readonly(&state.join("provisioning")).unwrap();
    let current: Value =
        serde_json::from_slice(&store.read("identity.json").unwrap().unwrap()).unwrap();
    assert_eq!(current["binding"], binding);
    assert_eq!(current["credential"], identity["credential"]);
    assert_eq!(current["config"]["sunshine_password"], "new-secret");
    assert_eq!(
        fs::read(state.join("journal/evidence")).unwrap(),
        b"unchanged execution facts"
    );
    let guard = MaintenanceGuard::acquire(&state).unwrap();
    assert_eq!(
        call(
            &state,
            &["credentials", "update", "--input-stdin"],
            Some(r#"{"sunshine_username":"other","sunshine_password":"blocked"}"#)
        )
        .0,
        5
    );
    assert_eq!(call(&state, &["status"], None).0, 0);
    drop(guard);
}
#[test]
fn configuration_revision_conflict_cannot_change_settings() {
    let temp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let state = temp.path().join("state");
    assert_eq!(call(&state, &["config", "init"], None).0, 0);
    let before = call(&state, &["config", "show"], None);
    assert_eq!(before.0, 0);
    let candidate = temp.path().join("candidate.json");
    private_file(
        &candidate,
        br#"{"sunshine_endpoint":"https://127.0.0.1:47991/","restart_allowed":false}"#,
    );
    let result = call(
        &state,
        &[
            "config",
            "apply",
            "--file",
            candidate.to_str().unwrap(),
            "--expected-revision",
            "old-revision",
        ],
        None,
    );
    assert_eq!(result.0, 5, "{}", result.1);
    assert_eq!(call(&state, &["config", "show"], None).1, before.1);
}
