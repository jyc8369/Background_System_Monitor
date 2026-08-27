use std::{error::Error, net::SocketAddr, sync::Arc};

use axum::{Router, routing::get};
use background_system_monitor::{SystemMonitor, get_system_snapshot, health};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt::init();

    let monitor = SystemMonitor::start();
    let app = Router::new()
        .route("/api/system", get(get_system_snapshot))
        .route("/health", get(health))
        .with_state(Arc::clone(&monitor));

    let bind_address = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_owned());
    let address: SocketAddr = bind_address.parse()?;
    let listener = TcpListener::bind(address).await?;

    tracing::info!(%address, "system monitor API listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut terminate = signal(SignalKind::terminate()).expect("install SIGTERM handler");

        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .expect("install Ctrl+C handler");
    }

    tracing::info!("shutdown signal received");
}
