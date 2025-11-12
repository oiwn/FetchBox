use crate::proto::{DeadLetterTask, DownloadTask};
use fjall::{Config, Keyspace, PartitionCreateOptions, PartitionHandle};
use prost::Message;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tracing::{debug, info};

#[derive(Error, Debug)]
pub enum QueueError {
    #[error("Fjall error: {0}")]
    Fjall(#[from] fjall::Error),

    #[error("Protobuf decode error: {0}")]
    ProtobufDecode(#[from] prost::DecodeError),

    #[error("Task not found: seq={0}")]
    TaskNotFound(u64),

    #[error("Invalid sequence number")]
    InvalidSequence,
}

pub type Result<T> = std::result::Result<T, QueueError>;

/// FjallQueue manages task persistence and DLQ using Fjall embedded database
///
/// Architecture:
/// - `tasks` partition: u64 (big-endian) → DownloadTask (protobuf)
/// - `metadata` partition: "next_seq" → u64 (atomic counter)
/// - `dlq` partition: u64 (big-endian) → DeadLetterTask (protobuf)
///
/// The queue uses sequential u64 IDs for efficient storage and indexing.
/// Tasks are persisted atomically before being sent to workers via mpsc channels.
pub struct FjallQueue {
    keyspace: Keyspace,
    tasks: PartitionHandle,
    metadata: PartitionHandle,
    dlq: PartitionHandle,
    seq_counter: Arc<AtomicU64>,
}

impl FjallQueue {
    /// Open or create a new FjallQueue at the specified path
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        info!("Opening FjallQueue at: {}", path.as_ref().display());

        let keyspace = Config::new(path).open()?;

        let tasks = keyspace.open_partition("tasks", PartitionCreateOptions::default())?;
        let metadata = keyspace.open_partition("metadata", PartitionCreateOptions::default())?;
        let dlq = keyspace.open_partition("dlq", PartitionCreateOptions::default())?;

        // Load the current sequence counter from metadata
        let current_seq = metadata
            .get(b"next_seq")?
            .map(|bytes| u64::from_be_bytes(bytes.as_ref().try_into().unwrap_or([0u8; 8])))
            .unwrap_or(0);

        info!("FjallQueue opened, current sequence: {}", current_seq);

        Ok(Self {
            keyspace,
            tasks,
            metadata,
            dlq,
            seq_counter: Arc::new(AtomicU64::new(current_seq)),
        })
    }

    /// Enqueue a task and return its sequence number
    ///
    /// This operation is atomic:
    /// 1. Increment sequence counter (in-memory)
    /// 2. Persist task to Fjall
    /// 3. Persist updated counter to metadata
    ///
    /// If any step fails, the counter is not persisted, maintaining consistency.
    pub fn enqueue(&self, task: &DownloadTask) -> Result<u64> {
        // Atomically increment counter (fast, in-memory)
        let seq = self.seq_counter.fetch_add(1, Ordering::SeqCst);

        // Serialize task
        let value = task.encode_to_vec();

        // Persist task with sequence as key
        let key = seq.to_be_bytes();
        self.tasks.insert(key, value)?;

        // Persist updated counter (for crash recovery)
        let next_seq = seq + 1;
        self.metadata.insert(b"next_seq", next_seq.to_be_bytes())?;

        debug!(seq, job_id = %task.job_id, "Task enqueued");

        Ok(seq)
    }

    /// Retrieve a task by sequence number
    pub fn get_task(&self, seq: u64) -> Result<Option<DownloadTask>> {
        let key = seq.to_be_bytes();

        if let Some(bytes) = self.tasks.get(key)? {
            let task = DownloadTask::decode(&*bytes)?;
            Ok(Some(task))
        } else {
            Ok(None)
        }
    }

    /// Move a task to the Dead Letter Queue (DLQ)
    ///
    /// Called when a task exhausts all retries or encounters a permanent failure.
    pub fn move_to_dlq(
        &self,
        seq: u64,
        failure_code: String,
        failure_message: String,
        attempts: u32,
    ) -> Result<()> {
        // Retrieve the original task
        let task = self
            .get_task(seq)?
            .ok_or(QueueError::TaskNotFound(seq))?;

        // Create DLQ entry
        let dlq_entry = DeadLetterTask {
            task: Some(task),
            failure_code,
            failure_message,
            attempts,
            failed_at_ms: now_ms(),
        };

        // Persist to DLQ partition
        let key = seq.to_be_bytes();
        let value = dlq_entry.encode_to_vec();
        self.dlq.insert(key, value)?;

        info!(seq, attempts, "Task moved to DLQ");

        // Optionally remove from tasks partition to save space
        // self.tasks.remove(key)?;

        Ok(())
    }

    /// Get a task from the DLQ by sequence number
    pub fn get_dlq_task(&self, seq: u64) -> Result<Option<DeadLetterTask>> {
        let key = seq.to_be_bytes();

        if let Some(bytes) = self.dlq.get(key)? {
            let dlq_task = DeadLetterTask::decode(&*bytes)?;
            Ok(Some(dlq_task))
        } else {
            Ok(None)
        }
    }

    /// List DLQ tasks (for debugging/inspection)
    pub fn list_dlq(&self, limit: usize) -> Result<Vec<(u64, DeadLetterTask)>> {
        let mut results = Vec::new();

        for item in self.dlq.iter().take(limit) {
            let (key, value) = item?;
            let seq = u64::from_be_bytes(key.as_ref().try_into().unwrap_or([0u8; 8]));
            let dlq_task = DeadLetterTask::decode(&*value)?;
            results.push((seq, dlq_task));
        }

        Ok(results)
    }

    /// Get current sequence counter value
    pub fn current_seq(&self) -> u64 {
        self.seq_counter.load(Ordering::SeqCst)
    }

    /// Flush all writes to disk
    pub fn flush(&self) -> Result<()> {
        self.keyspace.persist(fjall::PersistMode::SyncAll)?;
        Ok(())
    }

    /// Health check - verify database is accessible
    pub fn health_check(&self) -> Result<()> {
        // Try to read the sequence counter
        let _ = self.metadata.get(b"next_seq")?;
        Ok(())
    }
}

/// Get current Unix timestamp in milliseconds
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
    use crate::proto::fetchbox::jobs::DownloadTask;
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
        }
    }

    #[test]
    fn test_enqueue_and_retrieve() {
        let temp_dir = TempDir::new().unwrap();
        let queue = FjallQueue::open(temp_dir.path()).unwrap();

        let task = create_test_task("job1");
        let seq = queue.enqueue(&task).unwrap();

        assert_eq!(seq, 0); // First task gets seq 0

        let retrieved = queue.get_task(seq).unwrap().unwrap();
        assert_eq!(retrieved.job_id, "job1");
    }

    #[test]
    fn test_sequential_ids() {
        let temp_dir = TempDir::new().unwrap();
        let queue = FjallQueue::open(temp_dir.path()).unwrap();

        let seq1 = queue.enqueue(&create_test_task("job1")).unwrap();
        let seq2 = queue.enqueue(&create_test_task("job2")).unwrap();
        let seq3 = queue.enqueue(&create_test_task("job3")).unwrap();

        assert_eq!(seq1, 0);
        assert_eq!(seq2, 1);
        assert_eq!(seq3, 2);
    }

    #[test]
    fn test_move_to_dlq() {
        let temp_dir = TempDir::new().unwrap();
        let queue = FjallQueue::open(temp_dir.path()).unwrap();

        let task = create_test_task("failed_job");
        let seq = queue.enqueue(&task).unwrap();

        queue
            .move_to_dlq(seq, "NETWORK_ERROR".to_string(), "Connection timeout".to_string(), 3)
            .unwrap();

        let dlq_task = queue.get_dlq_task(seq).unwrap().unwrap();
        assert_eq!(dlq_task.failure_code, "NETWORK_ERROR");
        assert_eq!(dlq_task.attempts, 3);
        assert!(dlq_task.task.is_some());
    }

    #[test]
    fn test_persistence_across_reopens() {
        let temp_dir = TempDir::new().unwrap();

        let seq = {
            let queue = FjallQueue::open(temp_dir.path()).unwrap();
            queue.enqueue(&create_test_task("job1")).unwrap()
        };

        // Reopen and check sequence continues
        let queue = FjallQueue::open(temp_dir.path()).unwrap();
        assert_eq!(queue.current_seq(), 1); // Next seq after 0

        let seq2 = queue.enqueue(&create_test_task("job2")).unwrap();
        assert_eq!(seq2, 1);

        // Can still retrieve old task
        let old_task = queue.get_task(seq).unwrap().unwrap();
        assert_eq!(old_task.job_id, "job1");
    }
}
