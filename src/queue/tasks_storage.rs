use crate::proto::DownloadTask;
use fjall::{Config, Keyspace, PartitionCreateOptions, PartitionHandle};
use prost::Message;
use std::path::Path;
use thiserror::Error;
use tracing::{debug, info};
use uuid::Uuid;

#[derive(Error, Debug)]
pub enum QueueError {
    #[error("Fjall error: {0}")]
    Fjall(#[from] fjall::Error),

    #[error("Protobuf decode error: {0}")]
    ProtobufDecode(#[from] prost::DecodeError),

    #[error("Task not found: {0}")]
    TaskNotFound(Uuid),

    #[error("Invalid UUID key: {0}")]
    InvalidUuid(#[from] uuid::Error),
}

pub type Result<T> = std::result::Result<T, QueueError>;

/// Fjall-backed storage for persisted tasks with UUIDv7 identifiers
pub struct TasksStorage {
    keyspace: Keyspace,
    tasks: PartitionHandle,
}

impl TasksStorage {
    /// Open or create the tasks storage at the specified path
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        info!("Opening TasksStorage at: {}", path.as_ref().display());

        let keyspace = Config::new(path).open()?;

        let tasks =
            keyspace.open_partition("tasks", PartitionCreateOptions::default())?;

        info!("TasksStorage opened");

        Ok(Self { keyspace, tasks })
    }

    /// Persist a task and return its assigned UUIDv7
    pub fn enqueue(&self, task: &DownloadTask) -> Result<Uuid> {
        let task_id = Uuid::now_v7();
        let value = task.encode_to_vec();
        self.tasks.insert(task_id.as_bytes(), value)?;

        debug!(task_id = %task_id, job_id = %task.job_id, "Task enqueued");

        Ok(task_id)
    }

    /// Retrieve a task by its UUID identifier
    pub fn get_task(&self, task_id: Uuid) -> Result<Option<DownloadTask>> {
        if let Some(bytes) = self.tasks.get(task_id.as_bytes())? {
            let task = DownloadTask::decode(&*bytes)?;
            Ok(Some(task))
        } else {
            Ok(None)
        }
    }

    /// Flush pending writes to disk
    pub fn flush(&self) -> Result<()> {
        self.keyspace.persist(fjall::PersistMode::SyncAll)?;
        Ok(())
    }

    /// Return a snapshot of all tasks currently persisted, up to `limit`
    pub fn list(&self, limit: usize) -> Result<Vec<(Uuid, DownloadTask)>> {
        let mut items = Vec::new();

        for entry in self.tasks.iter().take(limit) {
            let (key, value) = entry?;
            let task_id = Uuid::from_slice(key.as_ref())?;
            let task = DownloadTask::decode(&*value)?;
            items.push((task_id, task));
        }

        Ok(items)
    }

    /// Simple health check that ensures partition is readable
    pub fn health_check(&self) -> Result<()> {
        let _ = self.tasks.len();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_task(job_id: &str) -> DownloadTask {
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
    fn test_enqueue_and_retrieve() {
        let temp_dir = TempDir::new().unwrap();
        let storage = TasksStorage::open(temp_dir.path()).unwrap();

        let task = create_test_task("job1");
        let task_id = storage.enqueue(&task).unwrap();

        let retrieved = storage.get_task(task_id).unwrap().unwrap();
        assert_eq!(retrieved.job_id, "job1");
    }

    #[test]
    fn test_monotonic_ids() {
        let temp_dir = TempDir::new().unwrap();
        let storage = TasksStorage::open(temp_dir.path()).unwrap();

        let id1 = storage.enqueue(&create_test_task("job1")).unwrap();
        let id2 = storage.enqueue(&create_test_task("job2")).unwrap();
        let id3 = storage.enqueue(&create_test_task("job3")).unwrap();

        assert!(id1 < id2);
        assert!(id2 < id3);
    }

    #[test]
    fn test_persistence_across_reopens() {
        let temp_dir = TempDir::new().unwrap();

        let task_id = {
            let storage = TasksStorage::open(temp_dir.path()).unwrap();
            storage.enqueue(&create_test_task("job1")).unwrap()
        };

        let storage = TasksStorage::open(temp_dir.path()).unwrap();
        let task_id2 = storage.enqueue(&create_test_task("job2")).unwrap();
        assert_ne!(task_id, task_id2);

        let old_task = storage.get_task(task_id).unwrap().unwrap();
        assert_eq!(old_task.job_id, "job1");
    }

    #[test]
    fn test_list_returns_all_tasks() {
        let temp_dir = TempDir::new().unwrap();
        let storage = TasksStorage::open(temp_dir.path()).unwrap();

        for idx in 0..3 {
            let task = create_test_task(&format!("job{idx}"));
            storage.enqueue(&task).unwrap();
        }

        let tasks = storage.list(10).unwrap();
        assert_eq!(tasks.len(), 3);
    }
}
