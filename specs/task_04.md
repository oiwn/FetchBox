# Spec: Fjall Persistence Layer

## Goal
Design and implement the ledger component that persists job snapshots, log cursors, manifest references, and pruning metadata using Fjall. This spec defines the data model, API surface, pruning behavior, and integration points with Axum ingress and downloader workers.

### Why Fjall if Iggy is append-only?
Iggy provides the durable event log (manifests, tasks, status, logs), but operators need instant `job_id` lookups without replaying streams. Fjall acts as the snapshot cache:
- stores the latest state per job (status, counts, manifest key) for `GET /operators/jobs/{job_id}` in O(1) time, instead of scanning log offsets;
- tracks log cursors so we can jump to the right offset when streaming `jobs.logs`;
- enforces its own retention window independent of Iggy (e.g., 30 days of snapshots even if streams retain longer).
Thus, Iggy remains the source of truth for history, while Fjall enables fast queries and pruning tailored to operator needs.

## Scope
1. Create `crates/fetchbox_ledger` encapsulating Fjall usage.
2. Define key spaces/buckets for jobs, metadata, retention config, and pruner state.
3. Provide repository APIs used by Axum (write queued state), workers (update progress/failures), and operator endpoints (read snapshots, query failed jobs).
4. Implement pruning/compaction strategy with metrics.
5. Document operational guidance (`docs/ledger.md`).

## 1. Fjall Layout

Architecture decisions:
- Fjall runs within the API/operator process (or a dedicated “ledger updater” service). Workers never touch Fjall directly; they publish `JobStatus` events to Iggy, and the ledger updater consumes those events to mutate snapshots.
- Only one process writes to the Fjall database path to simplify locking; other services query snapshots via internal RPC/HTTP if needed.
- Fjall storage path is configurable (e.g., `data/ledger`); deployments can place it on persistent volumes for backups.

Need to produce spec: data model (key layout), operations, pruning. Provide metrics.

Let's craft spec carefully referencing earlier context and user comments (limit complexity). They prefer simple approach; no more future sections? But spec style previous consistent.

Ok craft entire file. Need restructure: sections: Goal, Data Model, API Methods, Pruning, Metrics, Deliverables.

Important to mention features (job_by_id, list_recent_failures). Provide key names etc.
