use crate::proto::DownloadTask;
use crate::queue::{DlqStorage, QueueError, TasksStorage};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, info, warn};
use uuid::Uuid;

/// TaskEnvelope wraps a DownloadTask with its sequence number
#[derive(Clone, Debug)]
pub struct TaskEnvelope {
    pub task_id: Uuid,
    pub task: DownloadTask,
}

/// TaskBroker distributes tasks from the API to worker pool
///
/// Architecture:
/// 1. API calls `broker.enqueue(task)`
/// 2. Broker persists task to TasksStorage (atomic, get seq)
/// 3. Broker sends TaskEnvelope{seq, task} to worker via mpsc channel
/// 4. Round-robin distribution across worker pool
/// 5. Backpressure via bounded channels (default: 100 per worker)
///
/// The broker is NOT a separate task - it's just a struct with methods
/// called by API handlers. Distribution is synchronous via mpsc::send().
pub struct TaskBroker {
    tasks: Arc<RwLock<TasksStorage>>,
    #[allow(dead_code)]
    dlq: Arc<DlqStorage>,
    worker_channels: Vec<mpsc::Sender<TaskEnvelope>>,
    next_worker: AtomicUsize,
}

impl TaskBroker {
    /// Create a new TaskBroker with worker channels
    ///
    /// Returns:
    /// - TaskBroker instance (to be passed to API via Arc)
    /// - Vec of receivers (one per worker, for spawning workers)
    pub fn new(
        tasks: Arc<RwLock<TasksStorage>>,
        dlq: Arc<DlqStorage>,
        num_workers: usize,
        channel_size: usize,
    ) -> (Self, Vec<mpsc::Receiver<TaskEnvelope>>) {
        info!(
            num_workers,
            channel_size, "Creating TaskBroker with worker channels"
        );

        let mut worker_channels = Vec::with_capacity(num_workers);
        let mut worker_receivers = Vec::with_capacity(num_workers);

        for worker_id in 0..num_workers {
            let (tx, rx) = mpsc::channel(channel_size);
            worker_channels.push(tx);
            worker_receivers.push(rx);
            debug!(worker_id, "Created worker channel");
        }

        let broker = Self {
            tasks,
            dlq,
            worker_channels,
            next_worker: AtomicUsize::new(0),
        };

        (broker, worker_receivers)
    }

    /// Enqueue a task: persist to Fjall + distribute to worker
    ///
    /// This is called by API handlers. It's async because of the mpsc::send.
    ///
    /// Flow:
    /// 1. Lock queue (RwLock write)
    /// 2. Persist task → get sequence number
    /// 3. Unlock queue
    /// 4. Send envelope to next worker (round-robin)
    ///
    /// Returns the sequence number for the task.
    pub async fn enqueue(&self, task: DownloadTask) -> Result<Uuid, QueueError> {
        // Persist to Fjall (write lock)
        let task_id = {
            let tasks = self.tasks.write().await;
            tasks.enqueue(&task)?
        };

        debug!(
            task_id = %task_id,
            job_id = %task.job_id,
            resource_id = %task.resource_id,
            "Task persisted to queue"
        );

        // Create envelope
        let envelope = TaskEnvelope {
            task_id,
            task: task.clone(),
        };

        // Round-robin to next worker
        let worker_idx = self.next_worker.fetch_add(1, Ordering::Relaxed)
            % self.worker_channels.len();

        // Send to worker (bounded channel, may block if full = backpressure)
        match self.worker_channels[worker_idx].send(envelope).await {
            Ok(_) => {
                debug!(task_id = %task_id, worker_idx, "Task sent to worker");
                Ok(task_id)
            }
            Err(_) => {
                warn!(task_id = %task_id, worker_idx, "Worker channel closed, task not delivered");
                // Worker is dead, but task is already persisted in queue
                // Could implement retry to another worker here
                Ok(task_id) // Return id anyway, task is safe in Fjall
            }
        }
    }

    /// Get number of active workers
    pub fn num_workers(&self) -> usize {
        self.worker_channels.len()
    }

    /// Check if all worker channels are healthy (not closed)
    pub fn health_check(&self) -> bool {
        self.worker_channels.iter().all(|ch| !ch.is_closed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::{DlqStorage, TasksStorage};
    use tempfile::TempDir;

    fn create_test_task(job_id: &str, resource_id: &str) -> DownloadTask {
        DownloadTask {
            job_id: job_id.to_string(),
            job_type: "test".to_string(),
            resource_id: resource_id.to_string(),
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

    #[tokio::test]
    async fn test_broker_enqueue() {
        let temp_dir = TempDir::new().unwrap();
        let tasks_path = temp_dir.path().join("tasks");
        let dlq_path = temp_dir.path().join("dlq");
        let tasks = Arc::new(RwLock::new(TasksStorage::open(tasks_path).unwrap()));
        let dlq = Arc::new(DlqStorage::open(dlq_path).unwrap());

        let (broker, mut receivers) = TaskBroker::new(tasks.clone(), dlq, 2, 10);
        let broker = Arc::new(broker);

        // Enqueue a task
        let task = create_test_task("job1", "res1");
        let task_id = broker.enqueue(task.clone()).await.unwrap();

        // Should be received by first worker (round-robin starts at 0)
        let envelope = receivers[0].recv().await.unwrap();
        assert_eq!(envelope.task.job_id, "job1");
        assert_eq!(envelope.task_id, task_id);

        // Next task should go to worker 1
        let task2 = create_test_task("job2", "res2");
        let _ = broker.enqueue(task2).await.unwrap();

        let envelope2 = receivers[1].recv().await.unwrap();
        assert_eq!(envelope2.task.job_id, "job2");
    }

    #[tokio::test]
    async fn test_round_robin_distribution() {
        let temp_dir = TempDir::new().unwrap();
        let tasks_path = temp_dir.path().join("tasks");
        let dlq_path = temp_dir.path().join("dlq");
        let tasks = Arc::new(RwLock::new(TasksStorage::open(tasks_path).unwrap()));
        let dlq = Arc::new(DlqStorage::open(dlq_path).unwrap());

        let (broker, mut receivers) = TaskBroker::new(tasks, dlq, 3, 10);
        let broker = Arc::new(broker);

        // Enqueue 6 tasks
        for i in 0..6 {
            let task = create_test_task(&format!("job{}", i), &format!("res{}", i));
            broker.enqueue(task).await.unwrap();
        }

        // Each worker should receive 2 tasks
        for (worker_id, receiver) in receivers.iter_mut().enumerate().take(3) {
            let env1 = receiver.recv().await.unwrap();
            let env2 = receiver.recv().await.unwrap();

            // Worker 0 gets tasks 0, 3
            // Worker 1 gets tasks 1, 4
            // Worker 2 gets tasks 2, 5
            assert_eq!(env1.task.job_id, format!("job{}", worker_id));
            assert_eq!(env2.task.job_id, format!("job{}", worker_id + 3));
            assert_ne!(env1.task_id, env2.task_id);
        }
    }

    #[tokio::test]
    async fn test_persistence_before_distribution() {
        let temp_dir = TempDir::new().unwrap();
        let tasks_path = temp_dir.path().join("tasks");
        let dlq_path = temp_dir.path().join("dlq");
        let tasks = Arc::new(RwLock::new(TasksStorage::open(tasks_path).unwrap()));
        let dlq = Arc::new(DlqStorage::open(dlq_path).unwrap());

        let (broker, _receivers) = TaskBroker::new(tasks.clone(), dlq, 1, 10);
        // Drop receivers immediately - simulates worker crash

        let task = create_test_task("job1", "res1");
        let task_id = broker.enqueue(task).await.unwrap();

        // Task should still be in Fjall even though worker channel is closed
        let retrieved = tasks.read().await.get_task(task_id).unwrap().unwrap();
        assert_eq!(retrieved.job_id, "job1");
    }
}
