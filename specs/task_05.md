# Spec: Configuration Loader & Proxy Fallbacks

## Goal
Implement a configuration system that loads FetchBox settings (handlers, proxy pools, storage, Iggy/Fjall endpoints, retention) from layered sources (TOML + environment). This spec covers the config crate, schema, validation, and exposure to other components.

## Scope
1. Create `crates/fetchbox_config` providing strongly-typed config structs.
2. Support loading from:
   - Default values.
   - TOML file (path via `FETCHBOX_CONFIG` env or default `config/fetchbox.toml`).
   - Environment overrides (`FETCHBOX__...` names).
3. Validate handler registration, proxy fallback chains, storage credentials, and size limits.
4. Expose config watch/reload hooks (optional) or document restart requirements.
5. Document config schema for operators.

## 1. Crate Layout
- `crates/fetchbox_config/Cargo.toml`
- `src/lib.rs` exposing `Config::load()` and structs.
- Modules:
  - `sources.rs` — load from file/env.
  - `validation.rs` — cross-field checks.
  - `models.rs` — struct definitions.
  - `watch.rs` (optional) — file watcher for reload.

Dependencies: `serde`, `serde_with`, `serde_json`, `config` crate or `figment`, `derive_builder` optional, `thiserror`.

## 2. Configuration Schema

Top-level TOML example:
```toml
[server]
bind_addr = "0.0.0.0:8080"
max_manifest_bytes = "5MB"
fjall_path = "data/ledger"

[iggy]
endpoint = "iggy://localhost:8090"
client_id = "fetchbox-api"
client_secret = "secret"

[storage]
provider = "s3"
bucket = "fetchbox-default"
region = "us-east-1"
endpoint = "https://s3.amazonaws.com"
access_key = "minio"
secret_key = "minio123"

[handlers.default]
handler = "fetchbox_handlers::default::DefaultManifestHandler"
proxy_pool = "pools/default"

[handlers.gallery]
handler = "fetchbox_handlers::gallery::GalleryHandler"
proxy_pool = "pools/gallery"
storage_bucket = "fetchbox-gallery"
default_headers = { "User-Agent" = "FetchBox/1.0" }

[proxy.pools.default]
primary = ["http://proxy-a:8080", "http://proxy-b:8080"]
fallbacks = ["pools/global"]
retry_backoff_ms = 500

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
```

### Structs
```rust
pub struct Config {
    pub server: ServerConfig,
    pub iggy: IggyConfig,
    pub storage: StorageConfig,
    pub handlers: HashMap<String, HandlerConfig>,
    pub proxy: ProxyConfig,
    pub retention: RetentionConfig,
    pub telemetry: TelemetryConfig,
}
```

- `ServerConfig { bind_addr: SocketAddr, max_manifest_bytes: ByteSize, fjall_path: PathBuf }`
- `IggyConfig { endpoint: String, client_id: String, client_secret: SecretString }`
- `StorageConfig { provider: StorageProvider, bucket: String, endpoint: String, access_key: SecretString, secret_key: SecretString, region: Option<String> }`
- `HandlerConfig` includes:
  - `handler`: String path.
  - `proxy_pool`: String.
  - `storage_bucket`: Option<String>.
  - `key_prefix`: Option<String>.
  - `default_headers`: `BTreeMap<String, String>`.
  - `options`: `serde_json::Value`.
- `ProxyConfig`:
  ```rust
  pub struct ProxyPoolConfig {
      pub primary: Vec<String>, // URIs
      pub fallbacks: Vec<String>, // references to other pools by name
      pub retry_backoff_ms: u64,
      pub max_retries: u32,
  }
  pub struct ProxyConfig {
      pub pools: HashMap<String, ProxyPoolConfig>;
  }
  ```
- `RetentionConfig` for Fjall and Iggy TTL.
- `TelemetryConfig` for metrics/OTLP endpoints.

Byte size parsing via `humansize` or custom.

## 3. Load & Override Behavior
- `Config::load()`:
  1. Determine config path: env `FETCHBOX_CONFIG` or default `config/fetchbox.toml`.
  2. Load TOML file if exists (optional; allow running with env-only config).
  3. Apply environment overrides: env keys shaped like `FETCHBOX__SERVER__BIND_ADDR=0.0.0.0:9000`. Double underscore splits levels.
  4. Apply defaults (e.g., `max_manifest_bytes` defaults 5 MB).
- Provide helper `Config::from_sources(path, env)` for testing.

## 4. Validation Rules
- Ensure each handler references an existing proxy pool (either direct or fallback chain).
- Ensure proxy fallback references eventually terminate (no cycles). Detect cycles via DFS; return error.
- Validate `max_manifest_bytes` ≤ 5 MB (per spec) unless explicitly overridden.
- Ensure `handlers` map non-empty (at least `default`).
- Validate storage credentials present when provider = `s3`.
- Validate `iggy.endpoint` scheme `iggy://` or `tcp://`.
- Ensure retention TTLs positive.
- Optionally warn if `fallbacks` reference unknown pools.

Return `ConfigError` enumerations with descriptive messages.

## 5. Proxy Fallback Resolution API
- Provide helper function `ProxyGraph::resolve(pool_name) -> ResolvedProxyPool` where `ResolvedProxyPool` flattens primary endpoints and fallback order.
- Example result:
  ```rust
  pub struct ResolvedProxyPool {
      pub tiers: Vec<Vec<ProxyEndpoint>>, // each tier sequential; fallback tier used on failure
  }
  ```
- Worker uses this structure to rotate proxies.
- Cache resolved pools inside config crate; recompute on reload.

## 6. Reload Strategy
- For v0, document that config is loaded at startup. Provide `ConfigWatcher` feature flag that watches the TOML file and triggers callback (optional). If implemented:
  - Use `notify` crate.
  - On change, reload file + env, re-run validation, and atomically swap `Arc<Config>`.
  - Expose subscribe API for components.
- If watcher deferred, note in README (“restart service after changing config”).

## 7. Deliverables
- `crates/fetchbox_config` with load/validate functions, resolved proxy helper, tests for parsing/validation.
- Example config file under `config/fetchbox.example.toml`.
- Documentation `docs/configuration.md` describing schema, env overrides, and proxy fallback behavior.
