use thiserror::Error;

#[derive(Debug, Error)]
pub enum LedgerError {
    #[error("Fjall error: {0}")]
    Fjall(#[from] fjall::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Job not found: {0}")]
    JobNotFound(String),

    #[error("Invalid key format: {0}")]
    InvalidKey(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, LedgerError>;
