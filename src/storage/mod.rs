//! Object storage abstraction for manifests and artifacts
//! Uses Apache Arrow object_store crate

use async_trait::async_trait;
use object_store::{ObjectStore, path::Path as StoragePath};
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("Upload failed: {0}")]
    UploadFailed(String),

    #[error("Download failed: {0}")]
    DownloadFailed(String),

    #[error("Not found: {0}")]
    NotFound(String),

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
        Self {
            store: Arc::new(object_store::memory::InMemory::new()),
            bucket: "fetchbox-local".to_string(),
        }
    }

    /// Upload bytes to storage
    pub async fn upload(&self, key: &str, data: Vec<u8>) -> Result<UploadMetadata> {
        let path = StoragePath::from(key);
        let size = data.len();

        let put_result = self.store
            .put(&path, data.into())
            .await?;

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

        let result = self.store
            .get(&path)
            .await?;

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
