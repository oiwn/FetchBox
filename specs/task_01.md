# Spec: Axum Ingest Contract, Operator Surface, and Handler Registration

## Goal
Define FetchBox’s external contract so future PRs can implement the system without guessing. This document serves as the binding specification for `POST /jobs`, operator APIs, handler registration, retention expectations, and optional direct Iggy producers.

## 1. Client-Facing Ingest (`POST /jobs`)

### Endpoint
- Method: `POST`
- Path: `/jobs`
- Auth: none in v0 (assumes trusted network). Note future enhancements may add per-tenant keys or mTLS.
- Rate limiting: not enforced in v0; earmarked for future development.

### Required Headers
- `Content-Type: application/json`
- `X-Fetchbox-Job-Type: <string>` (maps to handler registration)
- Optional:
  - `Content-Encoding: gzip` (API must support transparent decompression)
  - `X-Fetchbox-Idempotency-Key: String` (client-provided dedupe key)
  - `X-Fetchbox-Tenant: <tenant-id>`

^^^ great idea with headers which set some options!

### Payload Schema (JSON)
```json
{
  "manifest_version": "v1",
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
- `metadata` must be JSON object; stored verbatim to canonical metadata file (e.g., `metadata.json`).
- `resources` array required (1..1000 entries by default). Each entry must define:
  - unique `id` (per manifest) ≤ 128 chars.
  - `url` (HTTP/HTTPS; worker enforces scheme).
  - optional `headers` (string map) merged with handler defaults.
  - optional `tags` (string map) passed to handler/storage.
- `attributes` optional JSON object for handler use (tenant, crawl_id, etc.).

### Size Limits
- Raw JSON ≤ 5 MB (after decompression). Rejected with `413 Payload Too Large`.
- Each header map ≤ 10 keys; values ≤ 1 KB.

### Processing Semantics
1. API validates headers/payload; rejects with `400` + error code on schema violations.
2. API computes/assigns `job_id` (UUIDv7 unless `X-Fetchbox-Idempotency-Key` dedup hits previous job).
3. Manifest stored to S3-compatible storage under default bucket/key pattern `manifests/{job_type}/{job_id}/metadata.json`.
  4. Fjall ledger entry created with status `queued`, manifest reference, resource count, timestamps.
  5. Optional handler logic can adjust headers/storage hints; default behavior uses manifest as submitted, so many deployments can keep the binary as-is.
  6. API publishes tasks directly to the `jobs.tasks` Iggy stream (one message per resource). No extra background service beyond Iggy itself.
  7. Response returned only after manifest persistence, Fjall write, and Iggy publish succeed.

### Response
`202 Accepted`
```json
{
  "job_id": "01HXYZ...",
  "manifest_key": "s3://fetchbox/manifests/gallery/01HXYZ.../metadata.json",
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
- Once `202` is returned, FetchBox guarantees the manifest is persisted and queued. Clients must not expect further callbacks; they rely on their own metadata store plus operator tooling.
- Downstream failures appear only via operator API/metrics/DLQ. Clients are responsible for reconciliation if necessary.

## 2. Operator API Surface

### Endpoints (require auth)
1. `GET /operators/jobs/{job_id}`
   - Returns Fjall snapshot:
     ```json
     {
       "job_id": "...",
       "job_type": "gallery",
       "status": "failed|completed|in_progress|queued",
       "created_at": "...",
       "updated_at": "...",
       "resource_total": 42,
       "resource_completed": 40,
       "manifest_key": "s3://...",
       "log_cursor": { "stream": "jobs.logs", "offset": 12345 },
       "last_error": { "code": "HTTP_403", "message": "..." }
     }
     ```
2. `GET /operators/jobs/{job_id}/logs?since_offset=...`
   - Streams structured log envelopes by replaying the `jobs.logs` Iggy stream (workers emit log events there). `since_offset` defaults to the stored cursor in Fjall.
3. `GET /operators/health`
   - Returns component status (Axum, Fjall, Iggy, storage, proxy pools). `503` when any critical dependency unhealthy.

Auth: not enforced in v0 but endpoint is intended for operator use only; future releases may add bearer tokens/mTLS and rate limiting.

## 3. Handler Registration & Config

### Config Structure (TOML)
```toml
[job_types.gallery]
handler = "fetchbox_handlers::gallery::GalleryHandler"
default_headers = { "User-Agent" = "FetchBox/1.0" }

[job_types.gallery.proxy]
primary = "proxies/eu-primary"
fallbacks = ["proxies/global", "proxies/emergency"]

[job_types.gallery.storage]
bucket = "fetchbox-gallery"
key_prefix = "gallery/"

[job_types.gallery.options]
max_parallel_downloads = 8
```

Rules:
- `handler` is fully-qualified Rust path registered at compile time.
- Proxy section lists named pools with ordered fallbacks; worker cycles through when failures occur.
- Storage section overrides default bucket/prefix; handler may further customize per resource.
- Additional handler-specific options allowed via `options` (loaded into handler init).

### Handler Trait (Summary)
```rust
pub trait JobHandler {
    fn prepare_manifest(&self, manifest: Manifest) -> anyhow::Result<PreparedManifest>;
    fn build_tasks(&self, ctx: PreparedManifest) -> anyhow::Result<Vec<DownloadTask>>;
    fn finalize_job(&self, summary: JobSummary) -> anyhow::Result<()>;
}
```
- `DownloadTask` includes resource `id`, `url`, combined headers (handler + manifest), proxy/storage hints, and metadata tags.
- Storage override hook allows handler to specify bucket/key/object metadata per task.
- Handlers run inside API (for manifest prep) and worker (for per-task logic consistency).

- Default handler ships with FetchBox; it simply uses the manifest data without extra transformation, making custom handlers optional until specialized behavior is needed.

## 4. Retention & Ledger Expectations
- Fjall snapshots retained 30 days (configurable). Background pruner enforces TTL + size cap (default 5 GB).
- `jobs.tasks` stream retention 7 days; `jobs.logs` 30 days; `jobs.dlq` 90 days.
- Operators may export Fjall snapshots before pruning using provided CLI.

## 5. Direct Iggy Producers
- Reserved for trusted clients on the same cluster. They publish compact task messages directly to `jobs.tasks`, using the same schema Axum emits.
- Requirements:
  - Client must upload metadata/manifest on its own and supply references in each task if needed.
  - Validation/dedupe is entirely client responsibility; FetchBox workers assume incoming tasks are correct.
  - The core service (Axum/worker) remains responsible for Fjall entries—direct producers do not write to Fjall.
- This path is optional; Axum ingress remains the standard workflow.

## 6. Open Questions
- Should the idempotency window be configurable per tenant via TOML?
- How will Fjall entries be mirrored when direct Iggy producers are in use (dedicated consumer vs Axum-only responsibility)?

## 7. Future Development
- Per-tenant API keys or mTLS enforcement.
- Rate limiting policies.
- Encryption-at-rest enhancements beyond default S3 support.
- Multipart upload support if manifest limits need to exceed the 5 MB cap.
