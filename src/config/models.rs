use crate::humanize::ByteSize;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::path::PathBuf;

/// Top-level configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub handlers: HashMap<String, HandlerConfig>,
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub retention: RetentionConfig,
    #[serde(default)]
    pub telemetry: TelemetryConfig,
}

/// Server configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerConfig {
    #[serde(default = "default_bind_addr")]
    pub bind_addr: SocketAddr,
    #[serde(default = "default_fjall_path")]
    pub fjall_path: PathBuf,
    /// API limits (configurable per spec §1.4)
    #[serde(default)]
    pub api: ApiLimits,
}

/// API request limits (spec §1.4)
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiLimits {
    #[serde(default = "default_max_payload_bytes")]
    pub max_payload_bytes: ByteSize,
    #[serde(default = "default_max_resources_per_manifest")]
    pub max_resources_per_manifest: usize,
    #[serde(default = "default_max_headers_per_resource")]
    pub max_headers_per_resource: usize,
    #[serde(default = "default_max_header_value_bytes")]
    pub max_header_value_bytes: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: default_bind_addr(),
            fjall_path: default_fjall_path(),
            api: ApiLimits::default(),
        }
    }
}

impl Default for ApiLimits {
    fn default() -> Self {
        Self {
            max_payload_bytes: default_max_payload_bytes(),
            max_resources_per_manifest: default_max_resources_per_manifest(),
            max_headers_per_resource: default_max_headers_per_resource(),
            max_header_value_bytes: default_max_header_value_bytes(),
        }
    }
}

fn default_bind_addr() -> SocketAddr {
    "0.0.0.0:8080".parse().unwrap()
}

fn default_max_payload_bytes() -> ByteSize {
    ByteSize(5 * 1024 * 1024) // 5 MB
}

fn default_max_resources_per_manifest() -> usize {
    1000
}

fn default_max_headers_per_resource() -> usize {
    10
}

fn default_max_header_value_bytes() -> usize {
    1024
}

fn default_fjall_path() -> PathBuf {
    PathBuf::from("data/ledger")
}

/// Storage provider type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageProvider {
    S3,
    Local,
}

/// Storage configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StorageConfig {
    #[serde(default)]
    pub provider: StorageProvider,
    #[serde(default = "default_bucket")]
    pub bucket: String,
    pub endpoint: Option<String>,
    /// S3 access key (loaded from environment, not from config file)
    #[serde(skip)]
    pub access_key: Option<String>,
    /// S3 secret key (loaded from environment, not from config file)
    #[serde(skip)]
    pub secret_key: Option<String>,
    pub region: Option<String>,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            provider: StorageProvider::Local,
            bucket: default_bucket(),
            endpoint: None,
            access_key: None,
            secret_key: None,
            region: None,
        }
    }
}

impl Default for StorageProvider {
    fn default() -> Self {
        StorageProvider::Local
    }
}

fn default_bucket() -> String {
    "fetchbox-default".to_string()
}

/// Handler configuration (spec §3.1)
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HandlerConfig {
    /// Handler implementation path (e.g., "fetchbox_handlers::default::DefaultManifestHandler")
    pub handler: String,
    /// Optional storage bucket override (spec §3.1)
    pub storage_bucket: Option<String>,
    /// Default headers to include in requests
    #[serde(default)]
    pub default_headers: BTreeMap<String, String>,
    /// Handler-specific options (arbitrary JSON)
    #[serde(default)]
    pub options: serde_json::Value,
    /// Optional proxy pool reference (future enhancement, not in v0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_pool: Option<String>,
}

/// Proxy pool configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProxyPoolConfig {
    /// Primary proxy URIs
    pub primary: Vec<String>,
    /// Fallback pool names (e.g., "pools/global")
    #[serde(default)]
    pub fallbacks: Vec<String>,
    /// Retry backoff in milliseconds
    #[serde(default = "default_retry_backoff_ms")]
    pub retry_backoff_ms: u64,
    /// Maximum number of retries
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
}

fn default_retry_backoff_ms() -> u64 {
    500
}

fn default_max_retries() -> u32 {
    3
}

/// Proxy configuration
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ProxyConfig {
    #[serde(default)]
    pub pools: HashMap<String, ProxyPoolConfig>,
}

/// Retention configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RetentionConfig {
    #[serde(default = "default_job_ttl_days")]
    pub job_ttl_days: u32,
    #[serde(default = "default_ledger_max_bytes")]
    pub ledger_max_bytes: ByteSize,
    #[serde(default = "default_logs_ttl_days")]
    pub logs_ttl_days: u32,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            job_ttl_days: default_job_ttl_days(),
            ledger_max_bytes: default_ledger_max_bytes(),
            logs_ttl_days: default_logs_ttl_days(),
        }
    }
}

fn default_job_ttl_days() -> u32 {
    30
}

fn default_ledger_max_bytes() -> ByteSize {
    ByteSize(50 * 1024 * 1024 * 1024) // 50 GB
}

fn default_logs_ttl_days() -> u32 {
    30
}

/// Telemetry configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TelemetryConfig {
    #[serde(default = "default_metrics_addr")]
    pub metrics_addr: SocketAddr,
    pub otlp_endpoint: Option<String>,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            metrics_addr: default_metrics_addr(),
            otlp_endpoint: None,
        }
    }
}

fn default_metrics_addr() -> SocketAddr {
    "0.0.0.0:9090".parse().unwrap()
}

/// Resolved proxy pool with flattened tiers
#[derive(Debug, Clone)]
pub struct ResolvedProxyPool {
    /// Each tier represents a fallback level
    /// Tier 0 = primary proxies, Tier 1 = first fallback, etc.
    pub tiers: Vec<Vec<ProxyEndpoint>>,
}

/// Proxy endpoint information
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyEndpoint {
    pub uri: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config {
            server: ServerConfig::default(),
            storage: StorageConfig::default(),
            handlers: HashMap::new(),
            proxy: ProxyConfig::default(),
            retention: RetentionConfig::default(),
            telemetry: TelemetryConfig::default(),
        };

        assert_eq!(config.server.bind_addr.to_string(), "0.0.0.0:8080");
        assert_eq!(config.server.api.max_payload_bytes.as_u64(), 5 * 1024 * 1024);
        assert_eq!(config.server.api.max_resources_per_manifest, 1000);
    }
}
