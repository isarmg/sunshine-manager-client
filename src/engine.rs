use serde::{Deserialize, Serialize};
use sunshine_client_protocol::{
    Binding, Command, DeliveryMode, Effectiveness, Rejection, Report, Task, Uncertainty,
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
    Save { target_revision: String },
    Restart,
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
    restart_allowed: bool,
    inner: Mutex<Inner<A, J>>,
}

struct Inner<A, J> {
    sunshine: A,
    journal: J,
}

impl<A: Sunshine, J: Journal> Executor<A, J> {
    pub fn new(binding: Binding, restart_allowed: bool, sunshine: A, journal: J) -> Self {
        Self {
            binding,
            restart_allowed,
            inner: Mutex::new(Inner { sunshine, journal }),
        }
    }

    /// One host, one serial execution lane. Heartbeats belong to the independent transport task.
    pub async fn deliver(&self, task: &Task, mode: DeliveryMode) -> Report {
        if let Err(reason) = task.validate(&self.binding, self.restart_allowed) {
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
        let configuration = match self.sunshine.read().await {
            Ok(configuration) => configuration,
            Err(error) => return self.finish(&task.operation_id, record, rejected_adapter(error)),
        };
        let expected = match &task.command {
            Command::ReadConfig {} => {
                return self.finish(
                    &task.operation_id,
                    record,
                    Report::ConfigRead {
                        snapshot: configuration.snapshot(Effectiveness::PendingVerification),
                    },
                );
            }
            Command::PatchConfig {
                expected_revision, ..
            }
            | Command::Restart {
                expected_revision, ..
            } => expected_revision,
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
            None => Report::Unknown {
                reason: Uncertainty::NoExecutionRecord,
            },
        }
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
            AdapterError::Unavailable | AdapterError::InvalidLocalEndpoint => {
                Rejection::SunshineUnavailable
            }
        },
    }
}
