# Spec: Axum Ingest Contract, Operator Surface, and Handler Registration

## Goal

This document serves as the binding specification for FetchBox's external API contract: `POST /jobs` ingest, operator APIs, handler registration, and retention expectations.

## 1. Client-Facing Ingest (`POST /jobs`)

Implementation note: the Axum service runs as the `fetchbox server` subcommand of
the unified CLI binary.

### Endpoint
- Method: `POST`
- Path: `/jobs`
- Auth: none in v0 (assumes trusted network). Note future enhancements may add
  per-tenant keys or mTLS.
- Rate limiting: not enforced in v0; earmarked for future development.

### Required Headers
- `Content-Type: application/json`
- `X-Fetchbox-Job-Type: <string>` (maps to handler registration)
- Optional:
  - `Content-Encoding: gzip` (API must support transparent decompression)
  - `X-Fetchbox-Idempotency-Key: String` (client-provided dedupe key, reserved for future use)
  - `X-Fetchbox-Tenant: <tenant-id>`

### Payload Schema (JSON)

```json
{
  "manifest_version": "v1",
  "storage": {
    "manifest_file": "metadata.json",
    "resource_key_prefix": "resources/2024/05/01/dependencies/"
  },
  "metadata": {
    "...": "arbitrary structured metadata persisted as canonical metadata.json"
  },
  "resources": [
    {
      "id": "resource_name_01",
      "url": "https://cdn.example.com/image.jpg",
      "headers": {
        "Referer": "https://example.com/page.html",
        "User-Agent": "Crawler/1.0"
      },
      "tags": {
        "content_type": "image/jpeg",
        "checksum_hint": "sha256:deadbeef"
      }
    }
  ],
  "attributes": {
    "tenant": "crawler-a",
    "crawl_id": "847458834325543643",
    "crawled_at": "2024-05-01T10:00Z",
    "priority": "normal"
  }
}
```

Rules:
- `manifest_version` required (current `v1`).
- `storage` required object defining S3 storage paths:
  - `manifest_file`: filename where this manifest will be stored.
  - `resource_key_prefix`: Base prefix for all downloaded resources from this job.
  - Client has full control over path structure (date-based, tenant-based, arbitrary hierarchy).
- `metadata` must be JSON object; stored verbatim to canonical metadata file (e.g., `metadata.json`).
- `resources` array required (1..1000 entries by default). Each entry must define:
  - unique `id` (per manifest) ≤ 128 chars.
  - `url` (HTTP/HTTPS; worker enforces scheme).
  - optional `headers` (string map) merged with handler defaults.
  - optional `tags` (string map) passed to handler/storage.
- `attributes` optional JSON object for handler use (tenant, crawl_id, etc.).

### Size Limits (configurable via TOML)

- Raw JSON ≤ 5 MB (after decompression). Rejected with `413 Payload Too Large`.
  - Config: `api.max_payload_bytes` (default: 5242880)
- Max resources per manifest: 1000 (default)
  - Config: `api.max_resources_per_manifest` (default: 1000)
- Each header map ≤ 10 keys; values ≤ 1 KB.
  - Config: `api.max_headers_per_resource` (default: 10), `api.max_header_value_bytes` (default: 1024)

### Processing Semantics

**Note: FetchBox uses the `uuid` crate (v7 feature) for job ID generation.**

1. API validates headers/payload; rejects with `400` + error code on schema violations.

2. API generates `job_id` (UUIDv7) as unique identifier for this job submission.
   - A **job** represents one manifest submission containing N resources to download.
   - Each resource becomes a separate **task** processed by workers.
   - Note: `X-Fetchbox-Idempotency-Key` is accepted but not enforced in v0; reserved for future deduplication.

3. Manifest stored to S3-compatible storage using the client-provided `storage.manifest_key`.
   - Full S3 path: `s3://{bucket}/{storage.manifest_key}`
   - Bucket configured per handler in server TOML config.

4. Fjall ledger entry created with:
   - `job_id` (UUIDv7)
   - Status: `queued`
   - Manifest S3 reference
   - Resource count
   - Timestamps (created_at, updated_at)

5. Handler invoked to transform manifest into download tasks (per-resource work units).

6. Tasks enqueued via TaskBroker, which persists to TasksStorage (UUIDv7 IDs) and distributes
   to worker pool via in-memory channels.

7. Response returned (with `job_id`) only after manifest persistence, Fjall write, and task enqueue succeed.

### Response

`202 Accepted`
```json
{
  "job_id": "01HXYZ...",
  "manifest_key": "s3://fetchbox/manifests/resource/2024/05/01/crawler-a/metadata.json",
  "resource_count": 42
}
```

Error responses include:
- `400` `INVALID_PAYLOAD`
- `401` `UNAUTHORIZED`
- `403` `UNSUPPORTED_JOB_TYPE`
- `413` `PAYLOAD_TOO_LARGE`
- `429` `RATE_LIMITED`
- `500` `INTERNAL_ERROR`

### Fire-and-Forget Contract
- Once `202` is returned, FetchBox guarantees the manifest is persisted and
  queued. Clients must not expect further callbacks; they rely on their own
  metadata store plus operator tooling.
- Downstream failures appear only via operator API/metrics. Clients are
  responsible for reconciliation if necessary.

## 2. Operator API Surface

### Endpoints (auth not enforced in v0)

1. **`GET /operators/jobs/{job_id}`**

   Returns Fjall ledger snapshot for the specified job:

   ```json
   {
     "job_id": "01HXYZ...",
     "job_type": "resource",
     "status": "queued|in_progress|completed|failed",
     "created_at": "2024-05-01T10:00:00Z",
     "updated_at": "2024-05-01T10:05:32Z",
     "resource_total": 42,
     "resource_completed": 40,
     "resource_failed": 2,
     "manifest_key": "s3://fetchbox/manifests/resource/2024/05/01/crawler-a/metadata.json",
     "errors": [
       {
         "resource_id": "resource_name_03",
         "code": "HTTP_403",
         "message": "Forbidden",
         "timestamp": "2024-05-01T10:03:15Z"
       },
       {
         "resource_id": "resource_name_07",
         "code": "HTTP_404",
         "message": "Not Found",
         "timestamp": "2024-05-01T10:04:22Z"
       }
     ],
     "tenant": "crawler-a"
   }
   ```

   - `job_id`: UUIDv7 string
   - `status`: Current aggregate status
   - `resource_*`: Progress counters
   - `errors`: Array of all task failures (optional, present only if failures occurred)

2. **`GET /operators/health`**

   Returns component health status:

   ```json
   {
     "status": "healthy",
     "components": {
       "api": "healthy",
       "fjall": "healthy",
       "task_broker": "healthy",
       "storage": "healthy"
     },
     "version": "0.1.0"
   }
   ```

   Returns `503` when any critical dependency is unhealthy.

Auth: not enforced in v0 but endpoint is intended for operator use only; future releases may add bearer tokens/mTLS and rate limiting.

## 3. Handler Registration & Config

### Config Structure (TOML)

```toml
[job_types.resource]
handler = "fetchbox_handlers::resource::ResourceHandler"
default_headers = { "User-Agent" = "FetchBox/1.0" }

[job_types.resource.storage]
bucket = "fetchbox-resource"

[job_types.gallery]
handler = "fetchbox_handlers::gallery::GalleryHandler"

[job_types.gallery.storage]
bucket = "fetchbox-gallery"

[job_types.gallery.options]
max_parallel_downloads = 8
```

Rules:
- `handler` is fully-qualified Rust path registered at compile time.
- Storage section defines the S3 bucket for this job type.
  - Client provides full key paths via manifest `storage` field.
- Handler-specific options via `options` table (loaded into handler init).

### Handler Trait (Summary)

```rust
pub trait JobHandler {
    fn prepare_manifest(&self, manifest: Manifest) -> anyhow::Result<PreparedManifest>;
    fn build_tasks(&self, ctx: PreparedManifest) -> anyhow::Result<Vec<DownloadTask>>;
    fn finalize_job(&self, summary: JobSummary) -> anyhow::Result<()>;
}
```

- `DownloadTask` includes resource `id`, `url`, combined headers (handler + manifest), storage key, and metadata tags.
- Storage key determined from manifest `storage.resource_key_prefix` + resource `id`.
- Handlers run inside API (for manifest prep) and worker (for per-task logic consistency).
- Default handler ships with FetchBox; it simply uses the manifest data without extra transformation, making custom handlers optional until specialized behavior is needed.

## 4. Retention & Ledger Expectations

- Fjall ledger entries: Configurable TTL (default 30 days). Background pruner enforces TTL + size cap (configurable, default 5 GB).
- Manifest files in S3: Retention managed via S3 bucket lifecycle policies (outside FetchBox scope).
- Downloaded resources in S3: Retention managed via S3 bucket lifecycle policies (outside FetchBox scope).
- Operators may export Fjall snapshots before pruning using provided CLI tools.

## 6. Future Development

- Per-tenant API keys or mTLS enforcement
- Rate limiting policies
- Idempotency key enforcement with configurable dedup windows
- Proxy pool support with fallback chains
- Dead letter queue (DLQ) for permanently failed tasks
- Detailed task-level logs/events endpoint
- path validation, 2 components path system (date/id, topic/partition)
