use super::models::{Config, StorageProvider};
use crate::humanize::ByteSize;
use std::collections::{BTreeMap, HashMap, HashSet};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("Handler '{handler}' references non-existent proxy pool '{pool}'")]
    InvalidProxyPoolReference { handler: String, pool: String },

    #[error("Proxy pool fallback cycle detected: {path}")]
    ProxyFallbackCycle { path: String },

    #[error("Proxy pool '{pool}' references non-existent fallback '{fallback}'")]
    InvalidFallbackReference { pool: String, fallback: String },

    #[error("max_payload_bytes ({actual}) exceeds limit of 5MB ({limit})")]
    ManifestSizeExceedsLimit { actual: u64, limit: u64 },

    #[error("No handlers configured (at least 'default' handler is required)")]
    NoHandlersConfigured,

    #[error("Storage provider is S3 but missing credentials (access_key or secret_key)")]
    MissingS3Credentials,

    #[error("Invalid Iggy endpoint scheme '{scheme}', expected 'iggy://' or 'tcp://'")]
    InvalidIggyScheme { scheme: String },

    #[error("Retention TTL must be positive: {field} = {value}")]
    InvalidRetentionTTL { field: String, value: u32 },

    #[error("Ledger max bytes must be positive")]
    InvalidLedgerMaxBytes,
}

/// Validate the entire configuration
pub fn validate(config: &Config) -> Result<(), ValidationError> {
    validate_handlers(config)?;
    validate_proxy_pools(config)?;
    validate_manifest_size(config)?;
    validate_storage(config)?;
    validate_retention(config)?;
    Ok(())
}

/// Ensure at least one handler exists and all handlers reference valid proxy pools
fn validate_handlers(config: &Config) -> Result<(), ValidationError> {
    if config.handlers.is_empty() {
        return Err(ValidationError::NoHandlersConfigured);
    }

    for (handler_name, handler_config) in &config.handlers {
        // Skip proxy validation if no proxy_pool configured (v0 allows optional proxies)
        if let Some(ref proxy_pool) = handler_config.proxy_pool {
            // Extract pool name from proxy_pool (may be "pools/default" or just "default")
            let pool_name = proxy_pool
                .strip_prefix("pools/")
                .unwrap_or(proxy_pool);

            if !config.proxy.pools.contains_key(pool_name) {
                return Err(ValidationError::InvalidProxyPoolReference {
                    handler: handler_name.clone(),
                    pool: proxy_pool.clone(),
                });
            }
        }
    }

    Ok(())
}

/// Validate proxy pool fallback chains for cycles and invalid references
fn validate_proxy_pools(config: &Config) -> Result<(), ValidationError> {
    // Check all fallback references exist
    for (pool_name, pool_config) in &config.proxy.pools {
        for fallback in &pool_config.fallbacks {
            let fallback_name = fallback.strip_prefix("pools/").unwrap_or(fallback);

            if !config.proxy.pools.contains_key(fallback_name) {
                return Err(ValidationError::InvalidFallbackReference {
                    pool: pool_name.clone(),
                    fallback: fallback.clone(),
                });
            }
        }
    }

    // Detect cycles using DFS
    for pool_name in config.proxy.pools.keys() {
        detect_cycles(pool_name, &config.proxy.pools, &mut HashSet::new(), &mut Vec::new())?;
    }

    Ok(())
}

/// DFS-based cycle detection in proxy fallback chains
fn detect_cycles(
    current: &str,
    pools: &HashMap<String, super::models::ProxyPoolConfig>,
    visited: &mut HashSet<String>,
    path: &mut Vec<String>,
) -> Result<(), ValidationError> {
    if path.contains(&current.to_string()) {
        // Cycle detected
        path.push(current.to_string());
        return Err(ValidationError::ProxyFallbackCycle {
            path: path.join(" -> "),
        });
    }

    if visited.contains(current) {
        return Ok(()); // Already explored this path
    }

    visited.insert(current.to_string());
    path.push(current.to_string());

    if let Some(pool) = pools.get(current) {
        for fallback in &pool.fallbacks {
            let fallback_name = fallback.strip_prefix("pools/").unwrap_or(fallback);
            detect_cycles(fallback_name, pools, visited, path)?;
        }
    }

    path.pop();
    Ok(())
}

/// Ensure max_payload_bytes doesn't exceed 5MB (unless explicitly overridden)
fn validate_manifest_size(config: &Config) -> Result<(), ValidationError> {
    const MAX_PAYLOAD_BYTES: u64 = 5 * 1024 * 1024; // 5 MB

    if config.server.api.max_payload_bytes.as_u64() > MAX_PAYLOAD_BYTES {
        return Err(ValidationError::ManifestSizeExceedsLimit {
            actual: config.server.api.max_payload_bytes.as_u64(),
            limit: MAX_PAYLOAD_BYTES,
        });
    }

    Ok(())
}

/// Validate storage credentials when provider is S3
fn validate_storage(config: &Config) -> Result<(), ValidationError> {
    if config.storage.provider == StorageProvider::S3 {
        if config.storage.access_key.is_none() || config.storage.secret_key.is_none() {
            return Err(ValidationError::MissingS3Credentials);
        }
    }

    Ok(())
}

/// Validate retention configuration
fn validate_retention(config: &Config) -> Result<(), ValidationError> {
    if config.retention.job_ttl_days == 0 {
        return Err(ValidationError::InvalidRetentionTTL {
            field: "job_ttl_days".to_string(),
            value: 0,
        });
    }

    if config.retention.logs_ttl_days == 0 {
        return Err(ValidationError::InvalidRetentionTTL {
            field: "logs_ttl_days".to_string(),
            value: 0,
        });
    }

    if config.retention.ledger_max_bytes.as_u64() == 0 {
        return Err(ValidationError::InvalidLedgerMaxBytes);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::models::*;
    use super::*;

    fn create_test_config() -> Config {
        let mut handlers = HashMap::new();
        handlers.insert(
            "default".to_string(),
            HandlerConfig {
                handler: "test::Handler".to_string(),
                proxy_pool: "default".to_string(),
                storage_bucket: None,
                key_prefix: None,
                default_headers: BTreeMap::new(),
                options: serde_json::Value::Null,
            },
        );

        let mut pools = HashMap::new();
        pools.insert(
            "default".to_string(),
            ProxyPoolConfig {
                primary: vec!["http://proxy:8080".to_string()],
                fallbacks: vec![],
                retry_backoff_ms: 500,
                max_retries: 3,
            },
        );

        Config {
            server: ServerConfig::default(),
            iggy: IggyConfig::default(),
            storage: StorageConfig::default(),
            handlers,
            proxy: ProxyConfig { pools },
            retention: RetentionConfig::default(),
            telemetry: TelemetryConfig::default(),
        }
    }

    #[test]
    fn test_valid_config() {
        let config = create_test_config();
        assert!(validate(&config).is_ok());
    }

    #[test]
    fn test_no_handlers() {
        let mut config = create_test_config();
        config.handlers.clear();

        let result = validate(&config);
        assert!(matches!(result, Err(ValidationError::NoHandlersConfigured)));
    }

    #[test]
    fn test_invalid_proxy_pool_reference() {
        let mut config = create_test_config();
        config.handlers.get_mut("default").unwrap().proxy_pool = "nonexistent".to_string();

        let result = validate(&config);
        assert!(matches!(
            result,
            Err(ValidationError::InvalidProxyPoolReference { .. })
        ));
    }

    #[test]
    fn test_cycle_detection() {
        let mut config = create_test_config();

        // Create a cycle: pool_a -> pool_b -> pool_a
        config.proxy.pools.insert(
            "pool_a".to_string(),
            ProxyPoolConfig {
                primary: vec!["http://a:8080".to_string()],
                fallbacks: vec!["pool_b".to_string()],
                retry_backoff_ms: 500,
                max_retries: 3,
            },
        );

        config.proxy.pools.insert(
            "pool_b".to_string(),
            ProxyPoolConfig {
                primary: vec!["http://b:8080".to_string()],
                fallbacks: vec!["pool_a".to_string()],
                retry_backoff_ms: 500,
                max_retries: 3,
            },
        );

        let result = validate(&config);
        assert!(matches!(
            result,
            Err(ValidationError::ProxyFallbackCycle { .. })
        ));
    }

    #[test]
    fn test_invalid_fallback_reference() {
        let mut config = create_test_config();
        config.proxy.pools.get_mut("default").unwrap().fallbacks =
            vec!["nonexistent".to_string()];

        let result = validate(&config);
        assert!(matches!(
            result,
            Err(ValidationError::InvalidFallbackReference { .. })
        ));
    }

    #[test]
    fn test_manifest_size_limit() {
        let mut config = create_test_config();
        config.server.api.max_payload_bytes = ByteSize(10 * 1024 * 1024); // 10 MB

        let result = validate(&config);
        assert!(matches!(
            result,
            Err(ValidationError::ManifestSizeExceedsLimit { .. })
        ));
    }

    #[test]
    fn test_s3_credentials_missing() {
        let mut config = create_test_config();
        config.storage.provider = StorageProvider::S3;
        config.storage.access_key = None;

        let result = validate(&config);
        assert!(matches!(
            result,
            Err(ValidationError::MissingS3Credentials)
        ));
    }

    #[test]
    fn test_invalid_iggy_scheme() {
        let mut config = create_test_config();
        config.iggy.endpoint = "http://localhost:8090".to_string();

        let result = validate(&config);
        assert!(matches!(
            result,
            Err(ValidationError::InvalidIggyScheme { .. })
        ));
    }

    #[test]
    fn test_zero_retention_ttl() {
        let mut config = create_test_config();
        config.retention.job_ttl_days = 0;

        let result = validate(&config);
        assert!(matches!(
            result,
            Err(ValidationError::InvalidRetentionTTL { .. })
        ));
    }
}
