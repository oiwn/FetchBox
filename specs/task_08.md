# Spec: Downloader Worker Core

## Goal
Implement the worker service (invoked via `fetchbox worker`) that consumes `jobs.tasks`, downloads resources (respecting proxy configuration and handler hints), uploads them to S3-compatible storage, and emits status/log events. Multiple worker replicas should run independently for horizontal scaling.

## Scope
1. New crate/binary `crates/fetchbox_worker`.
2. Task consumption loop using `fetchbox_iggy` consumer.
3. Proxy selection with fallback pools from config.
4. HTTP download pipeline with retries, timeouts, and header injection.
5. Storage upload via abstraction (default Arrow object store) or handler override.
6. Status/log emission using `fetchbox_iggy` producer + ledger updates via status consumer.
7. Graceful shutdown + metrics.

## 1. Binary Layout
- `src/main.rs` — bootstrap config, handler registry, Iggy consumer/producer, storage client.
- Modules:
  - `task_runner.rs` — orchestrates downloading a single `DownloadTask`.
  - `proxy.rs` — resolves proxies and handles rotation/fallback.
  - `http.rs` — HTTP client utilities (Reqwest-based).
  - `storage.rs` — integrates with storage abstraction.
  - `status.rs` — helper to emit status/log messages.
  - `metrics.rs`.

## 2. Task Consumption
- Use `FetchboxConsumer<DownloadTask>` targeting `jobs.tasks` with consumer group `workers`.
- Concurrency: configurable worker pool (e.g., `max_inflight_tasks` from config, default 32).
- Implementation approach:
  1. Fetch batch of tasks.
  2. Spawn per-task async job (bounded via semaphore).
  3. Ack message only after processing completes successfully; on failure, either retry (by not acking -> redelivery) or ack + publish to DLQ depending on error.
- Maintain per-job context (counts) to aggregate statuses.

## 3. Proxy & HTTP Download
- HTTP client: `reqwest` with `connect_timeout`, `request_timeout`, `tcp_keepalive`.
- For each task:
  - Determine proxy tier using `ProxyResolver` (flattened from config).
  - Attempt download through primary tier; on failure (network or HTTP status in retryable set), rotate to next proxy endpoint.
  - Support custom headers from task + default handler headers.
  - Support optional range downloads? (Not in scope). Basic GET only.
  - Retry policy: up to N attempts (configurable) with exponential backoff.
  - If all proxies fail, mark as fatal -> emit failure status/log, send to DLQ.
- Record metrics: download latency, bytes transferred, proxy failure counts.

## 4. Storage Upload
- After successful download:
  - Optionally compute checksum (SHA-256) if requested.
  - Use storage abstraction to upload stream directly (avoid full buffering when possible).
  - Determine destination bucket/key: prefer handler-provided `StorageHint`; fallback to config default `resources/{job_type}/{job_id}/{resource_id}`.
  - Attach metadata (content-type from headers/tags).
- On failure (e.g., S3 error), apply retry policy (distinct from download).

## 5. Status & Logs
- For each task outcome:
  - Emit `JobLog` entry with details (proxy used, retries, HTTP status, storage key).
  - Emit `JobStatus` update (increment completed/failed counts). This may be batched (e.g., flush every second or after N updates) to reduce chatter.
  - On unrecoverable failure after retries, send `DeadLetterTask`.
- Worker also updates internal metrics counters.

## 6. Graceful Shutdown
- Listen for `SIGINT/SIGTERM`.
- Stop fetching new tasks; wait for in-flight tasks to finish up to timeout.
- Flush status/log buffer.

## 7. Configurable Parameters
- `max_inflight_tasks`
- `download_timeout`, `download_retry_limit`
- `storage_retry_limit`
- `proxy_failover_order`
- `status_flush_interval`

All sourced from `fetchbox_config`.

## 8. Testing
- Unit tests for proxy resolver (ensures fallback order).
- Mock HTTP + storage using `wiremock` / local server to test retry logic.
- Integration test using docker-compose (Iggy + MinIO) to process sample task.

## 9. Deliverables
- `crates/fetchbox_worker` binary with task runner.
- Metrics + logging tied into observability stack.
- Documentation snippet on running worker (`cargo run -p fetchbox_worker` or docker-compose service).
