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
    #[serde(default)]
    pub acknowledged: bool,
    #[serde(default)]
    pub binding: Option<Binding>,
    #[serde(default)]
    pub accepted_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EffectIntent {
    Save {
        target_revision: String,
    },
    Restart,
    RestartObserved {
        before: String,
        target_revision: String,
    },
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
        #[serde(default)]
        before: Option<String>,
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
pub const MAX_RECORD_BYTES: usize = sunshine_client_protocol::MAX_MESSAGE_BYTES + 4096;

#[derive(Clone, Debug)]
pub struct ExecutionDeadline {
    expires_at: std::time::SystemTime,
    received_until: tokio::time::Instant,
    elapsed_received: Option<u64>,
    remaining_ms: u64,
}
impl ExecutionDeadline {
    pub fn received(expires_at_unix_ms: u64, remaining_ms: u64) -> Self {
        let remaining_ms = remaining_ms.min(sunshine_client_protocol::MAX_EXECUTION_SECONDS * 1000);
        Self {
            expires_at: std::time::UNIX_EPOCH
                .checked_add(std::time::Duration::from_millis(expires_at_unix_ms))
                .unwrap_or(std::time::UNIX_EPOCH),
            elapsed_received: crate::elapsed_clock::milliseconds(),
            remaining_ms,
            received_until: tokio::time::Instant::now()
                + std::time::Duration::from_millis(remaining_ms),
        }
    }
    pub(crate) fn expired(&self) -> bool {
        self.expired_with_elapsed_clock(crate::elapsed_clock::milliseconds())
    }
    fn expired_with_elapsed_clock(&self, elapsed_now: Option<u64>) -> bool {
        std::time::SystemTime::now() >= self.expires_at
            || tokio::time::Instant::now() >= self.received_until
            || match (self.elapsed_received, elapsed_now) {
                (Some(start), Some(now)) => now < start || now - start >= self.remaining_ms,
                _ => true,
            }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod deadline_tests {
    use super::ExecutionDeadline;

    #[test]
    fn suspend_or_clock_regression_cannot_extend_the_execution_budget() {
        let future = (std::time::SystemTime::now() + std::time::Duration::from_secs(600))
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let mut deadline = ExecutionDeadline::received(future, 900_000);
        assert!(!deadline.expired());
        deadline.elapsed_received = Some(1000);
        assert!(!deadline.expired_with_elapsed_clock(Some(90_000)));
        assert!(
            deadline.expired_with_elapsed_clock(Some(91_001)),
            "sleep must count even while active time has not advanced"
        );
        assert!(
            deadline.expired_with_elapsed_clock(Some(999)),
            "a clock regression must fail closed"
        );
        assert!(
            deadline.expired_with_elapsed_clock(None),
            "a missing elapsed clock must fail closed"
        );
    }
}

pub trait Journal: Send {
    fn pending_results(&mut self) -> Result<Vec<(String, ExecutionRecord)>, JournalError> {
        Ok(Vec::new())
    }
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
    deadline: Option<ExecutionDeadline>,
    volatile_read: bool,
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
            inner: Mutex::new(Inner {
                sunshine,
                journal,
                deadline: None,
                volatile_read: false,
            }),
        }
    }

    /// One host, one serial execution lane. Heartbeats belong to the independent transport task.
    pub async fn deliver(&self, task: &Task, mode: DeliveryMode) -> Report {
        self.deliver_before(task, mode, None).await
    }

    pub async fn pending_results(&self) -> Result<Vec<(String, ExecutionRecord)>, JournalError> {
        self.inner
            .lock()
            .await
            .journal
            .pending_results()
            .map(|records| {
                records
                    .into_iter()
                    .filter(|(_, record)| record.binding.as_ref() == Some(&self.binding))
                    .collect()
            })
    }

    pub async fn acknowledge(
        &self,
        id: &str,
        fingerprint: &str,
        digest: &str,
    ) -> Result<(), JournalError> {
        self.acknowledge_final(id, fingerprint, digest, false).await
    }
    pub async fn acknowledge_final(
        &self,
        id: &str,
        fingerprint: &str,
        digest: &str,
        finalized: bool,
    ) -> Result<(), JournalError> {
        let mut inner = self.inner.lock().await;
        let Some(mut record) = inner.journal.load(id)? else {
            return Ok(());
        };
        if record.acknowledged {
            return Ok(());
        }
        if record.fingerprint != fingerprint {
            return Err(JournalError::Storage);
        }
        if record
            .report
            .as_ref()
            .and_then(|report| sunshine_client_protocol::report_digest(report).ok())
            .as_deref()
            != Some(digest)
        {
            // A receipt for an older uncertain observation can race a new inspection result.
            return if record.accepted_digest.as_deref() == Some(digest) {
                Ok(())
            } else {
                Err(JournalError::Storage)
            };
        }
        // Keep uncertain intent for inspect-only reconciliation; compact only final results.
        if finalized || !matches!(record.report, Some(Report::Unknown { .. })) {
            record.acknowledged = true;
            record.report = None;
            record.effect = None;
        }
        record.accepted_digest = Some(digest.to_owned());
        inner.journal.replace(id, &record)?;
        Ok(())
    }

    pub async fn deliver_before(
        &self,
        task: &Task,
        mode: DeliveryMode,
        deadline: Option<ExecutionDeadline>,
    ) -> Report {
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
        inner.sunshine.set_execution_deadline(deadline.clone());
        inner.deadline = deadline;
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
            if record.acknowledged {
                return Report::Rejected {
                    reason: Rejection::OperationAlreadyCompleted,
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
            return inner.execute_bounded(task, &mut record).await;
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
            acknowledged: false,
            binding: Some(self.binding.clone()),
            accepted_digest: None,
        };
        match inner.journal.create(&task.operation_id, &record) {
            Ok(()) => inner.execute_bounded(task, &mut record).await,
            Err(JournalError::Full)
                if matches!(
                    task.command,
                    Command::ReadConfig {}
                        | Command::ListApplications {}
                        | Command::ListPendingPairings {}
                        | Command::ListPairedClients {}
                        | Command::ReadLogs { .. }
                        | Command::ReadDiagnostics {}
                        | Command::ReadVirtualInputStatus {}
                        | Command::ReadServiceStatus {}
                ) =>
            {
                // Reads are repeatable and do not consume permanent side-effect identities.
                inner.volatile_read = true;
                let report = inner.execute_bounded(task, &mut record).await;
                inner.volatile_read = false;
                report
            }
            Err(JournalError::Full) => Report::Rejected {
                reason: Rejection::JournalFull,
            },
            Err(JournalError::Storage) => persistence_failure(),
        }
    }
}

impl<A: Sunshine, J: Journal> Inner<A, J> {
    async fn execute_bounded(&mut self, task: &Task, record: &mut ExecutionRecord) -> Report {
        if self
            .deadline
            .as_ref()
            .is_some_and(ExecutionDeadline::expired)
        {
            return self.finish(
                &task.operation_id,
                record,
                Report::Rejected {
                    reason: Rejection::ExecutionExpired,
                },
            );
        }
        match tokio::time::timeout(
            std::time::Duration::from_secs(sunshine_client_protocol::MAX_EXECUTION_SECONDS),
            self.execute(task, record),
        )
        .await
        {
            Ok(report) => {
                if record.effect.is_none()
                    && self
                        .deadline
                        .as_ref()
                        .is_some_and(ExecutionDeadline::expired)
                    && matches!(report, Report::Unknown { .. })
                {
                    return self.finish(
                        &task.operation_id,
                        record,
                        Report::Rejected {
                            reason: Rejection::ExecutionExpired,
                        },
                    );
                }
                report
            }
            Err(_) => {
                let report = if record.effect.is_some() {
                    Report::Unknown {
                        reason: Uncertainty::EffectNotConfirmed,
                    }
                } else {
                    Report::Rejected {
                        reason: Rejection::SunshineUnavailable,
                    }
                };
                self.finish(&task.operation_id, record, report)
            }
        }
    }

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
            Command::SaveConfig { set, remove, .. } => {
                self.execute_config_overwrite(task, record, set, remove)
                    .await
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
                    Err(AdapterError::PairingRejected) => Report::Rejected {
                        reason: Rejection::PairingFailed,
                    },
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
            Command::ListPendingPairings {} => match self.sunshine.pending_pairings().await {
                Ok(pairings) => self.finish(
                    &task.operation_id,
                    record,
                    Report::PendingPairingsRead { pairings },
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
                let before = self.sunshine.process_generation().await;
                if !self.remember(
                    &task.operation_id,
                    record,
                    EffectIntent::ControlService {
                        action: *action,
                        expected,
                        before,
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

    async fn execute_config_overwrite(
        &mut self,
        task: &Task,
        record: &mut ExecutionRecord,
        set: &std::collections::BTreeMap<String, sunshine_client_protocol::config::FieldValue>,
        remove: &std::collections::BTreeSet<String>,
    ) -> Report {
        // The page owns the managed snapshot. Read only to preserve current local-only fields.
        let before = match self.sunshine.read().await {
            Ok(configuration) => configuration,
            Err(error) => return self.finish(&task.operation_id, record, rejected_adapter(error)),
        };
        let configuration = match before.merge(set, remove) {
            Ok(configuration) => configuration,
            Err(error) => return self.finish(&task.operation_id, record, rejected_adapter(error)),
        };
        if self
            .deadline
            .as_ref()
            .is_some_and(ExecutionDeadline::expired)
        {
            return self.finish(
                &task.operation_id,
                record,
                Report::Rejected {
                    reason: Rejection::ExecutionExpired,
                },
            );
        }
        record.effect = Some(EffectIntent::Save {
            target_revision: configuration.revision(),
        });
        if self.journal.replace(&task.operation_id, record).is_err() {
            return persistence_failure();
        }
        // A lost response can follow a successful write. Inspect the result without another POST.
        let _ = self.sunshine.save_overwrite(&before, &configuration).await;
        let report = self.reconcile(record).await;
        self.finish(&task.operation_id, record, report)
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
            None => match self.sunshine.process_generation().await {
                Some(before) => EffectIntent::RestartObserved {
                    before,
                    target_revision: expected.clone(),
                },
                None => EffectIntent::Restart,
            },
        });
        if self
            .deadline
            .as_ref()
            .is_some_and(ExecutionDeadline::expired)
        {
            record.effect = None;
            return self.finish(
                &task.operation_id,
                record,
                Report::Rejected {
                    reason: Rejection::ExecutionExpired,
                },
            );
        }
        if self.journal.replace(&task.operation_id, record).is_err() {
            return persistence_failure();
        }
        let report = match merged {
            Some(merged) => {
                // Even an HTTP error can occur after a successful write. Reconcile, don't retry.
                if let Err(AdapterError::ResourceConflict) =
                    self.sunshine.save_guarded(expected, &merged).await
                {
                    // Guard rejection is known to precede POST, so it is a definitive conflict.
                    return self.finish(
                        &task.operation_id,
                        record,
                        Report::Rejected {
                            reason: Rejection::ResourceConflict,
                        },
                    );
                }
                self.reconcile(record).await
            }
            None => {
                // Sunshine can close HTTPS during a successful restart; never retry the POST.
                let _ = self.sunshine.restart().await;
                let mut report = Report::Unknown {
                    reason: Uncertainty::RestartNotConfirmed,
                };
                for attempt in 0..10 {
                    if attempt > 0 {
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                    report = self.reconcile(record).await;
                    if !matches!(report, Report::Unknown { .. }) {
                        break;
                    }
                    if matches!(record.effect, Some(EffectIntent::Restart)) {
                        break;
                    }
                }
                report
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
            Some(EffectIntent::RestartObserved {
                before,
                target_revision,
            }) => match self.sunshine.process_generation().await {
                Some(after) if &after != before => match self.sunshine.read().await {
                    Ok(current) if &current.revision() == target_revision => {
                        Report::RestartAcknowledged {
                            snapshot: current.snapshot(Effectiveness::PendingVerification),
                            process_generation: after,
                        }
                    }
                    _ => Report::Unknown {
                        reason: Uncertainty::RestartNotConfirmed,
                    },
                },
                _ => Report::Unknown {
                    reason: Uncertainty::RestartNotConfirmed,
                },
            },
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
            Some(EffectIntent::ControlService {
                action,
                expected,
                before,
            }) => match self.sunshine.service_status().await {
                Ok(state)
                    if state == *expected
                        && (*action != ServiceAction::Restart
                            || match (before, self.sunshine.process_generation().await) {
                                (Some(before), Some(after)) => before != &after,
                                _ => false,
                            }) =>
                {
                    Report::ServiceControlled {
                        action: *action,
                        state,
                    }
                }
                _ => Report::Unknown {
                    reason: Uncertainty::ServiceTransitionNotConfirmed,
                },
            },
            Some(EffectIntent::Unobservable) => Report::Unknown {
                reason: Uncertainty::SideEffectNotConfirmed,
            },
            None => Report::Unknown {
                reason: Uncertainty::NoExecutionRecord,
            },
        }
    }

    fn remember(&mut self, id: &str, record: &mut ExecutionRecord, effect: EffectIntent) -> bool {
        if self
            .deadline
            .as_ref()
            .is_some_and(ExecutionDeadline::expired)
        {
            return false;
        }
        record.effect = Some(effect);
        self.journal.replace(id, record).is_ok()
    }

    fn finish(&mut self, id: &str, record: &mut ExecutionRecord, report: Report) -> Report {
        let report = if serde_json::to_vec(&report).map_or(true, |bytes| {
            bytes.len() > sunshine_client_protocol::MAX_REPORT_BYTES
        }) {
            if record.effect.is_some() {
                Report::Unknown {
                    reason: Uncertainty::ResultTooLarge,
                }
            } else {
                Report::Rejected {
                    reason: Rejection::ResultTooLarge,
                }
            }
        } else {
            report
        };
        if self.volatile_read {
            return report;
        }
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
            AdapterError::UnsupportedVersion { .. } => Rejection::UnsupportedVersion,
            AdapterError::UnsafeConfiguration => Rejection::UnsafeConfiguration,
            AdapterError::ResourceConflict => Rejection::ResourceConflict,
            AdapterError::UnsupportedCapability => Rejection::UnsupportedCapability,
            AdapterError::PairingRejected => Rejection::PairingFailed,
            AdapterError::CredentialsRejected
            | AdapterError::ApiUnavailable
            | AdapterError::InvalidLocalEndpoint => Rejection::SunshineUnavailable,
        },
    }
}
