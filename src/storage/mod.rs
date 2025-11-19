//! Object storage abstraction for manifests and artifacts
//! Uses Apache Arrow object_store crate

use object_store::{ObjectStore, aws::AmazonS3Builder, path::Path as StoragePath};
use std::sync::Arc;
use thiserror::Error;

use crate::config::{StorageConfig as AppStorageConfig, StorageProvider};

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("Upload failed: {0}")]
    UploadFailed(String),

    #[error("Download failed: {0}")]
    DownloadFailed(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Storage configuration error: {0}")]
    Configuration(String),

    #[error("Object store error: {0}")]
    ObjectStoreError(#[from] object_store::Error),
}

/// Storage result type
pub type Result<T> = std::result::Result<T, StorageError>;

/// Metadata returned after upload
#[derive(Debug, Clone)]
pub struct UploadMetadata {
    pub key: String,
    pub etag: Option<String>,
    pub size: usize,
}

/// Storage client wrapping object_store
#[derive(Clone)]
pub struct StorageClient {
    store: Arc<dyn ObjectStore>,
    pub bucket: String,
}

impl StorageClient {
    /// Create new storage client with any object_store backend
    pub fn new(store: Arc<dyn ObjectStore>, bucket: String) -> Self {
        Self { store, bucket }
    }

    /// Create in-memory storage for testing/development
    pub fn in_memory() -> Self {
        Self::in_memory_with_bucket("fetchbox-local")
    }

    /// Create in-memory storage with a custom bucket identifier
    pub fn in_memory_with_bucket(bucket: impl Into<String>) -> Self {
        Self {
            store: Arc::new(object_store::memory::InMemory::new()),
            bucket: bucket.into(),
        }
    }

    /// Build a storage client from application storage configuration
    pub fn from_config(config: &AppStorageConfig) -> Result<Self> {
        match config.provider {
            StorageProvider::Local => {
                Ok(Self::in_memory_with_bucket(config.bucket.clone()))
            }
            StorageProvider::S3 => Self::from_s3_config(config),
        }
    }

    fn from_s3_config(config: &AppStorageConfig) -> Result<Self> {
        let bucket = config.bucket.clone();
        let access_key = config.access_key.clone().ok_or_else(|| {
            StorageError::Configuration(
                "S3 access key missing (set S3_ACCESS_KEY or AWS_ACCESS_KEY_ID)"
                    .to_string(),
            )
        })?;
        let secret_key = config.secret_key.clone().ok_or_else(|| {
            StorageError::Configuration(
                "S3 secret key missing (set S3_SECRET_KEY or AWS_SECRET_ACCESS_KEY)"
                    .to_string(),
            )
        })?;

        let mut builder = AmazonS3Builder::new()
            .with_bucket_name(&bucket)
            .with_access_key_id(&access_key)
            .with_secret_access_key(&secret_key);

        let region = config
            .region
            .clone()
            .unwrap_or_else(|| "us-east-1".to_string());
        builder = builder.with_region(&region);

        if let Some(endpoint) = &config.endpoint {
            builder = builder.with_endpoint(endpoint);

            // Enable path-style HTTP access for local MinIO-style endpoints
            if endpoint.starts_with("http://") {
                builder = builder.with_allow_http(true);
            }
        }

        let store = builder.build()?;
        Ok(Self::new(Arc::new(store), bucket))
    }

    /// Upload bytes to storage
    pub async fn upload(&self, key: &str, data: Vec<u8>) -> Result<UploadMetadata> {
        let path = StoragePath::from(key);
        let size = data.len();

        let put_result = self.store.put(&path, data.into()).await?;

        tracing::info!(key, size, "Uploaded to storage");

        Ok(UploadMetadata {
            key: key.to_string(),
            etag: put_result.e_tag.clone(),
            size,
        })
    }

    /// Download from storage
    pub async fn download(&self, key: &str) -> Result<Vec<u8>> {
        let path = StoragePath::from(key);

        let result = self.store.get(&path).await?;

        let bytes = result.bytes().await?;

        tracing::info!(key, size = bytes.len(), "Downloaded from storage");

        Ok(bytes.to_vec())
    }

    /// Check if key exists
    pub async fn exists(&self, key: &str) -> Result<bool> {
        let path = StoragePath::from(key);

        match self.store.head(&path).await {
            Ok(_) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
}
