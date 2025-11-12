use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::api::models::{Manifest, Resource};

pub type HeadersMap = BTreeMap<String, String>;

/// Context passed to handlers during manifest preparation (spec §2)
#[derive(Debug, Clone)]
pub struct ManifestContext {
    pub job_id: String,
    pub job_type: String,
    pub manifest: Manifest,
}

/// Handler-prepared manifest context (spec §2)
#[derive(Debug, Clone)]
pub struct PreparedManifest {
    pub context: ManifestContext,
    /// Handler can store additional preparation state here
    pub handler_data: Option<Value>,
}

/// Individual download task emitted by handlers (spec §2)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadTask {
    pub resource_name: String,
    pub url: String,
    pub http_headers: Vec<(String, String)>,
    pub proxy_hint: Option<ProxyHint>,
    pub storage_hint: Option<StorageHint>,
    pub tags: BTreeMap<String, String>,
    /// Per-resource attributes from manifest
    pub attributes: Option<Value>,
}

/// Storage location override hint (spec §2)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageHint {
    pub bucket: String,
    pub key_prefix: String,
    /// Optional object metadata (content-type, cache-control, etc.)
    pub object_metadata: Option<BTreeMap<String, String>>,
}

/// Proxy pool hint with fallbacks (spec §2)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyHint {
    pub primary_pool: String,
    pub fallback_pools: Vec<String>,
}

/// Job completion summary for finalization (spec §2)
#[derive(Debug, Clone)]
pub struct JobSummary {
    pub job_id: String,
    pub job_type: String,
    pub total_resources: usize,
    pub completed_resources: usize,
    pub failed_resources: usize,
}

/// Task context containing job-level metadata for proto conversion
#[derive(Debug, Clone)]
pub struct TaskContext {
    pub job_id: String,
    pub job_type: String,
    pub tenant: String,
    pub manifest_key: String,
}

impl DownloadTask {
    pub fn from_resource(
        resource: &Resource,
        default_headers: &HeadersMap,
        storage_hint: Option<StorageHint>,
        proxy_hint: Option<ProxyHint>,
    ) -> Self {
        let mut headers = default_headers.clone();
        headers.extend(resource.headers.clone());

        let http_headers: Vec<(String, String)> = headers.into_iter().collect();

        Self {
            resource_name: resource.name.clone(),
            url: resource.url.clone(),
            http_headers,
            proxy_hint,
            storage_hint,
            tags: resource.tags.clone(),
            attributes: None,
        }
    }

    /// Convert handler DownloadTask to proto DownloadTask
    pub fn to_proto(&self, ctx: &TaskContext) -> crate::proto::DownloadTask {
        use crate::proto::{DownloadTask, HttpHeader, TaskAttributes};
        use uuid::Uuid;

        let attributes = TaskAttributes {
            tags: self.tags.clone().into_iter().collect(), // BTreeMap → HashMap
            checksum_hint: String::new(),
            mime_hint: String::new(),
            extra: self
                .attributes
                .as_ref()
                .map(|v| v.to_string().into_bytes())
                .unwrap_or_default(),
        };

        DownloadTask {
            job_id: ctx.job_id.clone(),
            job_type: ctx.job_type.clone(),
            resource_id: self.resource_name.clone(),
            url: self.url.clone(),
            headers: self
                .http_headers
                .iter()
                .map(|(name, value)| HttpHeader {
                    name: name.clone(),
                    value: value.clone(),
                })
                .collect(),
            storage_hint: self.storage_hint.clone().map(Into::into),
            proxy_hint: self.proxy_hint.clone().map(Into::into),
            attributes: Some(attributes),
            manifest_key: ctx.manifest_key.clone(),
            attempt: 1,
            tenant: ctx.tenant.clone(),
            trace_id: Uuid::new_v4().to_string(),
        }
    }
}

// Conversions between handler types and proto types (spec task_03.md §6)

impl From<StorageHint> for crate::proto::StorageHint {
    fn from(hint: StorageHint) -> Self {
        Self {
            bucket: hint.bucket,
            key_prefix: hint.key_prefix,
            metadata: hint.object_metadata.unwrap_or_default().into_iter().collect(),
        }
    }
}

impl From<crate::proto::StorageHint> for StorageHint {
    fn from(proto: crate::proto::StorageHint) -> Self {
        Self {
            bucket: proto.bucket,
            key_prefix: proto.key_prefix,
            object_metadata: if proto.metadata.is_empty() {
                None
            } else {
                Some(proto.metadata.into_iter().collect())
            },
        }
    }
}

impl From<ProxyHint> for crate::proto::ProxyHint {
    fn from(hint: ProxyHint) -> Self {
        Self {
            primary_pool: hint.primary_pool,
            fallbacks: hint.fallback_pools,
        }
    }
}

impl From<crate::proto::ProxyHint> for ProxyHint {
    fn from(proto: crate::proto::ProxyHint) -> Self {
        Self {
            primary_pool: proto.primary_pool,
            fallback_pools: proto.fallbacks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    #[test]
    fn test_storage_hint_conversion() {
        let hint = StorageHint {
            bucket: "test-bucket".to_string(),
            key_prefix: "prefix/".to_string(),
            object_metadata: Some([("content-type".to_string(), "image/jpeg".to_string())].into()),
        };

        let proto: crate::proto::StorageHint = hint.clone().into();
        assert_eq!(proto.bucket, "test-bucket");
        assert_eq!(proto.key_prefix, "prefix/");
        assert_eq!(proto.metadata.len(), 1);

        let back: StorageHint = proto.into();
        assert_eq!(back.bucket, hint.bucket);
        assert_eq!(back.key_prefix, hint.key_prefix);
        assert!(back.object_metadata.is_some());
    }

    #[test]
    fn test_storage_hint_empty_metadata() {
        let hint = StorageHint {
            bucket: "bucket".to_string(),
            key_prefix: "prefix/".to_string(),
            object_metadata: None,
        };

        let proto: crate::proto::StorageHint = hint.clone().into();
        assert!(proto.metadata.is_empty());

        let back: StorageHint = proto.into();
        assert!(back.object_metadata.is_none());
    }

    #[test]
    fn test_proxy_hint_conversion() {
        let hint = ProxyHint {
            primary_pool: "primary".to_string(),
            fallback_pools: vec!["fallback1".to_string(), "fallback2".to_string()],
        };

        let proto: crate::proto::ProxyHint = hint.clone().into();
        assert_eq!(proto.primary_pool, "primary");
        assert_eq!(proto.fallbacks.len(), 2);

        let back: ProxyHint = proto.into();
        assert_eq!(back.primary_pool, hint.primary_pool);
        assert_eq!(back.fallback_pools, hint.fallback_pools);
    }

    #[test]
    fn test_proto_serialization_roundtrip() {
        let proto_hint = crate::proto::ProxyHint {
            primary_pool: "pool1".to_string(),
            fallbacks: vec!["pool2".to_string()],
        };

        // Serialize
        let bytes = proto_hint.encode_to_vec();
        assert!(!bytes.is_empty());

        // Deserialize
        let decoded = crate::proto::ProxyHint::decode(&bytes[..]).unwrap();
        assert_eq!(decoded.primary_pool, "pool1");
        assert_eq!(decoded.fallbacks.len(), 1);
    }

    #[test]
    fn test_download_task_creation() {
        let resource = Resource {
            name: "resource-1.jpg".to_string(),
            url: "https://example.com/file.jpg".to_string(),
            headers: [("User-Agent".to_string(), "Test/1.0".to_string())].into(),
            tags: [("type".to_string(), "image".to_string())].into(),
        };

        let default_headers = [("Accept".to_string(), "*/*".to_string())].into();
        let storage = Some(StorageHint {
            bucket: "bucket".to_string(),
            key_prefix: "prefix/".to_string(),
            object_metadata: None,
        });
        let proxy = Some(ProxyHint {
            primary_pool: "primary".to_string(),
            fallback_pools: vec![],
        });

        let task = DownloadTask::from_resource(&resource, &default_headers, storage, proxy);

        assert_eq!(task.resource_name, "resource-1.jpg");
        assert_eq!(task.url, "https://example.com/file.jpg");
        assert_eq!(task.http_headers.len(), 2); // merged headers
        assert!(task.storage_hint.is_some());
        assert!(task.proxy_hint.is_some());
        assert_eq!(task.tags.len(), 1);
    }
}
