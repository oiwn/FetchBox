# Spec: Handler Pipeline & State Machine (Single Process)

This document supersedes the earlier handler spec and aligns with the decisions captured in `specs/queue_single_process_design.md`.

## 1. Goal
Expose a single handler interface that lets embedders define their own manifest → task → worker pipeline while FetchBox core handles persistence (Fjall), task fan-out via channels, retries through Tower middleware, and ledger updates. Handlers must be able to mutate manifests, store metadata, define tasks, execute them, and finalize jobs without touching API wiring.

## 2. Architecture Alignment
- API contract stays the same. After validation, the API hands the manifest to the handler; the API module itself (`src/api/mod.rs`) remains untouched.
- Tasks are persisted inside the local TasksStorage (Fjall-backed, UUIDv7 keys per `specs/queue_single_process_design.md`) and immediately forwarded to worker inboxes (`tokio::mpsc` channels). There is no external broker.
- Workers consume their inboxes, run a Tower service stack (rate limit + retry), and call back into the handler for the actual task logic.
- Ledger updates and DLQ movement happen in the orchestrator; handlers return typed results and errors.

```
Client POST /jobs
        ↓
Axum handler (initial validation + ledger snapshot)
        ↓
Handler.prepare_manifest → Handler.store_metadata → Handler.build_tasks
        ↓                                 ↓
 ledger writes                  TasksStorage (uuidv7 → TaskRecord)
        ↓                                 ↓
TaskBroker.enqueue(task_id, TaskRecord)
        ↓
Round-robin channel send → Worker Inbox (mpsc)
        ↓
Worker Tower stack → Handler.handle_task
        ↓
ledger/task log + optional storage/object operations
```

## 3. Shared Type Aliases & Data Structures

```rust
pub type JobId = String;
pub type TenantId = String;
pub type TaskId = uuid::Uuid;
pub type ManifestBlob = serde_json::Value; // API enforces JSON today; can swap to raw bytes later
pub type HandlerMetadata = std::collections::BTreeMap<String, String>;

pub struct ManifestEnvelope {
    pub job_id: JobId,
    pub tenant: TenantId,
    pub manifest: ManifestBlob,
    pub received_at: chrono::DateTime<chrono::Utc>,
}

pub struct PreparedManifest {
    pub envelope: ManifestEnvelope,
    /// Handler-specific state derived during prepare (e.g., normalized URLs, derived defaults, secrets refs).
    pub handler_context: ManifestBlob,
}

pub struct StoredManifest {
    pub prepared: PreparedManifest,
    /// Fully-qualified object-store path (e.g., `s3://bucket/key`). Optional because handlers may store elsewhere.
    pub manifest_location: Option<String>,
    /// Lightweight metadata mirrored into the ledger for operators (ordered for stable hashing).
    pub metadata: HandlerMetadata,
}

pub struct HandlerTask {
    pub task_id: TaskId,
    /// Handler-defined payload (JSON for now; could be CBOR/bytes later) interpreted by `handle_task`.
    pub payload: ManifestBlob,
    /// Arbitrary key/value metadata that the worker may use (headers, tags, MIME hints, etc.).
    pub metadata: HandlerMetadata,
    /// Optional override describing where to persist the fetched resource.
    pub storage_hint: Option<crate::handlers::StorageHint>,
    /// Optional proxy selection hint (primary + fallbacks) kept separate for networking middleware.
    pub proxy_hint: Option<crate::handlers::ProxyHint>,
}

pub struct TaskInvocation {
    /// UUIDv7 assigned when persisting into TasksStorage.
    pub task_id: TaskId,
    pub job_id: JobId,
    pub tenant: TenantId,
    pub task: HandlerTask,
    pub attempt: u32,
}

pub struct TaskOutcome {
    pub status: TaskStatus,
    /// Structured success payload (checksum, byte counts, derived metadata).
    pub output: Option<ManifestBlob>,
    pub error: Option<TaskError>,
}
```

`HandlerTask` values are persisted as `queue::TaskRecord` (protobuf) when enqueued. The orchestrator is responsible for serialization and assigning `seq` numbers; handlers only deal with structured data.

## 4. Handler Trait (Single Entry Point)

Handlers must implement every stage of the job lifecycle. We no longer split "job handler" vs "task executor"; the same type handles ingestion and worker callbacks so any shared state/config lives in one place.

```rust
use async_trait::async_trait;

#[async_trait]
pub trait JobHandler: Send + Sync {
    async fn prepare_manifest(
        &self,
        envelope: ManifestEnvelope,
    ) -> Result<PreparedManifest, HandlerError>;

    async fn store_metadata(
        &self,
        prepared: PreparedManifest,
        storage: &crate::storage::StorageClient,
    ) -> Result<StoredManifest, HandlerError>;

    async fn build_tasks(
        &self,
        stored: StoredManifest,
    ) -> Result<Vec<HandlerTask>, HandlerError>;

    async fn handle_task(
        &self,
        invocation: TaskInvocation,
        io: TaskIo,
    ) -> Result<TaskOutcome, HandlerError>;

    async fn finalize_job(
        &self,
        report: JobReport,
    ) -> Result<(), HandlerError>;
}
```

`TaskIo` exposes only the dependencies that every handler needs during task execution (storage, ledger, outbound HTTP). Metrics/tracing stay in the orchestrator layers so handler logic remains portable.

```rust
pub struct TaskIo<'a> {
    pub storage: &'a crate::storage::StorageClient,
    pub ledger: &'a crate::ledger::LedgerStorage,
    pub http: &'a reqwest::Client,
}
```


`JobReport` contains the final snapshot (`JobId`, totals, error summaries) gathered by the orchestrator before `finalize_job` runs so custom handlers can emit domain-specific rollups or downstream events.

## 5. Lifecycle Stages
1. **Prepare Manifest** – Runs inside the API task. Handler validates the opaque JSON, may generate IDs, expands defaults, and returns `PreparedManifest`. Failure keeps the job in `JobQueued` and responds with `4xx/5xx`.
2. **Store Metadata** – Handler persists the manifest/metadata via the provided `StorageClient`. The result (`StoredManifest`) carries a manifest location recorded in the ledger snapshot.
3. **Build Tasks** – Handler emits `HandlerTask` values. The orchestrator wraps them into `TaskRecord` protobufs, assigns UUIDv7 identifiers via `TasksStorage::enqueue`, and forwards them to the `TaskBroker` (which immediately fans out tasks across inbox channels).
4. **Handle Task** – Each worker loops on its `mpsc::Receiver`, passes the task through a Tower stack (rate limit + retry policy), and calls `handler.handle_task`. The handler uses `TaskIo` to download/upload data, write logs, etc. Returning `Err(HandlerError)` lets Tower decide whether to retry or drop into DLQ.
5. **Finalize Job** – Once the broker observes all tasks in terminal states (success, DLQ), the orchestrator composes a `JobReport` and calls `finalize_job` (best-effort). Failures here are logged but do not revert completion.

## 6. State Machine
State is persisted in the ledger (`job_snapshot.status`). Events map directly to handler stages:

| State                | Description                                             | Trigger / Next State                    |
|----------------------|---------------------------------------------------------|-----------------------------------------|
| `JobQueued`          | Manifest accepted, awaiting handler                     | API entry → `ManifestPreparing`         |
| `ManifestPreparing`  | Running `prepare_manifest`                              | success → `ManifestStored`, error → `JobFailed` |
| `ManifestStored`     | Metadata persisted via `store_manifest`                 | success → `TasksBuilding`, error → `JobFailed`  |
| `TasksBuilding`      | `build_tasks` executing                                 | success → `TasksDispatched`, error → `JobFailed`|
| `TasksDispatched`    | All tasks persisted + dispatched to channels            | worker start → `TaskRunning` or (no tasks) → `JobFinalizing` |
| `TaskRunning`        | At least one task in-flight                             | each completion updates counters; if retry scheduled → `TaskRetrying` |
| `TaskRetrying`       | Tower retry/backoff active                              | retry success → `TaskRunning`, retries exhausted → `JobFailing` |
| `JobFailing`         | DLQ/terminal failure encountered                        | orchestrator writes failure, → `JobFailed` |
| `JobFinalizing`      | All tasks done, `finalize_job` running                  | success → `JobCompleted`, error → `JobCompleted` (with warning) |
| `JobCompleted`       | Terminal success                                        | -                                       |
| `JobFailed`          | Terminal failure                                        | -                                       |

`TaskRetrying` and `JobFailing` are observable so metrics/alerts can fire when retries pile up.

## 7. Default Handler
- Lives in `src/handlers/default.rs` (same module as today).
- Continues to understand the current manifest schema (`manifest_version`, `resources`, `storage` block, optional headers/tags).
- `prepare_manifest`: validates `manifest_version == "v1"`, ensures each resource has `name`/`url`.
- `store_metadata`: uploads the manifest JSON to the configured object storage using the existing `{resource_key_prefix}{manifest_file}` rule and records the full `s3://bucket/key` in metadata.
- `build_tasks`: emits one `HandlerTask` per resource, merging handler-level headers and resource headers. No proxy/storage hints unless provided via config.
- `handle_task`: downloads the resource via `reqwest`, uploads bytes through `StorageClient`, and updates the ledger log (equivalent to the current worker).
- `finalize_job`: no-op for now.

## 8. Registry & Configuration
- `HandlerRegistry` now maps a single logical handler (no `job_type` fan-out yet) to an `Arc<dyn JobHandler>` plus `HandlerConfig` (default headers, storage overrides, etc.).
- `HandlerRegistry::with_defaults()` registers one instance of `DefaultHandler` that both the API and workers clone.
- Future work (separate spec) will reintroduce multi-handler routing once we have a routing key; for now everything uses the default entry to keep the single-process design simple.

## 9. Testing & Verification
- **Unit**: `DefaultHandler` tests cover `prepare_manifest`, `build_tasks`, and `handle_task` happy/negative paths with mocked storage.
- **Integration**: spawn the single-process runtime with a stub handler that records lifecycle invocations; ensure state transitions fire in order and tasks flow through channels.
- **Worker retry**: force `handle_task` to fail N times, assert Tower retries, DLQ insertion via `DlqStorage::record`, and status transitions (`TaskRunning` → `TaskRetrying` → `JobFailing`).
- **E2E**: API ingest → handler builds tasks → worker downloads fixture files → storage upload → ledger snapshot updates. These tests ensure the handler trait, TaskBroker, and state machine stay in sync.

## 10. Future Enhancements
- Load handler implementations from `config/` so operators can select different handlers per deployment.
- Stream task generation for extremely large manifests (emit tasks incrementally to the queue channels).
- Structured tracing between handler stages (link job_id/task_id into OpenTelemetry spans).
- Optional wasm sandbox for untrusted handler logic once ABI is stable.
