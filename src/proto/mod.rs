//! Generated protobuf types for FetchBox messaging (spec task_03.md)
//!
//! This module contains message schemas for Iggy streams:
//! - `DownloadTask` - Per-resource download task envelope
//! - `JobStatus` - Job-level status updates
//! - `JobLog` - Structured log entries
//! - `DeadLetterTask` - Permanently failed tasks
//!
//! ## Usage
//!
//! ```rust,ignore
//! use fetchbox::proto::{DownloadTask, JobState, JobStatus};
//!
//! let task = DownloadTask {
//!     job_id: "job-123".to_string(),
//!     resource_id: "resource-1".to_string(),
//!     url: "https://example.com/file.jpg".to_string(),
//!     ..Default::default()
//! };
//!
//! // Serialize for Iggy
//! let bytes = prost::Message::encode_to_vec(&task);
//!
//! // Deserialize from Iggy
//! let decoded = DownloadTask::decode(&bytes[..])?;
//! ```

// Include generated protobuf code
#[allow(clippy::all)]
#[allow(warnings)]
mod fetchbox_jobs {
    include!("fetchbox.jobs.rs");
}

// Re-export all types for convenience
pub use fetchbox_jobs::*;
