//! Execution facts use the embedding host's protected, durable storage.
use crate::host::RustDeskHost;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime};
use tokio::{sync::Semaphore, time::Instant};
use xscs_protocol::{Binding, Capabilities, DeliveryMode, Rejection, Report, Task, Uncertainty};

pub const MAX_RECORD_BYTES: usize = xscs_protocol::MAX_MESSAGE_BYTES + 4096;
pub const MAX_PENDING_RESULTS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRecord {
    pub fingerprint: String,
    pub binding: Binding,
    pub report: Option<Report>,
    pub acknowledged: bool,
    pub accepted_digest: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum JournalError {
    #[error("execution journal is full")]
    Full,
    #[error("execution journal unavailable")]
    Storage,
}

/// The host supplies an exclusive namespace and atomic durable commits. Preserve operation
/// IDs and fingerprints after acknowledgement; never put credentials into these records.
#[async_trait]
pub trait Journal: Send + Sync + 'static {
    async fn pending_results(&self) -> Result<Vec<(String, ExecutionRecord)>, JournalError>;
    async fn load(&self, operation_id: &str) -> Result<Option<ExecutionRecord>, JournalError>;
    async fn create(
        &self,
        operation_id: &str,
        record: &ExecutionRecord,
    ) -> Result<(), JournalError>;
    async fn replace(
        &self,
        operation_id: &str,
        record: &ExecutionRecord,
    ) -> Result<(), JournalError>;
}

#[derive(Clone, Debug)]
pub struct ExecutionDeadline {
    expires_at: SystemTime,
    received_until: Instant,
}
impl ExecutionDeadline {
    pub fn received(expires_at_unix_ms: u64, remaining_ms: u64) -> Self {
        Self {
            expires_at: SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_millis(expires_at_unix_ms))
                .unwrap_or(SystemTime::UNIX_EPOCH),
            received_until: Instant::now()
                + Duration::from_millis(
                    remaining_ms.min(xscs_protocol::MAX_EXECUTION_SECONDS * 1000),
                ),
        }
    }
    fn expired(&self) -> bool {
        SystemTime::now() >= self.expires_at || Instant::now() >= self.received_until
    }
}

pub struct Executor<H, J> {
    binding: Binding,
    capabilities: Capabilities,
    host: H,
    journal: J,
    lane: Semaphore,
}
impl<H: RustDeskHost, J: Journal> Executor<H, J> {
    pub fn new_with_capabilities(
        binding: Binding,
        capabilities: Capabilities,
        host: H,
        journal: J,
    ) -> Self {
        Self {
            binding,
            capabilities,
            host,
            journal,
            lane: Semaphore::new(1),
        }
    }
    pub async fn pending_results(&self) -> Result<Vec<(String, ExecutionRecord)>, JournalError> {
        let records = self.journal.pending_results().await?;
        if records.len() > MAX_PENDING_RESULTS {
            return Err(JournalError::Full);
        }
        for (_, record) in &records {
            self.validate_record(record)?;
        }
        Ok(records
            .into_iter()
            .filter(|(_, r)| r.binding == self.binding && !r.acknowledged)
            .collect())
    }
    fn validate_record(&self, record: &ExecutionRecord) -> Result<(), JournalError> {
        xscs_protocol::validate_revision(&record.fingerprint).map_err(|_| JournalError::Storage)?;
        if let Some(digest) = &record.accepted_digest {
            xscs_protocol::validate_revision(digest).map_err(|_| JournalError::Storage)?;
        }
        if let Some(report) = &record.report {
            xscs_protocol::validate_report(&xscs_protocol::Command::ReadDiagnostics {}, report)
                .map_err(|_| JournalError::Storage)?;
        }
        if record.binding != self.binding
            || (record.acknowledged
                && (record.report.is_some() || record.accepted_digest.is_none()))
            || serde_json::to_vec(record)
                .map_err(|_| JournalError::Storage)?
                .len()
                > MAX_RECORD_BYTES
        {
            return Err(JournalError::Storage);
        }
        Ok(())
    }
    pub async fn acknowledge_final(
        &self,
        id: &str,
        fingerprint: &str,
        digest: &str,
        finalized: bool,
    ) -> Result<(), JournalError> {
        let _lane = self
            .lane
            .acquire()
            .await
            .map_err(|_| JournalError::Storage)?;
        let mut record = self.journal.load(id).await?.ok_or(JournalError::Storage)?;
        self.validate_record(&record)?;
        if record.fingerprint != fingerprint {
            return Err(JournalError::Storage);
        }
        if record.acknowledged {
            return if record.accepted_digest.as_deref() == Some(digest) {
                Ok(())
            } else {
                Err(JournalError::Storage)
            };
        }
        let report = record.report.as_ref().ok_or(JournalError::Storage)?;
        if xscs_protocol::report_digest(report).map_err(|_| JournalError::Storage)? != digest {
            return Err(JournalError::Storage);
        }
        if !finalized {
            return Ok(());
        }
        record.acknowledged = true;
        record.report = None;
        record.accepted_digest = Some(digest.to_owned());
        self.journal.replace(id, &record).await
    }
    pub async fn deliver(&self, task: &Task, mode: DeliveryMode) -> Report {
        self.deliver_before(task, mode, None).await
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
        let Ok(fingerprint) = task.fingerprint() else {
            return rejected(Rejection::InvalidTask);
        };
        let Ok(_lane) = self.lane.acquire().await else {
            return persistence_failure();
        };
        let mut record = match self.journal.load(&task.operation_id).await {
            Ok(Some(record)) => {
                if self.validate_record(&record).is_err() {
                    return persistence_failure();
                }
                if record.fingerprint != fingerprint {
                    return rejected(Rejection::OperationIdReused);
                }
                if record.acknowledged {
                    return rejected(Rejection::OperationAlreadyCompleted);
                }
                if let Some(report) = &record.report {
                    return report.clone();
                }
                record
            }
            Ok(None) => {
                if mode == DeliveryMode::InspectOnly {
                    return Report::Unknown {
                        reason: Uncertainty::NoExecutionRecord,
                    };
                }
                if deadline.as_ref().is_some_and(ExecutionDeadline::expired) {
                    return rejected(Rejection::ExecutionExpired);
                }
                let pending = match self.journal.pending_results().await {
                    Ok(v) => v,
                    Err(_) => return persistence_failure(),
                };
                if pending.len() >= MAX_PENDING_RESULTS {
                    return rejected(Rejection::JournalFull);
                }
                let record = ExecutionRecord {
                    fingerprint,
                    binding: self.binding.clone(),
                    report: None,
                    acknowledged: false,
                    accepted_digest: None,
                };
                if let Err(error) = self.journal.create(&task.operation_id, &record).await {
                    return match error {
                        JournalError::Full => rejected(Rejection::JournalFull),
                        JournalError::Storage => persistence_failure(),
                    };
                }
                record
            }
            Err(_) => return persistence_failure(),
        };
        let report = if deadline.as_ref().is_some_and(ExecutionDeadline::expired) {
            rejected(Rejection::ExecutionExpired)
        } else {
            match tokio::time::timeout(
                Duration::from_secs(xscs_protocol::MAX_EXECUTION_SECONDS),
                self.host.diagnostics(),
            )
            .await
            {
                Ok(Ok(snapshot)) if snapshot.validate().is_ok() => {
                    Report::DiagnosticsRead { snapshot }
                }
                _ => rejected(Rejection::RustdeskUnavailable),
            }
        };
        record.report = Some(report.clone());
        if self
            .journal
            .replace(&task.operation_id, &record)
            .await
            .is_err()
        {
            persistence_failure()
        } else {
            report
        }
    }
}
fn rejected(reason: Rejection) -> Report {
    Report::Rejected { reason }
}
fn persistence_failure() -> Report {
    Report::Unknown {
        reason: Uncertainty::PersistenceFailure,
    }
}
