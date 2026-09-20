use serde::{Deserialize, Serialize};
use sunshine_client_protocol::{
    Binding, Capabilities, ClientOs, Command, DeliveryMode, Effectiveness, Rejection, Report,
    ServiceAction, ServiceState, Task, Uncertainty,
};
use tokio::sync::Mutex;

use crate::adapter::{AdapterError, Sunshine};

/// Minimum execution facts, not a competing task state machine. Contains no credentials/full config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRecord {
    pub fingerprint: String,
    pub effect: Option<EffectIntent>,
    pub report: Option<Report>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EffectIntent {
    Save {
        target_revision: String,
    },
    Restart,
    SaveApplication {
        previous: Option<String>,
        target: String,
    },
    DeleteApplication {
        target: String,
    },
    SetPairedClientEnabled {
        uuid: String,
        enabled: bool,
    },
    UnpairClient {
        uuid: String,
    },
    UnpairAllClients,
    ControlService {
        action: ServiceAction,
        expected: ServiceState,
    },
    Unobservable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum JournalError {
    #[error("execution journal is full")]
    Full,
    #[error("execution journal unavailable")]
    Storage,
}

/// Implementations must durably commit before returning. Never expire/evict operation IDs.
/// An exclusive process lock and a protected native state directory are prerequisites.
pub trait Journal: Send {
    fn load(&mut self, operation_id: &str) -> Result<Option<ExecutionRecord>, JournalError>;
    fn create(&mut self, operation_id: &str, record: &ExecutionRecord) -> Result<(), JournalError>;
    fn replace(&mut self, operation_id: &str, record: &ExecutionRecord)
    -> Result<(), JournalError>;
}

pub struct Executor<A, J> {
    binding: Binding,
    capabilities: Capabilities,
    inner: Mutex<Inner<A, J>>,
}

struct Inner<A, J> {
    sunshine: A,
    journal: J,
}

impl<A: Sunshine, J: Journal> Executor<A, J> {
    pub fn new(binding: Binding, sunshine: A, journal: J) -> Self {
        let capabilities = Capabilities {
            protocol: sunshine_client_protocol::PROTOCOL.into(),
            client_version: env!("CARGO_PKG_VERSION").into(),
            os: ClientOs::LinuxX86_64,
            sunshine_version: sunshine_client_protocol::SUNSHINE_VERSION.into(),
            restart_allowed: true,
            managed_fields: sunshine_client_protocol::config::FIELD_DEFINITIONS
                .iter()
                .map(|field| field.key.to_owned())
                .collect(),
            application_management: true,
            application_host_commands_allowed: true,
            moonlight_pairing_management: true,
            diagnostics: true,
            maintenance: true,
            service_control: false,
        };
        Self::new_with_capabilities(binding, capabilities, sunshine, journal)
    }
    pub fn new_with_capabilities(
        binding: Binding,
        capabilities: Capabilities,
        sunshine: A,
        journal: J,
    ) -> Self {
        Self {
            binding,
            capabilities,
            inner: Mutex::new(Inner { sunshine, journal }),
        }
    }

    /// One host, one serial execution lane. Heartbeats belong to the independent transport task.
    pub async fn deliver(&self, task: &Task, mode: DeliveryMode) -> Report {
        if let Err(reason) = task.validate(&self.binding, &self.capabilities) {
            return Report::Rejected { reason };
        }
        let fingerprint = match task.fingerprint() {
            Ok(fingerprint) => fingerprint,
            Err(_) => {
                return Report::Rejected {
                    reason: Rejection::InvalidTask,
                };
            }
        };
        let mut inner = self.inner.lock().await;
        let prior = match inner.journal.load(&task.operation_id) {
            Ok(prior) => prior,
            Err(_) => return persistence_failure(),
        };
        if let Some(mut record) = prior {
            if record.fingerprint != fingerprint {
                return Report::Rejected {
                    reason: Rejection::OperationIdReused,
                };
            }
            if let Some(report) = &record.report
                && !matches!(report, Report::Unknown { .. })
            {
                return report.clone();
            }
            // A persisted intent is an ambiguous point of no return, never permission to retry.
            if record.effect.is_some() {
                let report = inner.reconcile(&record).await;
                return inner.finish(&task.operation_id, &mut record, report);
            }
            if let Some(report) = &record.report {
                return report.clone();
            }
            if mode == DeliveryMode::InspectOnly {
                return Report::Unknown {
                    reason: Uncertainty::NoExecutionRecord,
                };
            }
            return inner.execute(task, &mut record).await;
        }
        if mode == DeliveryMode::InspectOnly {
            return Report::Unknown {
                reason: Uncertainty::NoExecutionRecord,
            };
        }
        let mut record = ExecutionRecord {
            fingerprint,
            effect: None,
            report: None,
        };
        match inner.journal.create(&task.operation_id, &record) {
            Ok(()) => inner.execute(task, &mut record).await,
            Err(JournalError::Full) => Report::Rejected {
                reason: Rejection::JournalFull,
            },
            Err(JournalError::Storage) => persistence_failure(),
        }
    }
}

impl<A: Sunshine, J: Journal> Inner<A, J> {
    async fn execute(&mut self, task: &Task, record: &mut ExecutionRecord) -> Report {
        match &task.command {
            Command::ReadConfig {} => match self.sunshine.read().await {
                Ok(configuration) => self.finish(
                    &task.operation_id,
                    record,
                    Report::ConfigRead {
                        snapshot: configuration.snapshot(Effectiveness::PendingVerification),
                    },
                ),
                Err(error) => self.finish(&task.operation_id, record, rejected_adapter(error)),
            },
            Command::PatchConfig { .. } | Command::Restart { .. } => {
                self.execute_config(task, record).await
            }
            Command::ListApplications {} => match self.sunshine.applications().await {
                Ok(snapshot) => self.finish(
                    &task.operation_id,
                    record,
                    Report::ApplicationsRead { snapshot },
                ),
                Err(error) => self.finish(&task.operation_id, record, rejected_adapter(error)),
            },
            Command::SaveApplication {
                expected_revision,
                target,
                application,
                ..
            } => {
                let current = match self.sunshine.applications().await {
                    Ok(snapshot) => snapshot,
                    Err(error) => {
                        return self.finish(&task.operation_id, record, rejected_adapter(error));
                    }
                };
                if current.revision != *expected_revision
                    || target.as_ref().is_some_and(|target| {
                        current
                            .applications
                            .iter()
                            .filter(|app| app.reference == *target)
                            .count()
                            != 1
                    })
                {
                    return self.finish(
                        &task.operation_id,
                        record,
                        Report::Conflict {
                            actual_revision: current.revision,
                        },
                    );
                }
                let target_reference =
                    match sunshine_client_protocol::application_reference(application) {
                        Ok(value) => value.fingerprint,
                        Err(_) => {
                            return self.finish(
                                &task.operation_id,
                                record,
                                Report::Rejected {
                                    reason: Rejection::InvalidTask,
                                },
                            );
                        }
                    };
                if !self.remember(
                    &task.operation_id,
                    record,
                    EffectIntent::SaveApplication {
                        previous: target.as_ref().map(|value| value.fingerprint.clone()),
                        target: target_reference,
                    },
                ) {
                    return persistence_failure();
                }
                let _ = self
                    .sunshine
                    .save_application(expected_revision, target.as_ref(), application)
                    .await;
                let report = self.reconcile(record).await;
                self.finish(&task.operation_id, record, report)
            }
            Command::DeleteApplication {
                expected_revision,
                target,
                ..
            } => {
                let current = match self.sunshine.applications().await {
                    Ok(snapshot) => snapshot,
                    Err(error) => {
                        return self.finish(&task.operation_id, record, rejected_adapter(error));
                    }
                };
                if current.revision != *expected_revision
                    || current
                        .applications
                        .iter()
                        .filter(|app| app.reference == *target)
                        .count()
                        != 1
                {
                    return self.finish(
                        &task.operation_id,
                        record,
                        Report::Conflict {
                            actual_revision: current.revision,
                        },
                    );
                }
                if !self.remember(
                    &task.operation_id,
                    record,
                    EffectIntent::DeleteApplication {
                        target: target.fingerprint.clone(),
                    },
                ) {
                    return persistence_failure();
                }
                let _ = self
                    .sunshine
                    .delete_application(expected_revision, target)
                    .await;
                let report = self.reconcile(record).await;
                self.finish(&task.operation_id, record, report)
            }
            Command::CloseApplication { .. } => {
                if !self.remember(&task.operation_id, record, EffectIntent::Unobservable) {
                    return persistence_failure();
                }
                let report = match self.sunshine.close_application().await {
                    Ok(()) => Report::ApplicationClosed {},
                    Err(_) => Report::Unknown {
                        reason: Uncertainty::SideEffectNotConfirmed,
                    },
                };
                self.finish(&task.operation_id, record, report)
            }
            Command::UploadCover {
                key, png_base64, ..
            } => {
                use base64::Engine as _;
                let png = match base64::engine::general_purpose::STANDARD.decode(png_base64) {
                    Ok(value) => value,
                    Err(_) => {
                        return self.finish(
                            &task.operation_id,
                            record,
                            Report::Rejected {
                                reason: Rejection::InvalidTask,
                            },
                        );
                    }
                };
                if !self.remember(&task.operation_id, record, EffectIntent::Unobservable) {
                    return persistence_failure();
                }
                let report = match self.sunshine.upload_cover(key, &png).await {
                    Ok(path) => Report::CoverUploaded { path },
                    Err(_) => Report::Unknown {
                        reason: Uncertainty::SideEffectNotConfirmed,
                    },
                };
                self.finish(&task.operation_id, record, report)
            }
            Command::SubmitPairingPin {
                pairing_id,
                pin,
                name,
            } => {
                if !self.remember(&task.operation_id, record, EffectIntent::Unobservable) {
                    return persistence_failure();
                }
                let report = match self
                    .sunshine
                    .submit_pairing_pin(pairing_id, pin, name)
                    .await
                {
                    Ok(()) => Report::PairingPinSubmitted {},
                    Err(_) => Report::Unknown {
                        reason: Uncertainty::SideEffectNotConfirmed,
                    },
                };
                self.finish(&task.operation_id, record, report)
            }
            Command::ListPairedClients {} => match self.sunshine.paired_clients().await {
                Ok(snapshot) => self.finish(
                    &task.operation_id,
                    record,
                    Report::PairedClientsRead { snapshot },
                ),
                Err(error) => self.finish(&task.operation_id, record, rejected_adapter(error)),
            },
            Command::SetPairedClientEnabled { uuid, enabled, .. } => {
                if !self.remember(
                    &task.operation_id,
                    record,
                    EffectIntent::SetPairedClientEnabled {
                        uuid: uuid.clone(),
                        enabled: *enabled,
                    },
                ) {
                    return persistence_failure();
                }
                let _ = self
                    .sunshine
                    .set_paired_client_enabled(uuid, *enabled)
                    .await;
                let report = self.reconcile(record).await;
                self.finish(&task.operation_id, record, report)
            }
            Command::UnpairClient { uuid, .. } => {
                if !self.remember(
                    &task.operation_id,
                    record,
                    EffectIntent::UnpairClient { uuid: uuid.clone() },
                ) {
                    return persistence_failure();
                }
                let _ = self.sunshine.unpair_client(uuid).await;
                let report = self.reconcile(record).await;
                self.finish(&task.operation_id, record, report)
            }
            Command::UnpairAllClients { .. } => {
                if !self.remember(&task.operation_id, record, EffectIntent::UnpairAllClients) {
                    return persistence_failure();
                }
                let _ = self.sunshine.unpair_all_clients().await;
                let report = self.reconcile(record).await;
                self.finish(&task.operation_id, record, report)
            }
            Command::ReadLogs {
                cursor,
                limit_bytes,
            } => match self.sunshine.logs(cursor.as_ref(), *limit_bytes).await {
                Ok(page) => self.finish(&task.operation_id, record, Report::LogsRead { page }),
                Err(error) => self.finish(&task.operation_id, record, rejected_adapter(error)),
            },
            Command::ReadDiagnostics {} => {
                let service_state = self
                    .sunshine
                    .service_status()
                    .await
                    .unwrap_or(ServiceState::Unknown);
                let (
                    sunshine_version,
                    platform,
                    api_reachable,
                    authentication_accepted,
                    configuration_revision,
                ) = match self.sunshine.read().await {
                    Ok(config) => (
                        Some(config.sunshine_version().to_owned()),
                        Some(config.platform().to_owned()),
                        true,
                        true,
                        Some(config.revision()),
                    ),
                    Err(AdapterError::CredentialsRejected) => (None, None, true, false, None),
                    Err(_) => (None, None, false, false, None),
                };
                self.finish(
                    &task.operation_id,
                    record,
                    Report::DiagnosticsRead {
                        snapshot: sunshine_client_protocol::DiagnosticSnapshot {
                            sunshine_version,
                            platform,
                            api_reachable,
                            authentication_accepted,
                            service_state,
                            configuration_revision,
                        },
                    },
                )
            }
            Command::ReadVirtualInputStatus {} => {
                match self.sunshine.virtual_input_status().await {
                    Ok(status) => self.finish(
                        &task.operation_id,
                        record,
                        Report::VirtualInputStatusRead { status },
                    ),
                    Err(error) => self.finish(&task.operation_id, record, rejected_adapter(error)),
                }
            }
            Command::RunMaintenance { action, .. } => {
                if !self.remember(&task.operation_id, record, EffectIntent::Unobservable) {
                    return persistence_failure();
                }
                let report = match self.sunshine.maintenance(*action).await {
                    Ok(()) => Report::MaintenanceCompleted { action: *action },
                    Err(_) => Report::Unknown {
                        reason: Uncertainty::SideEffectNotConfirmed,
                    },
                };
                self.finish(&task.operation_id, record, report)
            }
            Command::ReadServiceStatus {} => match self.sunshine.service_status().await {
                Ok(state) => self.finish(
                    &task.operation_id,
                    record,
                    Report::ServiceStatusRead { state },
                ),
                Err(error) => self.finish(&task.operation_id, record, rejected_adapter(error)),
            },
            Command::ControlService { action, .. } => {
                let expected = match action {
                    ServiceAction::Stop => ServiceState::Stopped,
                    _ => ServiceState::Running,
                };
                if !self.remember(
                    &task.operation_id,
                    record,
                    EffectIntent::ControlService {
                        action: *action,
                        expected,
                    },
                ) {
                    return persistence_failure();
                }
                let _ = self.sunshine.control_service(*action).await;
                let report = self.reconcile(record).await;
                self.finish(&task.operation_id, record, report)
            }
        }
    }

    async fn execute_config(&mut self, task: &Task, record: &mut ExecutionRecord) -> Report {
        let configuration = match self.sunshine.read().await {
            Ok(configuration) => configuration,
            Err(error) => return self.finish(&task.operation_id, record, rejected_adapter(error)),
        };
        let expected = match &task.command {
            Command::PatchConfig {
                expected_revision, ..
            }
            | Command::Restart {
                expected_revision, ..
            } => expected_revision,
            _ => {
                return self.finish(
                    &task.operation_id,
                    record,
                    Report::Rejected {
                        reason: Rejection::InvalidTask,
                    },
                );
            }
        };
        if configuration.revision() != *expected {
            return self.finish(
                &task.operation_id,
                record,
                Report::Conflict {
                    actual_revision: configuration.revision(),
                },
            );
        }
        let merged = match &task.command {
            Command::PatchConfig { set, remove, .. } => match configuration.merge(set, remove) {
                Ok(merged) => Some(merged),
                Err(error) => {
                    return self.finish(&task.operation_id, record, rejected_adapter(error));
                }
            },
            _ => None,
        };
        // Reduce (not eliminate) the local-editor race. Sunshine has no native atomic compare-and-set.
        match self.sunshine.read().await {
            Ok(current) if current.revision() == *expected => {}
            Ok(current) => {
                return self.finish(
                    &task.operation_id,
                    record,
                    Report::Conflict {
                        actual_revision: current.revision(),
                    },
                );
            }
            Err(error) => return self.finish(&task.operation_id, record, rejected_adapter(error)),
        }
        record.effect = Some(match &merged {
            Some(merged) => EffectIntent::Save {
                target_revision: merged.revision(),
            },
            None => EffectIntent::Restart,
        });
        if self.journal.replace(&task.operation_id, record).is_err() {
            return persistence_failure();
        }
        let report = match merged {
            Some(merged) => {
                // Even an HTTP error can occur after a successful write. Reconcile, don't retry.
                let _ = self.sunshine.save(&merged).await;
                self.reconcile(record).await
            }
            None => {
                if self.sunshine.restart().await.is_ok() {
                    // Reaching the API after restart is necessary, but never runtime-effect proof.
                    let mut snapshot = None;
                    for attempt in 0..5 {
                        if attempt > 0 {
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        }
                        if let Ok(current) = self.sunshine.read().await
                            && current.revision() == *expected
                        {
                            snapshot = Some(current.snapshot(Effectiveness::PendingVerification));
                            break;
                        }
                    }
                    match snapshot {
                        Some(snapshot) => Report::RestartAcknowledged { snapshot },
                        None => Report::Unknown {
                            reason: Uncertainty::RestartNotConfirmed,
                        },
                    }
                } else {
                    Report::Unknown {
                        reason: Uncertainty::RestartNotConfirmed,
                    }
                }
            }
        };
        self.finish(&task.operation_id, record, report)
    }

    async fn reconcile(&mut self, record: &ExecutionRecord) -> Report {
        match &record.effect {
            Some(EffectIntent::Save { target_revision }) => match self.sunshine.read().await {
                Ok(current) if current.revision() == *target_revision => Report::ConfigSaved {
                    snapshot: current.snapshot(Effectiveness::AwaitingRestart),
                },
                _ => Report::Unknown {
                    reason: Uncertainty::EffectNotConfirmed,
                },
            },
            // A matching file or an online Sunshine cannot prove that a restart happened.
            Some(EffectIntent::Restart) => Report::Unknown {
                reason: Uncertainty::RestartNotConfirmed,
            },
            Some(EffectIntent::SaveApplication { previous, target }) => {
                match self.sunshine.applications().await {
                    Ok(snapshot)
                        if snapshot
                            .applications
                            .iter()
                            .filter(|app| app.reference.fingerprint == *target)
                            .count()
                            == 1
                            && previous.as_ref().is_none_or(|old| {
                                old == target
                                    || !snapshot
                                        .applications
                                        .iter()
                                        .any(|app| app.reference.fingerprint == *old)
                            }) =>
                    {
                        Report::ApplicationSaved { snapshot }
                    }
                    _ => Report::Unknown {
                        reason: Uncertainty::EffectNotConfirmed,
                    },
                }
            }
            Some(EffectIntent::DeleteApplication { target }) => {
                match self.sunshine.applications().await {
                    Ok(snapshot)
                        if !snapshot
                            .applications
                            .iter()
                            .any(|app| app.reference.fingerprint == *target) =>
                    {
                        Report::ApplicationDeleted { snapshot }
                    }
                    _ => Report::Unknown {
                        reason: Uncertainty::EffectNotConfirmed,
                    },
                }
            }
            Some(EffectIntent::SetPairedClientEnabled { uuid, enabled }) => {
                match self.sunshine.paired_clients().await {
                    Ok(snapshot)
                        if snapshot
                            .clients
                            .iter()
                            .any(|client| client.uuid == *uuid && client.enabled == *enabled) =>
                    {
                        Report::PairedClientUpdated { snapshot }
                    }
                    _ => Report::Unknown {
                        reason: Uncertainty::EffectNotConfirmed,
                    },
                }
            }
            Some(EffectIntent::UnpairClient { uuid }) => match self.sunshine.paired_clients().await
            {
                Ok(snapshot) if !snapshot.clients.iter().any(|client| client.uuid == *uuid) => {
                    Report::PairedClientUpdated { snapshot }
                }
                _ => Report::Unknown {
                    reason: Uncertainty::EffectNotConfirmed,
                },
            },
            Some(EffectIntent::UnpairAllClients) => match self.sunshine.paired_clients().await {
                Ok(snapshot) if snapshot.clients.is_empty() => {
                    Report::PairedClientUpdated { snapshot }
                }
                _ => Report::Unknown {
                    reason: Uncertainty::EffectNotConfirmed,
                },
            },
            Some(EffectIntent::ControlService { action, expected }) => {
                match self.sunshine.service_status().await {
                    Ok(state) if state == *expected => Report::ServiceControlled {
                        action: *action,
                        state,
                    },
                    _ => Report::Unknown {
                        reason: Uncertainty::ServiceTransitionNotConfirmed,
                    },
                }
            }
            Some(EffectIntent::Unobservable) => Report::Unknown {
                reason: Uncertainty::SideEffectNotConfirmed,
            },
            None => Report::Unknown {
                reason: Uncertainty::NoExecutionRecord,
            },
        }
    }

    fn remember(&mut self, id: &str, record: &mut ExecutionRecord, effect: EffectIntent) -> bool {
        record.effect = Some(effect);
        self.journal.replace(id, record).is_ok()
    }

    fn finish(&mut self, id: &str, record: &mut ExecutionRecord, report: Report) -> Report {
        record.report = Some(report.clone());
        if self.journal.replace(id, record).is_err() {
            return persistence_failure();
        }
        report
    }
}

fn persistence_failure() -> Report {
    Report::Unknown {
        reason: Uncertainty::PersistenceFailure,
    }
}

fn rejected_adapter(error: AdapterError) -> Report {
    Report::Rejected {
        reason: match error {
            AdapterError::UnsupportedVersion => Rejection::UnsupportedVersion,
            AdapterError::UnsafeConfiguration => Rejection::UnsafeConfiguration,
            AdapterError::ResourceConflict => Rejection::ResourceConflict,
            AdapterError::UnsupportedCapability => Rejection::UnsupportedCapability,
            AdapterError::CredentialsRejected
            | AdapterError::ApiUnavailable
            | AdapterError::InvalidLocalEndpoint => Rejection::SunshineUnavailable,
        },
    }
}
