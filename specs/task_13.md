# Spec: Integration & Load Tests

## Goal
Ensure FetchBox components work end-to-end by providing integration tests (manifest ingestion through resource upload) and load-test scaffolding to simulate high throughput.

## Scope
1. Integration test suite (`tests/integration/`) using `tokio::test` or `cargo nextest`.
2. Test harness spins up Dockerized dependencies (Iggy, MinIO).
3. Load-test scripts (e.g., `k6` or Rust-based) to stress API/worker pipeline.
4. Documentation describing how to run tests locally and in CI.

## 1. Integration Tests

### Setup
- Use `testcontainers` or `dockertest` crate to spin up Iggy + MinIO per test module.
- Start API and worker processes (maybe via `Command` launching `cargo run`) or embed main logic via test harness.
- Provide helper to submit manifest (HTTP) and await completion by polling operator API (or reading ledger).

### Test Cases
1. **Single manifest, multiple resources**:
   - POST manifest with 3 resources.
   - Assert resources stored in MinIO with expected keys.
   - Assert ledger shows `completed`.
2. **Proxy fallback**:
   - Use mock proxy server that fails first endpoint, ensure worker rotates to fallback.
3. **Retry + DLQ**:
   - Force HTTP 500 responses to trigger retries; ensure DLQ entry created after limit.
4. **Handler override**:
   - Sample handler customizing storage bucket; verify key.
5. **Direct Iggy producer** (optional):
   - Publish `DownloadTask` directly and ensure worker handles it.

### Utilities
- `tests/common/mod.rs` with:
  - `TestContext { api_addr, operator_addr, iggy_client, minio_client }`
  - `submit_manifest(manifest_json)` helper.
  - `wait_for_job(job_id, status)` helper with timeout.

## 2. Load Testing
- Provide script `scripts/load_test.rs` or `scripts/load_test.js` (k6) that:
  - Sends manifests at configurable QPS.
  - Measures API latency, worker throughput.
- Document recommended load (e.g., 10 requests/s for dev).
- Optional GitHub Actions job to run light load test nightly.

## 3. CI Integration
- Add `cargo test --test integration` target (may be behind `INTEGRATION_TESTS=1` env).
- Provide `make test-integration` to run with docker-compose.

## 4. Documentation
- `docs/testing.md` describing:
  - Unit vs integration vs load tests.
  - Setup instructions (Docker, env vars).
  - How to interpret load-test results.

## 5. Deliverables
- Integration test suite with at least the cases above.
- Load-test script(s).
- Documentation for running tests.
