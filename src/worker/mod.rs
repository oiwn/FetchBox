//! In-process download workers built on Tower services.
//!
mod error;
pub mod http;
mod service;

use crate::api::models::{JobError, JobStatus};
use crate::config::WorkerRuntimeConfig;
use crate::ledger::LedgerStorage;
use crate::queue::{DlqStorage, TaskEnvelope};
use crate::storage::StorageClient;
use crate::worker::http::HttpConfig;
use crate::worker::service::{DownloadService, TaskOutcome};
use chrono::Utc;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc, watch};
use tower::timeout::TimeoutLayer;
use tower::{
    BoxError, Service, ServiceBuilder, ServiceExt, limit::RateLimitLayer,
    util::BoxService,
};
use tracing::{debug, error, info, warn};

pub use error::WorkerError;
pub use service::determine_storage_key;

/// Download worker consuming TaskEnvelopes via mpsc channel
pub struct DownloadWorker {
    worker_id: String,
    inbox: mpsc::Receiver<TaskEnvelope>,
    service: BoxService<TaskEnvelope, TaskOutcome, WorkerError>,
    ledger: Arc<LedgerStorage>,
    ledger_lock: Arc<Mutex<()>>,
    dlq: Arc<DlqStorage>,
    shutdown_rx: watch::Receiver<bool>,
    retry_backoff: Duration,
    max_retries: u32,
}

/// Shared dependencies required to run a worker instance.
pub struct WorkerContext {
    pub ledger: Arc<LedgerStorage>,
    pub ledger_lock: Arc<Mutex<()>>,
    pub dlq: Arc<DlqStorage>,
    pub shutdown_rx: watch::Receiver<bool>,
}

impl DownloadWorker {
    pub fn new(
        worker_id: String,
        inbox: mpsc::Receiver<TaskEnvelope>,
        storage: StorageClient,
        context: WorkerContext,
        config: WorkerConfig,
    ) -> Result<Self, WorkerError> {
        let http = crate::worker::http::HttpClient::new(config.http.clone(), None)?;
        let base_service = DownloadService::new(worker_id.clone(), http, storage);
        let service = build_service_stack(base_service, &config);

        Ok(Self::with_service(
            worker_id, inbox, service, context, config,
        ))
    }

    fn with_service(
        worker_id: String,
        inbox: mpsc::Receiver<TaskEnvelope>,
        service: BoxService<TaskEnvelope, TaskOutcome, WorkerError>,
        context: WorkerContext,
        config: WorkerConfig,
    ) -> Self {
        let WorkerContext {
            ledger,
            ledger_lock,
            dlq,
            shutdown_rx,
        } = context;
        Self {
            worker_id,
            inbox,
            service,
            ledger,
            ledger_lock,
            dlq,
            shutdown_rx,
            retry_backoff: Duration::from_millis(config.retry_backoff_ms.max(1)),
            max_retries: config.max_retries,
        }
    }

    pub async fn run(mut self) -> Result<(), WorkerError> {
        info!(worker_id = %self.worker_id, "Worker starting");

        loop {
            tokio::select! {
                biased;
                changed = self.shutdown_rx.changed() => {
                    if changed.is_ok() && *self.shutdown_rx.borrow() {
                        info!(worker_id = %self.worker_id, "Worker shutting down");
                        break;
                    }
                }
                envelope = self.inbox.recv() => {
                    match envelope {
                        Some(task) => self.handle_task(task).await,
                        None => {
                            info!(worker_id = %self.worker_id, "Inbox closed, stopping worker");
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn handle_task(&mut self, envelope: TaskEnvelope) {
        debug!(
            worker_id = %self.worker_id,
            job_id = %envelope.task.job_id,
            resource_id = %envelope.task.resource_id,
            "Processing task",
        );

        match self.execute_with_retries(envelope.clone()).await {
            Ok((outcome, attempts)) => {
                if let Err(err) =
                    self.record_success(&envelope, outcome, attempts).await
                {
                    error!(worker_id = %self.worker_id, error = %err, "Failed to record success");
                }
            }
            Err((error, attempts)) => {
                error!(
                    worker_id = %self.worker_id,
                    job_id = %envelope.task.job_id,
                    resource_id = %envelope.task.resource_id,
                    attempts,
                    err = %error,
                    "Task failed after retries",
                );

                if let Err(err) =
                    self.record_failure(&envelope, &error, attempts).await
                {
                    error!(worker_id = %self.worker_id, error = %err, "Failed to record failure");
                }

                if let Err(err) =
                    self.send_to_dlq(envelope.clone(), &error, attempts).await
                {
                    error!(worker_id = %self.worker_id, error = %err, "Failed to enqueue DLQ entry");
                }
            }
        }
    }

    async fn execute_with_retries(
        &mut self,
        envelope: TaskEnvelope,
    ) -> Result<(TaskOutcome, u32), (WorkerError, u32)> {
        let mut attempt = 0u32;
        let max_attempts = self.max_retries.saturating_add(1);

        loop {
            attempt += 1;

            let ready_service = match self.service.ready().await {
                Ok(svc) => svc,
                Err(err) => return Err((err, attempt)),
            };

            match ready_service.call(envelope.clone()).await {
                Ok(outcome) => return Ok((outcome, attempt)),
                Err(err) => {
                    if attempt >= max_attempts || !err.is_retryable() {
                        return Err((err, attempt));
                    }

                    let backoff =
                        self.retry_backoff.mul_f64(2f64.powi((attempt - 1) as i32));
                    debug!(worker_id = %self.worker_id, attempt, backoff_ms = backoff.as_millis(), "Retrying task");
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }

    async fn record_success(
        &self,
        envelope: &TaskEnvelope,
        outcome: TaskOutcome,
        attempts: u32,
    ) -> Result<(), WorkerError> {
        info!(
            worker_id = %self.worker_id,
            job_id = %envelope.task.job_id,
            resource_id = %envelope.task.resource_id,
            storage_key = %outcome.storage_key,
            bytes = outcome.bytes_written,
            attempts,
            "Task completed",
        );

        let _guard = self.ledger_lock.lock().await;
        match self.ledger.get(&envelope.task.job_id) {
            Ok(Some(mut snapshot)) => {
                if matches!(snapshot.status, JobStatus::Queued) {
                    snapshot.status = JobStatus::InProgress;
                }
                snapshot.resource_completed =
                    (snapshot.resource_completed + 1).min(snapshot.resource_total);
                if snapshot.resource_completed == snapshot.resource_total
                    && snapshot.resource_failed == 0
                {
                    snapshot.status = JobStatus::Completed;
                }
                snapshot.updated_at = Utc::now();
                self.ledger
                    .upsert(snapshot)
                    .map_err(|e| WorkerError::Ledger(e.to_string()))?
            }
            Ok(None) => {
                warn!(worker_id = %self.worker_id, job_id = %envelope.task.job_id, "Missing job snapshot during success update");
            }
            Err(err) => return Err(WorkerError::Ledger(err.to_string())),
        }

        Ok(())
    }

    async fn record_failure(
        &self,
        envelope: &TaskEnvelope,
        error: &WorkerError,
        attempts: u32,
    ) -> Result<(), WorkerError> {
        let _guard = self.ledger_lock.lock().await;
        match self.ledger.get(&envelope.task.job_id) {
            Ok(Some(mut snapshot)) => {
                snapshot.resource_failed =
                    snapshot.resource_failed.saturating_add(1);
                snapshot.status = JobStatus::Failed;
                snapshot.updated_at = Utc::now();
                snapshot.errors.push(JobError {
                    resource_name: envelope.task.resource_id.clone(),
                    code: error.code().to_string(),
                    message: format!("{} (attempts: {})", error, attempts),
                    timestamp: Utc::now(),
                });

                self.ledger
                    .upsert(snapshot)
                    .map_err(|e| WorkerError::Ledger(e.to_string()))?
            }
            Ok(None) => {
                warn!(worker_id = %self.worker_id, job_id = %envelope.task.job_id, "Missing job snapshot during failure update")
            }
            Err(err) => return Err(WorkerError::Ledger(err.to_string())),
        }

        Ok(())
    }

    async fn send_to_dlq(
        &self,
        envelope: TaskEnvelope,
        error: &WorkerError,
        attempts: u32,
    ) -> Result<(), WorkerError> {
        self.dlq
            .record(
                envelope.task_id,
                envelope.task.clone(),
                error.code().to_string(),
                error.to_string(),
                attempts,
            )
            .map_err(|e| WorkerError::Dlq(e.to_string()))
    }
}

/// Worker runtime configuration
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub rate_limit_per_worker: u64,
    pub max_retries: u32,
    pub retry_backoff_ms: u64,
    pub http: HttpConfig,
    pub task_timeout_ms: u64,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            rate_limit_per_worker: 16,
            max_retries: 3,
            retry_backoff_ms: 500,
            http: HttpConfig::default(),
            task_timeout_ms: 60_000,
        }
    }
}

impl From<&WorkerRuntimeConfig> for WorkerConfig {
    fn from(value: &WorkerRuntimeConfig) -> Self {
        Self {
            rate_limit_per_worker: value.rate_limit_per_worker,
            max_retries: value.max_retries,
            retry_backoff_ms: value.retry_backoff_ms,
            http: HttpConfig::default(),
            task_timeout_ms: value.task_timeout_ms,
        }
    }
}

fn build_service_stack(
    base_service: DownloadService,
    config: &WorkerConfig,
) -> BoxService<TaskEnvelope, TaskOutcome, WorkerError> {
    let service = ServiceBuilder::new()
        .layer(RateLimitLayer::new(
            config.rate_limit_per_worker,
            Duration::from_secs(1),
        ))
        .layer(TimeoutLayer::new(Duration::from_millis(
            config.task_timeout_ms.max(1),
        )))
        .service(base_service)
        .map_err(map_stack_error);

    BoxService::new(service)
}

fn map_stack_error(err: BoxError) -> WorkerError {
    let err = match err.downcast::<WorkerError>() {
        Ok(worker_err) => return *worker_err,
        Err(err) => err,
    };

    match err.downcast::<tower::timeout::error::Elapsed>() {
        Ok(_) => WorkerError::Timeout("task timed out".to_string()),
        Err(err) => WorkerError::Download(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::{JobSnapshot, JobStatus};
    use tempfile::TempDir;
    use tokio::sync::{mpsc, watch};
    use tower::service_fn;
    use uuid::Uuid;

    #[tokio::test]
    async fn execute_with_retries_eventually_succeeds() {
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let service_attempts = attempts.clone();
        let service = service_fn(move |_req: TaskEnvelope| {
            let service_attempts = service_attempts.clone();
            async move {
                let current = service_attempts
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    + 1;
                if current < 3 {
                    Err(WorkerError::Download(format!("boom {current}")))
                } else {
                    Ok(TaskOutcome {
                        storage_key: "key".into(),
                        bytes_written: 42,
                    })
                }
            }
        });
        let mut harness = WorkerHarness::new(BoxService::new(service));
        let envelope = sample_envelope("job-success", "res-1");

        let (outcome, attempts_used) = harness
            .worker
            .execute_with_retries(envelope)
            .await
            .expect("task should eventually succeed");

        assert_eq!(attempts_used, 3);
        assert_eq!(outcome.storage_key, "key");
    }

    #[tokio::test]
    async fn execute_with_retries_stops_on_non_retryable_error() {
        let service = service_fn(|_req: TaskEnvelope| async {
            Err(WorkerError::InvalidTask("bad".into()))
        });
        let mut harness = WorkerHarness::new(BoxService::new(service));
        let envelope = sample_envelope("job-fail", "res-2");

        let err = harness
            .worker
            .execute_with_retries(envelope)
            .await
            .expect_err("should fail immediately");

        assert_eq!(err.1, 1);
        assert!(matches!(err.0, WorkerError::InvalidTask(_)));
    }

    #[tokio::test]
    async fn record_success_advances_job_snapshot() {
        let service = service_fn(|_req: TaskEnvelope| async {
            Ok(TaskOutcome {
                storage_key: "k".into(),
                bytes_written: 1,
            })
        });
        let harness = WorkerHarness::new(BoxService::new(service));
        let envelope = sample_envelope("job-ledger", "res-1");
        insert_snapshot(&harness.ledger, &envelope.task.job_id, 2);

        harness
            .worker
            .record_success(
                &envelope,
                TaskOutcome {
                    storage_key: "k1".into(),
                    bytes_written: 1,
                },
                1,
            )
            .await
            .unwrap();

        let snapshot = harness.ledger.get(&envelope.task.job_id).unwrap().unwrap();
        assert_eq!(snapshot.resource_completed, 1);
        assert!(matches!(snapshot.status, JobStatus::InProgress));

        harness
            .worker
            .record_success(
                &envelope,
                TaskOutcome {
                    storage_key: "k2".into(),
                    bytes_written: 1,
                },
                1,
            )
            .await
            .unwrap();

        let snapshot = harness.ledger.get(&envelope.task.job_id).unwrap().unwrap();
        assert_eq!(snapshot.resource_completed, 2);
        assert!(matches!(snapshot.status, JobStatus::Completed));
    }

    #[tokio::test]
    async fn record_failure_and_dlq_capture_errors() {
        let service = service_fn(|_req: TaskEnvelope| async {
            Err(WorkerError::Download("boom".into()))
        });
        let harness = WorkerHarness::new(BoxService::new(service));
        let envelope = sample_envelope("job-error", "res-err");
        insert_snapshot(&harness.ledger, &envelope.task.job_id, 1);

        let worker_error = WorkerError::Download("boom".into());
        harness
            .worker
            .record_failure(&envelope, &worker_error, 2)
            .await
            .unwrap();
        harness
            .worker
            .send_to_dlq(envelope.clone(), &worker_error, 2)
            .await
            .unwrap();

        let snapshot = harness.ledger.get(&envelope.task.job_id).unwrap().unwrap();
        assert_eq!(snapshot.resource_failed, 1);
        assert_eq!(snapshot.errors.len(), 1);
        assert!(matches!(snapshot.status, JobStatus::Failed));

        let entries = harness.dlq.list(10).unwrap();
        assert_eq!(entries.len(), 1);
        let (_, task) = &entries[0];
        assert_eq!(task.failure_code, "DOWNLOAD_ERROR");
        assert_eq!(task.attempts, 2);
    }

    fn sample_envelope(job_id: &str, resource_id: &str) -> TaskEnvelope {
        TaskEnvelope {
            task_id: Uuid::now_v7(),
            task: crate::proto::DownloadTask {
                job_id: job_id.to_string(),
                job_type: "default".into(),
                resource_id: resource_id.to_string(),
                url: "https://example.test/file".into(),
                headers: vec![],
                proxy_hint: None,
                storage_hint: None,
                attributes: None,
                manifest_key: format!("s3://bucket/{job_id}.json"),
                attempt: 1,
                tenant: "tenant".into(),
                trace_id: "trace".into(),
            },
        }
    }

    fn insert_snapshot(ledger: &LedgerStorage, job_id: &str, total: usize) {
        let now = chrono::Utc::now();
        let snapshot = JobSnapshot {
            job_id: job_id.to_string(),
            status: JobStatus::Queued,
            created_at: now,
            updated_at: now,
            resource_total: total,
            resource_completed: 0,
            resource_failed: 0,
            manifest_key: "manifest".into(),
            errors: vec![],
            tenant: "tenant".into(),
        };
        ledger.upsert(snapshot).unwrap();
    }

    struct WorkerHarness {
        worker: DownloadWorker,
        ledger: Arc<LedgerStorage>,
        dlq: Arc<DlqStorage>,
        _temp: TempDir,
    }

    impl WorkerHarness {
        fn new(
            service: BoxService<TaskEnvelope, TaskOutcome, WorkerError>,
        ) -> Self {
            let temp_dir = TempDir::new().unwrap();
            let ledger_path = temp_dir.path().join("ledger.fjall");
            let dlq_path = temp_dir.path().join("dlq.fjall");
            let ledger = Arc::new(LedgerStorage::open(&ledger_path).unwrap());
            let dlq = Arc::new(DlqStorage::open(&dlq_path).unwrap());
            let ledger_lock = Arc::new(Mutex::new(()));
            let (_tx, inbox) = mpsc::channel(8);
            let (_shutdown_tx, shutdown_rx) = watch::channel(false);

            let context = WorkerContext {
                ledger: ledger.clone(),
                ledger_lock,
                dlq: dlq.clone(),
                shutdown_rx,
            };

            let worker = DownloadWorker::with_service(
                "worker-test".into(),
                inbox,
                service,
                context,
                WorkerConfig::default(),
            );

            Self {
                worker,
                ledger,
                dlq,
                _temp: temp_dir,
            }
        }
    }
}
