//! Synthetic offline identity for disposable native CI service tests only.
use serde_json::json;
use sunshine_client::{
    engine::{EffectIntent, ExecutionRecord, Journal},
    journal::FileJournal,
    storage::{ProtectedState, prepare_root},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
        || std::env::var("RUNNER_ENVIRONMENT").as_deref() != Ok("github-hosted")
    {
        return Err("fixture is restricted to disposable GitHub-hosted acceptance jobs".into());
    }
    let path = std::env::args_os().nth(1).ok_or("state path required")?;
    let path = std::path::Path::new(&path);
    let _root = prepare_root(path)?;
    let store = ProtectedState::open(&path.join("provisioning"))?;
    if store.read("identity.json")?.is_some() {
        return Err("refusing to overwrite an existing identity".into());
    }
    store.put("identity.json", &serde_json::to_vec(&json!({
        "binding": {"manager_id": uuid::Uuid::new_v4(), "device_id": uuid::Uuid::new_v4(), "installation_id": uuid::Uuid::new_v4()},
        "credential": "a".repeat(64), "enrolled": true,
        "config": {"manager_endpoint": "wss://127.0.0.1:9/sunshine-client/v1/connect", "enrollment_token": "",
            "sunshine_endpoint": "https://127.0.0.1:9/", "sunshine_username": "native-fixture", "sunshine_password": "offline-fixture-secret", "restart_allowed": false}
    }))?)?;
    drop(store);
    let mut journal = FileJournal::open(&path.join("journal"))?;
    journal.create(
        "op_00000000-0000-4000-8000-000000000001",
        &ExecutionRecord {
            fingerprint: "b".repeat(64),
            effect: Some(EffectIntent::Restart),
            report: None,
        },
    )?;
    Ok(())
}
