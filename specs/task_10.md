# Spec: Failure Taxonomy & Retry/DLQ Policies

## Goal
Standardize how FetchBox categorizes errors, applies retries, and publishes failure details to DLQ so operators can understand and replay problematic tasks.

## Scope
1. Define error categories and codes used across API and worker.
2. Map categories to retry policies (Axum ingest, worker download/upload).
3. Specify DLQ publishing format and conditions.
4. Document how Fjall snapshots capture failure info.
5. Provide testing guidelines.

## 1. Error Categories

Define canonical codes (string constants) for reuse:

| Category        | Code Prefix      | Description                                  |
|-----------------|------------------|----------------------------------------------|
| Validation      | `VALIDATION_*`   | Client payload issues                        |
| Authorization   | `AUTH_*`         | Missing/invalid credentials                   |
| Handler         | `HANDLER_*`      | Custom handler threw error                    |
| Download        | `DOWNLOAD_*`     | Network/proxy/HTTP errors fetching resource  |
| Upload          | `UPLOAD_*`       | Storage upload failures                       |
| Storage         | `STORAGE_*`      | Manifest/storage metadata issues              |
| System          | `SYSTEM_*`       | Unexpected internal errors                    |

Examples:
- `VALIDATION_MISSING_FIELD`
- `DOWNLOAD_TIMEOUT`
- `DOWNLOAD_HTTP_4XX`
- `DOWNLOAD_PROXY_EXHAUSTED`
- `UPLOAD_S3_ERROR`
- `SYSTEM_PANIC`

Each error record contains:
```json
{
  "code": "DOWNLOAD_TIMEOUT",
  "message": "GET https://... timed out after 30s",
  "context": {
    "proxy": "proxies/eu-primary#1",
    "attempt": 3,
    "status": null
  }
}
```

## 2. Retry Policies

### Axum Ingest
- Validation/auth errors: no retry.
- Storage/Iggy transient errors: automatic retry inside service (up to 3 attempts with exponential backoff). If still failing, return `503` to client.

### Worker Download
- Retryable errors: network timeouts, 5xx responses, DNS errors.
  - Policy: up to `download_retry_limit` attempts per task; rotate proxies between attempts.
- Non-retryable: 4xx responses (except 408/429/499), invalid URL, handler refused (e.g., missing metadata). Immediately fail.

### Storage Upload
- Retryable: network errors, 5xx, throttling.
- Non-retryable: `AccessDenied`, invalid bucket.
- Policy: `storage_retry_limit` attempts with exponential backoff.

### DLQ Condition
- After retries exhausted for download/upload categories → send to DLQ.
- Handler/system errors → DLQ immediately.
- Validation/auth errors should never reach worker; if they do, treat as fatal and DLQ for investigation.

## 3. DLQ Payload
- Use `DeadLetterTask` proto message (from `specs/task_03.md`).
- Populate `failure_code`, `failure_message`, `attempts`, `failed_at_ms`.
- Additional context encoded in `failure_message` or extend schema with `map<string,string> metadata`.
- Worker publishes to `jobs.dlq` and increments metrics counter.

## 4. Fjall Snapshot Integration
- When ledger receives `JobStatus` with failure info:
  - Update `JobSnapshot.last_error = FailureInfo`.
  - Increment `resources_failed`.
  - If job transitions to terminal failure (all tasks failed or any non-retryable), set `status = failed` and store pointer to DLQ entry (offset).

## 5. Testing Guidelines
- Unit tests for retry policy evaluation (given error code, ensure classification correct).
- Worker integration test simulating repeated download failure -> verify DLQ entry created.
- API test verifying storage transient error triggers internal retry before returning 503.

## 6. Deliverables
- Shared `failure` module (maybe `fetchbox_domain`) enumerating codes and helpers.
- Worker/API code updated to use taxonomy.
- DLQ publisher integrated with metrics.
- Documentation in `docs/failures.md` describing categories, retry behavior, and operator remediation steps.
