use crate::storage::StorageError;
use crate::worker::http::DownloadError;
use thiserror::Error;

/// Worker-level errors that determine retry/DLQ behavior
#[derive(Debug, Error, Clone)]
pub enum WorkerError {
    #[error("download error: {0}")]
    Download(String),
    #[error("upload error: {0}")]
    Upload(String),
    #[error("invalid task: {0}")]
    InvalidTask(String),
    #[error("ledger error: {0}")]
    Ledger(String),
    #[error("dlq error: {0}")]
    Dlq(String),
    #[error("task timed out: {0}")]
    Timeout(String),
}

impl WorkerError {
    /// Whether the error should be retried automatically
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            WorkerError::Download(_)
                | WorkerError::Upload(_)
                | WorkerError::Timeout(_)
        )
    }

    /// Canonical failure code used for DLQ + job snapshot errors
    pub fn code(&self) -> &'static str {
        match self {
            WorkerError::Download(_) => "DOWNLOAD_ERROR",
            WorkerError::Upload(_) => "UPLOAD_ERROR",
            WorkerError::InvalidTask(_) => "TASK_INVALID",
            WorkerError::Ledger(_) => "SYSTEM_LEDGER_ERROR",
            WorkerError::Dlq(_) => "SYSTEM_DLQ_ERROR",
            WorkerError::Timeout(_) => "TASK_TIMEOUT",
        }
    }
}

impl From<DownloadError> for WorkerError {
    fn from(value: DownloadError) -> Self {
        Self::Download(value.to_string())
    }
}

impl From<StorageError> for WorkerError {
    fn from(value: StorageError) -> Self {
        Self::Upload(value.to_string())
    }
}
