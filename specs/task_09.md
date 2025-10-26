# Spec: Storage Abstraction & Overrides

## Goal
Wrap the Arrow object store client (or async S3-compatible SDK) into a reusable abstraction that supports default uploads plus handler-specific overrides. Provide APIs for the API service (manifest writes) and worker (resource uploads), with consistent error handling and metadata tagging.

## Scope
1. New crate `crates/fetchbox_storage`.
2. Interfaces for:
   - Storing manifests (`metadata.json`).
   - Uploading resources (streaming).
   - Downloading manifests (for worker/operator if needed).
3. Support handler overrides (custom bucket/key/metadata).
4. Integrate encryption flags, checksum calculation, and multipart thresholds.
5. Provide tests with MinIO.

## 1. Crate Layout
- `lib.rs` exports `StorageClient`.
- `config.rs` – parse storage config (provider, endpoint, bucket, credentials).
- `client.rs` – actual implementation using Arrow `object_store` crate (S3 backend).
- `manifest.rs` – helpers for writing/reading manifest files.
- `resource.rs` – uploading resources with metadata.
- `errors.rs` – error enum mapping S3/object store errors into FetchBox codes.
- `util/checksum.rs` – optional SHA-256 computation.

## 2. StorageClient API

```rust
pub struct StorageClient {
    inner: Arc<dyn ObjectStore>,
    default_bucket: String,
    default_prefix: String,
    multipart_threshold: ByteSize,
}

pub struct UploadOptions {
    pub bucket: Option<String>,
    pub key: Option<String>,
    pub metadata: BTreeMap<String, String>,
    pub content_type: Option<String>,
    pub encryption: Option<EncryptionMode>,
    pub checksum: Option<String>,
}

impl StorageClient {
    pub async fn upload_manifest(
        &self,
        job_type: &str,
        job_id: &str,
        data: bytes::Bytes,
    ) -> Result<StorageKey>;

    pub async fn upload_resource(
        &self,
        job_type: &str,
        job_id: &str,
        resource_id: &str,
        body: impl AsyncRead + Send + Unpin + 'static,
        options: UploadOptions,
    ) -> Result<StorageKey>;

    pub async fn download_manifest(&self, key: &StorageKey) -> Result<bytes::Bytes>;
}
```

`StorageKey` includes `bucket`, `key`, `etag`, `size_bytes`.

## 3. Handler Overrides
- `UploadOptions` consumed by worker to pass `StorageHint` from handler.
- Storage client merges hints with defaults: `bucket = handler.bucket.unwrap_or(default_bucket)`, `key = handler.key.unwrap_or(format!("resources/{job_type}/{job_id}/{resource_id}"))`.
- Additional metadata from handler appended to base metadata (content-type, checksum).
- Provide helper `StoragePlanner` that given `DownloadTask` returns `UploadOptions`.

## 4. Multipart & Streaming
- If body size > threshold (configurable, default 5 MB), use multipart upload via object_store (S3).
- For manifests, assume ≤ 5 MB; simple PUT.
- Provide `upload_resource_streaming` to pipe download stream directly into upload (avoid buffering entire file). Use `tokio_util::io::StreamReader`.

## 5. Error Handling
- Map object_store errors to `StorageError` with codes:
  - `StorageError::AccessDenied`
  - `StorageError::NotFound`
  - `StorageError::Network`
  - `StorageError::Throttled`
  - `StorageError::Other`
- Provide `is_retryable()` helper used by worker retry logic.

## 6. Testing
- Unit tests mocking object_store via `object_store::test_util::TempObjectStore`.
- Integration tests using MinIO (docker-compose):
  - Upload manifest -> verify stored bytes.
  - Upload resource with override -> confirm key.
  - Multipart threshold behavior.

## 7. Docs
- `docs/storage.md` describing default key patterns, how to configure buckets, and how handler overrides work.
- Document environment variables for credentials.

## 8. Deliverables
- `crates/fetchbox_storage` crate with APIs above, tests, and docs.
- Example usage in API + worker crates once integrated.
