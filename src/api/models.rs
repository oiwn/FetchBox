//! API models for FetchBox ingest and status endpoints.
//!
//! This module defines the core data structures used in FetchBox's external API contract:
//! - Client-facing ingest via `POST /jobs` accepts a [`Manifest`] payload
//! - Status endpoints return [`JobSnapshot`] for job tracking
//! - Clients can check job status by `job_id` on the status endpoint
//!
//! # Manifest Structure
//!
//! A complete manifest submission example (as JSON):
//!
//! ```json
//! {
//!   "manifest_version": "v1",
//!   "storage": {
//!     "manifest_file": "metadata.json",
//!     "resource_key_prefix": "resources/2024/05/01/dependencies/"
//!   },
//!   "metadata": {
//!     "...": "arbitrary structured metadata persisted as canonical metadata.json"
//!   },
//!   "resources": [
//!     {
//!       "name": "resource_name_01.jpg",
//!       "url": "https://cdn.example.com/image.jpg",
//!       "headers": {
//!         "Referer": "https://example.com/page.html",
//!         "User-Agent": "Crawler/1.0"
//!       },
//!       "tags": {
//!         "content_type": "image/jpeg",
//!         "checksum_hint": "sha256:deadbeef"
//!       }
//!     }
//!   ],
//!   "attributes": {
//!     "tenant": "crawler-a",
//!     "crawl_id": "847458834325543643",
//!     "crawled_at": "2024-05-01T10:00Z",
//!     "priority": "normal"
//!   }
//! }
//! ```
//!
//! # Key Concepts
//!
//! - **Job**: One manifest submission containing N resources; identified by UUIDv7 `job_id`
//! - **Resource**: Individual download unit within a job; becomes a worker task
//! - **Manifest**: Client-provided JSON describing storage paths, metadata, and resources
//! - **Storage Config**: Client controls full S3 key structure via `manifest_file` and `resource_key_prefix`
//!
//! See [`specs/task_01_api_contract.md`](../../specs/task_01_api_contract.md) for complete API specification.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

pub type HeadersMap = BTreeMap<String, String>;

#[derive(Debug, Deserialize, Clone)]
pub struct Manifest {
    pub manifest_version: String,
    pub storage: StorageConfig,
    pub metadata: Value,
    pub resources: Vec<Resource>,
    #[serde(default)]
    pub attributes: Option<Value>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct StorageConfig {
    pub manifest_file: String,
    pub resource_key_prefix: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Resource {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub headers: HeadersMap,
    #[serde(default)]
    pub tags: HeadersMap,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JobAcceptedResponse {
    pub job_id: String,
    pub manifest_key: String,
    pub resource_count: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JobSnapshot {
    pub job_id: String,
    pub status: JobStatus,
    #[serde(with = "chrono::serde::ts_seconds")]
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(with = "chrono::serde::ts_seconds")]
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub resource_total: usize,
    pub resource_completed: usize,
    pub resource_failed: usize,
    pub manifest_key: String,
    pub errors: Vec<JobError>,
    pub tenant: String,
}

#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    InProgress,
    Completed,
    Failed,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JobError {
    pub resource_name: String,
    pub code: String,
    pub message: String,
    #[serde(with = "chrono::serde::ts_seconds")]
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub components: HashMap<String, String>,
    pub version: String,
}
