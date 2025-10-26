# Spec: Iggy Client Utilities

## Goal
Provide reusable producer/consumer components for interacting with Iggy streams defined in `specs/task_03.md`. These utilities should handle connection management, retries, backpressure, and graceful shutdown so API, worker, and operator services can share the same code path.

## Scope
1. New crate `crates/fetchbox_iggy` wrapping the official Iggy client.
2. Producer abstraction for publishing `DownloadTask`, `JobStatus`, `JobLog`, and `DeadLetterTask`.
3. Consumer abstraction supporting async stream interface, backpressure, and graceful shutdown for `jobs.tasks`, `jobs.status`, `jobs.logs`, `jobs.dlq`.
4. Retry/backoff strategy and connection health checks.
5. Optional CLI helper or example for direct producers.

## 1. Crate Layout
- `crates/fetchbox_iggy`
  - `lib.rs` exporting producer/consumer builders.
  - `config.rs` (endpoint, auth, TLS options).
  - `producer.rs`
  - `consumer.rs`
  - `errors.rs`
  - `health.rs`
  - `cli.rs` (optional example `fetchbox-iggy-cli` binary)

Dependencies: official `iggy` Rust client (if available) or HTTP/gRPC? Need to confirm. Suppose there is crate `iggy`. Include `tokio`, `futures`, `thiserror`, `prost` (for message structs), `tracing`.

## 2. Producer Abstraction

### Builder
```rust
pub struct ProducerBuilder {
    pub endpoint: String,
    pub client_id: String,
    pub client_secret: SecretString,
    pub default_stream: StreamId,
    pub compression: Option<Compression>,
    pub retries: RetryPolicy,
}
```

`RetryPolicy` includes `max_retries`, `base_backoff_ms`, `max_backoff_ms`.

### API
```rust
pub struct FetchboxProducer {
    inner: iggy::Producer,
}

impl FetchboxProducer {
    pub async fn publish_download_task(&self, task: &DownloadTask) -> Result<()>;
    pub async fn publish_job_status(&self, status: &JobStatus) -> Result<()>;
    pub async fn publish_job_log(&self, log: &JobLog) -> Result<()>;
    pub async fn publish_dead_letter(&self, dead: &DeadLetterTask) -> Result<()>;
}
```

Implementation details:
- Accept `partition_key: Option<&str>` for tasks (default `job_id`).
- Serialize messages via `prost::Message::encode`.
- Wrap `iggy::Producer::send` with retry/backoff (exponential).
- Emit tracing spans/tags for stream/partition.
- Provide `health_check()` that performs a lightweight metadata call to Iggy.

## 3. Consumer Abstraction

### Stream Consumer
```rust
pub struct ConsumerBuilder {
    pub endpoint: String,
    pub client_id: String,
    pub client_secret: SecretString,
    pub stream: StreamId,
    pub consumer_group: Option<String>,
    pub batch_size: u16,
    pub ack_mode: AckMode,
    pub retries: RetryPolicy,
}

pub struct FetchboxConsumer<T> {
    stream: StreamId,
    decoder: fn(Vec<u8>) -> Result<T>,
    // internal state...
}
```

Expose async iterator:
```rust
impl<T> FetchboxConsumer<T> {
    pub async fn next(&mut self) -> Result<Option<Message<T>>>;
}

pub struct Message<T> {
    pub payload: T,
    pub offset: u64,
    pub partition: u32,
    pub ack: AckHandle,
}
```

`AckHandle` ack/nack to Iggy. Provide manual and auto-ack modes.

Consumers used by:
- Worker to read `jobs.tasks`.
- Ledger updater to read `jobs.status`.
- Log streaming service to read `jobs.logs`.

Implement backpressure by limiting in-flight messages to `batch_size`. Provide `ack().await` to release.

## 4. Retry & Backpressure
- Use `tokio::time::sleep` with exponential backoff on connection failures.
- For publish retries, ensure idempotency by not mutating payload; log warnings after exhaustion.
- For consumers, on decode errors, send message to DLQ (if tasks) or log error + skip (if logs/status).

## 5. Graceful Shutdown
- Provide `shutdown()` method to close connections.
- Hook into `tokio::signal` in API/worker crates to call shutdown.
- Consumers should break `next()` loops when shutdown triggered.

## 6. CLI Helper (Optional)
- Feature-flagged binary `fetchbox-iggy-cli` with commands:
  - `publish-task --file task.json`
  - `tail-status --job-id ...`
- Useful for manual testing/direct producers.

## 7. Docs & Tests
- `docs/messaging.md` should be updated to reference this crate’s usage.
- Unit tests mocking Iggy client (if possible) or using `iggy-testkit`.
- Integration test that spins up local Iggy (docker) and ensures publish/consume works (optional for later).

## 8. Deliverables
- `crates/fetchbox_iggy` with producer/consumer APIs, retry logic, tests.
- (Optional) CLI binary.
- Documentation snippet for developers explaining how to obtain a producer/consumer.
