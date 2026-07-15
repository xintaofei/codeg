use std::sync::Arc;

use anyhow::Context;
use codeg_relay::{app, AppState, Config};
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "codeg_relay=info".into()),
        )
        .init();

    let config = Config::from_env()?;
    let bind = config.bind;
    let state = Arc::new(AppState::new(config).await?);
    let listener = TcpListener::bind(bind)
        .await
        .with_context(|| format!("failed to bind Codeg Relay on {bind}"))?;
    info!(%bind, "relay listening");

    axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("relay server failed")
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
