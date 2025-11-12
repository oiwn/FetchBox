//! Download worker service
//!
//! Tower-based worker that receives tasks from mpsc channels,
//! downloads resources, uploads to storage, and emits status/log updates.

pub mod http;
pub mod runner;

// TODO: Implement Tower Service-based worker (Phase 4)

type AnyError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Worker configuration
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub max_inflight_tasks: usize,
    pub poll_interval_ms: u64,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            max_inflight_tasks: 32,
            poll_interval_ms: 100,
        }
    }
}

// Old Iggy-based worker implementation removed
// Will be replaced with Tower Service-based worker in Phase 4
