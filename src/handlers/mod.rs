//! Handler system for FetchBox (spec task_02.md)
//!
//! This module provides the handler trait and types for customizing
//! how manifests are processed into download tasks.
//!
//! ## Key Components
//!
//! - [`JobHandler`] - Main trait for implementing custom handlers
//! - [`DefaultHandler`] - Built-in handler that echoes manifests
//! - [`HandlerRegistry`] - Registry for managing handler instances
//! - [`ManifestContext`] - Context passed to handlers
//! - [`DownloadTask`] - Individual download task emitted by handlers
//!
//! ## Example
//!
//! ```rust,ignore
//! use fetchbox::handlers::{HandlerRegistry, ManifestContext};
//!
//! let registry = HandlerRegistry::with_defaults();
//! let handler = registry.get("default")?;
//!
//! let ctx = ManifestContext { /* ... */ };
//! let prepared = handler.prepare_manifest(ctx).await?;
//! let tasks = handler.build_tasks(prepared).await?;
//! ```

mod default;
mod registry;
mod traits;
pub(crate) mod types;

pub use default::DefaultHandler;
pub use registry::{
    HandlerConfig, HandlerRegistry, ProxyConfig, RegistryError, StorageConfig,
};
pub use traits::{HandlerError, JobHandler};
pub use types::{
    DownloadTask, HeadersMap, JobSummary, ManifestContext, PreparedManifest,
    ProxyHint, StorageHint, TaskContext,
};
