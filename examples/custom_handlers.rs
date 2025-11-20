//! Demonstrates registering a bespoke handler that inspects manifest metadata,
//! injects proxy/storage hints, and emits per-resource download attributes.
//!
//! Run with `cargo run --example custom_handlers`.
//! The main FetchBox runtime currently wires in `HandlerRegistry::with_defaults()`
//! inside `api::server`, so integrating this handler into the live API would
//! still require editing that bootstrap code. This standalone walkthrough lets
//! us validate the trait surface and confirm registry customization is viable
//! with today's configuration.

use async_trait::async_trait;
use chrono::Utc;
use fetchbox::api::models::{Manifest, Resource, StorageConfig as ManifestStorage};
use fetchbox::handlers::{
    DownloadTask, HandlerConfig, HandlerError, HandlerRegistry, HeadersMap,
    JobHandler, ManifestEnvelope, PreparedManifest, ProxyConfig,
    StorageConfig as HandlerStorageConfig,
};
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc};
use uuid::Uuid;

const JOB_TYPE: &str = "lookbook.custom";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "
Custom handler registry demo
============================
This example registers `{JOB_TYPE}` with a handler that derives album metadata,
adds default headers/proxy hints, and builds DownloadTask values without
touching the running FetchBox API.
"
    );

    let manifest = sample_manifest();
    let total_resources = manifest.resources.len();
    let registry = bootstrap_registry();

    println!("Registry booted with defaults + `{JOB_TYPE}` override.");
    let handler = registry
        .get(JOB_TYPE)
        .expect("custom handler must be registered");

    let job_id = Uuid::now_v7().to_string();
    let envelope = ManifestEnvelope {
        job_id: job_id.clone(),
        job_type: JOB_TYPE.to_string(),
        tenant: "gallery-team".to_string(),
        manifest,
        manifest_location: Some(format!(
            "s3://fetchbox-gallery/manifests/{job_id}.json"
        )),
        received_at: Utc::now(),
    };

    println!("Preparing manifest for tenant `{}`...", envelope.tenant);
    let prepared = handler.prepare_manifest(envelope).await?;
    let tasks = handler.build_tasks(prepared).await?;

    println!(
        "Generated {} tasks (manifest declared {total_resources}).\n",
        tasks.len()
    );

    for (idx, task) in tasks.iter().enumerate() {
        println!("Task #{idx}: {} -> {}", task.resource_name, task.url);

        if let Some(storage) = &task.storage_hint {
            println!(
                "  storage_hint: s3://{}/{}",
                storage.bucket, storage.key_prefix
            );
        }

        if let Some(proxy_hint) = &task.proxy_hint {
            println!(
                "  proxy pools: {} (fallbacks = {:?})",
                proxy_hint.primary_pool, proxy_hint.fallback_pools
            );
        }

        println!("  tags: {:?}", task.tags);
        if let Some(attributes) = &task.attributes {
            println!("  attributes: {}", attributes);
        }
        println!();
    }

    println!(
        "Note: The API still calls `HandlerRegistry::with_defaults()`. To serve job type \
`{JOB_TYPE}` in the live server you would inject this registry inside \
`api::server::run_with_config_inner`."
    );

    Ok(())
}

#[derive(Clone)]
struct LookbookHandler {
    config: HandlerConfig,
}

impl LookbookHandler {
    fn new(config: HandlerConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl JobHandler for LookbookHandler {
    async fn prepare_manifest(
        &self,
        envelope: ManifestEnvelope,
    ) -> Result<PreparedManifest, HandlerError> {
        if envelope.manifest.manifest_version != "v1" {
            return Err(HandlerError::InvalidManifest(
                "only manifest_version=v1 is supported".into(),
            ));
        }

        if envelope.manifest.resources.is_empty() {
            return Err(HandlerError::InvalidManifest(
                "manifest must contain at least one resource".into(),
            ));
        }

        let album = envelope
            .manifest
            .metadata
            .get("album")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                HandlerError::InvalidManifest(
                    "metadata.album is required".to_string(),
                )
            })?;

        let variants: Vec<String> = envelope
            .manifest
            .metadata
            .get("variants")
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();

        let preferred_format = self
            .config
            .options
            .get("preferred_format")
            .and_then(|value| value.as_str())
            .unwrap_or("webp");

        let handler_context = json!({
            "album": album,
            "album_slug": slugify(album),
            "preferred_format": preferred_format,
            "variants": variants,
            "tenant": envelope.tenant,
        });

        Ok(PreparedManifest {
            envelope,
            handler_context,
        })
    }

    async fn build_tasks(
        &self,
        prepared: PreparedManifest,
    ) -> Result<Vec<DownloadTask>, HandlerError> {
        let handler_context = prepared.handler_context;
        let album_slug = handler_context
            .get("album_slug")
            .and_then(|value| value.as_str())
            .unwrap_or("album")
            .to_string();
        let preferred_format = handler_context
            .get("preferred_format")
            .and_then(|value| value.as_str())
            .unwrap_or("webp")
            .to_string();

        let envelope = prepared.envelope;
        let proxy_hint = self.config.proxy.as_ref().map(|proxy| proxy.to_hint());
        let handler_options = self.config.options.clone();
        let tenant = envelope.tenant.clone();

        let tasks = envelope
            .manifest
            .resources
            .into_iter()
            .enumerate()
            .map(|(idx, resource)| {
                let tenant = tenant.clone();
                let handler_options = handler_options.clone();
                let preferred_format = preferred_format.clone();
                let storage_hint = self.config.storage.as_ref().map(|storage| {
                    storage.to_hint(&envelope.job_id, &resource.name)
                });

                let mut task = DownloadTask::from_resource(
                    &resource,
                    &self.config.default_headers,
                    storage_hint,
                    proxy_hint.clone(),
                );

                task.tags.insert("album".to_string(), album_slug.clone());
                task.tags.insert(
                    "variant".to_string(),
                    resource
                        .tags
                        .get("variant")
                        .cloned()
                        .unwrap_or_else(|| format!("asset-{idx}")),
                );

                task.attributes = Some(json!({
                    "preferred_format": preferred_format,
                    "handler_options": handler_options,
                    "tenant": tenant,
                }));

                task
            })
            .collect();

        Ok(tasks)
    }
}

fn bootstrap_registry() -> HandlerRegistry {
    let mut registry = HandlerRegistry::with_defaults();

    let mut default_headers: HeadersMap = HeadersMap::new();
    default_headers
        .insert("User-Agent".into(), "FetchBox-CustomHandler/0.1".into());
    default_headers.insert("Accept".into(), "image/*".into());

    let handler_config = HandlerConfig {
        handler: "examples::custom_handlers::LookbookHandler".to_string(),
        default_headers,
        proxy: Some(ProxyConfig {
            primary: "residential-us".to_string(),
            fallbacks: vec!["residential-eu".to_string()],
        }),
        storage: Some(HandlerStorageConfig {
            bucket: "fetchbox-gallery".to_string(),
            key_prefix: "custom-handlers/".to_string(),
        }),
        options: json!({
            "preferred_format": "webp",
            "optimize_images": true,
        }),
    };

    let handler = Arc::new(LookbookHandler::new(handler_config.clone()));
    registry.register(JOB_TYPE, handler, handler_config);
    registry
}

fn sample_manifest() -> Manifest {
    Manifest {
        manifest_version: "v1".to_string(),
        storage: ManifestStorage {
            manifest_file: "lookbook.json".to_string(),
            resource_key_prefix: "lookbooks/raw/2025/".to_string(),
        },
        metadata: json!({
            "album": "Winter Lookbook 2025",
            "variants": ["hero", "detail", "lifestyle"],
        }),
        resources: vec![
            Resource {
                name: "hero.jpg".to_string(),
                url: "https://httpbin.org/image/jpeg".to_string(),
                headers: BTreeMap::from([(
                    "Accept".to_string(),
                    "image/jpeg".to_string(),
                )]),
                tags: BTreeMap::from([("variant".to_string(), "hero".to_string())]),
            },
            Resource {
                name: "detail.jpg".to_string(),
                url: "https://httpbin.org/image/png".to_string(),
                headers: BTreeMap::from([
                    ("Accept".to_string(), "image/png".to_string()),
                    ("X-Image-Variant".to_string(), "detail".to_string()),
                ]),
                tags: BTreeMap::from([(
                    "variant".to_string(),
                    "detail".to_string(),
                )]),
            },
            Resource {
                name: "lifestyle.jpg".to_string(),
                url: "https://httpbin.org/image/webp".to_string(),
                headers: BTreeMap::new(),
                tags: BTreeMap::from([(
                    "variant".to_string(),
                    "lifestyle".to_string(),
                )]),
            },
        ],
        attributes: Some(json!({
            "requested_by": "studio-team",
            "campaign": "winter-2025",
        })),
    }
}

fn slugify(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else if ch.is_whitespace() || ch == '-' {
                '-'
            } else {
                '_'
            }
        })
        .collect()
}
