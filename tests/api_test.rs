use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt; // for `oneshot`

use fetchbox::api::models::{JobAcceptedResponse, JobSnapshot};
use fetchbox::api::state::AppState;
use fetchbox::config::Config;
use fetchbox::handlers::HandlerRegistry;
use fetchbox::ledger::FjallStore;
use fetchbox::queue::{FjallQueue, TaskBroker};
use fetchbox::storage::StorageClient;
use tokio::sync::RwLock;

/// Creates a minimal config for testing
/// We use the default config which should have sensible defaults
fn create_test_config() -> Config {
    // Create a minimal Config with defaults
    // The actual config loading would normally happen via Config::load()
    // but for tests we bypass that and create it directly
    let config_toml = r#"
[server]
host = "127.0.0.1"
port = 8080
fjall_path = "/tmp/test.fjall"

[storage]
provider = "s3"
bucket = "test-bucket"
region = "us-east-1"

[handlers.default]
handler = "default"
    "#;

    toml::from_str(config_toml).expect("Failed to parse test config")
}

/// Builds a test app with isolated dependencies
async fn build_test_app() -> (Router, TempDir) {
    // Create temporary directory for Fjall stores
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let store_path = temp_dir.path().join("test.fjall");
    let queue_path = temp_dir.path().join("queue.fjall");

    // Open Fjall store in temp location
    let store = FjallStore::open(store_path.to_str().unwrap())
        .expect("Failed to open test Fjall store");

    // Use in-memory storage (no real S3)
    let storage = StorageClient::in_memory();

    // Initialize queue and broker (new architecture)
    let queue = Arc::new(RwLock::new(
        FjallQueue::open(&queue_path).expect("Failed to open test queue"),
    ));

    let (broker, _worker_receivers) = TaskBroker::new(queue, 4, 100);
    let broker = Arc::new(broker);

    // Create minimal test config (bypassing file-based config)
    let config = create_test_config();

    // Initialize handler registry
    let registry = HandlerRegistry::with_defaults();

    // Create app state
    let state = AppState::new(config, registry, store, storage, broker);

    // Build router with all routes and middleware
    let app = Router::new()
        .route(
            "/jobs",
            axum::routing::post(fetchbox::api::services::ingest_job),
        )
        .route(
            "/operators/jobs/{job_id}",
            axum::routing::get(fetchbox::api::services::get_job),
        )
        .route(
            "/operators/health",
            axum::routing::get(fetchbox::api::services::health),
        )
        .route(
            "/health",
            axum::routing::get(fetchbox::api::services::health),
        )
        .with_state(state)
        .layer(tower_http::decompression::RequestDecompressionLayer::new());

    (app, temp_dir)
}

/// Creates a valid test manifest
fn valid_manifest() -> serde_json::Value {
    json!({
        "manifest_version": "v1",
        "storage": {
            "manifest_file": "metadata.json",
            "resource_key_prefix": "resources/test/"
        },
        "metadata": {},
        "resources": [
            {
                "name": "resource1.txt",
                "url": "https://example.com/file1.txt"
            },
            {
                "name": "resource2.txt",
                "url": "https://example.com/file2.txt"
            }
        ]
    })
}

/// Creates an invalid manifest (wrong version)
fn invalid_manifest() -> serde_json::Value {
    json!({
        "manifest_version": "v2",
        "metadata": {},
        "resources": [
            {
                "name": "resource1.txt",
                "url": "https://example.com/file1.txt"
            }
        ]
    })
}

/// Helper to build a POST /jobs request
fn post_job_request(manifest: serde_json::Value) -> Request<Body> {
    Request::builder()
        .uri("/jobs")
        .method("POST")
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-Fetchbox-Tenant", "test-tenant")
        .body(Body::from(serde_json::to_string(&manifest).unwrap()))
        .unwrap()
}

#[tokio::test]
async fn test_ingest_job_success() {
    let (app, _temp_dir) = build_test_app().await;

    let request = post_job_request(valid_manifest());

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::ACCEPTED);

    // Parse response body
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let job_response: JobAcceptedResponse = serde_json::from_slice(&body).unwrap();

    // Verify response contains job_id and resource_count
    assert!(!job_response.job_id.is_empty());
    assert_eq!(job_response.resource_count, 2);
    assert!(job_response.manifest_key.starts_with("s3://"));
}

#[tokio::test]
async fn test_ingest_job_idempotency() {
    let (app, _temp_dir) = build_test_app().await;

    let manifest = valid_manifest();

    // First request with idempotency key
    let request1 = Request::builder()
        .uri("/jobs")
        .method("POST")
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-Fetchbox-Tenant", "test-tenant")
        .header("X-Fetchbox-Idempotency-Key", "test-key-123")
        .body(Body::from(serde_json::to_string(&manifest).unwrap()))
        .unwrap();

    let response1 = ServiceExt::<Request<Body>>::oneshot(app.clone(), request1).await.unwrap();
    assert_eq!(response1.status(), StatusCode::ACCEPTED);

    let body1 = axum::body::to_bytes(response1.into_body(), usize::MAX)
        .await
        .unwrap();
    let job1: JobAcceptedResponse = serde_json::from_slice(&body1).unwrap();

    // Second request with same idempotency key
    let request2 = Request::builder()
        .uri("/jobs")
        .method("POST")
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-Fetchbox-Tenant", "test-tenant")
        .header("X-Fetchbox-Idempotency-Key", "test-key-123")
        .body(Body::from(serde_json::to_string(&manifest).unwrap()))
        .unwrap();

    let response2 = ServiceExt::<Request<Body>>::oneshot(app, request2).await.unwrap();
    assert_eq!(response2.status(), StatusCode::ACCEPTED);

    let body2 = axum::body::to_bytes(response2.into_body(), usize::MAX)
        .await
        .unwrap();
    let job2: JobAcceptedResponse = serde_json::from_slice(&body2).unwrap();

    // Both requests should return the same job_id
    assert_eq!(job1.job_id, job2.job_id);
}

#[tokio::test]
async fn test_ingest_job_invalid_content_type() {
    let (app, _temp_dir) = build_test_app().await;

    let request = Request::builder()
        .uri("/jobs")
        .method("POST")
        .header(header::CONTENT_TYPE, "text/plain")
        .header("X-Fetchbox-Tenant", "test-tenant")
        .body(Body::from(
            serde_json::to_string(&valid_manifest()).unwrap(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_ingest_job_missing_content_type() {
    let (app, _temp_dir) = build_test_app().await;

    let request = Request::builder()
        .uri("/jobs")
        .method("POST")
        .header("X-Fetchbox-Tenant", "test-tenant")
        .body(Body::from(
            serde_json::to_string(&valid_manifest()).unwrap(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_ingest_job_missing_tenant() {
    let (app, _temp_dir) = build_test_app().await;

    let request = Request::builder()
        .uri("/jobs")
        .method("POST")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_string(&valid_manifest()).unwrap(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_ingest_job_invalid_manifest() {
    let (app, _temp_dir) = build_test_app().await;

    let request = post_job_request(invalid_manifest());

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_get_job_success() {
    let (app, _temp_dir) = build_test_app().await;

    // First, ingest a job
    let ingest_request = post_job_request(valid_manifest());
    let ingest_response = ServiceExt::<Request<Body>>::oneshot(app.clone(), ingest_request).await.unwrap();
    let ingest_body = axum::body::to_bytes(ingest_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let job: JobAcceptedResponse = serde_json::from_slice(&ingest_body).unwrap();

    // Now, get the job
    let get_request = Request::builder()
        .uri(format!("/operators/jobs/{}", job.job_id))
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let get_response = ServiceExt::<Request<Body>>::oneshot(app, get_request).await.unwrap();

    assert_eq!(get_response.status(), StatusCode::OK);

    let get_body = axum::body::to_bytes(get_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let snapshot: JobSnapshot = serde_json::from_slice(&get_body).unwrap();

    assert_eq!(snapshot.job_id, job.job_id);
    assert_eq!(snapshot.resource_total, 2);
    assert_eq!(snapshot.resource_completed, 0);
    assert_eq!(snapshot.resource_failed, 0);
    assert!(snapshot.errors.is_empty());
}

#[tokio::test]
async fn test_get_job_not_found() {
    let (app, _temp_dir) = build_test_app().await;

    let request = Request::builder()
        .uri("/operators/jobs/nonexistent-job-id")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_health_endpoint() {
    let (app, _temp_dir) = build_test_app().await;

    let request = Request::builder()
        .uri("/health")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    // Should be OK (our test setup has all mocks healthy)
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let health: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Verify structure matches spec (status, components HashMap, version)
    assert_eq!(health.get("status").and_then(|v| v.as_str()), Some("healthy"));
    assert!(health.get("components").is_some());
    assert!(health.get("components").unwrap().is_object());
    assert!(health.get("version").is_some());

    // Verify expected components
    let components = health.get("components").unwrap().as_object().unwrap();
    assert!(components.contains_key("api"));
    assert!(components.contains_key("fjall"));
    assert!(components.contains_key("task_broker"));
    assert!(components.contains_key("storage"));
}
