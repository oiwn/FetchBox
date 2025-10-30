use axum::{Router, http::StatusCode, response::IntoResponse, routing::get};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tracing::info;

type AnyError = Box<dyn std::error::Error + Send + Sync + 'static>;

pub async fn run(address: SocketAddr) -> Result<(), AnyError> {
    let app = Router::new().route("/health", get(health));

    let listener = TcpListener::bind(address).await?;
    info!(%address, "FetchBox server listening");

    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

async fn shutdown_signal() {
    // Wait for Ctrl+C
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
