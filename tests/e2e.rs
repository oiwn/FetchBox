//! End-to-end job ingestion test (single-process runtime)
//! Runs the Axum API with in-memory storage, TaskBroker, and mock workers.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    routing::get,
};
use fetchbox::api::{
    models::{JobAcceptedResponse, JobSnapshot},
    services::{get_job, ingest_job},
    state::AppState,
};
use fetchbox::config::Config;
use fetchbox::handlers::HandlerRegistry;
use fetchbox::ledger::LedgerStorage;
use fetchbox::queue::{DlqStorage, TaskBroker, TasksStorage};
use fetchbox::storage::StorageClient;
use fetchbox::worker::{DownloadWorker, WorkerConfig, WorkerContext};
use http_body_util::BodyExt;
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::{
    net::TcpListener,
    sync::{Mutex as TokioMutex, RwLock, watch},
    task::JoinHandle,
    time::{Duration, sleep, timeout},
};
use tower::ServiceExt;
use tower_http::decompression::RequestDecompressionLayer;

#[tokio::test]
async fn job_ingest_dispatches_tasks_end_to_end() {
    let ctx = E2EHarness::new().await;
    let manifest = sample_manifest(&ctx.http_server.base_url);
    let resource_count = manifest["resources"].as_array().unwrap().len();

    let response = ctx.submit_job(manifest).await;
    assert_eq!(response.resource_count, resource_count);

    let snapshot = ctx
        .wait_for_completion(&response.job_id, resource_count)
        .await;
    assert_eq!(snapshot.resource_completed, resource_count);
    assert_eq!(snapshot.resource_total, resource_count);
    assert_eq!(snapshot.tenant, "test-tenant");
}

struct E2EHarness {
    router: Router,
    ledger: Arc<LedgerStorage>,
    dlq: Arc<DlqStorage>,
    http_server: MockHttpServer,
    _temp_dir: TempDir,
    worker_handles: Vec<JoinHandle<()>>,
    shutdown_tx: watch::Sender<bool>,
}

impl E2EHarness {
    async fn new() -> Self {
        let temp_dir = TempDir::new().expect("temp dir");
        let ledger_path = temp_dir.path().join("ledger.fjall");
        let tasks_path = temp_dir.path().join("tasks.fjall");
        let dlq_path = temp_dir.path().join("dlq.fjall");
        let http_server = MockHttpServer::start().await;

        let ledger_storage =
            LedgerStorage::open(&ledger_path).expect("open ledger");
        let ledger = Arc::new(ledger_storage.clone());

        let tasks_storage =
            TasksStorage::open(&tasks_path).expect("open task storage");
        let tasks = Arc::new(RwLock::new(tasks_storage));

        let dlq = Arc::new(DlqStorage::open(&dlq_path).expect("open dlq storage"));

        let config = create_test_config();
        let registry = HandlerRegistry::with_defaults();
        let storage_client = StorageClient::in_memory();
        let worker_storage = storage_client.clone();

        let (broker, receivers) =
            TaskBroker::new(tasks.clone(), dlq.clone(), 2, 16);
        let broker = Arc::new(broker);

        let worker_config = WorkerConfig::default();
        let ledger_lock = Arc::new(TokioMutex::new(()));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let mut worker_handles = Vec::new();
        for (idx, rx) in receivers.into_iter().enumerate() {
            let worker_id = format!("worker-test-{}", idx);
            let ledger_clone = ledger.clone();
            let lock = ledger_lock.clone();
            let dlq_clone = dlq.clone();
            let shutdown = shutdown_rx.clone();
            let storage = worker_storage.clone();
            let config = worker_config.clone();
            let context = WorkerContext {
                ledger: ledger_clone,
                ledger_lock: lock,
                dlq: dlq_clone,
                shutdown_rx: shutdown,
            };
            let worker = DownloadWorker::new(
                worker_id.clone(),
                rx,
                storage,
                context,
                config,
            )
            .expect("create test worker");

            worker_handles.push(tokio::spawn(async move {
                let _ = worker.run().await;
            }));
        }

        let state =
            AppState::new(config, registry, ledger_storage, storage_client, broker);

        let router = Router::new()
            .route("/jobs", axum::routing::post(ingest_job))
            .route("/operators/jobs/{job_id}", get(get_job))
            .with_state(state)
            .layer(RequestDecompressionLayer::new());

        Self {
            router,
            ledger,
            dlq,
            http_server,
            _temp_dir: temp_dir,
            worker_handles,
            shutdown_tx,
        }
    }

    async fn submit_job(&self, manifest: serde_json::Value) -> JobAcceptedResponse {
        let request = Request::builder()
            .method("POST")
            .uri("/jobs")
            .header("content-type", "application/json")
            .header("x-fetchbox-tenant", "test-tenant")
            .body(Body::from(manifest.to_string()))
            .unwrap();

        let response = self.router.clone().oneshot(request).await.expect("request");
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        serde_json::from_slice(&bytes).expect("response json")
    }

    async fn wait_for_completion(&self, job_id: &str, total: usize) -> JobSnapshot {
        timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(Some(snapshot)) = self.ledger.get(job_id)
                    && snapshot.resource_completed >= total
                {
                    break snapshot;
                }
                sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("timed out waiting for completion")
    }

    async fn wait_for_dlq_entries(
        &self,
    ) -> Vec<(uuid::Uuid, fetchbox::proto::DeadLetterTask)> {
        timeout(Duration::from_secs(20), async {
            loop {
                let entries = self.dlq.list(16).expect("list dlq entries");
                if !entries.is_empty() {
                    break entries;
                }
                sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("timed out waiting for dlq entries")
    }
}

impl Drop for E2EHarness {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(true);
        for handle in &self.worker_handles {
            handle.abort();
        }
    }
}

fn sample_manifest(base_url: &str) -> serde_json::Value {
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
                "url": format!("{}/files/resource1.txt", base_url)
            },
            {
                "name": "resource2.txt",
                "url": format!("{}/files/resource2.txt", base_url)
            }
        ]
    })
}

fn failing_manifest(base_url: &str) -> serde_json::Value {
    json!({
        "manifest_version": "v1",
        "storage": {
            "manifest_file": "metadata.json",
            "resource_key_prefix": "resources/test/"
        },
        "metadata": {},
        "resources": [
            {
                "name": "resource_fail.txt",
                "url": format!("{}/files/resource_fail.txt", base_url)
            }
        ]
    })
}

fn create_test_config() -> Config {
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

    toml::from_str(config_toml).expect("config")
}

struct MockHttpServer {
    base_url: String,
    handle: JoinHandle<()>,
}

impl MockHttpServer {
    async fn start() -> Self {
        let router = Router::new().route("/files/{name}", get(mock_file_handler));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, router.into_make_service()).await;
        });

        Self {
            base_url: format!("http://{}", addr),
            handle,
        }
    }
}

impl Drop for MockHttpServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn mock_file_handler(
    axum::extract::Path(name): axum::extract::Path<String>,
) -> impl axum::response::IntoResponse {
    if name.contains("fail") {
        return (StatusCode::INTERNAL_SERVER_ERROR, "boom");
    }

    (StatusCode::OK, "payload")
}

#[tokio::test]
async fn job_failure_populates_dlq() {
    let ctx = E2EHarness::new().await;
    let manifest = failing_manifest(&ctx.http_server.base_url);
    let response = ctx.submit_job(manifest).await;

    let dlq_entries = ctx.wait_for_dlq_entries().await;
    let (_, entry) = &dlq_entries[0];
    assert_eq!(entry.failure_code, "DOWNLOAD_ERROR");
    assert!(entry.attempts >= 1);
    assert_eq!(
        entry.task.as_ref().unwrap().resource_id,
        "resource_fail.txt"
    );

    let snapshot = ctx.ledger.get(&response.job_id).unwrap().unwrap();
    assert!(snapshot.resource_failed >= 1);
    assert!(!snapshot.errors.is_empty());
}
