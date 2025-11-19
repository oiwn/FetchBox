use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use axum::{Router, routing::get, routing::post};
use tokio::net::TcpListener;
use tower_http::decompression::RequestDecompressionLayer;
use tracing::{error, info};

use super::{
    services::{get_job, health, ingest_job},
    state::AppState,
};
use crate::config::Config;
use crate::handlers::HandlerRegistry;
use crate::ledger::LedgerStorage;
use crate::queue::{DlqStorage, TaskBroker, TasksStorage};
use crate::storage::StorageClient;
use crate::worker::{DownloadWorker, WorkerConfig, WorkerContext};
use tokio::sync::{Mutex, RwLock, watch};

type AnyError = Box<dyn std::error::Error + Send + Sync + 'static>;
type ShutdownFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

pub async fn run(
    address: SocketAddr,
    _ledger_path: String,
) -> Result<(), AnyError> {
    info!("Loading configuration from default sources");
    let config =
        Config::load().map_err(|e| format!("Failed to load config: {}", e))?;
    run_with_config_inner(address, config, None).await
}

pub async fn run_until<F>(
    address: SocketAddr,
    _ledger_path: String,
    shutdown: F,
) -> Result<(), AnyError>
where
    F: Future<Output = ()> + Send + 'static,
{
    info!("Loading configuration from default sources");
    let config =
        Config::load().map_err(|e| format!("Failed to load config: {}", e))?;
    run_with_config_inner(address, config, Some(Box::pin(shutdown))).await
}

pub async fn run_with_config(
    address: SocketAddr,
    config: Config,
) -> Result<(), AnyError> {
    run_with_config_inner(address, config, None).await
}

pub async fn run_with_config_until<F>(
    address: SocketAddr,
    config: Config,
    shutdown: F,
) -> Result<(), AnyError>
where
    F: Future<Output = ()> + Send + 'static,
{
    run_with_config_inner(address, config, Some(Box::pin(shutdown))).await
}

async fn run_with_config_inner(
    address: SocketAddr,
    config: Config,
    shutdown: Option<ShutdownFuture>,
) -> Result<(), AnyError> {
    info!("Starting FetchBox runtime");

    // Open Fjall store
    info!(path = %config.server.fjall_path.display(), "Opening Fjall store");
    let store = LedgerStorage::open(config.server.fjall_path.to_str().unwrap())
        .map_err(|e| format!("Failed to open Fjall store: {}", e))?;
    let worker_ledger = Arc::new(store.clone());

    // Initialize storage backend based on configuration
    let storage = StorageClient::from_config(&config.storage)
        .map_err(|e| format!("Failed to initialize storage: {}", e))?;
    let worker_storage = storage.clone();

    // Initialize queue + DLQ storages
    let tasks_path = &config.queue.path;
    info!(path = ?tasks_path, "Opening TasksStorage");
    let tasks = Arc::new(RwLock::new(
        TasksStorage::open(tasks_path)
            .map_err(|e| format!("Failed to open queue: {}", e))?,
    ));

    let dlq_path = tasks_path.with_file_name("dlq");
    info!(path = ?dlq_path, "Opening DLQ storage");
    let dlq = Arc::new(
        DlqStorage::open(&dlq_path)
            .map_err(|e| format!("Failed to open DLQ storage: {}", e))?,
    );

    // Initialize TaskBroker with worker channels
    let (broker, worker_receivers) = TaskBroker::new(
        tasks.clone(),
        dlq.clone(),
        config.queue.workers,
        config.queue.channel_size,
    );
    let broker = Arc::new(broker);

    let worker_config = WorkerConfig::from(&config.queue.worker);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let ledger_lock = Arc::new(Mutex::new(()));
    let mut worker_handles = Vec::new();

    for (idx, receiver) in worker_receivers.into_iter().enumerate() {
        let worker_id = format!("worker-{}", idx);
        let ledger = worker_ledger.clone();
        let dlq = dlq.clone();
        let shutdown = shutdown_rx.clone();
        let lock = ledger_lock.clone();
        let storage_client = worker_storage.clone();
        let config = worker_config.clone();

        let context = WorkerContext {
            ledger,
            ledger_lock: lock,
            dlq,
            shutdown_rx: shutdown,
        };

        let worker = DownloadWorker::new(
            worker_id.clone(),
            receiver,
            storage_client,
            context,
            config,
        )
        .map_err(|e| format!("Failed to create worker: {}", e))?;

        worker_handles.push(tokio::spawn(async move {
            if let Err(err) = worker.run().await {
                error!(worker_id = %worker_id, error = %err, "Worker exited with error");
            }
        }));
    }

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

    let worker_shutdown = shutdown_tx.clone();
    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(async move {
            match shutdown {
                Some(mut external) => {
                    tokio::select! {
                        _ = shutdown_signal() => {},
                        _ = external.as_mut() => {},
                    }
                }
                None => {
                    shutdown_signal().await;
                }
            }
            let _ = worker_shutdown.send(true);
        })
        .await?;

    let _ = shutdown_tx.send(true);
    for handle in worker_handles {
        let _ = handle.await;
    }

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
