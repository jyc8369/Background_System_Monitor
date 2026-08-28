#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::{error::Error, net::SocketAddr, sync::Arc};

use axum::{
    Router,
    routing::{get, post},
};
use background_system_monitor::{SystemMonitor, control_shutdown, get_system_snapshot, health};
use tokio::net::TcpListener;

const BIND_ADDRESS_ENV: &str = "BACKGROUND_SYSTEM_MONITOR_BIND_ADDR";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt::init();

    let monitor = SystemMonitor::start();
    let app = Router::new()
        .route("/api/system", get(get_system_snapshot))
        .route("/health", get(health))
        .route("/control/shutdown", post(control_shutdown))
        .with_state(Arc::clone(&monitor));

    let bind_address =
        std::env::var(BIND_ADDRESS_ENV).unwrap_or_else(|_| "127.0.0.1:3001".to_owned());
    let address: SocketAddr = bind_address.parse()?;
    let listener = TcpListener::bind(address).await?;

    tracing::info!(%address, "system monitor API listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(monitor))
        .await?;

    Ok(())
}

async fn shutdown_signal(monitor: Arc<SystemMonitor>) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut terminate = signal(SignalKind::terminate()).expect("install SIGTERM handler");

        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = monitor.wait_for_shutdown() => {}
            _ = terminate.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = monitor.wait_for_shutdown() => {}
        }
    }

    tracing::info!("shutdown signal received");
}
