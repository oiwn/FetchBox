# Spec: Axum Ingress & Operator API

## Goal
Define the Axum-based HTTP service that:
1. Accepts manifest uploads via `POST /jobs`.
2. Seeds Fjall ledger entries and publishes tasks to Iggy.
3. Exposes operator endpoints for job lookup, log references, and health checks.

This spec translates the requirements in `specs/task_01.md` into concrete Axum handlers, routing, middleware, and integration points.

## Scope
1. Axum service structure (crates/modules, router composition, middleware).
2. Manifest ingest pipeline (streaming body handling, validation, storage write, handler invocation, Iggy publish).
3. Operator endpoints (job lookup, logs pointer, health).
4. Error handling, request tracing, and metrics integration.
5. Tests covering core flows.

## 1. Service Layout

- `crates/fetchbox_api`
  - `main.rs` — bootstraps config, logging, K/V connections, Iggy client, handler registry.
  - `routes/mod.rs` — router composition.
  - `routes/jobs.rs` — `POST /jobs`.
  - `routes/operator.rs` — operator endpoints.
  - `state.rs` — `AppState` struct (config, handler registry, storage client, ledger handle, iggy producer, metrics).
  - `middleware/request_id.rs` — inject job/trace ids.
  - `errors.rs` — error types + `IntoResponse`.
  - `ingest/` — manifest processing pipeline.

Axum features:
- Use `Router::with_state(AppState)`.
- Include `tower::ServiceBuilder` stack: tracing layer, timeout, body limit (based on `config.server.max_manifest_bytes`).

## 2. Manifest Ingest Handler (`POST /jobs`)

Flow:
1. **Header extraction**: `X-Fetchbox-Job-Type`, optional `X-Fetchbox-Idempotency-Key`, `Content-Encoding`.
2. **Body streaming**: use `axum::body::Body` -> `bytes::Bytes`. If `gzip`, decompress on the fly (`async_compression`).
3. **Size enforcement**: wrap body with `tower_http::limit::RequestBodyLimitLayer` and double-check after buffering (if necessary).
4. **JSON parse**: parse into `ManifestPayload` struct (matching spec). Validate counts, resource fields.
5. **Idempotency**:
   - If header present, compute dedupe key `tenant + job_type + header`.
   - Check Fjall ledger for existing job; if found and still queued/in-progress, return same `job_id`.
6. **Job ID**: generate `uuid::Uuid::now_v7().to_string()`.
7. **Manifest storage**:
   - Upload canonical `metadata.json` to storage abstraction via streaming upload (use arrow object store client). Key pattern `manifests/{job_type}/{job_id}/metadata.json`.
   - Return storage key/etag.
8. **Ledger seed**: call `ledger.create_job(...)`.
9. **Handler invocation**:
   - Build `ManifestContext` and call registered handler (default if none). Handler returns tasks/overrides.
10. **Iggy publish**:
    - For each resource, create `DownloadTask` message (per `proto/jobs.proto`) and publish to `jobs.tasks`. Use partition key hash on `job_id`.
11. **Response**: `202 Accepted` with JSON body.

Error handling:
- Validation errors -> `400`.
- Unknown handler -> `403`.
- Storage failure -> `502`.
- Iggy publish failure -> `503`.
- All errors include JSON body `{ "error": { "code": "...", "message": "...", "job_id": optional } }`.

Tracing:
- Use `tower-http` trace layer, include `job_id`, `job_type`, `tenant`.
- Emit metrics counters for accepted jobs, invalid payloads, publish failures.

## 3. Operator Endpoints

### `GET /operators/jobs/{job_id}`
- Fetch snapshot via ledger; return JSON as defined in spec.
- If missing: `404`.

### `GET /operators/jobs/{job_id}/logs?since_offset=...`
- Fetch log cursor from ledger; if `since_offset` provided, override.
- Connect to Iggy `jobs.logs`, stream events as Server-Sent Events (SSE) or chunked JSON array (choose SSE for simplicity).
- Include pagination via `?limit=1000`.

### `GET /operators/health`
- Compose component health:
  - `Fjall`: perform `ledger.health_check()`.
  - `Iggy`: ping producer connection (optional).
  - `Storage`: `HEAD` bucket or assume healthy if last check < 60s.
- Return JSON `{ "status": "ok|degraded", "components": { ... } }`. On degraded, HTTP 503.

Auth:
- For now, wrap operator routes with optional middleware stub that checks header `X-Fetchbox-Operator-Token` against config (if set).

## 4. App State & Dependencies

`AppState` fields:
```rust
pub struct AppState {
    pub config: Arc<Config>,
    pub handler_registry: Arc<HandlerRegistry>,
    pub ledger: Arc<FetchboxLedger>,
    pub storage: Arc<StorageClient>,
    pub iggy: Arc<IggyProducer>,
    pub metrics: MetricsHandle,
}
```

- `IggyProducer` uses messaging crate to publish tasks.
- `StorageClient` wraps Arrow object store.
- `MetricsHandle` (from observability task) increments counters.

Graceful shutdown:
- Use `axum::Server::with_graceful_shutdown`.
- Close Iggy producer, storage client.

## 5. Testing
- Unit tests:
  - Payload validation (missing required fields -> 400).
  - Idempotency (same header returns same job_id, no duplicate tasks).
  - Handler failure -> 500 with error message.
- Integration tests (Tokio):
  - Mock storage, ledger, iggy to assert tasks published.
  - Operator endpoints retrieving snapshots.

## 6. Deliverables
- `crates/fetchbox_api` with Axum service, routes, middleware, tests.
- Config wiring to `fetchbox_config`.
- Documentation snippet in README describing how to run the API (`cargo run -p fetchbox_api`).
