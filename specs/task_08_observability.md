# Spec: Observability & Log Streaming

## Goal
Provide tracing, logging, and metrics infrastructure so operators can monitor FetchBox health, inspect per-job logs via Iggy, and expose Prometheus/Otel metrics.

## Scope
1. Implement tracing/logging setup shared across services.
2. Emit structured logs to stdout and `jobs.logs` stream.
3. Metrics exporter (Prometheus endpoint) for API and worker.
4. Health/readiness probes integrated with metrics.
5. Operator documentation for consuming logs/metrics.

## 1. Tracing & Logging
- Use `tracing` crate with `tracing_subscriber`.
- Format: JSON logs by default, include `timestamp`, `level`, `target`, `job_id`, `job_type`, `tenant`, `trace_id`.
- Provide `TraceContextLayer` to propagate `traceparent` header from API to worker via `DownloadTask.trace_id`.
- API:
  - Wrap Axum router with `tower_http::trace::TraceLayer`.
  - Each request gets a `request_id` (UUID).
- Worker:
  - Each task runner spawns span `task { job_id, resource_id }`.
  - On completion/failure, log events (sent to stdout + `jobs.logs`).
- Implement `LogEmitter` using `fetchbox_iggy` producer to send `JobLog` messages.
  - Buffer logs per job and flush at interval or when severity >= WARN (immediate).

## 2. Metrics
- Use `metrics` crate with Prometheus exporter (`metrics-exporter-prometheus`).
- API metrics:
  - `fetchbox_api_requests_total{status,route}`
  - `fetchbox_ingest_jobs_total{result}`
  - `fetchbox_ingest_bytes_total`
  - `fetchbox_ingest_duration_seconds`
  - `fetchbox_operator_requests_total`
- Worker metrics:
  - `fetchbox_worker_tasks_inflight`
  - `fetchbox_worker_download_latency_seconds`
  - `fetchbox_worker_download_failures_total{code}`
  - `fetchbox_worker_storage_failures_total{code}`
  - `fetchbox_worker_dlq_total`
  - `fetchbox_proxy_failures_total{pool}`
- Ledger metrics (from task_04):
  - `fetchbox_ledger_pruned_jobs_total`, etc.
- Expose Prometheus endpoint at `config.telemetry.metrics_addr`.
- Support OTLP export by adding optional `tracing_opentelemetry` and `opentelemetry-otlp` integration (enabled via config).

## 3. Health & Readiness
- Define two endpoints per service:
  - `/health/live` (always 200 while process running).
  - `/health/ready` (ensures dependencies healthy: Iggy connected, storage reachable, ledger ready).
- Tie readiness to metrics gauge `service_ready{service=api}` etc.
- Worker readiness requires ability to consume tasks (Iggy) and upload to storage (probe).

## 4. Operator Documentation
- `docs/observability.md` covering:
  - How to scrape Prometheus endpoint.
  - Example Grafana dashboards (queue lag, throughput, proxy failure rate).
  - How to tail logs:
    - CLI command using `fetchbox-iggy-cli tail-logs --job-id`.
    - SSE endpoint from operator API.
  - Troubleshooting checklist (e.g., high DLQ, backlog growth).

## 5. Deliverables
- Shared observability module (maybe `fetchbox_observability`) or common code in API/worker.
- Log emitter hooking into Iggy `jobs.logs`.
- Prometheus endpoint wired into both API and worker.
- Documentation file with instructions and sample alerts.
