use async_trait::async_trait;
use serde_json::Value;

use super::registry::HandlerConfig;
use super::traits::{HandlerError, JobHandler};
use super::types::{DownloadTask, JobSummary, ManifestEnvelope, PreparedManifest};

/// Default handler implementation (spec task_02.md §3)
///
/// This handler simply echoes the manifest into tasks without transformation.
/// It's used when no custom handler logic is needed.
#[derive(Debug, Clone)]
pub struct DefaultHandler {
    config: HandlerConfig,
}

impl DefaultHandler {
    pub fn new(config: HandlerConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl JobHandler for DefaultHandler {
    async fn prepare_manifest(
        &self,
        envelope: ManifestEnvelope,
    ) -> Result<PreparedManifest, HandlerError> {
        // Validate manifest version
        if envelope.manifest.manifest_version != "v1" {
            return Err(HandlerError::InvalidManifest(format!(
                "unsupported manifest version: {}",
                envelope.manifest.manifest_version
            )));
        }

        Ok(PreparedManifest {
            envelope,
            handler_context: Value::Null,
        })
    }

    async fn build_tasks(
        &self,
        prepared: PreparedManifest,
    ) -> Result<Vec<DownloadTask>, HandlerError> {
        let envelope = prepared.envelope;
        let job_id = envelope.job_id;
        let manifest = envelope.manifest;

        let tasks = manifest
            .resources
            .iter()
            .map(|resource| {
                let storage_hint = self
                    .config
                    .storage
                    .as_ref()
                    .map(|s| s.to_hint(job_id.as_str(), &resource.name));

                let proxy_hint = self.config.proxy.as_ref().map(|p| p.to_hint());

                DownloadTask::from_resource(
                    resource,
                    &self.config.default_headers,
                    storage_hint,
                    proxy_hint,
                )
            })
            .collect();

        Ok(tasks)
    }

    async fn finalize_job(&self, _summary: JobSummary) -> Result<(), HandlerError> {
        // Default handler does nothing on finalization
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::{Manifest, Resource, StorageConfig};
    use crate::handlers::types::{HeadersMap, ManifestEnvelope};
    use chrono::Utc;
    use serde_json::{Map, Value};

    fn sample_manifest() -> Manifest {
        Manifest {
            manifest_version: "v1".to_string(),
            storage: StorageConfig {
                manifest_file: "manifest.json".to_string(),
                resource_key_prefix: "resources/test/".to_string(),
            },
            metadata: Value::Object(Map::new()),
            resources: vec![Resource {
                name: "resource-1".to_string(),
                url: "https://example.com/file.jpg".to_string(),
                headers: HeadersMap::new(),
                tags: HeadersMap::new(),
            }],
            attributes: Some(Value::Object(Map::new())),
        }
    }

    #[tokio::test]
    async fn test_default_handler_prepare_manifest() {
        let config = HandlerConfig {
            handler: "test".to_string(),
            default_headers: HeadersMap::new(),
            proxy: None,
            storage: None,
            options: Value::Object(Map::new()),
        };

        let handler = DefaultHandler::new(config);
        let ctx = ManifestEnvelope {
            job_id: "test-job".to_string(),
            job_type: "default".to_string(),
            tenant: "tenant-a".to_string(),
            manifest: sample_manifest(),
            manifest_location: None,
            received_at: Utc::now(),
        };

        let result = handler.prepare_manifest(ctx).await;
        assert!(result.is_ok());

        let prepared = result.unwrap();
        assert_eq!(prepared.envelope.job_id, "test-job");
        assert_eq!(prepared.envelope.manifest.resources.len(), 1);
    }

    #[tokio::test]
    async fn test_default_handler_build_tasks() {
        let config = HandlerConfig {
            handler: "test".to_string(),
            default_headers: HeadersMap::new(),
            proxy: None,
            storage: None,
            options: Value::Object(Map::new()),
        };

        let handler = DefaultHandler::new(config);
        let ctx = ManifestEnvelope {
            job_id: "test-job".to_string(),
            job_type: "default".to_string(),
            tenant: "tenant-a".to_string(),
            manifest: sample_manifest(),
            manifest_location: None,
            received_at: Utc::now(),
        };

        let prepared = handler.prepare_manifest(ctx).await.unwrap();
        let tasks = handler.build_tasks(prepared).await.unwrap();

        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].resource_name, "resource-1");
        assert_eq!(tasks[0].url, "https://example.com/file.jpg");
    }

    #[tokio::test]
    async fn test_default_handler_rejects_bad_version() {
        let config = HandlerConfig {
            handler: "test".to_string(),
            default_headers: HeadersMap::new(),
            proxy: None,
            storage: None,
            options: Value::Object(Map::new()),
        };

        let handler = DefaultHandler::new(config);
        let mut manifest = sample_manifest();
        manifest.manifest_version = "v2".to_string();

        let ctx = ManifestEnvelope {
            job_id: "test-job".to_string(),
            job_type: "default".to_string(),
            tenant: "tenant-a".to_string(),
            manifest,
            manifest_location: None,
            received_at: Utc::now(),
        };

        let result = handler.prepare_manifest(ctx).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            HandlerError::InvalidManifest(_)
        ));
    }
}
