# FetchBox Specification Overview

This document indexes all specifications and provides a complete guide to FetchBox's architecture.

## Project Summary

**FetchBox** is a self-contained download orchestration service that accepts job manifests via HTTP API, distributes download tasks to an embedded worker pool, and stores results in S3-compatible storage while tracking state in Fjall databases.

### Key Features

- **Single-process architecture** - API + Workers in same binary
- **No external dependencies** - Self-contained with embedded databases
- **Tower-based workers** - Automatic retries, rate limiting
- **Three Fjall databases** - Separate queue, ledger, and DLQ
- **Extensible handlers** - Custom job expansion logic
- **Production-ready** - Observability, testing, documentation

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

## Technology Stack

- **Language**: Rust (edition 2024, toolchain 1.90.0)
- **Web Framework**: Axum 0.8.6 (HTTP API)
- **Async Runtime**: Tokio 1.48.0 (async/await)
- **Databases**: Fjall 2.11.2 (embedded KV store, 3 instances)
- **Middleware**: Tower (retries, rate limiting)
- **Storage**: Arrow object_store 0.12.4 (S3-compatible)
- **Serialization**: Prost (protobuf), Serde (JSON/TOML)
- **Configuration**: config, toml, dotenvy
- **Observability**: tracing, metrics

## Specification Index

### Core Implementation

| File | Description | Status | Lines |
|------|-------------|--------|-------|
| [task_01_api_contract.md](task_01_api_contract.md) | HTTP API endpoints, validation, request/response contracts | ✅ Implemented | ~500 |
| [task_02_handlers.md](task_02_handlers.md) | Handler trait system for custom job expansion logic | ✅ Implemented | ~400 |
| [task_03_queue_workers.md](task_03_queue_workers.md) | Queue/DLQ databases, TaskBroker, Tower-based workers | 📝 New Spec | 916 |
| [task_04_ledger.md](task_04_ledger.md) | Job state persistence, logs, idempotency keys | ✅ Implemented | 779 |
| [task_05_configuration.md](task_05_configuration.md) | Config loading (TOML + env), validation, proxy resolution | ✅ Implemented | ~600 |

### Supporting Systems

| File | Description | Status | Lines |
|------|-------------|--------|-------|
| [task_06_storage.md](task_06_storage.md) | S3-compatible storage abstraction, handler overrides | ✅ Implemented | ~300 |
| [task_07_failure_handling.md](task_07_failure_handling.md) | Error taxonomy, retry policies, DLQ rules | 📝 Update needed | ~400 |
| [task_08_observability.md](task_08_observability.md) | Metrics, structured logging, tracing integration | 📝 Update needed | ~400 |

### Development & Operations

| File | Description | Status | Lines |
|------|-------------|--------|-------|
| [task_09_development_testing.md](task_09_development_testing.md) | Dev environment (Docker), integration tests, load testing | 📝 New Spec | ~800 |
| [task_10_documentation.md](task_10_documentation.md) | Deployment guides, runbooks, API documentation | 📝 Update needed | ~300 |

**Total**: 10 specifications (consolidated from 14)

### Removed/Merged Specifications

- ~~task_06 (Axum Integration)~~ → Merged into task_01 (redundant with API contract)
- ~~task_07 (Iggy Client)~~ → **Deleted** (Iggy removed, using in-memory channels)
- ~~task_08 (Worker)~~ → Merged into task_03 (now comprehensive queue/worker spec)
- ~~task_12 (Dev Environment)~~ → Merged into task_09
- ~~task_13 (Testing)~~ → Merged into task_09

## Specification Glossary

### task_01_api_contract.md
**API Contract & Validation**

Defines the HTTP API surface:
- `POST /jobs` - Submit job manifests
- `GET /operators/jobs/{job_id}` - Query job status
- `GET /operators/jobs/{job_id}/logs` - Stream job logs
- `GET /operators/health` - Health check endpoint

Covers request validation, header extraction, content encoding, size limits, and error responses.

**Key Topics**: Axum handlers, request validation, manifest schema, error codes

---

### task_02_handlers.md
**Handler Trait System**

Defines the `JobHandler` trait for custom manifest expansion:
- `prepare()` - Validate and prepare manifest
- `build_tasks()` - Expand manifest into download tasks

Includes `DefaultHandler` implementation, handler registry, and configuration.

**Key Topics**: Trait definition, task expansion, handler configuration, type safety

---

### task_03_queue_workers.md
**Queue & Worker System** ⭐ *Comprehensive*

The core of FetchBox's task processing:

**Fjall Queue (queue.db)**:
- Sequential task storage with u64 IDs
- Atomic sequence counter
- 7-day retention

**Fjall DLQ (dlq.db)**:
- Isolated failure storage
- Failure analytics by error code
- 90-day retention
- Task replay capability

**Task Broker**:
- In-memory mpsc channels
- Round-robin distribution
- Backpressure via bounded channels

**Worker Pool**:
- Tower Service implementation
- Retry middleware (exponential backoff)
- Rate limiting per worker
- Graceful shutdown

**Key Topics**: Fjall databases, mpsc channels, Tower middleware, protobuf schemas

---

### task_04_ledger.md
**Ledger Database**

Job state persistence (ledger.db):

**Partitions**:
- `jobs` - Job snapshots (JobState, progress counters)
- `logs` - Structured logs per job (with offsets)
- `idempotency` - Idempotency key mapping

**Operations**:
- Job CRUD (create, read, update)
- Log append and query
- Idempotency checking
- Pruning/retention

**Key Topics**: Job snapshots, log streaming, idempotency, retention policies

---

### task_05_configuration.md
**Configuration System**

Layered configuration loading:
- Defaults → TOML file → .env file → Environment variables

**Config Sections**:
- Server (bind address)
- Queue (worker count, retries, rate limits)
- Ledger (paths, retention)
- DLQ (path, retention, size limits)
- Storage (S3 credentials, buckets)
- Handlers (job type mappings)
- Proxies (pools, fallbacks)

**Validation**:
- Cross-field validation
- Proxy cycle detection
- Handler reference checking

**Key Topics**: Config loading, validation, proxy resolution, secrets management

---

### task_06_storage.md
**Storage Abstraction**

S3-compatible storage via Arrow object_store:

**Features**:
- Manifest upload/download
- Resource streaming upload
- Handler storage overrides (custom bucket/key)
- Multipart upload support
- Error classification

**Storage Keys**:
- Manifests: `manifests/{job_type}/{job_id}/metadata.json`
- Resources: `resources/{job_type}/{job_id}/{resource_id}`

**Key Topics**: Object store integration, streaming uploads, handler overrides

---

### task_07_failure_handling.md
**Failure Taxonomy & DLQ**

Error classification and handling:

**Error Categories**:
- Transient (network, 5xx) → Retry
- Permanent (4xx, DNS) → DLQ immediately
- Throttled (429) → Backoff

**Retry Strategy**:
- Tower retry middleware
- Exponential backoff
- Configurable max attempts

**DLQ Rules**:
- Permanent failures
- Retry exhaustion
- Failure analytics

**Key Topics**: Error codes, retry policies, DLQ criteria, Tower integration

---

### task_08_observability.md
**Observability & Metrics**

Structured logging and metrics:

**Logging**:
- tracing-subscriber with env filter
- Structured fields (job_id, worker_id, etc.)
- Log levels (trace, debug, info, warn, error)
- Log streaming via ledger.db

**Metrics** (Prometheus format):
- `fetchbox_jobs_accepted_total`
- `fetchbox_tasks_completed_total`
- `fetchbox_tasks_failed_total`
- `fetchbox_queue_depth`
- `fetchbox_dlq_size`
- `fetchbox_worker_busy_count`

**Key Topics**: tracing, metrics, Prometheus export, log aggregation

---

### task_09_development_testing.md
**Development Environment & Testing** ⭐ *Consolidated*

Complete development setup:

**Dev Environment**:
- Docker Compose (MinIO, Prometheus, Grafana)
- Makefile targets (dev-up, dev-down, run, test)
- Environment configuration (.env)

**Sample Handler**:
- Gallery handler example
- Custom storage hints
- Proxy configuration

**Integration Tests**:
- End-to-end pipeline tests
- Idempotency testing
- Retry and DLQ testing
- Test helpers and utilities

**Load Testing**:
- K6 scripts
- Performance benchmarks

**CI/CD**:
- GitHub Actions workflows
- Automated testing

**Key Topics**: Docker Compose, integration tests, load testing, CI/CD

---

### task_10_documentation.md
**Documentation & Runbooks**

Operator and developer documentation:

**Deployment Guides**:
- Single instance deployment
- Multi-instance with load balancer
- Configuration best practices

**Runbooks**:
- Common operations
- Troubleshooting
- DLQ management
- Backup/restore procedures

**API Documentation**:
- OpenAPI/Swagger specs
- Client examples (curl, SDK)

**Key Topics**: Deployment, operations, troubleshooting, API docs

---

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

## Implementation Status

### ✅ Completed (6 specs)
- task_01 - API Contract
- task_02 - Handlers
- task_04 - Ledger
- task_05 - Configuration
- task_06 - Storage
- task_09 - Development & Testing

### 📝 New/Updated Specs (4 specs)
- task_03 - Queue & Workers (new comprehensive spec)
- task_07 - Failure Handling (needs Tower retry updates)
- task_08 - Observability (needs ledger log integration)
- task_10 - Documentation (needs single-process deployment guide)

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
⚠️ **No work stealing** - Tasks stay with their instance

**Mitigation**: These trade-offs are acceptable for most use cases. Horizontal scaling provides sufficient capacity.

## Next Steps

1. **Complete remaining implementations** (task_03 - Queue & Workers)
2. **Update specs** (task_07, 08, 10 for new architecture)
3. **Integration testing** (end-to-end pipeline verification)
4. **Production deployment** (single instance → multi-instance)

---

**Last Updated**: 2025-11-09
**Architecture Version**: v2 (Single-Process Design)
