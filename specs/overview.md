# FetchBox Specification Overview

This document indexes the current specifications and captures the overarching architecture/work plan for FetchBox.

## Project Snapshot
- **Goal**: Provide a reusable service where crawlers POST manifests containing metadata + resource URLs; FetchBox stores metadata, fans out resource download tasks via Iggy, and workers store assets in S3-compatible storage while tracking job state inside Fjall.
- **Key Components**:
  - Axum API ingress (`POST /jobs` + operator endpoints).
  - Iggy streams for tasks, status, logs, DLQ.
  - Fjall ledger for job snapshots/log cursors.
  - User-defined handlers (`fetchbox_plugins`) for manifest → task expansion, HTTP header injection, storage overrides.
  - Worker fleet consuming tasks, honoring proxy fallbacks, performing downloads/uploads.
  - Storage abstraction built on Arrow object store (S3-compatible) with handler hints.
  - Observability stack (structured logs via Iggy, Prometheus/Otel metrics).
- **Stack**: Rust, Axum, Iggy, Fjall, Arrow object-store, Reqwest, Tokio.
  - Axum (`tokio-rs/axum`) powers the HTTP ingress and operator API layer.
  - Iggy (`apache/iggy`) provides the message queue for manifests, per-resource tasks, status, and log streams.
  - Fjall (`fjall-rs/fjall`) serves as the embedded key-value store for job snapshots and retention/pruning.
  - Arrow object store (`apache/arrow-rs-object-store`) backs the S3-compatible storage abstraction for metadata and resource payload uploads.

## Spec Index
| File | Description |
|------|-------------|
| `task_01.md` | External contract for Axum ingest, operator APIs, handler registration. |
| `task_02.md` | Handler trait + plugin crate design. |
| `task_03.md` | Iggy stream definitions and protobuf schemas. |
| `task_04.md` | Fjall ledger layout and APIs. |
| `task_05.md` | Configuration loader, proxy fallback schema, validation. |
| `task_06.md` | Axum ingress + operator API service spec. |
| `task_07.md` | Iggy client utilities (producer/consumer). |
| `task_08.md` | Downloader worker core functionality. |
| `task_09.md` | Storage abstraction + handler overrides. |
| `task_10.md` | Failure taxonomy, retry policy, DLQ rules. |
| `task_11.md` | Observability + log streaming requirements. |
| `task_12.md` | Dev environment, docker-compose, sample handler. |
| `task_13.md` | Integration/load testing plan. |
| `task_14.md` | Documentation + operator runbook plan. |

## Current Priorities
1. Finalize requirements doc (`task_01`) → produce `docs/requirements.md`.
2. Implement handler/plugin crate (`task_02`) to unlock API/worker integration.
3. Nail messaging schemas + proto generation (`task_03`).
4. Build config system (`task_05`) to power API + worker.

## Implementation Notes
- The repository starts as a single binary crate `fetchbox`; `src/main.rs` will expose CLI subcommands (e.g., `fetchbox api`, `fetchbox worker`) that wire into modules under `src/`.
- Shared code (config, storage, messaging helpers) should be implemented as modules within the crate; additional workspace members can be introduced later if the surface area grows.
- Deployment model: ship one container image and run different subcommands per workload (`fetchbox api` for ingress, `fetchbox worker` for download workers). Worker replicas scale horizontally.

## Outstanding Questions
- Fjall writer topology (single writer via status consumer vs multi-writer) — clarify in `task_04`.
- Direct Iggy producer responsibilities (validation, Fjall mirroring).
- Future auth/rate-limiting strategy (tracked as future work in requirements spec).

## References
- `README.md` — will link to finished docs as they land.

Keep this overview updated as specs evolve or new tasks emerge.
