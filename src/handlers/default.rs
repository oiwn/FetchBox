use async_trait::async_trait;

use super::r#trait::{HandlerError, JobHandler};
use super::registry::HandlerConfig;
use super::types::{DownloadTask, JobSummary, ManifestContext, PreparedManifest};

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
        ctx: ManifestContext,
    ) -> Result<PreparedManifest, HandlerError> {
        // Validate manifest version
        if ctx.manifest.manifest_version != "v1" {
            return Err(HandlerError::InvalidManifest(format!(
                "unsupported manifest version: {}",
                ctx.manifest.manifest_version
            )));
        }

        Ok(PreparedManifest {
            context: ctx,
            handler_data: None,
        })
    }

    async fn build_tasks(
        &self,
        prepared: PreparedManifest,
    ) -> Result<Vec<DownloadTask>, HandlerError> {
        let ctx = prepared.context;
        let job_id = &ctx.job_id;

        let tasks = ctx
            .manifest
            .resources
            .iter()
            .map(|resource| {
                let storage_hint = self
                    .config
                    .storage
                    .as_ref()
                    .map(|s| s.to_hint(job_id, &resource.name));

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
    use crate::api::models::{Manifest, Resource};
    use crate::handlers::types::HeadersMap;
    use serde_json::{Map, Value};

    fn sample_manifest() -> Manifest {
        Manifest {
            manifest_version: "v1".to_string(),
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
        let ctx = ManifestContext {
            job_id: "test-job".to_string(),
            job_type: "default".to_string(),
            manifest: sample_manifest(),
        };

        let result = handler.prepare_manifest(ctx).await;
        assert!(result.is_ok());

        let prepared = result.unwrap();
        assert_eq!(prepared.context.job_id, "test-job");
        assert_eq!(prepared.context.manifest.resources.len(), 1);
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
        let ctx = ManifestContext {
            job_id: "test-job".to_string(),
            job_type: "default".to_string(),
            manifest: sample_manifest(),
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

        let ctx = ManifestContext {
            job_id: "test-job".to_string(),
            job_type: "default".to_string(),
            manifest,
        };

        let result = handler.prepare_manifest(ctx).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), HandlerError::InvalidManifest(_)));
    }
}
