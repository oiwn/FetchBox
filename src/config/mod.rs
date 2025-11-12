//! Configuration management for FetchBox
//!
//! This module provides a layered configuration system that loads settings from:
//! 1. Default values (embedded in structs)
//! 2. TOML configuration file
//! 3. Environment variables (highest priority)
//!
//! # Usage
//!
//! ```no_run
//! use fetchbox::config::Config;
//!
//! let config = Config::load().expect("Failed to load configuration");
//! println!("Server listening on: {}", config.server.bind_addr);
//! ```
//!
//! # Environment Variables
//!
//! Configuration can be overridden using environment variables with the pattern:
//! `FETCHBOX__<section>__<key>`
//!
//! Examples:
//! - `FETCHBOX__SERVER__BIND_ADDR=0.0.0.0:9000`
//! - `FETCHBOX__IGGY__ENDPOINT=iggy://prod-server:8090`
//! - `FETCHBOX__SERVER__MAX_MANIFEST_BYTES=10MB`
//!
//! # Configuration File
//!
//! By default, the configuration is loaded from `config/fetchbox.toml`.
//! This can be overridden using the `FETCHBOX_CONFIG` environment variable.

mod models;
mod resolver;
mod sources;
mod validation;

// Re-export public types
pub use crate::humanize::ByteSize;
pub use models::{
    ApiLimits, Config, HandlerConfig, ProxyConfig, ProxyEndpoint, ProxyPoolConfig,
    ResolvedProxyPool, RetentionConfig, ServerConfig, StorageConfig, StorageProvider,
    TelemetryConfig,
};
pub use resolver::{ProxyGraph, ResolverError};
pub use validation::ValidationError;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Failed to load configuration: {0}")]
    LoadError(#[from] config::ConfigError),

    #[error("Configuration validation failed: {0}")]
    ValidationError(#[from] ValidationError),

    #[error("Proxy resolution failed: {0}")]
    ResolverError(#[from] ResolverError),
}

impl Config {
    /// Load configuration from all sources (file + environment)
    ///
    /// Configuration is loaded with the following priority (highest to lowest):
    /// 1. Environment variables (`FETCHBOX__*`)
    /// 2. TOML file (default: `config/fetchbox.toml`)
    /// 3. Default values
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Configuration file is malformed
    /// - Validation fails (cycles, invalid references, etc.)
    pub fn load() -> Result<Self, ConfigError> {
        let config = sources::load()?;
        validation::validate(&config)?;
        Ok(config)
    }

    /// Load configuration from a specific path
    ///
    /// Useful for testing with custom configuration files.
    pub fn load_from_path(path: std::path::PathBuf) -> Result<Self, ConfigError> {
        let config = sources::load_from_sources(path)?;
        validation::validate(&config)?;
        Ok(config)
    }

    /// Get a proxy resolver for this configuration
    pub fn proxy_resolver(&self) -> ProxyGraph<'_> {
        ProxyGraph::new(&self.proxy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_load_minimal_config() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("test.toml");

        let toml_content = r#"
[handlers.default]
handler = "test::Handler"
proxy_pool = "default"

[proxy.pools.default]
primary = ["http://proxy:8080"]
        "#;

        fs::write(&config_path, toml_content).unwrap();

        let config = Config::load_from_path(config_path).unwrap();
        assert_eq!(config.handlers.len(), 1);
        assert_eq!(config.proxy.pools.len(), 1);
    }

    #[test]
    fn test_validation_catches_missing_pool() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("test.toml");

        let toml_content = r#"
[handlers.default]
handler = "test::Handler"
proxy_pool = "nonexistent"
        "#;

        fs::write(&config_path, toml_content).unwrap();

        let result = Config::load_from_path(config_path);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigError::ValidationError(ValidationError::InvalidProxyPoolReference { .. })
        ));
    }

    #[test]
    fn test_proxy_resolver_integration() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("test.toml");

        let toml_content = r#"
[handlers.default]
handler = "test::Handler"
proxy_pool = "primary"

[proxy.pools.primary]
primary = ["http://primary:8080"]
fallbacks = ["backup"]

[proxy.pools.backup]
primary = ["http://backup:8080"]
fallbacks = []
        "#;

        fs::write(&config_path, toml_content).unwrap();

        let config = Config::load_from_path(config_path).unwrap();
        let resolver = config.proxy_resolver();
        let resolved = resolver.resolve("primary").unwrap();

        assert_eq!(resolved.tiers.len(), 2);
        assert_eq!(resolved.tiers[0][0].uri, "http://primary:8080");
        assert_eq!(resolved.tiers[1][0].uri, "http://backup:8080");
    }

    #[test]
    fn test_full_config_example() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("test.toml");

        let toml_content = r#"
[server]
bind_addr = "0.0.0.0:8080"
max_manifest_bytes = "5MB"
fjall_path = "data/ledger"

[iggy]
endpoint = "iggy://localhost:8090"
client_id = "fetchbox-api"

[storage]
provider = "local"
bucket = "fetchbox-default"

[handlers.default]
handler = "fetchbox_handlers::default::DefaultManifestHandler"
proxy_pool = "default"

[handlers.gallery]
handler = "fetchbox_handlers::gallery::GalleryHandler"
proxy_pool = "gallery"
storage_bucket = "fetchbox-gallery"

[proxy.pools.default]
primary = ["http://proxy-a:8080", "http://proxy-b:8080"]
fallbacks = ["global"]
retry_backoff_ms = 500
max_retries = 3

[proxy.pools.gallery]
primary = ["http://gallery-proxy:8080"]
fallbacks = ["global"]

[proxy.pools.global]
primary = ["http://global-1:8080"]
fallbacks = []

[retention]
job_ttl_days = 30
ledger_max_bytes = "50GB"
logs_ttl_days = 30

[telemetry]
metrics_addr = "0.0.0.0:9090"
otlp_endpoint = "http://otel-collector:4317"
        "#;

        fs::write(&config_path, toml_content).unwrap();

        let config = Config::load_from_path(config_path).unwrap();

        // Verify all sections loaded correctly
        assert_eq!(config.server.bind_addr.to_string(), "0.0.0.0:8080");
        assert_eq!(config.handlers.len(), 2);
        assert_eq!(config.proxy.pools.len(), 3);
        assert_eq!(config.retention.job_ttl_days, 30);
        assert!(config.telemetry.otlp_endpoint.is_some());

        // Test proxy resolution
        let resolver = config.proxy_resolver();
        let resolved_default = resolver.resolve("default").unwrap();
        assert_eq!(resolved_default.tiers.len(), 2); // default -> global

        let resolved_gallery = resolver.resolve("gallery").unwrap();
        assert_eq!(resolved_gallery.tiers.len(), 2); // gallery -> global
    }
}
