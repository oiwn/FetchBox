//! Observability stubs (metrics, tracing)

use std::sync::atomic::{AtomicU64, Ordering};

/// Metrics handle for recording counters/gauges
#[derive(Debug, Default)]
pub struct Metrics {
    jobs_accepted: AtomicU64,
    jobs_failed: AtomicU64,
    tasks_published: AtomicU64,
}

impl Metrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn job_accepted(&self) {
        self.jobs_accepted.fetch_add(1, Ordering::Relaxed);
        tracing::debug!(counter = "jobs_accepted", "Metric incremented");
    }

    pub fn job_failed(&self) {
        self.jobs_failed.fetch_add(1, Ordering::Relaxed);
        tracing::debug!(counter = "jobs_failed", "Metric incremented");
    }

    pub fn task_published(&self) {
        self.tasks_published.fetch_add(1, Ordering::Relaxed);
        tracing::debug!(counter = "tasks_published", "Metric incremented");
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            jobs_accepted: self.jobs_accepted.load(Ordering::Relaxed),
            jobs_failed: self.jobs_failed.load(Ordering::Relaxed),
            tasks_published: self.tasks_published.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub jobs_accepted: u64,
    pub jobs_failed: u64,
    pub tasks_published: u64,
}
