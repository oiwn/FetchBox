//! Task runner - processes individual DownloadTask messages

use super::http::{HttpClient, HttpConfig};
use crate::proto::{DownloadTask, JobLog, LogLevel};
use crate::storage::StorageClient;
use std::sync::Arc;
use thiserror::Error;
use time::OffsetDateTime;
use tracing::{error, info};

#[derive(Debug, Error)]
pub enum TaskError {
    #[error("Download failed: {0}")]
    DownloadFailed(String),

    #[error("Upload failed: {0}")]
    UploadFailed(String),

    #[error("Invalid task: {0}")]
    InvalidTask(String),
}

pub type Result<T> = std::result::Result<T, TaskError>;

// TODO: Rewrite process_task to use Fjall ledger instead of Iggy producer (Phase 4)

/*
/// Process a single download task
pub async fn process_task(
    task: DownloadTask,
    storage: Arc<StorageClient>,
    ledger: Arc<FjallStore>,
    proxy_url: Option<&str>,
) -> Result<()> {
    let job_id = task.job_id.clone();
    let resource_id = task.resource_id.clone();
    let url = task.url.clone();

    info!(job_id, resource_id, url, "Processing task");

    // Step 1: Download resource
    let http_config = HttpConfig::default();
    let client = HttpClient::new(http_config, proxy_url)
        .map_err(|e| TaskError::DownloadFailed(e.to_string()))?;

    // Convert headers from proto format
    let headers: Vec<(String, String)> = task
        .headers
        .iter()
        .map(|h| (h.name.clone(), h.value.clone()))
        .collect();

    let bytes = client
        .download(&url, headers)
        .await
        .map_err(|e| TaskError::DownloadFailed(e.to_string()))?;

    info!(
        job_id,
        resource_id,
        size = bytes.len(),
        "Download completed"
    );

    // Step 2: Upload to storage
    let storage_key = determine_storage_key(&task);

    storage
        .upload(&storage_key, bytes.to_vec())
        .await
        .map_err(|e| TaskError::UploadFailed(e.to_string()))?;

    info!(job_id, resource_id, storage_key, "Upload completed");

    // Step 3: Emit success log
    let log = JobLog {
        job_id: job_id.clone(),
        resource_id: resource_id.clone(),
        level: LogLevel::Info as i32,
        message: format!("Successfully downloaded and stored: {}", storage_key),
        fields: vec![
            (
                "storage_key".to_string(),
                storage_key.clone(),
            ),
            ("url".to_string(), url.clone()),
        ]
        .into_iter()
        .collect(),
        timestamp_ms: OffsetDateTime::now_utc().unix_timestamp() as u64 * 1000,
        trace_id: task.trace_id.clone(),
    };

    if let Err(e) = producer.publish_job_log(&log).await {
        error!(job_id, resource_id, error = %e, "Failed to publish log");
        // Don't fail the task if logging fails
    }

    Ok(())
}
*/

/// Determine storage key from task hints or defaults
pub fn determine_storage_key(task: &DownloadTask) -> String {
    if let Some(hint) = &task.storage_hint {
        // Use handler-provided storage hint
        if !hint.key_prefix.is_empty() {
            return format!("{}/{}", hint.key_prefix, task.resource_id);
        }
    }

    // Default: resources/{job_type}/{job_id}/{resource_id}
    format!(
        "resources/{}/{}/{}",
        task.job_type, task.job_id, task.resource_id
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::StorageHint;

    #[test]
    fn test_determine_storage_key_with_hint() {
        let task = DownloadTask {
            job_id: "job-123".to_string(),
            job_type: "test".to_string(),
            resource_id: "res-1".to_string(),
            storage_hint: Some(StorageHint {
                bucket: "test-bucket".to_string(),
                key_prefix: "custom/path".to_string(),
                metadata: Default::default(),
            }),
            ..Default::default()
        };

        assert_eq!(determine_storage_key(&task), "custom/path/res-1");
    }

    #[test]
    fn test_determine_storage_key_default() {
        let task = DownloadTask {
            job_id: "job-123".to_string(),
            job_type: "gallery".to_string(),
            resource_id: "img-456".to_string(),
            ..Default::default()
        };

        assert_eq!(
            determine_storage_key(&task),
            "resources/gallery/job-123/img-456"
        );
    }
}
