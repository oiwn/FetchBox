use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use thiserror::Error;

use super::r#trait::JobHandler;
use super::types::{HeadersMap, ProxyHint, StorageHint};

/// Handler configuration from TOML (spec task_01.md §3)
#[derive(Clone, Debug)]
pub struct HandlerConfig {
    pub handler: String,
    pub default_headers: HeadersMap,
    pub proxy: Option<ProxyConfig>,
    pub storage: Option<StorageConfig>,
    pub options: Value,
}

/// Proxy configuration (spec task_01.md §3)
#[derive(Clone, Debug)]
pub struct ProxyConfig {
    pub primary: String,
    pub fallbacks: Vec<String>,
}

impl ProxyConfig {
    pub fn to_hint(&self) -> ProxyHint {
        ProxyHint {
            primary_pool: self.primary.clone(),
            fallback_pools: self.fallbacks.clone(),
        }
    }
}

/// Storage configuration (spec task_01.md §3)
#[derive(Clone, Debug)]
pub struct StorageConfig {
    pub bucket: String,
    pub key_prefix: String,
}

impl StorageConfig {
    pub fn to_hint(&self, job_id: &str, resource_id: &str) -> StorageHint {
        StorageHint {
            bucket: self.bucket.clone(),
            key_prefix: format!("{}{}/{}", self.key_prefix, job_id, resource_id),
            object_metadata: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("handler not found: {0}")]
    NotFound(String),
}

/// Registry mapping job types to handler instances (spec task_02.md §4)
#[derive(Clone)]
pub struct HandlerRegistry {
    handlers: BTreeMap<String, Arc<dyn JobHandler>>,
    configs: BTreeMap<String, HandlerConfig>,
}

impl HandlerRegistry {
    pub fn new() -> Self {
        Self {
            handlers: BTreeMap::new(),
            configs: BTreeMap::new(),
        }
    }

    pub fn register(
        &mut self,
        job_type: impl Into<String>,
        handler: Arc<dyn JobHandler>,
        config: HandlerConfig,
    ) {
        let job_type = job_type.into();
        self.handlers.insert(job_type.clone(), handler);
        self.configs.insert(job_type, config);
    }

    pub fn get(
        &self,
        job_type: &str,
    ) -> Result<Arc<dyn JobHandler>, RegistryError> {
        self.handlers
            .get(job_type)
            .cloned()
            .ok_or_else(|| RegistryError::NotFound(job_type.to_string()))
    }

    pub fn get_config(&self, job_type: &str) -> Option<&HandlerConfig> {
        self.configs.get(job_type)
    }

    pub fn has_handler(&self, job_type: &str) -> bool {
        self.handlers.contains_key(job_type)
    }

    /// Create default registry with built-in handlers
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();

        // Default handler config
        let default_config = HandlerConfig {
            handler: "fetchbox::handlers::DefaultHandler".to_string(),
            default_headers: HeadersMap::new(),
            proxy: None,
            storage: None,
            options: Value::Object(Map::new()),
        };

        // Register default handler for both "default" and "gallery" job types
        let default_handler =
            Arc::new(super::default::DefaultHandler::new(default_config.clone()));
        registry.register(
            "default",
            default_handler.clone(),
            default_config.clone(),
        );
        registry.register("gallery", default_handler, default_config);

        registry
    }
}

impl Default for HandlerRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}
