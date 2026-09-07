use async_trait::async_trait;
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use sunshine_client::{
    adapter::{AdapterError, Configuration, Sunshine},
    engine::*,
};
use sunshine_client_protocol::{config::FieldValue, *};
use uuid::Uuid;

fn configuration(qp: &str) -> Configuration {
    Configuration::from_response(json!({
        "status": true, "platform": "linux", "version": SUNSHINE_VERSION,
        "qp": qp, "sunshine_name": "主机", "global_prep_cmd": "[{\"do\":\"local-only\"}]",
        "file_apps": "/private/apps.json", "unmanaged_option": "leave-me-alone"
    }))
    .unwrap()
}

struct Fixture {
    configuration: Configuration,
    reads: usize,
    saves: usize,
    restarts: usize,
    lose_write_receipt: bool,
    lose_restart_receipt: bool,
    fail_read: bool,
    conflict_on_second_read: bool,
}

impl Default for Fixture {
    fn default() -> Self {
        Self {
            configuration: configuration("28"),
            reads: 0,
            saves: 0,
            restarts: 0,
            lose_write_receipt: false,
            lose_restart_receipt: false,
            fail_read: false,
            conflict_on_second_read: false,
        }
    }
}

#[derive(Clone, Default)]
struct FakeSunshine(Arc<Mutex<Fixture>>);

#[async_trait]
impl Sunshine for FakeSunshine {
    async fn read(&mut self) -> Result<Configuration, AdapterError> {
        let mut fixture = self.0.lock().unwrap();
        fixture.reads += 1;
        if fixture.fail_read {
            return Err(AdapterError::Unavailable);
        }
        if fixture.conflict_on_second_read && fixture.reads == 2 {
            fixture.configuration = configuration("30");
        }
        Ok(fixture.configuration.clone())
    }
    async fn save(&mut self, configuration: &Configuration) -> Result<(), AdapterError> {
        let mut fixture = self.0.lock().unwrap();
        fixture.saves += 1;
        fixture.configuration = configuration.clone();
        if fixture.lose_write_receipt {
            Err(AdapterError::Unavailable)
        } else {
            Ok(())
        }
    }
    async fn restart(&mut self) -> Result<(), AdapterError> {
        let mut fixture = self.0.lock().unwrap();
        fixture.restarts += 1;
        if fixture.lose_restart_receipt {
            Err(AdapterError::Unavailable)
        } else {
            Ok(())
        }
    }
}

#[derive(Default)]
struct Records {
    values: BTreeMap<String, ExecutionRecord>,
    writes: usize,
    fail_write: Option<usize>,
    full: bool,
}

#[derive(Clone, Default)]
struct MemoryJournal(Arc<Mutex<Records>>);

impl Journal for MemoryJournal {
    fn load(&mut self, id: &str) -> Result<Option<ExecutionRecord>, JournalError> {
        Ok(self.0.lock().unwrap().values.get(id).cloned())
    }
    fn create(&mut self, id: &str, record: &ExecutionRecord) -> Result<(), JournalError> {
        if self.0.lock().unwrap().full {
            return Err(JournalError::Full);
        }
        self.replace(id, record)
    }
    fn replace(&mut self, id: &str, record: &ExecutionRecord) -> Result<(), JournalError> {
        let mut records = self.0.lock().unwrap();
        records.writes += 1;
        if records.fail_write == Some(records.writes) {
            return Err(JournalError::Storage);
        }
        records.values.insert(id.into(), record.clone());
        Ok(())
    }
}

fn binding() -> Binding {
    Binding {
        manager_id: Uuid::from_u128(1),
        device_id: Uuid::from_u128(2),
        installation_id: Uuid::from_u128(3),
    }
}

fn patch() -> Task {
    Task {
        protocol: PROTOCOL.into(),
        operation_id: format!("op_{}", Uuid::new_v4()),
        binding: binding(),
        permission: Permission::WriteConfig,
        command: Command::PatchConfig {
            expected_revision: configuration("28").revision(),
            set: BTreeMap::from([("qp".into(), FieldValue::Integer(29))]),
            remove: Default::default(),
            restart_policy: RestartPolicy::Manual,
        },
    }
}

fn restart() -> Task {
    let mut task = patch();
    task.permission = Permission::Restart;
    task.command = Command::Restart {
        expected_revision: configuration("28").revision(),
        administrator_confirmed: true,
    };
    task
}

#[tokio::test]
async fn partial_patch_preserves_all_untouched_fields_and_never_restarts() {
    let sunshine = FakeSunshine::default();
    let executor = Executor::new(binding(), true, sunshine.clone(), MemoryJournal::default());
    let report = executor.deliver(&patch(), DeliveryMode::Execute).await;
    let Report::ConfigSaved { snapshot } = report else {
        panic!("{report:?}")
    };
    assert_eq!(snapshot.effectiveness, Effectiveness::AwaitingRestart);
    assert_eq!(snapshot.fields["qp"], "29");
    assert!(!snapshot.fields.contains_key("global_prep_cmd"));
    assert!(!snapshot.fields.contains_key("file_apps"));
    let fixture = sunshine.0.lock().unwrap();
    assert_eq!(
        fixture.configuration.revision(),
        configuration("29").revision()
    );
    assert_eq!(fixture.saves, 1);
    assert_eq!(fixture.restarts, 0);
}

#[tokio::test]
async fn duplicate_delivery_returns_result_without_repeating_side_effects() {
    let sunshine = FakeSunshine::default();
    let executor = Executor::new(binding(), true, sunshine.clone(), MemoryJournal::default());
    let task = patch();
    let first = executor.deliver(&task, DeliveryMode::Execute).await;
    assert_eq!(first, executor.deliver(&task, DeliveryMode::Execute).await);
    assert_eq!(
        first,
        executor.deliver(&task, DeliveryMode::InspectOnly).await
    );
    assert_eq!(sunshine.0.lock().unwrap().saves, 1);
}

#[tokio::test]
async fn removal_only_deletes_the_requested_managed_field() {
    let sunshine = FakeSunshine::default();
    let executor = Executor::new(binding(), true, sunshine.clone(), MemoryJournal::default());
    let mut task = patch();
    let Command::PatchConfig { set, remove, .. } = &mut task.command else {
        unreachable!()
    };
    set.clear();
    remove.insert("qp".into());
    let report = executor.deliver(&task, DeliveryMode::Execute).await;
    let Report::ConfigSaved { snapshot } = report else {
        panic!("{report:?}")
    };
    assert!(!snapshot.fields.contains_key("qp"));
    let expected = Configuration::from_response(json!({
        "status": true, "platform": "linux", "version": SUNSHINE_VERSION,
        "sunshine_name": "主机", "global_prep_cmd": "[{\"do\":\"local-only\"}]",
        "file_apps": "/private/apps.json", "unmanaged_option": "leave-me-alone"
    }))
    .unwrap();
    assert_eq!(snapshot.revision, expected.revision());
    assert_eq!(sunshine.0.lock().unwrap().saves, 1);
}

#[tokio::test]
async fn an_uncertain_save_is_not_retried_even_when_old_configuration_is_still_present() {
    let sunshine = FakeSunshine::default();
    let journal = MemoryJournal::default();
    let task = patch();
    journal.0.lock().unwrap().values.insert(
        task.operation_id.clone(),
        ExecutionRecord {
            fingerprint: task.fingerprint().unwrap(),
            effect: Some(EffectIntent::Save {
                target_revision: configuration("29").revision(),
            }),
            report: None,
        },
    );
    let executor = Executor::new(binding(), true, sunshine.clone(), journal);
    for mode in [DeliveryMode::Execute, DeliveryMode::InspectOnly] {
        assert_eq!(
            executor.deliver(&task, mode).await,
            Report::Unknown {
                reason: Uncertainty::EffectNotConfirmed
            }
        );
    }
    assert_eq!(sunshine.0.lock().unwrap().saves, 0);
}

#[tokio::test]
async fn reused_operation_id_with_changed_content_is_rejected() {
    let executor = Executor::new(
        binding(),
        true,
        FakeSunshine::default(),
        MemoryJournal::default(),
    );
    let task = patch();
    executor.deliver(&task, DeliveryMode::Execute).await;
    let mut changed = task;
    changed.command = Command::ReadConfig {};
    changed.permission = Permission::ReadConfig;
    assert_eq!(
        executor.deliver(&changed, DeliveryMode::Execute).await,
        Report::Rejected {
            reason: Rejection::OperationIdReused
        }
    );
}

#[tokio::test]
async fn simultaneous_modifications_with_same_revision_have_one_winner() {
    let sunshine = FakeSunshine::default();
    let executor = Executor::new(binding(), true, sunshine.clone(), MemoryJournal::default());
    let a = patch();
    let b = patch();
    let (a, b) = tokio::join!(
        executor.deliver(&a, DeliveryMode::Execute),
        executor.deliver(&b, DeliveryMode::Execute)
    );
    assert!(matches!(a, Report::ConfigSaved { .. }));
    assert!(matches!(b, Report::Conflict { .. }));
    assert_eq!(sunshine.0.lock().unwrap().saves, 1);
}

#[tokio::test]
async fn conflict_on_pre_effect_read_never_writes() {
    let sunshine = FakeSunshine::default();
    sunshine.0.lock().unwrap().conflict_on_second_read = true;
    let executor = Executor::new(binding(), true, sunshine.clone(), MemoryJournal::default());
    assert!(matches!(
        executor.deliver(&patch(), DeliveryMode::Execute).await,
        Report::Conflict { .. }
    ));
    assert_eq!(sunshine.0.lock().unwrap().saves, 0);
}

#[tokio::test]
async fn lost_save_receipt_is_verified_by_full_readback() {
    let sunshine = FakeSunshine::default();
    sunshine.0.lock().unwrap().lose_write_receipt = true;
    let executor = Executor::new(binding(), true, sunshine.clone(), MemoryJournal::default());
    assert!(matches!(
        executor.deliver(&patch(), DeliveryMode::Execute).await,
        Report::ConfigSaved { .. }
    ));
    assert_eq!(sunshine.0.lock().unwrap().saves, 1);
}

#[tokio::test]
async fn crash_after_save_before_result_persistence_reconciles_without_reexecution() {
    let sunshine = FakeSunshine::default();
    let journal = MemoryJournal::default();
    journal.0.lock().unwrap().fail_write = Some(3);
    let task = patch();
    let executor = Executor::new(binding(), true, sunshine.clone(), journal.clone());
    assert_eq!(
        executor.deliver(&task, DeliveryMode::Execute).await,
        Report::Unknown {
            reason: Uncertainty::PersistenceFailure
        }
    );
    drop(executor);
    let recovered = Executor::new(binding(), true, sunshine.clone(), journal);
    assert!(matches!(
        recovered.deliver(&task, DeliveryMode::InspectOnly).await,
        Report::ConfigSaved { .. }
    ));
    assert_eq!(sunshine.0.lock().unwrap().saves, 1);
}

#[tokio::test]
async fn persistence_failure_before_effect_fails_closed() {
    for fail_write in [1, 2] {
        let sunshine = FakeSunshine::default();
        let journal = MemoryJournal::default();
        journal.0.lock().unwrap().fail_write = Some(fail_write);
        let executor = Executor::new(binding(), true, sunshine.clone(), journal);
        assert!(matches!(
            executor.deliver(&patch(), DeliveryMode::Execute).await,
            Report::Unknown { .. }
        ));
        assert_eq!(sunshine.0.lock().unwrap().saves, 0);
    }
}

#[tokio::test]
async fn lost_restart_receipt_never_triggers_another_restart() {
    let sunshine = FakeSunshine::default();
    sunshine.0.lock().unwrap().lose_restart_receipt = true;
    let journal = MemoryJournal::default();
    let task = restart();
    let executor = Executor::new(binding(), true, sunshine.clone(), journal.clone());
    assert_eq!(
        executor.deliver(&task, DeliveryMode::Execute).await,
        Report::Unknown {
            reason: Uncertainty::RestartNotConfirmed
        }
    );
    drop(executor);
    let recovered = Executor::new(binding(), true, sunshine.clone(), journal);
    assert_eq!(
        recovered.deliver(&task, DeliveryMode::Execute).await,
        Report::Unknown {
            reason: Uncertainty::RestartNotConfirmed
        }
    );
    assert_eq!(sunshine.0.lock().unwrap().restarts, 1);
}

#[tokio::test]
async fn restart_receipt_and_reachability_are_not_runtime_effectiveness() {
    let executor = Executor::new(
        binding(),
        true,
        FakeSunshine::default(),
        MemoryJournal::default(),
    );
    let Report::RestartAcknowledged { snapshot } =
        executor.deliver(&restart(), DeliveryMode::Execute).await
    else {
        panic!("expected acknowledgement")
    };
    assert_eq!(snapshot.effectiveness, Effectiveness::PendingVerification);
}

#[tokio::test]
async fn inspect_unknown_operation_is_read_only_and_does_not_guess() {
    let sunshine = FakeSunshine::default();
    let executor = Executor::new(binding(), true, sunshine.clone(), MemoryJournal::default());
    assert_eq!(
        executor
            .deliver(&restart(), DeliveryMode::InspectOnly)
            .await,
        Report::Unknown {
            reason: Uncertainty::NoExecutionRecord
        }
    );
    assert_eq!(sunshine.0.lock().unwrap().restarts, 0);
}

#[tokio::test]
async fn journal_capacity_exhaustion_rejects_instead_of_evicting_deduplication() {
    let sunshine = FakeSunshine::default();
    let journal = MemoryJournal::default();
    journal.0.lock().unwrap().full = true;
    let executor = Executor::new(binding(), true, sunshine.clone(), journal);
    assert_eq!(
        executor.deliver(&patch(), DeliveryMode::Execute).await,
        Report::Rejected {
            reason: Rejection::JournalFull
        }
    );
    assert_eq!(sunshine.0.lock().unwrap().reads, 0);
}

#[test]
fn configuration_metadata_and_unsupported_versions_are_not_saved() {
    assert_eq!(
        Configuration::from_response(
            json!({"status": true, "platform": "linux", "version": "master"})
        )
        .err(),
        Some(AdapterError::UnsupportedVersion)
    );
    assert_eq!(Configuration::from_response(json!({"status": true, "platform": "linux", "version": SUNSHINE_VERSION, "status_code": 200})).err(), Some(AdapterError::UnsafeConfiguration));
    let empty = Configuration::from_response(
        json!({"status": true, "platform": "linux", "version": SUNSHINE_VERSION}),
    )
    .unwrap();
    assert!(
        empty
            .snapshot(Effectiveness::PendingVerification)
            .fields
            .is_empty()
    );
}

#[test]
fn endpoint_policy_rejects_remote_hosts_plaintext_credentials_and_paths() {
    use sarmg_client_secure_http::Url;
    use sunshine_client::adapter::validate_local_endpoint;
    for url in [
        "http://127.0.0.1:47990",
        "https://example.com",
        "https://192.168.1.2",
        "https://localhost",
        "https://user:secret@127.0.0.1",
        "https://127.0.0.1/api",
        "https://127.0.0.1/?query",
        "https://127.0.0.1/#fragment",
    ] {
        assert!(
            validate_local_endpoint(&Url::parse(url).unwrap()).is_err(),
            "{url}"
        );
    }
    for url in ["https://127.0.0.1:47990", "https://[::1]:47990"] {
        assert!(validate_local_endpoint(&Url::parse(url).unwrap()).is_ok());
    }
}
