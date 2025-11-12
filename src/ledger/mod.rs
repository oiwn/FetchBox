/// Fjall-based persistence layer for job snapshots, logs, and metadata
///
/// This module provides durable storage for FetchBox's job state, replacing
/// the temporary in-memory store from Task 01. It uses Fjall (an embedded LSM
/// key-value store) to persist:
///
/// - Job snapshots (status, progress, manifest references)
/// - Log entries (structured logs per job/task)
/// - Idempotency keys (deduplication for POST /jobs)
/// - Metadata (pruning state, retention cursors)
///
/// ## Architecture
///
/// Fjall runs within the API/operator process. Workers publish JobStatus events
/// to Iggy streams, and a ledger updater (Task 07) consumes those events to
/// update Fjall snapshots.
///
/// ## Retention Policies
///
/// - Jobs: 30 days (configurable via TOML in Task 05)
/// - Logs: 30 days (aligned with Iggy `jobs.logs` stream)
/// - Idempotency: 14 days (shorter window for dedup)
///
/// Pruning is triggered manually via `FjallStore::prune_expired()`.
///
/// ## Usage
///
/// ```rust,ignore
/// use fetchbox::ledger::FjallStore;
///
/// let store = FjallStore::open("data/ledger")?;
/// store.upsert(job_snapshot)?;
/// let snapshot = store.get("job_123")?;
/// ```

pub mod error;
pub mod partitions;
pub mod pruning;
pub mod store;

pub use error::{LedgerError, Result};
pub use pruning::{
    PruneStats, RETENTION_IDEMPOTENCY_DAYS, RETENTION_JOBS_DAYS, RETENTION_LOGS_DAYS,
};
pub use store::{FjallStore, StoreStats};
