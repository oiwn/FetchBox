//! Message streaming abstraction
//!
//! Note: Iggy integration has been removed in favor of single-process
//! architecture with Fjall-based queue and TaskBroker.

use async_trait::async_trait;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MessagingError {
    #[error("Publish failed: {0}")]
    PublishFailed(String),

    #[error("Connection error: {0}")]
    ConnectionError(String),
}

pub type Result<T> = std::result::Result<T, MessagingError>;

/// Message producer for publishing tasks
#[async_trait]
pub trait MessageProducer: Send + Sync {
    /// Publish message to stream
    async fn publish(&self, stream: &str, message: Vec<u8>) -> Result<()>;

    /// Health check
    async fn health(&self) -> bool;
}

/// Mock producer for development
#[derive(Debug, Clone, Default)]
pub struct MockProducer;

impl MockProducer {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl MessageProducer for MockProducer {
    async fn publish(&self, stream: &str, message: Vec<u8>) -> Result<()> {
        tracing::info!(stream, size = message.len(), "Mock publish");
        Ok(())
    }

    async fn health(&self) -> bool {
        true
    }
}
