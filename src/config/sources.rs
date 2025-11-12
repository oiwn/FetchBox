use super::models::Config;
use config::{ConfigError, Environment, File};
use std::env;
use std::path::PathBuf;

const CONFIG_ENV_VAR: &str = "FETCHBOX_CONFIG";
const DEFAULT_CONFIG_PATH: &str = "config/fetchbox.toml";
const ENV_PREFIX: &str = "FETCHBOX";
const ENV_SEPARATOR: &str = "__";

/// Load configuration from multiple sources with priority:
/// 1. Defaults (embedded in structs)
/// 2. TOML file (if exists)
/// 3. Environment variables from .env file (via dotenvy)
/// 4. System environment variables (highest priority)
pub fn load() -> Result<Config, ConfigError> {
    // Load .env file if it exists (ignore errors if file doesn't exist)
    let _ = dotenvy::dotenv();

    let config_path = env::var(CONFIG_ENV_VAR)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_CONFIG_PATH));

    let mut config = load_from_sources(config_path)?;

    // Load secrets from environment variables
    load_secrets(&mut config);

    Ok(config)
}

/// Load secrets from environment variables into config
/// Secrets are never stored in TOML files, only in environment
fn load_secrets(config: &mut Config) {
    // Load S3 credentials
    if let Ok(access_key) = env::var("S3_ACCESS_KEY") {
        config.storage.access_key = Some(access_key);
    }
    if let Ok(secret_key) = env::var("S3_SECRET_KEY") {
        config.storage.secret_key = Some(secret_key);
    }

    // Alternative: AWS-style environment variable names
    if config.storage.access_key.is_none() {
        if let Ok(access_key) = env::var("AWS_ACCESS_KEY_ID") {
            config.storage.access_key = Some(access_key);
        }
    }
    if config.storage.secret_key.is_none() {
        if let Ok(secret_key) = env::var("AWS_SECRET_ACCESS_KEY") {
            config.storage.secret_key = Some(secret_key);
        }
    }
}

/// Load configuration from a specific path and environment
/// Useful for testing with custom config files
pub fn load_from_sources(config_path: PathBuf) -> Result<Config, ConfigError> {
    let mut builder = config::Config::builder();

    // Start with defaults (handled by struct Default implementations)
    // Add TOML file if it exists (optional)
    if config_path.exists() {
        tracing::info!("Loading configuration from: {}", config_path.display());
        builder = builder.add_source(File::from(config_path).required(false));
    } else {
        tracing::warn!(
            "Configuration file not found at {}, using defaults and environment overrides",
            config_path.display()
        );
    }

    // Add environment variable overrides
    // FETCHBOX__SERVER__BIND_ADDR -> server.bind_addr
    builder = builder.add_source(
        Environment::with_prefix(ENV_PREFIX)
            .separator(ENV_SEPARATOR)
            .try_parsing(true),
    );

    let config = builder.build()?;
    config.try_deserialize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_load_defaults_only() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("nonexistent.toml");

        let config = load_from_sources(config_path).unwrap();
        assert_eq!(config.server.bind_addr.to_string(), "0.0.0.0:8080");
    }

    #[test]
    fn test_load_from_toml() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("test.toml");

        let toml_content = r#"
[server]
bind_addr = "127.0.0.1:9000"
max_manifest_bytes = "10MB"

[iggy]
endpoint = "iggy://test-server:8090"
client_id = "test-client"
        "#;

        fs::write(&config_path, toml_content).unwrap();

        let config = load_from_sources(config_path).unwrap();
        assert_eq!(config.server.bind_addr.to_string(), "127.0.0.1:9000");
        assert_eq!(config.server.api.max_payload_bytes.as_u64(), 10 * 1024 * 1024);
    }

    // Note: test_env_overrides removed due to unsafe env::set_var usage
    // Environment variable overrides are tested in integration tests

    #[test]
    fn test_complex_config() {
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
provider = "s3"
bucket = "fetchbox-default"
region = "us-east-1"

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

[proxy.pools.global]
primary = ["http://global-1:8080"]
fallbacks = []

[retention]
job_ttl_days = 30
ledger_max_bytes = "50GB"
logs_ttl_days = 30

[telemetry]
metrics_addr = "0.0.0.0:9090"
        "#;

        fs::write(&config_path, toml_content).unwrap();

        let config = load_from_sources(config_path).unwrap();

        // Verify server config
        assert_eq!(config.server.bind_addr.to_string(), "0.0.0.0:8080");
        assert_eq!(config.server.api.max_payload_bytes.as_u64(), 5 * 1024 * 1024);

        // Verify handlers
        assert_eq!(config.handlers.len(), 2);
        assert!(config.handlers.contains_key("default"));
        assert!(config.handlers.contains_key("gallery"));

        let default_handler = &config.handlers["default"];
        assert_eq!(default_handler.proxy_pool, "default");

        // Verify proxy pools
        assert_eq!(config.proxy.pools.len(), 2);
        let default_pool = &config.proxy.pools["default"];
        assert_eq!(default_pool.primary.len(), 2);
        assert_eq!(default_pool.fallbacks, vec!["global"]);

        // Verify retention
        assert_eq!(config.retention.job_ttl_days, 30);
        assert_eq!(
            config.retention.ledger_max_bytes.as_u64(),
            50 * 1024 * 1024 * 1024
        );
    }
}
