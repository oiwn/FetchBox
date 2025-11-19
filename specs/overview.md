# FetchBox Specification Overview

This document indexes all specifications and provides a complete guide to FetchBox's architecture.

## Project Summary

**FetchBox** is a self-contained download orchestration service that accepts job manifests via HTTP API, distributes download tasks to an embedded worker pool, and stores results in S3-compatible storage while tracking state in Fjall databases.

### Key Features

- **Single-process runtime** – Axum API, TaskBroker, and workers live in one binary.
- **Embedded persistence** – Fjall powers queue, ledger, and DLQ without external brokers.
- **Channel-fed workers** – Tasks move through bounded `tokio::mpsc` inboxes with round-robin delivery.
- **Tower middleware** – Retry, rate limit, and graceful shutdown are implemented as Tower layers per worker.
- **Extensible handlers** – Embedders customize manifest processing + task execution via the handler trait.
- **Operational tooling** – Idempotency, structured logs, and metrics are first-class in the orchestrator.

## Release Prep Snapshot (2025-11-19)

- ✅ Toolchain validation + full `cargo fmt`/`cargo check`/`cargo test --all`/`cargo clippy --all-targets -- -D warnings` suite passed locally, so lint/test gates are already green ahead of tagging.
- ✅ `AGENTS.md` documents the release hygiene (docs, linting, secrets) we expect every contributor to follow.
- 🛠️ Refreshing this overview and `specs/ctx.md` is in progress to keep specs authoritative before README + spec cleanup land.
- 🧹 Deprecated specs (`queue_single_process_design`, `task_09_development_testing`, `task_10_documentation`) were removed after diverging from the shipping single-process + README-based workflows.
- ⏳ README refresh, deprecated-spec removal, and the repo-wide secrets sweep are still pending and tracked in `specs/ctx.md`.

## Architecture Diagram

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
│  │  POST /jobs ────┼───→│ Persist task │───→ Inbox (mpsc)│ │
│  │                 │    │ + assign seq │   │  (Tower svc)│ │
│  │  GET /status    │    │ Round-robin  │   │             │ │
│  │                 │    │ delivery     │   │             │ │
│  └─────────────────┘    └──────────────┘   └─────────────┘ │
│                                                ↓   ↓         │
│  ┌──────────────────────────────────────────────────────┐   │
│  │ Fjall Storage (3 databases)                          │   │
│  │                                                      │   │
│  │  queue.db/          ledger.db/        dlq.db/        │   │
│  │  ├─ tasks           ├─ jobs           ├─ failed      │   │
│  │  └─ metadata        ├─ logs           ├─ metadata    │   │
│  │                     └─ idempotency    └─ analysis    │   │
│  │                                                      │   │
│  │  [Active Queue]     [Job State]       [Failures]     │   │
│  └──────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────┘
```

## Technology Stack

- **Language**: Rust (edition 2024, toolchain 1.90.0)
- **Web Framework**: Axum 0.8.6 (HTTP API)
- **Async Runtime**: Tokio 1.48.0 (async/await)
- **Databases**: Fjall 2.11.2 (embedded KV store, 3 instances)
- **Middleware/Functions Wrapper**: Tower (retries, rate limiting)
- **Storage**: Arrow object_store 0.12.4 (S3-compatible)
- **Serialization**: Prost (protobuf) for queue/DLQ records, Serde for JSON/TOML configs
- **Configuration**: config, toml, dotenvy
- **Observability**: tracing, metrics

## Specification Index

### Core Implementation

| File | Description | Status |
|------|-------------|--------|
| [task_01_api_contract.md](task_01_api_contract.md) | HTTP API endpoints, validation, request/response contracts | ✅ Implemented |
| [task_02_handlers.md](task_02_handlers.md) | Handler pipeline + worker integration | ✅ Implemented |
| [task_03_queue_workers.md](task_03_queue_workers.md) | Fjall queue/DLQ schema, TaskBroker, Tower-based workers | ✅ Implemented |
| [task_04_ledger.md](task_04_ledger.md) | Job state persistence, logs, idempotency keys | ✅ Implemented |
| [task_05_configuration.md](task_05_configuration.md) | Config loading (TOML + env), validation, proxy resolution | ✅ Implemented |

### Supporting Systems

| File | Description | Status |
|------|-------------|--------|
| [task_06_storage.md](task_06_storage.md) | S3-compatible storage abstraction, handler overrides | ✅ Implemented |
| [task_07_failure_handling.md](task_07_failure_handling.md) | Error taxonomy, retry policies, DLQ rules | 🛠️ Refresh in progress |
| [task_08_observability.md](task_08_observability.md) | Metrics, structured logging, tracing integration | 🛠️ Refresh in progress |

### Development & Operations

- Legacy specs for development/test workflows and documentation runbooks were removed (`task_09` + `task_10`) because they referenced the old Docker Compose/Iggy era; current guidance now lives in `README.md`, `AGENTS.md`, and the living plan in `specs/ctx.md`.

## Specification Summaries

- **task_01_api_contract.md** – Defines the Axum endpoints (`POST /jobs`, operator status + health), header semantics, manifest validation, and error responses.
- **task_02_handlers.md** – Documents the single-process handler lifecycle (prepare/store/build/handle/finalize) and how it feeds the Fjall queue + worker inboxes.
- **task_03_queue_workers.md** – Covers Fjall queue/DLQ schema, TaskBroker channel fan-out, Tower-based workers, and retry/termination behavior, matching the shipping implementation (bounded inboxes + Fjall-backed sequencing).
- **task_04_ledger.md** – Explains ledger partitions (jobs, logs, idempotency) and how job state transitions are persisted.
- **task_05_configuration.md** – Details config layering (defaults, file, env) plus validation and proxy resolution.
- **task_06_storage.md** – Describes the object storage abstraction, default in-memory backend, and handler overrides.
- **task_07_failure_handling.md** – Sets the error taxonomy, retry budget, DLQ policies, and alerting hooks; refresh work is queued to capture the new S3 timeout/DLQ signals exercised by `examples/httpbin_minio`.
- **task_08_observability.md** – Enumerates metrics, tracing, and structured logging expectations for API, broker, and workers; the release refresh will map these expectations to concrete `tracing` spans + Fjall metrics.
- **Development/testing + doc runbooks** – The stale specs that referenced Docker Compose, Makefiles, and Iggy tooling have been removed to avoid contradictions; the release plan now tracks README + AGENTS updates directly.

## Examples & Validation

- `examples/httpbin_minio` renders a temporary config, rewrites manifest storage so manifests and resources land under `s3://fetchbox-httpbin/YYYY/MM/DD/httpbin_minio/`, and spins up three workers to mirror the production parallelism.
- The StorageClient now honors `storage.provider = "s3"` by instantiating the real S3 backend (pulling credentials from the environment), so the example exercises the entire persistence pipeline.
- `README.md` documents the runbook (start MinIO, export credentials, run the example, inspect the bucket) and the example highlights both success and timeout failure paths plus Fjall/DLQ state for inspection.
- `timeout.json` provides an intentional failure to prove DLQ/error handling; verbose `lsm_tree` logs stay suppressed so operators focus on FetchBox/Fjall events.

## Data Flow

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
7. On success: Log to ledger.db
8. On failure: Write to dlq.db
```

## Configuration Example

File: `config/fetchbox.toml`

```toml
[server]
bind_addr = "0.0.0.0:8080"

[queue]
path = "data/queue"
workers = 8
rate_limit_per_worker = 10
max_retries = 3
retry_backoff_ms = 1000

[queue.retention]
completed_days = 7

[ledger]
path = "data/ledger"
retention_days_jobs = 30
retention_days_logs = 30
retention_days_idempotency = 14

[dlq]
path = "data/dlq"
retention_days = 90
max_size_gb = 10
enable_metrics = true

[storage]
backend = "s3"
bucket = "fetchbox-artifacts"

[handlers.gallery]
handler = "fetchbox::handlers::DefaultHandler"
proxy.primary = "residential-us"
storage.bucket = "fetchbox-artifacts"
```

## Scaling Strategy

### Vertical Scaling
- Increase worker count: `[queue.workers] = 16` or `32`
- I/O-bound workload scales well
- Limited by system resources

### Horizontal Scaling
```
       ┌─────────────┐
       │   Router    │  (nginx, HAProxy)
       └──────┬──────┘
              │
              ├──→ FetchBox Instance 1 (queue.db#1)
              ├──→ FetchBox Instance 2 (queue.db#2)
              └──→ FetchBox Instance 3 (queue.db#3)
```

Each instance is independent with its own queue.db.

## Architecture Benefits

✅ **Self-contained** - Single binary, no external services
✅ **Simple deployment** - Just run the binary
✅ **Fast** - In-memory channels, no network overhead
✅ **Reliable** - Tower retries, persistent queue, DLQ
✅ **Debuggable** - Sequential task IDs, isolated DLQ
✅ **Scalable** - Horizontal scaling via load balancer
✅ **Observable** - Metrics, logs, failure analytics

## Trade-offs

⚠️ **Not distributed** - Workers tied to API instance
⚠️ **No cross-instance rebalancing** - Tasks published to one instance stay there; scaling uses external routing

**Mitigation**: These trade-offs are acceptable for most use cases. Horizontal scaling provides sufficient capacity.
