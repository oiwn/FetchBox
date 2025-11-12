use async_trait::async_trait;
use thiserror::Error;

use super::types::{DownloadTask, JobSummary, ManifestContext, PreparedManifest};

/// Handler errors (spec §2)
#[derive(Debug, Error)]
pub enum HandlerError {
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),
    #[error("task generation failed: {0}")]
    TaskGeneration(String),
    #[error("finalization failed: {0}")]
    Finalization(String),
    #[error("fatal handler error: {0}")]
    Fatal(String),
}

/// Job handler trait for manifest processing (spec §2)
///
/// Handlers implement this trait to customize how manifests are processed
/// into download tasks. The trait is async to allow I/O operations.
#[async_trait]
pub trait JobHandler: Send + Sync {
    /// Prepare/validate the manifest before task generation
    async fn prepare_manifest(
        &self,
        ctx: ManifestContext,
    ) -> Result<PreparedManifest, HandlerError>;

    /// Build download tasks from the prepared manifest
    async fn build_tasks(
        &self,
        prepared: PreparedManifest,
    ) -> Result<Vec<DownloadTask>, HandlerError>;

    /// Finalize job after all tasks complete (optional hook)
    async fn finalize_job(&self, _summary: JobSummary) -> Result<(), HandlerError> {
        Ok(())
    }
}
