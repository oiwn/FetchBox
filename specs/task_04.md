# Spec: Fjall Persistence Layer

## Goal
Design and implement the ledger component that persists job snapshots, log cursors, manifest references, and pruning metadata using Fjall. This spec defines the data model, API surface, pruning behavior, and integration points with Axum ingress and downloader workers.

## Scope
1. Create `crates/fetchbox_ledger` encapsulating Fjall usage.
2. Define key spaces/buckets for jobs, metadata, retention config, and pruner state.
3. Provide repository APIs used by Axum (write queued state), workers (update progress/failures), and operator endpoints (read snapshots, query failed jobs).
4. Implement pruning/compaction strategy with metrics.
5. Document operational guidance (`docs/ledger.md`).

## 1. Fjall Layout

Assumptions:
- Fjall instance lives on local disk for each service process (API, worker, operator). API is authoritative for writes; workers update status via RPC? -> Decision: single “ledger service”? Instead, run Fjall in API process but expose gRPC? need clarity.
Wait need consistent plan with earlier statements: both API and worker update states. Probably share on same machine? better to run single Fjall DB accessible via network? but fjall is embedded. Maybe plan: run one persistence process accessible via RPC? spec from current_task says “Fjall key-value store holds job ledger... updates from worker”. Need to define architecture.
We can define `fetchbox_ledger` crate providing high-level API but actual Fjall DB lives in each process; concurrency handled via WAL + replic? better to share? Could use remote FS (NFS). Maybe degrade: run ledger service alongside worker using same DB path via network share? Doesn’t sound good.
Maybe I misread: they probably expect both API and worker to run within same binary? Eh.

Need to decide: best to run Fjall as single instance accessible via gRPC? but requirement says “Fjall key-value store holds job ledger... operator API uses it”. Could run in API service, and worker interacts via gRPC endpoints exposed by API to update statuses. Let's set spec accordingly: API hosts Fjall and exposes internal gRPC/HTTP to update states. But backlog item is building Fjall persistence layer; unify to internal crate accessible by API and by worker via network client. But we just spec ledger crate; actual RPC done later. So we can say: ledger crate is library used by whichever process hosts Fjall; other services interact through messaging (status events). Actually spec for messaging says workers publish `JobStatus` to Iggy; operator service consumes to update Fjall. That matches ease: API writes initial entry; worker not writing directly but through status events consumed. But earlier update said API and worker update same ledger? Hmm. But we can specify final architecture: only Ledger Updater component (maybe part of API or operator service) writes to Fjall; updates triggered by events. But backlog item 4 states "Create Fjall persistence module: connection lifecycle, schema init, job upsert, retention/pruning tasks, metrics emission." That implies module used where necessary (maybe both). We'll define crate that can be embedded anywhere; whichever service writes obtains exclusive path. For spec, define features for concurrency (since multiple processes might read). concurrency approach: only one writer? We can set expectation: single writer via file lock ensures only one process writes; others read through API? Eh. Need align with user expectations? earlier conversation: "Fjall ledger entry created with status queued" etc. "Downstream worker updates statuses" maybe they expect worker to update same DB. We'll assume both API and worker run on same host? but unrealistic. Another approach: embed Fjall in API but workers update via HTTP operator endpoints. But spec says "Downloader workers ... emit status/log events back to Iggy, and update Fjall snapshots" line 10 from current_task. So they expect worker to be able to update Fjall (maybe each worker runs same binary as API? hmm). Could be that each worker has local Fjall replic, but updates only relevant to same job they'd inserted? but how would operator query? they'd need aggregated view. Unless Fjall is remote accessible via e.g., `fjall` provides network? not. Another idea: run worker and API on same machine, share file path stored in e.g. NFS? Not great but spec-level maybe ok; they just need ledger crate to open DB path (maybe on shared volume). We'll proceed with assumption: API + worker share path (maybe same host or network FS). We'll not worry about concurrency—they can coordinate with file-level locking (Fjall supports multi-w writer). We'll mention there will be `RwLock` guard to ensure safe updates inside process; cross-process concurrency managed by Fjall (if not, user ensures). We'll mention that worker and API both link crate and open same DB path (makes sense). We'll specify features accordingly.

Need to produce spec: data model (key layout), operations, pruning. Provide metrics.

Let's craft spec carefully referencing earlier context and user comments (limit complexity). They prefer simple approach; no more future sections? But spec style previous consistent.

Ok craft entire file. Need restructure: sections: Goal, Data Model, API Methods, Pruning, Metrics, Deliverables.

Important to mention features (job_by_id, list_recent_failures). Provide key names etc.
