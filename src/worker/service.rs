use crate::proto::DownloadTask;
use crate::storage::StorageClient;
use crate::worker::error::WorkerError;
use crate::worker::http::HttpClient;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use tower::Service;

use crate::queue::TaskEnvelope;

/// Successful task execution metadata
#[derive(Debug, Clone)]
pub struct TaskOutcome {
    pub storage_key: String,
    pub bytes_written: usize,
}

/// Tower service responsible for performing the download + upload pipeline
pub struct DownloadService {
    worker_id: String,
    http_client: HttpClient,
    storage: StorageClient,
}

impl DownloadService {
    pub fn new(
        worker_id: String,
        http_client: HttpClient,
        storage: StorageClient,
    ) -> Self {
        Self {
            worker_id,
            http_client,
            storage,
        }
    }
}

impl Service<TaskEnvelope> for DownloadService {
    type Response = TaskOutcome;
    type Error = WorkerError;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(
        &mut self,
        _cx: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, envelope: TaskEnvelope) -> Self::Future {
        let http = self.http_client.clone();
        let storage = self.storage.clone();
        let worker_id = self.worker_id.clone();
        Box::pin(async move {
            let task = envelope.task;
            let headers = task
                .headers
                .iter()
                .map(|header| (header.name.clone(), header.value.clone()))
                .collect();

            let body = http.download(&task.url, headers).await?;

            let storage_key = determine_storage_key(&task);
            let metadata = storage
                .upload(&storage_key, body.to_vec())
                .await
                .map_err(WorkerError::from)?;

            tracing::info!(
                worker_id = %worker_id,
                job_id = %task.job_id,
                resource_id = %task.resource_id,
                storage_key,
                "Task stored successfully",
            );

            Ok(TaskOutcome {
                storage_key,
                bytes_written: metadata.size,
            })
        })
    }
}

/// Determine final storage key for a task
pub fn determine_storage_key(task: &DownloadTask) -> String {
    if let Some(hint) = &task.storage_hint
        && !hint.key_prefix.is_empty()
    {
        return format!("{}/{}", hint.key_prefix, task.resource_id);
    }

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
    fn storage_key_prefers_hint() {
        let task = DownloadTask {
            job_id: "job-1".into(),
            job_type: "default".into(),
            resource_id: "res-1".into(),
            storage_hint: Some(StorageHint {
                bucket: "bucket".into(),
                key_prefix: "custom".into(),
                metadata: Default::default(),
            }),
            ..Default::default()
        };

        assert_eq!(determine_storage_key(&task), "custom/res-1");
    }

    #[test]
    fn storage_key_falls_back_to_default() {
        let task = DownloadTask {
            job_id: "job-1".into(),
            job_type: "default".into(),
            resource_id: "res-1".into(),
            ..Default::default()
        };

        assert_eq!(
            determine_storage_key(&task),
            "resources/default/job-1/res-1"
        );
    }
}
