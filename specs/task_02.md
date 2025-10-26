# Spec: Handler Trait & Plugin Crate (`fetchbox_plugins`)

^^^ Why such naming? Pretty sure it could be better.

## Goal
Define the shared handler interface and supporting data structures so FetchBox can plug in job-type specific logic (now or later). This spec covers the initial Rust crate, default handler implementation, configuration mapping expectations, and extensibility hooks for future WASM parity.

^^^ we should have possibility to add user defined handler but for simplicity for now client will form tasks in POST request to API endpoint.

## Scope
1. **Crate layout**: new crate `crates/fetchbox_plugins` exporting the handler traits, data models, and registration helpers.

2. **Traits**:
   - `JobHandler`: lifecycle hooks for manifest preparation, task generation, completion.
   - `ManifestContext`: wraps manifest JSON, headers, attributes.
   - `DownloadTask`: describes per-resource download request, including HTTP headers, proxy hint, storage hint, tags.
   - `StorageHint`: optional override for bucket/key/metadata.
   - `ProxyHint`: primary/fallback pool hints per task.
3. **Default handler**: `DefaultManifestHandler` that simply echoes the manifest into tasks.
4. **Registration mechanism**: builder or macro to map `job_type` strings to handler instances at startup.
5. **Documentation**: explain how custom handlers interact with Axum ingress and downloader worker.
6. **Forward compatibility**: note how the trait shape would be implemented by WASM modules in future (identical ABI boundaries).

^^^ do not think about WASM yet this could be mentioned in "Future development" section.

## 1. Crate Structure
- `crates/fetchbox_plugins/Cargo.toml`
- Modules:
  - `manifest.rs` — data types for manifests and attributes (`Manifest`, `Resource`, `Attributes`).
  - `task.rs` — `DownloadTask`, `HttpHeader`, `ProxyHint`, `StorageHint`.
  - `handler.rs` — trait definitions and `HandlerRegistration`.
  - `default.rs` — default handler.
  - `registry.rs` — helper for mapping job types to handlers (for API/worker to share).
  - `prelude.rs` — convenient re-exports.

## 2. Trait Definitions

```rust
pub struct ManifestContext {
    pub job_id: JobId,
    pub job_type: String,
    pub manifest_version: String,
    pub metadata: serde_json::Value,
    pub resources: Vec<ResourceSpec>,
    pub attributes: serde_json::Value,
}

pub trait JobHandler: Send + Sync {
    fn prepare_manifest(
        &self,
        ctx: &ManifestContext,
    ) -> Result<PreparedManifest, HandlerError>;

    fn build_tasks(
        &self,
        prepared: PreparedManifest,
        emitter: &mut dyn TaskEmitter,
    ) -> Result<(), HandlerError>;

    fn finalize_job(&self, summary: JobSummary) -> Result<(), HandlerError> {
        Ok(())
    }
}
```

^^^ i think we'll work in async context, maybe make traits async and return futures? 

Supporting types:
- `PreparedManifest` — handler-specific context (default just clones manifest).
- `TaskEmitter` — trait allowing handlers to push `DownloadTask` entries without allocating entire vectors.
- `DownloadTask` fields:
  - `resource_id: String`
  - `url: String`
  - `http_headers: Vec<(String, String)>`
  - `proxy_hint: ProxyHint` (optional)
  - `storage_hint: StorageHint` (optional)
  - `tags: BTreeMap<String, String>`
  - `attributes: serde_json::Value` (per-resource)

`StorageHint` includes `bucket`, `key_prefix`, `object_metadata`.
`ProxyHint` includes `primary_pool` and `Vec<fallback_pools>`.

`HandlerError` standardizes error categorization (`InvalidManifest`, `TaskGeneration`, `Fatal`, etc.).

## 3. Default Handler
- `DefaultManifestHandler`: implements `JobHandler`.
  - `prepare_manifest` validates manifest version and returns resources unchanged.
  - `build_tasks` iterates `ctx.resources` and emits tasks with manifest-provided headers/tags, default proxy/storage hints (from config).
  - `finalize_job` no-op.
- Provide unit tests verifying it echoes resources and respects header merges.

## 4. Registration Mechanism
- Provide `HandlerRegistry` with API:
  ```rust
  let mut registry = HandlerRegistry::new();
  registry.register("default", Arc::new(DefaultManifestHandler::new()));
  let handler = registry.get("gallery")?;
  ```
- Registry should be `Arc`-friendly for sharing across API and worker.
- Optionally support compile-time macro for static registration later (not required now).

## 5. Documentation
- Add `README.md` inside crate explaining how to implement custom handlers, expected invariants (e.g., do not mutate manifest metadata), and how Axum + worker call the trait methods.
- Include notes about how to supply default HTTP headers or storage hints.

## 6. WASM Notes
- Document that the trait is designed to be host-call friendly: all inputs/outputs are serializable (serde-friendly), enabling future WASM modules to implement the same interface via a shim.
- No WASM runtime work required now—just ensure types avoid non-FFI-friendly constructs.

## 7. Deliverables
- `crates/fetchbox_plugins` crate with trait definitions, default handler, registry, and README.
- Basic tests verifying default handler/task emission logic.
- Update workspace `Cargo.toml` to include the new crate.
- Reference the crate in root README or docs to guide handler authors.
