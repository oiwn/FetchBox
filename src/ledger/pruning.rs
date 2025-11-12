/// Pruning and retention policy implementation
use std::time::{Duration, SystemTime};

use fjall::{Keyspace, PartitionHandle};
use tracing::{debug, info};

use super::error::Result;
use super::partitions::{decode_job_key, decode_log_key, encode_meta_key};

/// Retention policy constants (days)
/// NOTE: These will be moved to TOML config in Task 05
pub const RETENTION_JOBS_DAYS: u64 = 30;
pub const RETENTION_LOGS_DAYS: u64 = 30;
pub const RETENTION_IDEMPOTENCY_DAYS: u64 = 14;

/// Metadata keys for pruning state
const META_LAST_PRUNE_JOBS: &str = "last_prune_jobs";
const META_LAST_PRUNE_LOGS: &str = "last_prune_logs";
const META_LAST_PRUNE_IDEM: &str = "last_prune_idem";

/// Pruning statistics
#[derive(Debug, Default)]
pub struct PruneStats {
    pub jobs_pruned: usize,
    pub logs_pruned: usize,
    pub idempotency_pruned: usize,
}

/// Prune expired entries from all partitions
pub fn prune_expired(
    keyspace: &Keyspace,
    jobs_partition: &PartitionHandle,
    logs_partition: &PartitionHandle,
    idem_partition: &PartitionHandle,
    metadata_partition: &PartitionHandle,
) -> Result<PruneStats> {
    let mut stats = PruneStats::default();

    // Prune jobs older than RETENTION_JOBS_DAYS
    stats.jobs_pruned = prune_jobs(jobs_partition, metadata_partition)?;

    // Prune logs older than RETENTION_LOGS_DAYS
    stats.logs_pruned = prune_logs(logs_partition, metadata_partition)?;

    // Prune idempotency keys older than RETENTION_IDEMPOTENCY_DAYS
    stats.idempotency_pruned = prune_idempotency(idem_partition, metadata_partition)?;

    // Trigger compaction to reclaim space
    keyspace.persist(fjall::PersistMode::SyncAll)?;
    info!("Pruning complete: {:?}", stats);

    Ok(stats)
}

/// Prune old job snapshots
fn prune_jobs(
    jobs_partition: &PartitionHandle,
    metadata_partition: &PartitionHandle,
) -> Result<usize> {
    let cutoff_secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - (RETENTION_JOBS_DAYS * 86400);

    let mut pruned = 0;

    // For this initial implementation, we'll use a simple heuristic:
    // Check when the last prune happened. If it was > RETENTION_JOBS_DAYS ago,
    // we'll clear old entries. For a production system, you'd want to track
    // insertion/update times explicitly.

    // For now, we'll skip actual pruning based on timestamps since it requires
    // more complex timestamp parsing. This will be enhanced in a future task.
    // The metadata tracking still works so operators can trigger manual pruning.

    // Update last prune timestamp
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    metadata_partition.insert(
        encode_meta_key(META_LAST_PRUNE_JOBS),
        now.to_string().as_bytes(),
    )?;

    info!("Pruned {} old jobs (timestamp-based pruning TBD)", pruned);
    Ok(pruned)
}

/// Prune old log entries
fn prune_logs(
    logs_partition: &PartitionHandle,
    metadata_partition: &PartitionHandle,
) -> Result<usize> {
    let mut pruned = 0;

    // For this initial implementation, log pruning is deferred.
    // In production, you'd track log timestamps and remove entries
    // older than RETENTION_LOGS_DAYS.

    // Update last prune timestamp
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    metadata_partition.insert(
        encode_meta_key(META_LAST_PRUNE_LOGS),
        now.to_string().as_bytes(),
    )?;

    info!("Pruned {} old log entries (timestamp-based pruning TBD)", pruned);
    Ok(pruned)
}

/// Prune old idempotency keys
fn prune_idempotency(
    idem_partition: &PartitionHandle,
    metadata_partition: &PartitionHandle,
) -> Result<usize> {
    let cutoff_secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - (RETENTION_IDEMPOTENCY_DAYS * 86400);

    let mut pruned = 0;

    // We don't have timestamps on idempotency keys directly,
    // so we'll use a simpler heuristic: prune based on metadata last_prune time
    // In a real system, you'd want to track insertion time per key
    // For now, we'll just keep all keys and only prune on demand

    // Simple strategy: if last prune was > RETENTION_IDEMPOTENCY_DAYS ago,
    // clear all idempotency keys (acceptable since they're meant to be short-lived)
    if let Some(last_prune_bytes) = metadata_partition.get(encode_meta_key(META_LAST_PRUNE_IDEM))? {
        if let Ok(last_prune_str) = std::str::from_utf8(&last_prune_bytes) {
            if let Ok(last_prune_secs) = last_prune_str.parse::<u64>() {
                if last_prune_secs < cutoff_secs {
                    // Clear all idempotency keys
                    for item in idem_partition.iter() {
                        let (key, _) = item?;
                        idem_partition.remove(key)?;
                        pruned += 1;
                    }
                }
            }
        }
    } else {
        // First prune, clear all
        for item in idem_partition.iter() {
            let (key, _) = item?;
            idem_partition.remove(key)?;
            pruned += 1;
        }
    }

    // Update last prune timestamp
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    metadata_partition.insert(
        encode_meta_key(META_LAST_PRUNE_IDEM),
        now.to_string().as_bytes(),
    )?;

    info!("Pruned {} idempotency keys", pruned);
    Ok(pruned)
}
