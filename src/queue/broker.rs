use crate::proto::DownloadTask;
use crate::queue::store::{FjallQueue, QueueError};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, warn};

/// TaskEnvelope wraps a DownloadTask with its sequence number
#[derive(Clone, Debug)]
pub struct TaskEnvelope {
    pub seq: u64,
    pub task: DownloadTask,
}

/// TaskBroker distributes tasks from the API to worker pool
///
/// Architecture:
/// 1. API calls `broker.enqueue(task)`
/// 2. Broker persists task to FjallQueue (atomic, get seq)
/// 3. Broker sends TaskEnvelope{seq, task} to worker via mpsc channel
/// 4. Round-robin distribution across worker pool
/// 5. Backpressure via bounded channels (default: 100 per worker)
///
/// The broker is NOT a separate task - it's just a struct with methods
/// called by API handlers. Distribution is synchronous via mpsc::send().
pub struct TaskBroker {
    queue: Arc<RwLock<FjallQueue>>,
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
        queue: Arc<RwLock<FjallQueue>>,
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
            queue,
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
    pub async fn enqueue(&self, task: DownloadTask) -> Result<u64, QueueError> {
        // Persist to Fjall (write lock)
        let seq = {
            let queue = self.queue.write().await;
            queue.enqueue(&task)?
        };

        debug!(
            seq,
            job_id = %task.job_id,
            resource_id = %task.resource_id,
            "Task persisted to queue"
        );

        // Create envelope
        let envelope = TaskEnvelope {
            seq,
            task: task.clone(),
        };

        // Round-robin to next worker
        let worker_idx = self.next_worker.fetch_add(1, Ordering::Relaxed) % self.worker_channels.len();

        // Send to worker (bounded channel, may block if full = backpressure)
        match self.worker_channels[worker_idx].send(envelope).await {
            Ok(_) => {
                debug!(seq, worker_idx, "Task sent to worker");
                Ok(seq)
            }
            Err(_) => {
                warn!(seq, worker_idx, "Worker channel closed, task not delivered");
                // Worker is dead, but task is already persisted in queue
                // Could implement retry to another worker here
                Ok(seq) // Return seq anyway, task is safe in Fjall
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
    use crate::queue::FjallQueue;
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
        }
    }

    #[tokio::test]
    async fn test_broker_enqueue() {
        let temp_dir = TempDir::new().unwrap();
        let queue = Arc::new(RwLock::new(FjallQueue::open(temp_dir.path()).unwrap()));

        let (broker, mut receivers) = TaskBroker::new(queue.clone(), 2, 10);
        let broker = Arc::new(broker);

        // Enqueue a task
        let task = create_test_task("job1", "res1");
        let seq = broker.enqueue(task.clone()).await.unwrap();

        assert_eq!(seq, 0);

        // Should be received by first worker (round-robin starts at 0)
        let envelope = receivers[0].recv().await.unwrap();
        assert_eq!(envelope.seq, 0);
        assert_eq!(envelope.task.job_id, "job1");

        // Next task should go to worker 1
        let task2 = create_test_task("job2", "res2");
        let seq2 = broker.enqueue(task2).await.unwrap();
        assert_eq!(seq2, 1);

        let envelope2 = receivers[1].recv().await.unwrap();
        assert_eq!(envelope2.seq, 1);
        assert_eq!(envelope2.task.job_id, "job2");
    }

    #[tokio::test]
    async fn test_round_robin_distribution() {
        let temp_dir = TempDir::new().unwrap();
        let queue = Arc::new(RwLock::new(FjallQueue::open(temp_dir.path()).unwrap()));

        let (broker, mut receivers) = TaskBroker::new(queue.clone(), 3, 10);
        let broker = Arc::new(broker);

        // Enqueue 6 tasks
        for i in 0..6 {
            let task = create_test_task(&format!("job{}", i), &format!("res{}", i));
            broker.enqueue(task).await.unwrap();
        }

        // Each worker should receive 2 tasks
        for worker_id in 0..3 {
            let env1 = receivers[worker_id].recv().await.unwrap();
            let env2 = receivers[worker_id].recv().await.unwrap();

            // Worker 0 gets tasks 0, 3
            // Worker 1 gets tasks 1, 4
            // Worker 2 gets tasks 2, 5
            assert_eq!(env1.seq, worker_id as u64);
            assert_eq!(env2.seq, (worker_id + 3) as u64);
        }
    }

    #[tokio::test]
    async fn test_persistence_before_distribution() {
        let temp_dir = TempDir::new().unwrap();
        let queue = Arc::new(RwLock::new(FjallQueue::open(temp_dir.path()).unwrap()));

        let (broker, _receivers) = TaskBroker::new(queue.clone(), 1, 10);
        // Drop receivers immediately - simulates worker crash

        let task = create_test_task("job1", "res1");
        let seq = broker.enqueue(task).await.unwrap();

        // Task should still be in Fjall even though worker channel is closed
        let retrieved = queue.read().await.get_task(seq).unwrap().unwrap();
        assert_eq!(retrieved.job_id, "job1");
    }
}
