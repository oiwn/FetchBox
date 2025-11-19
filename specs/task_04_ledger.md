# Spec: Fjall Persistence Layer

> Implementation note: the runtime now exposes this component as `LedgerStorage` (`src/ledger/storage.rs`). It is functionally identical to the `FjallStore` described below; only the naming changed to align with the other storage adapters (`TasksStorage`, `DlqStorage`).

## Goal
Design and implement the persistence layer using Fjall for FetchBox's three databases: **ledger.db** (job state & logs), **queue.db** (active task queue), and **dlq.db** (dead letter queue). This spec defines the data models, API surface, pruning behavior, and integration points.

## Architecture Decision

In the new single-process architecture, we use **three separate Fjall databases**:

1. **ledger.db** - Job snapshots, logs, idempotency (this spec)
2. **queue.db** - Active task queue (covered in task_03.md)
3. **dlq.db** - Dead letter queue for failures (covered in task_03.md)

This separation provides:
- **Clean boundaries** - Each database has a single responsibility
- **Independent retention** - Different retention policies per database
- **Isolated operations** - DLQ research doesn't impact production queue
- **Simplified backups** - Backup strategy per database type

## Scope

This spec covers **ledger.db only** (job state persistence). Queue and DLQ databases are specified in task_03.md.

1. Create `src/ledger/` module for job state management
2. Define partitions for jobs, logs, idempotency
3. Provide APIs for Axum API and workers
4. Implement pruning/retention strategy
5. Integration with handlers and observability

## 1. Ledger Database Schema

### Path: `data/ledger/` (default, configurable)

Module: `src/ledger/store.rs`

```rust
pub struct LedgerStorage {
    keyspace: Keyspace,
    jobs: PartitionHandle,          // job_id → JobSnapshot
    logs: PartitionHandle,           // job_id:offset → LogEntry
    idempotency: PartitionHandle,    // idempotency_key → job_id
}
```

### Partitions

#### 1. `jobs` - Job Snapshots

**Key**: `job_id` (String, UTF-8)
**Value**: `JobSnapshot` (JSON or Protobuf)

```rust
pub struct JobSnapshot {
    pub job_id: String,
    pub job_type: String,
    pub tenant: String,
    pub state: JobState,              // Queued, InProgress, Completed, Failed
    pub resources_total: u32,
    pub resources_completed: u32,
    pub resources_failed: u32,
    pub manifest_key: String,         // S3 key to manifest
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub completed_at_ms: Option<u64>,
    pub last_error_code: Option<String>,
    pub last_error_message: Option<String>,
    pub log_count: u64,               // Number of log entries
}
```

**Usage**:
- Created when job is accepted (state = Queued)
- Updated by workers as tasks complete/fail
- Queried by operator API: `GET /operators/jobs/{job_id}`

#### 2. `logs` - Job Logs

**Key**: `{job_id}:{offset}` (composite, zero-padded offset for ordering)
**Value**: `LogEntry` (Protobuf)

```rust
pub struct LogEntry {
    pub level: LogLevel,              // Info, Warn, Error
    pub message: String,
    pub resource_id: Option<String>,  // Which resource this log relates to
    pub timestamp_ms: u64,
    pub trace_id: Option<String>,
    pub fields: HashMap<String, String>,  // Structured fields
}
```

**Key Format**: `{job_id}:{offset:020}` (e.g., `job-123:00000000000000000001`)

**Usage**:
- Workers append logs as they process tasks
- Operator API streams logs: `GET /operators/jobs/{job_id}/logs`
- Ordered by offset for chronological replay

#### 3. `idempotency` - Idempotency Keys

**Key**: `idempotency_key` (SHA-256 hash of user-provided key)
**Value**: `job_id` (String)

**Usage**:
- API checks idempotency key before accepting job
- Returns existing job_id if key already used
- Prevents duplicate job submission

### Retention Policies

Configured in `config/fetchbox.toml`:

```toml
[ledger]
path = "data/ledger"
retention_days_jobs = 30          # Keep job snapshots for 30 days
retention_days_logs = 30          # Keep logs for 30 days
retention_days_idempotency = 14   # Keep idempotency keys for 14 days
```

## 2. API Methods

Module: `src/ledger/store.rs`

### Initialization

```rust
impl FjallStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let keyspace = Keyspace::open(path)?;

        let jobs = keyspace.open_partition(
            "jobs",
            PartitionCreateOptions::default(),
        )?;

        let logs = keyspace.open_partition(
            "logs",
            PartitionCreateOptions::default(),
        )?;

        let idempotency = keyspace.open_partition(
            "idempotency",
            PartitionCreateOptions::default(),
        )?;

        Ok(Self { keyspace, jobs, logs, idempotency })
    }
}
```

### Job Snapshot Operations

```rust
impl FjallStore {
    /// Create or update job snapshot
    pub fn upsert(&self, snapshot: JobSnapshot) -> Result<()> {
        let key = snapshot.job_id.as_bytes();
        let value = serde_json::to_vec(&snapshot)?;
        self.jobs.insert(key, value)?;
        Ok(())
    }

    /// Get job snapshot by ID
    pub fn get(&self, job_id: &str) -> Result<Option<JobSnapshot>> {
        let key = job_id.as_bytes();
        self.jobs.get(key)?
            .map(|v| serde_json::from_slice(&v))
            .transpose()
            .map_err(Into::into)
    }

    /// Update job state (worker progress tracking)
    pub fn update_job_state(&self, job_id: &str, state: JobState) -> Result<()> {
        let mut snapshot = self.get(job_id)?
            .ok_or(Error::JobNotFound)?;

        snapshot.state = state;
        snapshot.updated_at_ms = now_ms();

        if matches!(state, JobState::Completed | JobState::Failed) {
            snapshot.completed_at_ms = Some(now_ms());
        }

        self.upsert(snapshot)
    }

    /// Increment completed/failed counters
    pub fn record_task_result(
        &self,
        job_id: &str,
        success: bool,
        error_code: Option<String>,
        error_message: Option<String>,
    ) -> Result<()> {
        let mut snapshot = self.get(job_id)?
            .ok_or(Error::JobNotFound)?;

        if success {
            snapshot.resources_completed += 1;
        } else {
            snapshot.resources_failed += 1;
            snapshot.last_error_code = error_code;
            snapshot.last_error_message = error_message;
        }

        snapshot.updated_at_ms = now_ms();

        // Auto-update state based on progress
        if snapshot.resources_completed + snapshot.resources_failed == snapshot.resources_total {
            snapshot.state = if snapshot.resources_failed > 0 {
                JobState::Partial
            } else {
                JobState::Completed
            };
            snapshot.completed_at_ms = Some(now_ms());
        } else if snapshot.resources_completed > 0 {
            snapshot.state = JobState::InProgress;
        }

        self.upsert(snapshot)
    }
}
```

### Log Operations

```rust
impl FjallStore {
    /// Append log entry to job logs
    pub fn append_log(&self, job_id: &str, entry: LogEntry) -> Result<u64> {
        // Get current log count from job snapshot
        let mut snapshot = self.get(job_id)?
            .ok_or(Error::JobNotFound)?;

        let offset = snapshot.log_count;
        snapshot.log_count += 1;
        snapshot.updated_at_ms = now_ms();

        // Update snapshot with new log count
        self.upsert(snapshot)?;

        // Write log entry with composite key
        let key = format_log_key(job_id, offset);
        let value = serde_json::to_vec(&entry)?;
        self.logs.insert(key.as_bytes(), value)?;

        Ok(offset)
    }

    /// Get logs for a job (with optional offset range)
    pub fn get_logs(
        &self,
        job_id: &str,
        start_offset: u64,
        limit: usize,
    ) -> Result<Vec<(u64, LogEntry)>> {
        let mut results = Vec::new();
        let prefix = format!("{}:", job_id);

        for item in self.logs.iter() {
            let (key, value) = item?;
            let key_str = String::from_utf8_lossy(&key);

            if !key_str.starts_with(&prefix) {
                continue;
            }

            // Parse offset from key
            let offset_str = key_str.strip_prefix(&prefix)
                .ok_or(Error::InvalidLogKey)?;
            let offset: u64 = offset_str.parse()?;

            if offset < start_offset {
                continue;
            }

            let entry: LogEntry = serde_json::from_slice(&value)?;
            results.push((offset, entry));

            if results.len() >= limit {
                break;
            }
        }

        Ok(results)
    }
}

fn format_log_key(job_id: &str, offset: u64) -> String {
    format!("{}:{:020}", job_id, offset)
}
```

### Idempotency Operations

```rust
impl FjallStore {
    /// Check if idempotency key exists, return associated job_id
    pub fn check_idempotency(&self, key: &str) -> Result<Option<String>> {
        let hash = hash_idempotency_key(key);

        self.idempotency.get(hash.as_bytes())?
            .map(|v| String::from_utf8(v.to_vec()))
            .transpose()
            .map_err(Into::into)
    }

    /// Register idempotency key for a job
    pub fn register_idempotency(&self, key: &str, job_id: &str) -> Result<()> {
        let hash = hash_idempotency_key(key);
        self.idempotency.insert(hash.as_bytes(), job_id.as_bytes())?;
        Ok(())
    }
}

fn hash_idempotency_key(key: &str) -> String {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    format!("{:x}", hasher.finalize())
}
```

## 3. Pruning & Retention

Module: `src/ledger/pruning.rs`

### Retention Strategy

```rust
pub struct RetentionPolicy {
    pub jobs_retention_days: u32,
    pub logs_retention_days: u32,
    pub idempotency_retention_days: u32,
}

impl FjallStore {
    /// Prune old jobs based on retention policy
    pub fn prune_jobs(&self, policy: &RetentionPolicy) -> Result<PruneMetrics> {
        let cutoff_ms = now_ms() - (policy.jobs_retention_days as u64 * 86400 * 1000);
        let mut deleted = 0;

        for item in self.jobs.iter() {
            let (key, value) = item?;
            let snapshot: JobSnapshot = serde_json::from_slice(&value)?;

            if snapshot.updated_at_ms < cutoff_ms {
                self.jobs.remove(&key)?;
                deleted += 1;
            }
        }

        Ok(PruneMetrics {
            partition: "jobs".to_string(),
            deleted_count: deleted,
        })
    }

    /// Prune old logs based on retention policy
    pub fn prune_logs(&self, policy: &RetentionPolicy) -> Result<PruneMetrics> {
        let cutoff_ms = now_ms() - (policy.logs_retention_days as u64 * 86400 * 1000);
        let mut deleted = 0;

        for item in self.logs.iter() {
            let (key, value) = item?;
            let entry: LogEntry = serde_json::from_slice(&value)?;

            if entry.timestamp_ms < cutoff_ms {
                self.logs.remove(&key)?;
                deleted += 1;
            }
        }

        Ok(PruneMetrics {
            partition: "logs".to_string(),
            deleted_count: deleted,
        })
    }

    /// Prune old idempotency keys
    pub fn prune_idempotency(&self, policy: &RetentionPolicy) -> Result<PruneMetrics> {
        let cutoff_ms = now_ms() - (policy.idempotency_retention_days as u64 * 86400 * 1000);
        let mut deleted = 0;

        // Note: We don't have timestamps on idempotency keys directly
        // Option 1: Store timestamp in value
        // Option 2: Use job snapshot's created_at as proxy
        // For now, we'll need to enhance the value structure

        // TODO: Implement once idempotency value includes timestamp

        Ok(PruneMetrics {
            partition: "idempotency".to_string(),
            deleted_count: deleted,
        })
    }
}

pub struct PruneMetrics {
    pub partition: String,
    pub deleted_count: usize,
}
```

### Background Pruning Task

```rust
pub async fn run_pruner(
    store: Arc<FjallStore>,
    policy: RetentionPolicy,
    interval_hours: u64,
) -> Result<()> {
    let mut interval = tokio::time::interval(Duration::from_hours(interval_hours));

    loop {
        interval.tick().await;

        info!("Starting retention pruning");

        let jobs_metrics = store.prune_jobs(&policy)?;
        info!("Pruned {} old jobs", jobs_metrics.deleted_count);

        let logs_metrics = store.prune_logs(&policy)?;
        info!("Pruned {} old logs", logs_metrics.deleted_count);

        let idempotency_metrics = store.prune_idempotency(&policy)?;
        info!("Pruned {} old idempotency keys", idempotency_metrics.deleted_count);
    }
}
```

## 4. Integration with API

### Job Submission (Axum Handler)

```rust
async fn handle_post_jobs(
    Extension(ledger): Extension<Arc<FjallStore>>,
    Extension(broker): Extension<Arc<TaskBroker>>,
    Extension(handler_registry): Extension<Arc<HandlerRegistry>>,
    headers: HeaderMap,
    Json(manifest): Json<JobManifest>,
) -> Result<Json<JobResponse>, ApiError> {
    // Check idempotency
    if let Some(idempotency_key) = headers.get("X-Fetchbox-Idempotency-Key") {
        if let Some(existing_job_id) = ledger.check_idempotency(idempotency_key.to_str()?)? {
            return Ok(Json(JobResponse {
                job_id: existing_job_id,
                status: "duplicate".to_string(),
            }));
        }
    }

    // Create job snapshot
    let job_id = Uuid::new_v7().to_string();
    let snapshot = JobSnapshot {
        job_id: job_id.clone(),
        job_type: manifest.job_type.clone(),
        tenant: headers.get("X-Fetchbox-Tenant")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("default")
            .to_string(),
        state: JobState::Queued,
        resources_total: manifest.resources.len() as u32,
        resources_completed: 0,
        resources_failed: 0,
        manifest_key: format!("manifests/{}/{}", manifest.job_type, job_id),
        created_at_ms: now_ms(),
        updated_at_ms: now_ms(),
        completed_at_ms: None,
        last_error_code: None,
        last_error_message: None,
        log_count: 0,
    };

    ledger.upsert(snapshot)?;

    // Register idempotency key
    if let Some(idempotency_key) = headers.get("X-Fetchbox-Idempotency-Key") {
        ledger.register_idempotency(idempotency_key.to_str()?, &job_id)?;
    }

    // Get handler and build tasks
    let handler = handler_registry.get(&manifest.job_type)?;
    let tasks = handler.build_tasks(manifest).await?;

    // Enqueue tasks via broker
    for task in tasks {
        broker.enqueue(task).await?;
    }

    Ok(Json(JobResponse {
        job_id,
        status: "queued".to_string(),
    }))
}
```

### Job Status Query

```rust
async fn handle_get_job(
    Extension(ledger): Extension<Arc<FjallStore>>,
    Path(job_id): Path<String>,
) -> Result<Json<JobSnapshot>, ApiError> {
    let snapshot = ledger.get(&job_id)?
        .ok_or(ApiError::NotFound)?;

    Ok(Json(snapshot))
}
```

### Log Streaming

```rust
async fn handle_get_logs(
    Extension(ledger): Extension<Arc<FjallStore>>,
    Path(job_id): Path<String>,
    Query(params): Query<LogQueryParams>,
) -> Result<Json<LogResponse>, ApiError> {
    let logs = ledger.get_logs(
        &job_id,
        params.offset.unwrap_or(0),
        params.limit.unwrap_or(100),
    )?;

    Ok(Json(LogResponse {
        job_id,
        logs: logs.into_iter()
            .map(|(offset, entry)| LogEntryResponse { offset, entry })
            .collect(),
    }))
}
```

## 5. Integration with Workers

Workers update job state as they process tasks:

```rust
// In worker after task completion
async fn process_task(&mut self, envelope: TaskEnvelope) {
    match self.service.ready().await?.call(envelope.clone()).await {
        Ok(_) => {
            // Log success
            let _ = self.ledger.append_log(&envelope.task.job_id, LogEntry {
                level: LogLevel::Info,
                message: format!("Downloaded {}", envelope.task.resource_id),
                resource_id: Some(envelope.task.resource_id.clone()),
                timestamp_ms: now_ms(),
                trace_id: Some(envelope.task.trace_id.clone()),
                fields: HashMap::new(),
            });

            // Update job progress
            let _ = self.ledger.record_task_result(
                &envelope.task.job_id,
                true,  // success
                None,
                None,
            );
        }
        Err(e) => {
            // Log failure
            let _ = self.ledger.append_log(&envelope.task.job_id, LogEntry {
                level: LogLevel::Error,
                message: format!("Failed to download {}: {}", envelope.task.resource_id, e),
                resource_id: Some(envelope.task.resource_id.clone()),
                timestamp_ms: now_ms(),
                trace_id: Some(envelope.task.trace_id.clone()),
                fields: HashMap::new(),
            });

            // Update job progress with error
            let _ = self.ledger.record_task_result(
                &envelope.task.job_id,
                false,  // failure
                Some(classify_error(&e)),
                Some(e.to_string()),
            );
        }
    }
}
```

## 6. Module Structure

```
src/ledger/
├── mod.rs          # Public API, re-exports
├── store.rs        # FjallStore implementation
├── partitions.rs   # Key encoding/decoding utilities
├── pruning.rs      # Retention and pruning logic
└── error.rs        # LedgerError types
```

## 7. Configuration

File: `config/fetchbox.toml`

```toml
[ledger]
path = "data/ledger"
retention_days_jobs = 30
retention_days_logs = 30
retention_days_idempotency = 14
pruning_interval_hours = 24    # Run pruning daily
```

## 8. Metrics & Observability

Track ledger operations:

```rust
pub struct LedgerMetrics {
    pub jobs_created: AtomicU64,
    pub jobs_updated: AtomicU64,
    pub logs_written: AtomicU64,
    pub idempotency_hits: AtomicU64,
    pub idempotency_misses: AtomicU64,
    pub pruned_jobs: AtomicU64,
    pub pruned_logs: AtomicU64,
}
```

Exposed via Prometheus endpoint (task_11):

```
fetchbox_ledger_jobs_created_total
fetchbox_ledger_jobs_updated_total
fetchbox_ledger_logs_written_total
fetchbox_ledger_idempotency_hits_total
fetchbox_ledger_idempotency_misses_total
fetchbox_ledger_pruned_jobs_total
fetchbox_ledger_pruned_logs_total
```

## 9. Error Handling

```rust
#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error("Job not found: {0}")]
    JobNotFound(String),

    #[error("Invalid log key format")]
    InvalidLogKey,

    #[error("Fjall error: {0}")]
    Fjall(#[from] fjall::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
```

## 10. Testing Strategy

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_snapshot_crud() {
        let tmpdir = tempfile::tempdir().unwrap();
        let store = FjallStore::open(tmpdir.path()).unwrap();

        let snapshot = JobSnapshot {
            job_id: "job-123".to_string(),
            job_type: "gallery".to_string(),
            tenant: "default".to_string(),
            state: JobState::Queued,
            resources_total: 10,
            resources_completed: 0,
            resources_failed: 0,
            manifest_key: "manifests/gallery/job-123".to_string(),
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
            completed_at_ms: None,
            last_error_code: None,
            last_error_message: None,
            log_count: 0,
        };

        store.upsert(snapshot.clone()).unwrap();

        let retrieved = store.get("job-123").unwrap().unwrap();
        assert_eq!(retrieved.job_id, "job-123");
        assert_eq!(retrieved.state, JobState::Queued);
    }

    #[test]
    fn test_log_append_and_query() {
        let tmpdir = tempfile::tempdir().unwrap();
        let store = FjallStore::open(tmpdir.path()).unwrap();

        // Create job first
        let snapshot = JobSnapshot { /* ... */ };
        store.upsert(snapshot).unwrap();

        // Append logs
        let entry = LogEntry {
            level: LogLevel::Info,
            message: "Test log".to_string(),
            resource_id: Some("res-1".to_string()),
            timestamp_ms: now_ms(),
            trace_id: None,
            fields: HashMap::new(),
        };

        let offset = store.append_log("job-123", entry).unwrap();
        assert_eq!(offset, 0);

        // Query logs
        let logs = store.get_logs("job-123", 0, 10).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].1.message, "Test log");
    }

    #[test]
    fn test_idempotency() {
        let tmpdir = tempfile::tempdir().unwrap();
        let store = FjallStore::open(tmpdir.path()).unwrap();

        let key = "user-provided-key-123";

        // First check - should be None
        assert!(store.check_idempotency(key).unwrap().is_none());

        // Register
        store.register_idempotency(key, "job-123").unwrap();

        // Second check - should return job_id
        assert_eq!(
            store.check_idempotency(key).unwrap().unwrap(),
            "job-123"
        );
    }
}
```

## 11. Deliverables

- ✅ `src/ledger/` module with FjallStore implementation
- ✅ Job snapshot CRUD operations
- ✅ Log append and query operations
- ✅ Idempotency key management
- ✅ Pruning framework with retention policies
- ✅ Integration with API handlers
- ✅ Integration with workers
- ✅ Unit tests with >80% coverage
- ✅ Error handling and metrics
- ✅ Configuration support

## 12. Benefits

✅ **Fast lookups** - O(1) job status queries
✅ **Ordered logs** - Chronological log streaming
✅ **Idempotency** - Prevent duplicate submissions
✅ **Retention control** - Independent retention policies
✅ **Simple design** - No complex indexing, just key-value
✅ **Thread-safe** - Fjall handles concurrency
✅ **Embedded** - No external database needed

## 13. Relationship to Other Databases

- **ledger.db** (this spec) - Job state, logs, idempotency
- **queue.db** (task_03) - Active task queue with sequential IDs
- **dlq.db** (task_03) - Failed tasks with failure analytics

All three are separate Fjall keyspaces with independent retention policies.
