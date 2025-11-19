use crate::proto::{DeadLetterTask, DownloadTask};
use fjall::{Config, Keyspace, PartitionCreateOptions, PartitionHandle};
use prost::Message;
use std::path::Path;
use thiserror::Error;
use tracing::info;
use uuid::Uuid;

#[derive(Error, Debug)]
pub enum DlqError {
    #[error("Fjall error: {0}")]
    Fjall(#[from] fjall::Error),

    #[error("Protobuf decode error: {0}")]
    ProtobufDecode(#[from] prost::DecodeError),

    #[error("Invalid UUID key: {0}")]
    InvalidUuid(uuid::Error),
}

pub type Result<T> = std::result::Result<T, DlqError>;

/// Fjall-backed storage for dead-lettered tasks
pub struct DlqStorage {
    keyspace: Keyspace,
    entries: PartitionHandle,
}

impl DlqStorage {
    /// Open or create the DLQ storage at the specified path
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        info!("Opening DLQ storage at: {}", path.as_ref().display());

        let keyspace = Config::new(path).open()?;
        let entries =
            keyspace.open_partition("dlq", PartitionCreateOptions::default())?;

        Ok(Self { keyspace, entries })
    }

    /// Record a failed task in the DLQ
    pub fn record(
        &self,
        task_id: Uuid,
        task: DownloadTask,
        failure_code: String,
        failure_message: String,
        attempts: u32,
    ) -> Result<()> {
        let dlq_entry = DeadLetterTask {
            task: Some(task),
            failure_code,
            failure_message,
            attempts,
            failed_at_ms: now_ms(),
        };

        let value = dlq_entry.encode_to_vec();
        self.entries.insert(task_id.as_bytes(), value)?;
        Ok(())
    }

    /// Fetch a DLQ entry by sequence number
    pub fn get(&self, task_id: Uuid) -> Result<Option<DeadLetterTask>> {
        if let Some(bytes) = self.entries.get(task_id.as_bytes())? {
            let task = DeadLetterTask::decode(&*bytes)?;
            Ok(Some(task))
        } else {
            Ok(None)
        }
    }

    /// List DLQ entries up to a given limit (ascending sequence order)
    pub fn list(&self, limit: usize) -> Result<Vec<(Uuid, DeadLetterTask)>> {
        let mut results = Vec::new();

        for entry in self.entries.iter().take(limit) {
            let (key, value) = entry?;
            let task_id =
                Uuid::from_slice(key.as_ref()).map_err(DlqError::InvalidUuid)?;
            let dlq_task = DeadLetterTask::decode(&*value)?;
            results.push((task_id, dlq_task));
        }

        Ok(results)
    }

    /// Flush pending writes to disk
    pub fn flush(&self) -> Result<()> {
        self.keyspace.persist(fjall::PersistMode::SyncAll)?;
        Ok(())
    }
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_task(job_id: &str) -> DownloadTask {
        DownloadTask {
            job_id: job_id.to_string(),
            job_type: "test".to_string(),
            resource_id: "res1".to_string(),
            url: "https://example.com/file".to_string(),
            headers: vec![],
            storage_hint: None,
            proxy_hint: None,
            attempt: 1,
            tenant: "default".to_string(),
            trace_id: "trace123".to_string(),
            attributes: None,
            manifest_key: format!("s3://bucket/{job_id}.json"),
        }
    }

    #[test]
    fn test_record_and_get() {
        let temp_dir = TempDir::new().unwrap();
        let dlq = DlqStorage::open(temp_dir.path()).unwrap();

        let task = create_task("failed_job");
        let task_id = Uuid::now_v7();
        dlq.record(
            task_id,
            task,
            "NETWORK_ERROR".to_string(),
            "Connection timeout".to_string(),
            3,
        )
        .unwrap();

        let entry = dlq.get(task_id).unwrap().unwrap();
        assert_eq!(entry.failure_code, "NETWORK_ERROR");
        assert_eq!(entry.attempts, 3);
        assert!(entry.task.is_some());
    }

    #[test]
    fn test_list_entries() {
        let temp_dir = TempDir::new().unwrap();
        let dlq = DlqStorage::open(temp_dir.path()).unwrap();

        let mut order = Vec::new();
        for i in 0..5 {
            let task_id = Uuid::now_v7();
            dlq.record(
                task_id,
                create_task(&format!("job{}", i)),
                "ERROR".to_string(),
                "failure".to_string(),
                1,
            )
            .unwrap();
            order.push((task_id, format!("job{}", i)));
        }

        let entries = dlq.list(3).unwrap();
        assert_eq!(entries.len(), 3);
        // UUIDv7 preserves insertion order
        for (idx, (task_id, dlq_task)) in entries.iter().enumerate() {
            assert_eq!(
                dlq_task.task.as_ref().unwrap().job_id,
                format!("job{}", idx)
            );
            assert!(order.iter().any(|(id, _)| id == task_id));
        }
    }
}
