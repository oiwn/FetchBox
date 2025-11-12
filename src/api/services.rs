use axum::{Json, extract::State, http::HeaderMap, response::IntoResponse};
use http_body_util::BodyExt;
use uuid::Uuid;

use super::{
    models::{JobSnapshot, Manifest},
    state::AppState,
    validation::ManifestValidationError,
};
use crate::api::error::ApiError;

// TODO: Move to config::Settings - this should be configurable per deployment
const MAX_PAYLOAD_SIZE: usize = 5 * 1024 * 1024; // 5MB

/// Primary job ingestion endpoint (POST /api/jobs)
///
/// This is the main entry point for submitting work to FetchBox. It handles:
/// - Content-Type and encoding validation (gzip support)
/// - Job type resolution via handler registry
/// - Idempotency via X-Fetchbox-Idempotency-Key header
/// - Manifest validation and storage (S3)
/// - Job state initialization in ledger
/// - Task generation via handler.build_tasks()
/// - Publishing tasks to the jobs.tasks stream for workers
///
/// ## Flow:
/// 1. Validate headers (Content-Type, Job-Type, optional Tenant/Idempotency-Key)
/// 2. Check idempotency - return existing job if key matches
/// 3. Read and decompress body (supports gzip), enforce size limits
/// 4. Deserialize and validate manifest against schema
/// 5. Generate UUIDv7 job_id, upload manifest to S3
/// 6. Create JobSnapshot (Queued status) and persist to ledger
/// 7. Invoke handler to generate tasks from manifest
/// 8. Publish all tasks to jobs.tasks stream for worker consumption
/// 9. Return 202 Accepted with job_id and resource count
///
/// ## Idempotency:
/// If X-Fetchbox-Idempotency-Key is provided and matches an existing job,
/// returns the existing job without reprocessing. This prevents duplicate
/// work when clients retry requests.
pub async fn ingest_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<impl IntoResponse, ApiError> {
    // Validate Content-Type header (spec §1.2)
    // Must be application/json (optionally with charset parameter)
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            ApiError::InvalidPayload("missing Content-Type header".into())
        })?;

    // Parse and validate media type (rejects application/jsonp, etc.)
    super::utils::parse_content_type(content_type)?;

    // Extract and validate job type (required)
    // The job type determines which handler processes this manifest
    // Use default job type (simplified - only one job type supported)
    let job_type = "default";

    // Verify handler exists for this job type before proceeding
    state
        .registry
        .get(job_type)
        .map_err(|_| ApiError::UnsupportedJobType(job_type.to_string()))?;

    // Extract required tenant identifier
    let tenant = headers
        .get("X-Fetchbox-Tenant")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::InvalidPayload("X-Fetchbox-Tenant header is required".to_string()))?;

    // Extract optional idempotency key (for safe retries)
    let idempotency_key = headers
        .get("X-Fetchbox-Idempotency-Key")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .filter(|value| !value.is_empty());

    // Idempotency check: if we've seen this key before, return the existing job
    // This allows clients to safely retry POST requests without creating duplicates
    if let Some(ref key) = idempotency_key {
        if let Ok(Some(existing_job_id)) = state.store.get_idempotent(key) {
            if let Ok(Some(existing_snapshot)) = state.store.get(&existing_job_id) {
                let response = super::models::JobAcceptedResponse {
                    job_id: existing_snapshot.job_id,
                    manifest_key: existing_snapshot.manifest_key,
                    resource_count: existing_snapshot.resource_total,
                };

                return Ok((axum::http::StatusCode::ACCEPTED, Json(response)));
            }
        }
    }

    // Read request body (decompression already handled by RequestDecompressionLayer middleware)
    let body_bytes = read_body(body).await?;

    // Parse manifest JSON and validate against schema
    let manifest: Manifest = serde_json::from_slice(&body_bytes)?;
    super::validation::validate_manifest(&manifest).map_err(map_manifest_error)?;

    // Generate time-sortable UUIDv7 for this job
    let job_id = Uuid::now_v7().to_string();

    // Upload manifest to S3 for persistence and worker access
    // Path uses client-provided storage configuration (spec §1.3.3)
    // Full path: {resource_key_prefix}{manifest_file}
    let storage_key = format!(
        "{}{}",
        manifest.storage.resource_key_prefix,
        manifest.storage.manifest_file
    );
    let upload_result = state
        .storage
        .upload(&storage_key, body_bytes.to_vec())
        .await
        .map_err(|e| ApiError::Internal(format!("Storage upload failed: {}", e)))?;

    let manifest_key =
        format!("s3://{}/{}", state.storage.bucket, upload_result.key);

    // Generate timestamp for job creation
    let timestamp = chrono::Utc::now();

    // Create initial job snapshot with Queued status
    // This is the primary state representation stored in the ledger
    let snapshot = JobSnapshot {
        job_id: job_id.clone(),
        status: super::models::JobStatus::Queued,
        created_at: timestamp,
        updated_at: timestamp,
        resource_total: manifest.resources.len(),
        resource_completed: 0,
        resource_failed: 0,
        manifest_key: manifest_key.clone(),
        errors: Vec::new(),
        tenant: tenant.clone(),
    };

    // Persist idempotency mapping if key was provided
    // This must happen before upserting the job to ensure atomicity
    if let Some(ref key) = idempotency_key {
        state
            .store
            .remember_idempotency(key.clone(), job_id.clone())
            .map_err(|e| {
                ApiError::Internal(format!(
                    "Failed to store idempotency key: {}",
                    e
                ))
            })?;
    }

    // Persist job snapshot to ledger (Fjall KV store)
    state
        .store
        .upsert(snapshot)
        .map_err(|e| ApiError::Internal(format!("Failed to store job: {}", e)))?;

    // Invoke handler to generate tasks from the manifest
    // The handler is job-type-specific and knows how to break down work
    let handler = state.registry.get(job_type).unwrap(); // Safe: already validated above

    // Wrap manifest with context for handler
    let prepared = crate::handlers::types::PreparedManifest {
        context: crate::handlers::types::ManifestContext {
            job_id: job_id.clone(),
            job_type: job_type.to_string(),
            manifest: manifest.clone(),
        },
        handler_data: None, // Reserved for handler-specific state
    };

    // Generate tasks - this is where job-specific logic transforms
    // the manifest into executable work units
    let tasks = handler
        .build_tasks(prepared)
        .await
        .map_err(|e| ApiError::Internal(format!("Handler failed: {}", e)))?;

    // Create task context for proto conversion (shared across all tasks in this job)
    let task_context = crate::handlers::TaskContext {
        job_id: job_id.clone(),
        job_type: job_type.to_string(),
        tenant: tenant.clone(),
        manifest_key: manifest_key.clone(),
    };

    // Enqueue all tasks via TaskBroker
    // Broker persists to Fjall and distributes to workers via mpsc channels
    for task in &tasks {
        // Convert handler DownloadTask to protobuf DownloadTask
        let proto_task = task.to_proto(&task_context);

        state
            .broker
            .enqueue(proto_task)
            .await
            .map_err(|e| {
                ApiError::Internal(format!("Failed to enqueue task: {}", e))
            })?;

        state.metrics.task_published();
    }

    state.metrics.job_accepted();

    // Return 202 Accepted - job is queued but not yet complete
    let response = super::models::JobAcceptedResponse {
        job_id,
        manifest_key,
        resource_count: manifest.resources.len(),
    };

    Ok((axum::http::StatusCode::ACCEPTED, Json(response)))
}

/// Maps manifest validation errors to API errors
fn map_manifest_error(err: ManifestValidationError) -> ApiError {
    ApiError::InvalidPayload(err.to_string())
}

/// Reads request body and validates size
///
/// Note: Decompression is handled transparently by RequestDecompressionLayer middleware,
/// so this function receives already-decompressed data.
async fn read_body(body: axum::body::Body) -> Result<Vec<u8>, ApiError> {
    let data = body
        .collect()
        .await
        .map_err(|err| ApiError::Internal(err.to_string()))?
        .to_bytes()
        .to_vec();

    // Enforce size limit on body data
    super::utils::validate_body_size(&data, MAX_PAYLOAD_SIZE)?;

    Ok(data)
}

/// Job status endpoint (GET /api/jobs/:job_id)
///
/// Returns the current JobSnapshot for a given job_id.
/// Includes status, progress, timestamps, and error information.
pub async fn get_job(
    State(state): State<AppState>,
    axum::extract::Path(job_id): axum::extract::Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let snapshot = state
        .store
        .get(&job_id)
        .map_err(|e| ApiError::Internal(format!("Failed to get job: {}", e)))?
        .ok_or_else(|| ApiError::NotFound(format!("job {job_id}")))?;

    Ok((axum::http::StatusCode::OK, Json(snapshot)))
}

/// Health check endpoint (GET /health)
///
/// Returns health status of all FetchBox components:
/// - api: Axum HTTP server
/// - fjall: Ledger (Fjall KV store)
/// - task_broker: Task queue broker
/// - storage: S3-compatible storage
///
/// Returns 503 Service Unavailable if any component is unhealthy.
/// Returns 200 OK otherwise.
pub async fn health(State(state): State<AppState>) -> impl IntoResponse {
    use std::collections::HashMap;

    let mut components = HashMap::new();

    // Check each component - in v0 we assume healthy if running
    components.insert("api".to_string(), "healthy".to_string());
    components.insert("fjall".to_string(), "healthy".to_string());
    components.insert("task_broker".to_string(), "healthy".to_string());
    components.insert("storage".to_string(), "healthy".to_string());

    // TODO: Add actual health checks for each component
    // For now, if we can respond, we're healthy

    let all_healthy = components.values().all(|status| status == "healthy");
    let overall_status = if all_healthy {
        "healthy"
    } else {
        "unhealthy"
    };

    let status_code = if all_healthy {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    };

    let response = super::models::HealthResponse {
        status: overall_status.to_string(),
        components,
        version: env!("CARGO_PKG_VERSION").to_string(),
    };

    (status_code, Json(response))
}
