use std::path::Path;

use fjall::{Config, Keyspace, PartitionCreateOptions, PartitionHandle};
use tracing::{debug, info};

use crate::api::models::JobSnapshot;

use super::error::Result;
use super::partitions::{
    encode_idem_key, encode_job_key, encode_log_key, encode_log_prefix, encode_meta_key,
};
use super::pruning::{prune_expired, PruneStats};

/// Fjall-backed persistent storage for job snapshots, logs, and metadata
#[derive(Clone)]
pub struct FjallStore {
    keyspace: Keyspace,
    jobs: PartitionHandle,
    logs: PartitionHandle,
    idempotency: PartitionHandle,
    metadata: PartitionHandle,
}

impl FjallStore {
    /// Open or create a Fjall store at the given path
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        info!("Opening Fjall store at: {}", path.display());

        // Create parent directory if it doesn't exist
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Open or create keyspace
        let keyspace = Config::new(path).open()?;

        // Create partitions
        let jobs = keyspace.open_partition("jobs", PartitionCreateOptions::default())?;
        let logs = keyspace.open_partition("logs", PartitionCreateOptions::default())?;
        let idempotency = keyspace.open_partition("idempotency", PartitionCreateOptions::default())?;
        let metadata = keyspace.open_partition("metadata", PartitionCreateOptions::default())?;

        info!("Fjall store opened successfully");
        Ok(Self {
            keyspace,
            jobs,
            logs,
            idempotency,
            metadata,
        })
    }

    /// Store or update a job snapshot
    pub fn upsert(&self, snapshot: JobSnapshot) -> Result<()> {
        let key = encode_job_key(&snapshot.job_id);
        let value = serde_json::to_vec(&snapshot)?;
        self.jobs.insert(key, value)?;
        debug!("Upserted job: {}", snapshot.job_id);
        Ok(())
    }

    /// Get a job snapshot by ID
    pub fn get(&self, job_id: &str) -> Result<Option<JobSnapshot>> {
        let key = encode_job_key(job_id);
        match self.jobs.get(key)? {
            Some(value) => {
                let snapshot = serde_json::from_slice(&value)?;
                Ok(Some(snapshot))
            }
            None => Ok(None),
        }
    }

    /// Remember an idempotency key -> job_id mapping
    pub fn remember_idempotency(&self, key: String, job_id: String) -> Result<()> {
        let idem_key = encode_idem_key(&key);
        self.idempotency.insert(idem_key, job_id.as_bytes())?;
        debug!("Remembered idempotency: {} -> {}", key, job_id);
        Ok(())
    }

    /// Check if an idempotency key exists and return the associated job_id
    pub fn get_idempotent(&self, key: &str) -> Result<Option<String>> {
        let idem_key = encode_idem_key(key);
        match self.idempotency.get(idem_key)? {
            Some(value) => {
                let job_id = String::from_utf8_lossy(&value).to_string();
                Ok(Some(job_id))
            }
            None => Ok(None),
        }
    }

    /// Prune expired entries based on retention policies
    pub fn prune_expired(&self) -> Result<PruneStats> {
        info!("Starting pruning process");
        let stats = prune_expired(
            &self.keyspace,
            &self.jobs,
            &self.logs,
            &self.idempotency,
            &self.metadata,
        )?;
        info!("Pruning completed: {:?}", stats);
        Ok(stats)
    }

    /// Persist all pending writes to disk
    pub fn persist(&self) -> Result<()> {
        self.keyspace.persist(fjall::PersistMode::SyncAll)?;
        Ok(())
    }

    /// Get internal statistics (for debugging/monitoring)
    pub fn stats(&self) -> Result<StoreStats> {
        let mut job_count = 0;
        let mut log_count = 0;
        let mut idem_count = 0;

        for item in self.jobs.iter() {
            item?;
            job_count += 1;
        }

        for item in self.logs.iter() {
            item?;
            log_count += 1;
        }

        for item in self.idempotency.iter() {
            item?;
            idem_count += 1;
        }

        Ok(StoreStats {
            job_count,
            log_count,
            idem_count,
        })
    }
}

#[derive(Debug, Clone)]
pub struct StoreStats {
    pub job_count: usize,
    pub log_count: usize,
    pub idem_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::JobStatus;
    use tempfile::TempDir;

    fn create_test_store() -> (FjallStore, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let store = FjallStore::open(temp_dir.path().join("test_ledger")).unwrap();
        (store, temp_dir)
    }

    fn create_test_snapshot(job_id: &str) -> JobSnapshot {
        let now = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        JobSnapshot {
            job_id: job_id.to_string(),
            job_type: "test".to_string(),
            status: JobStatus::Queued,
            created_at: now.clone(),
            updated_at: now,
            resource_total: 10,
            resource_completed: 0,
            resource_failed: 0,
            manifest_key: "manifests/test.json".to_string(),
            errors: Vec::new(),
            tenant: Some("test-tenant".to_string()),
        }
    }

    #[test]
    fn test_open_store() {
        let temp_dir = TempDir::new().unwrap();
        let store = FjallStore::open(temp_dir.path().join("test_ledger"));
        assert!(store.is_ok());
    }

    #[test]
    fn test_upsert_and_get_job() {
        let (store, _temp) = create_test_store();
        let snapshot = create_test_snapshot("job_123");

        store.upsert(snapshot.clone()).unwrap();
        let retrieved = store.get("job_123").unwrap();

        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.job_id, "job_123");
        assert_eq!(retrieved.job_type, "test");
        assert_eq!(retrieved.resource_total, 10);
    }

    #[test]
    fn test_get_nonexistent_job() {
        let (store, _temp) = create_test_store();
        let result = store.get("nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_idempotency() {
        let (store, _temp) = create_test_store();

        store
            .remember_idempotency("key_123".to_string(), "job_456".to_string())
            .unwrap();

        let result = store.get_idempotent("key_123").unwrap();
        assert_eq!(result, Some("job_456".to_string()));

        let missing = store.get_idempotent("missing_key").unwrap();
        assert_eq!(missing, None);
    }

    #[test]
    fn test_stats() {
        let (store, _temp) = create_test_store();

        let snapshot = create_test_snapshot("job_1");
        store.upsert(snapshot).unwrap();
        store
            .remember_idempotency("key_1".to_string(), "job_1".to_string())
            .unwrap();

        let stats = store.stats().unwrap();
        assert_eq!(stats.job_count, 1);
        assert_eq!(stats.idem_count, 1);
    }

    #[test]
    fn test_persist() {
        let (store, _temp) = create_test_store();
        let snapshot = create_test_snapshot("job_persist");
        store.upsert(snapshot).unwrap();

        // Persist should not error
        store.persist().unwrap();
    }
}
