# Spec: Queue & Worker System

## Goal
Design and implement a single-process queue and worker system using Fjall for persistence and in-memory channels for task distribution. This replaces the original Iggy-based messaging architecture with a simpler, self-contained solution that runs API server and workers in the same binary.

## Scope
1. **Fjall Queue Database** (`queue.db`) - Active task queue with sequential IDs
2. **Fjall DLQ Database** (`dlq.db`) - Isolated dead letter queue for failed tasks
3. **Task Broker** - In-memory channel distributor using tokio mpsc
4. **Worker Pool** - Tower service-based workers with retry middleware
5. **Protobuf Schemas** - Message definitions for tasks and failures
6. **Control Channels** - Graceful shutdown and health monitoring

## Architecture Overview

```
┌──────────────────────────────────────────────────────────────┐
│  FetchBox Service (single process)                           │
│                                                               │
│  ┌─────────────────────────────────────────────────────┐    │
│  │ main.rs                                             │    │
│  │                                                     │    │
│  │  tokio::runtime::Runtime                           │    │
│  │  ├─ spawn(axum_server())                           │    │
│  │  ├─ spawn(task_broker())                           │    │
│  │  ├─ spawn(worker(1))                               │    │
│  │  ├─ spawn(worker(2))                               │    │
│  │  └─ spawn(worker(N))  // configurable              │    │
│  └─────────────────────────────────────────────────────┘    │
│                                                               │
│  ┌─────────────────┐    ┌──────────────┐   ┌─────────────┐ │
│  │  Axum API       │    │ Task Broker  │   │  Workers    │ │
│  │                 │    │              │   │             │ │
│  │  POST /jobs ────┼───→│ Queue Tasks  │───→ Inbox (mpsc)│ │
│  │                 │    │              │   │             │ │
│  │  GET /status    │    │ Round-robin  │   │ Tower       │ │
│  │                 │    │ delivery     │   │ Middleware  │ │
│  └─────────────────┘    └──────────────┘   └─────────────┘ │
│                                                ↓   ↓         │
│  ┌──────────────────────────────────────────────────────┐   │
│  │ Fjall Storage (3 databases)                          │   │
│  │                                                      │   │
│  │  queue.db/          ledger.db/        dlq.db/       │   │
│  │  ├─ tasks           ├─ jobs           ├─ failed     │   │
│  │  └─ metadata        ├─ logs           ├─ metadata   │   │
│  │                     └─ idempotency    └─ analysis   │   │
│  │                                                      │   │
│  │  [Active Queue]     [Job State]       [Failures]    │   │
│  │   7 day retention   30 day retention  90 day retention│   │
│  └──────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────┘
```

## 1. Protobuf Schemas

File: `proto/jobs.proto` (already exists, minimal changes needed)

### Core Message Types

```proto
syntax = "proto3";
package fetchbox.jobs;

message HttpHeader {
  string name = 1;
  string value = 2;
}

message StorageHint {
  string bucket = 1;
  string key_prefix = 2;
  map<string, string> metadata = 3;
}

message ProxyHint {
  string primary_pool = 1;
  repeated string fallbacks = 2;
}

message TaskAttributes {
  map<string, string> tags = 1;
  string checksum_hint = 2;
  string mime_hint = 3;
  bytes extra = 4;
}

message DownloadTask {
  string job_id = 1;
  string job_type = 2;
  string resource_id = 3;
  string url = 4;
  repeated HttpHeader headers = 5;
  ProxyHint proxy_hint = 6;
  StorageHint storage_hint = 7;
  TaskAttributes attributes = 8;
  string manifest_key = 9;
  uint32 attempt = 10;
  string tenant = 11;
  string trace_id = 12;
}

message DeadLetterTask {
  DownloadTask task = 1;
  string failure_code = 2;
  string failure_message = 3;
  uint32 attempts = 4;
  uint64 failed_at_ms = 5;
}

enum JobState {
  JOB_STATE_UNKNOWN = 0;
  JOB_STATE_QUEUED = 1;
  JOB_STATE_IN_PROGRESS = 2;
  JOB_STATE_PARTIAL = 3;
  JOB_STATE_COMPLETED = 4;
  JOB_STATE_FAILED = 5;
}

enum LogLevel {
  LOG_LEVEL_TRACE = 0;
  LOG_LEVEL_DEBUG = 1;
  LOG_LEVEL_INFO = 2;
  LOG_LEVEL_WARN = 3;
  LOG_LEVEL_ERROR = 4;
}

message JobLog {
  string job_id = 1;
  string resource_id = 2;
  LogLevel level = 3;
  string message = 4;
  map<string, string> fields = 5;
  uint64 timestamp_ms = 6;
  string trace_id = 7;
}
```

**Note**: `JobStatus` and `JobLog` messages go to the Fjall ledger, not a message queue.

## 2. Tasks Storage (Fjall)

### Schema: `queue.db/`

Module: `src/queue/tasks_storage.rs`

```rust
pub struct TasksStorage {
    keyspace: Keyspace,
    tasks: PartitionHandle,
}

impl TasksStorage {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let keyspace = Config::new(path).open()?;
        let tasks = keyspace.open_partition("tasks", PartitionCreateOptions::default())?;
        Ok(Self { keyspace, tasks })
    }

    /// Persist task and return UUIDv7 identifier
    pub fn enqueue(&self, task: &DownloadTask) -> Result<Uuid> {
        let task_id = Uuid::now_v7();
        self.tasks.insert(task_id.as_bytes(), task.encode_to_vec())?;
        Ok(task_id)
    }

    pub fn get_task(&self, task_id: Uuid) -> Result<Option<DownloadTask>> {
        self.tasks
            .get(task_id.as_bytes())?
            .map(|bytes| DownloadTask::decode(&*bytes))
            .transpose()
            .map_err(Into::into)
    }

    pub fn flush(&self) -> Result<()> {
        self.keyspace.persist(fjall::PersistMode::SyncAll)?;
        Ok(())
    }
}
```

**Design Notes**:
- UUIDv7 IDs preserve chronological ordering without shared counters or metadata partitions.
- Fjall handles concurrency; API callers wrap the storage in an `Arc<RwLock<_>>` to ensure consistent writes.
- Append-only design keeps queue traffic isolated from ledger persistence and simplifies crash recovery.
- Retention/cleanup can iterate by UUID ordering or rely on time-based pruning.

## 3. Fjall DLQ Database

### Schema: `dlq.db/`

Module: `src/queue/dlq_storage.rs`

```rust
pub struct DlqStorage {
    keyspace: Keyspace,
    entries: PartitionHandle,
}

impl DlqStorage {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let keyspace = Config::new(path).open()?;
        let entries = keyspace.open_partition("dlq", PartitionCreateOptions::default())?;
        Ok(Self { keyspace, entries })
    }

    pub fn record(
        &self,
        task_id: Uuid,
        task: DownloadTask,
        failure_code: String,
        failure_message: String,
        attempts: u32,
    ) -> Result<()> {
        let entry = DeadLetterTask {
            task: Some(task),
            failure_code,
            failure_message,
            attempts,
            failed_at_ms: now_ms(),
        };
        self.entries.insert(task_id.as_bytes(), entry.encode_to_vec())?;
        Ok(())
    }

    pub fn list(&self, limit: usize) -> Result<Vec<(Uuid, DeadLetterTask)>> {
        self.entries
            .iter()
            .take(limit)
            .map(|kv| {
                let (key, value) = kv?;
                let task_id = Uuid::from_slice(key.as_ref()).map_err(DlqError::InvalidUuid)?;
                let task = DeadLetterTask::decode(&*value)?;
                Ok((task_id, task))
            })
            .collect()
    }
}
```

**Design Notes**:
- Keys share the same UUIDv7 as the originating task so correlation is trivial.
- No side partitions; analytics can be layered later by iterating the partition.
- Replay is performed by reading the entry, resetting `attempt`, and re-enqueuing via `TasksStorage`.
- Retention mirrors ledger policy (90 days) but can be tuned independently.

## 4. Task Broker

### In-Memory Channel Distribution

Module: `src/queue/broker.rs`

```rust
pub struct TaskBroker {
    tasks: Arc<RwLock<TasksStorage>>,
    dlq: Arc<DlqStorage>,
    worker_channels: Vec<mpsc::Sender<TaskEnvelope>>,
    next_worker: AtomicUsize,
}

#[derive(Clone, Debug)]
pub struct TaskEnvelope {
    pub task_id: Uuid,
    pub task: DownloadTask,
}

impl TaskBroker {
    pub fn new(
        tasks: Arc<RwLock<TasksStorage>>,
        dlq: Arc<DlqStorage>,
        num_workers: usize,
        channel_size: usize,
    ) -> (Self, Vec<mpsc::Receiver<TaskEnvelope>>) { /* ... */ }

    pub async fn enqueue(&self, task: DownloadTask) -> Result<Uuid, QueueError> {
        let task_id = {
            let tasks = self.tasks.write().await;
            tasks.enqueue(&task)?
        };

        let envelope = TaskEnvelope { task_id, task: task.clone() };
        let worker_idx = self.next_worker.fetch_add(1, Ordering::Relaxed)
            % self.worker_channels.len();

        self.worker_channels[worker_idx]
            .send(envelope)
            .await
            .map_err(|_| QueueError::WorkerChannelClosed)?;

        Ok(task_id)
    }
}
```

**Design Notes**:
- Broker sits on the API hot-path; it persists first, then performs round-robin fan-out over bounded mpsc queues.
- Tasks storage is behind an `RwLock` to serialize Fjall writes while allowing future read APIs.
- DLQ is injected for future replay APIs even though enqueue does not yet use it directly.
- Bounded channels (configurable, default 16) enforce backpressure if workers stall.

## 5. Worker Pool (Tower Services)

### Worker Implementation

Module: `src/worker/mod.rs`

```rust
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

fn build_service_stack(base: DownloadService, cfg: &WorkerConfig)
    -> BoxService<TaskEnvelope, TaskOutcome, WorkerError>
{
    ServiceBuilder::new()
        .layer(RateLimitLayer::new(cfg.rate_limit_per_worker, Duration::from_secs(1)))
        .layer(TimeoutLayer::new(Duration::from_millis(cfg.task_timeout_ms)))
        .service(base)
        .map_err(map_stack_error)
        .boxed()
}

impl DownloadWorker {
    async fn execute_with_retries(
        &mut self,
        env: TaskEnvelope,
    ) -> Result<(TaskOutcome, u32), (WorkerError, u32)> {
        let mut attempt = 0u32;
        let max_attempts = self.max_retries.saturating_add(1);

        loop {
            attempt += 1;
            let svc = self.service.ready().await.map_err(|err| (err, attempt))?;
            match svc.call(env.clone()).await {
                Ok(outcome) => return Ok((outcome, attempt)),
                Err(err) if attempt < max_attempts && err.is_retryable() => {
                    let backoff = self.retry_backoff.mul_f64(2f64.powi((attempt - 1) as i32));
                    tokio::time::sleep(backoff).await;
                }
                Err(err) => return Err((err, attempt)),
            }
        }
    }

    async fn record_success(&self, env: &TaskEnvelope, outcome: TaskOutcome, attempts: u32) {
        // Acquire the ledger lock, update counters, flip status to Completed when appropriate.
    }

    async fn record_failure(&self, env: &TaskEnvelope, err: &WorkerError, attempts: u32) {
        // Increment failed counters, append JobError, mark snapshot failed.
    }

    async fn send_to_dlq(&self, env: TaskEnvelope, err: &WorkerError, attempts: u32) {
        // Persist DeadLetterTask keyed by env.task_id via DlqStorage.
    }
}
```

**Design Notes**
- Tower handles rate limiting + deadlines; retries/backoff stay explicit so DLQ entries capture accurate attempt counts and error codes.
- Services are boxed, making it easy to inject mocks in unit tests or bolt on new middleware (buffer, hedge, etc.).
- Ledger updates happen behind a shared mutex to keep `JobSnapshot` counters consistent when multiple workers finish simultaneously.
- Unit tests now exercise retry logic, ledger updates, and DLQ persistence via a TempDir-backed harness.

## 6. Main Binary Integration

- `src/main.rs` loads `Config`, including `queue.path`, `queue.workers`, `queue.channel_size`, and nested `queue.worker` knobs (`rate_limit_per_worker`, `max_retries`, `retry_backoff_ms`, `task_timeout_ms`).
- Fjall stores open via `LedgerStorage::open`, `TasksStorage::open`, and `DlqStorage::open`; the resulting handles are wrapped in `Arc`.
- `TaskBroker::new(tasks, dlq, queue.workers, queue.channel_size)` returns the broker plus per-worker receivers; the broker is shared with Axum `AppState` so the API can enqueue tasks.
- For each receiver, `DownloadWorker::new` is spawned with the shared storage client, ledger lock, DLQ handle, and shutdown watcher.
- Axum routes (`POST /jobs`, operator endpoints) enqueue manifests, while `watch::channel`-driven shutdown ensures workers drain gracefully before exit.

## 7. Configuration

File: `config/fetchbox.toml`

```toml
[server]
bind_addr = "0.0.0.0:8080"

[queue]
path = "data/queue"
workers = 8                    # Worker pool size
rate_limit_per_worker = 10     # requests/sec per worker
max_retries = 3
retry_backoff_ms = 1000

[queue.retention]
completed_days = 7             # Clean up successful tasks

[ledger]
path = "data/ledger"
retention_days_jobs = 30
retention_days_logs = 30
retention_days_idempotency = 14

[dlq]
path = "data/dlq"
retention_days = 90            # Keep failures longer
max_size_gb = 10               # Prevent unbounded growth
enable_metrics = true          # Track failure patterns

[storage]
backend = "s3"
bucket = "fetchbox-artifacts"

[handlers.gallery]
handler = "fetchbox::handlers::DefaultHandler"
proxy.primary = "residential-us"
storage.bucket = "fetchbox-artifacts"
```

## 8. Module Structure

```
src/
├── queue/
│   ├── mod.rs         # Public API, re-exports
│   ├── store.rs       # FjallQueue implementation
│   ├── dlq.rs         # FjallDLQ implementation
│   ├── broker.rs      # TaskBroker with channels
│   └── error.rs       # Queue-specific errors
│
├── worker/
│   ├── mod.rs         # DownloadWorker main loop
│   ├── service.rs     # Tower Service implementation
│   ├── retry.rs       # RetryPolicy for Tower
│   ├── http.rs        # HTTP downloader
│   └── error.rs       # Worker errors
│
├── proto/
│   ├── mod.rs         # Generated protobuf types
│   └── fetchbox.jobs.rs
│
├── ledger/            # Existing (no changes)
├── storage/           # Existing (no changes)
├── handlers/          # Existing (no changes)
├── config/            # Update to remove Iggy config
└── api/               # Update to use TaskBroker
```

## 9. Data Flow

### Job Submission Flow

```
1. Client POST /jobs (manifest)
   ↓
2. Axum handler validates manifest
   ↓
3. Handler expands manifest → N DownloadTasks
   ↓
4. For each task:
   ├─ broker.enqueue(task)
   │  ├─ Persist to queue.db (get seq_id)
   │  └─ Send to worker inbox (mpsc channel)
   ↓
5. Worker receives TaskEnvelope{seq, task}
   ↓
6. Tower middleware:
   ├─ Rate limiting (10 req/sec per worker)
   ├─ Retry logic (exponential backoff, max 3)
   └─ Download → Upload → Log
   ↓
7. On success: Log to ledger
8. On failure: Write to dlq.db
```

### Graceful Shutdown Flow

```
1. SIGTERM/SIGINT received
   ↓
2. shutdown_tx.send(true)
   ↓
3. Workers stop accepting new tasks
   ↓
4. Workers finish in-flight tasks
   ↓
5. Axum server stops accepting connections
   ↓
6. All resources released
```

## 10. Testing Strategy

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_enqueue_dequeue() {
        let tmpdir = tempfile::tempdir().unwrap();
        let queue = FjallQueue::open(tmpdir.path()).unwrap();

        let task = DownloadTask {
            job_id: "job-123".to_string(),
            // ... other fields
        };

        let seq = queue.enqueue(&task).unwrap();
        assert_eq!(seq, 1);

        let retrieved = queue.get_task(seq).unwrap().unwrap();
        assert_eq!(retrieved.job_id, "job-123");
    }

    #[tokio::test]
    async fn test_broker_distribution() {
        let queue = Arc::new(FjallQueue::open(tempfile::tempdir().unwrap().path()).unwrap());
        let (broker, mut receivers) = TaskBroker::new(queue, 2);

        let task = DownloadTask { /* ... */ };
        broker.enqueue(task.clone()).await.unwrap();

        // First worker should receive task
        let envelope = receivers[0].recv().await.unwrap();
        assert_eq!(envelope.task.job_id, task.job_id);
    }
}
```

### Integration Tests

```rust
#[tokio::test]
async fn test_end_to_end_task_processing() {
    // 1. Start mock S3 server
    // 2. Initialize queue, ledger, DLQ
    // 3. Spawn workers
    // 4. Submit job via API
    // 5. Wait for completion
    // 6. Verify uploaded to S3
    // 7. Verify ledger updated
}
```

## 11. Deliverables

- ✅ Updated `proto/jobs.proto` (minimal changes from existing)
- ✅ `src/queue/` module with FjallQueue, FjallDLQ, TaskBroker
- ✅ `src/worker/` module refactored to Tower services
- ✅ Updated `src/main.rs` to spawn broker + workers
- ✅ Updated configuration (remove Iggy, add queue config)
- ✅ Unit tests for queue, DLQ, broker
- ✅ Integration tests for full pipeline
- ✅ Documentation updates

## 12. Benefits of This Design

✅ **Self-contained** - Single binary, no external message queue
✅ **Simple** - In-memory channels, no network overhead
✅ **Fast** - Direct task distribution, Fjall persistence
✅ **Reliable** - Tower retries, DLQ for permanent failures
✅ **Debuggable** - Sequential task IDs, isolated DLQ
✅ **Scalable** - Run multiple instances with load balancer
✅ **Observable** - Failure analytics in DLQ, metrics in ledger

## 13. Trade-offs

⚠️ **Not distributed** - Workers tied to API instance
⚠️ **No cross-instance work stealing** - Each instance has own queue

**Mitigation**: Horizontal scaling (multiple instances) + load balancer provides sufficient scalability for most use cases. Vertical scaling (16-32 workers per instance) handles significant load.

## 14. Future Enhancements

- [ ] Task priority queuing
- [ ] Worker health checks endpoint
- [ ] DLQ replay via API endpoint
- [ ] Metrics export (Prometheus)
- [ ] Graceful task preemption
- [ ] Worker auto-scaling based on queue depth
# Spec: Queue & Workers (single process)

> **Implementation note:** The current codebase replaces the original `FjallQueue`/`FjallDLQ` structs described here with `TasksStorage` (UUIDv7-backed task log) and `DlqStorage`. Mentions of `FjallQueue` in this document correspond to those adapters.
