# Spec: Dev Environment & Sample Handler

## Goal
Create a reproducible local environment for running FetchBox API + worker + dependencies, along with a sample handler crate demonstrating plugin usage.

## Scope
1. Docker Compose stack (Iggy, MinIO, optional Grafana/Prometheus).
2. Scripts to run API and worker against local stack.
3. Sample handler crate `crates/fetchbox_handlers_sample`.
4. Proxy config examples (dummy proxies).
5. Documentation for onboarding developers.

## 1. Docker Compose
- File: `docker-compose.dev.yml`
- Services:
  - `iggy`: official image, expose 8090.
  - `minio`: S3-compatible storage; configure access/secret keys.
  - `prometheus` + `grafana` (optional, maybe later).
  - `otel-collector` (optional) for tracing sink.
- Provide `.env` with credentials.
- Include `Makefile` targets:
  - `make dev-up`
  - `make dev-down`
  - `make dev-logs`

## 2. Run Scripts
- `scripts/run_api.sh`:
  - Sets necessary env vars (config path, AWS creds, Iggy endpoint).
  - Runs `cargo run -p fetchbox_api`.
- `scripts/run_worker.sh` similarly for worker.
- Provide `scripts/setup_iggy.sh` to create streams (from task_03).

## 3. Sample Handler
- Crate `crates/fetchbox_handlers_sample`.
- Implements `GalleryHandler` demonstrating:
  - Custom headers (e.g., referer).
  - Storage override for images vs CSS.
  - Proxy hints (prefers EU proxies).
- Provide manifest sample in `examples/manifests/gallery.json`.
- Document how to register sample handler via config.

## 4. Proxy Config Examples
- Under `config/proxies/`, provide sample TOML showing primary + fallback pools.
- Possibly include `scripts/mock_proxy.py` to act as simple HTTP proxy for testing? (optional; maybe use real proxies or skip).

## 5. Documentation
- `docs/development.md` covering:
  - Prereqs (Docker, Rust).
  - Steps to start stack, run API/worker, submit sample job (curl).
  - Where outputs stored (MinIO bucket).
  - How to inspect logs/status via CLI.

## 6. Deliverables
- `docker-compose.dev.yml`, `.env.example`.
- Scripts under `scripts/`.
- Sample handler crate + manifest example.
- Docs for developer onboarding.
