use std::net::SocketAddr;
use std::sync::Arc;

use axum::{Router, routing::get, routing::post};
use tokio::net::TcpListener;
use tower_http::decompression::RequestDecompressionLayer;
use tracing::info;

use super::{
    services::{get_job, health, ingest_job},
    state::AppState,
};
use crate::config::Config;
use crate::handlers::HandlerRegistry;
use crate::ledger::FjallStore;
use crate::queue::{FjallQueue, TaskBroker};
use crate::storage::StorageClient;
use tokio::sync::RwLock;

type AnyError = Box<dyn std::error::Error + Send + Sync + 'static>;

pub async fn run(
    address: SocketAddr,
    _ledger_path: String,
) -> Result<(), AnyError> {
    // Load config
    info!("Loading configuration");
    let config =
        Config::load().map_err(|e| format!("Failed to load config: {}", e))?;

    // Open Fjall store
    info!(path = %config.server.fjall_path.display(), "Opening Fjall store");
    let store = FjallStore::open(config.server.fjall_path.to_str().unwrap())
        .map_err(|e| format!("Failed to open Fjall store: {}", e))?;

    // Initialize storage (in-memory for now)
    let storage = StorageClient::in_memory();

    // Initialize queue
    let queue_path = config.server.fjall_path.parent().unwrap().join("queue");
    info!(path = ?queue_path, "Opening FjallQueue");
    let queue = Arc::new(RwLock::new(
        FjallQueue::open(&queue_path)
            .map_err(|e| format!("Failed to open queue: {}", e))?,
    ));

    // Initialize TaskBroker with worker channels
    // TODO: Make num_workers and channel_size configurable
    let (broker, _worker_receivers) = TaskBroker::new(queue.clone(), 8, 100);
    let broker = Arc::new(broker);

    // TODO: Spawn workers here (will be done in Phase 5)

    // Initialize handler registry
    let registry = HandlerRegistry::with_defaults();

    let state = AppState::new(config, registry, store, storage, broker);

    let app = Router::new()
        .route("/jobs", post(ingest_job))
        .route("/operators/jobs/{job_id}", get(get_job))
        .route("/operators/health", get(health))
        .route("/health", get(health))
        .with_state(state)
        // Automatically decompress gzip/deflate/brotli request bodies
        // Handles Content-Encoding header transparently at the middleware level
        .layer(RequestDecompressionLayer::new());

    let listener = TcpListener::bind(address).await?;
    info!(%address, "FetchBox API listening");

    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = signal(SignalKind::terminate())
            .expect("failed to install signal handler");
        sigterm.recv().await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Shutdown signal received");
}
