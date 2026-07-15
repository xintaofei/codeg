use codeg_relay_bridge::{config::default_config_path, Bridge, BridgeConfig};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .compact()
        .init();

    let config_path = default_config_path();
    let config = BridgeConfig::load(&config_path).await?;
    info!(path = %config_path.display(), "Starting Codeg Relay bridge");
    Bridge::new(config)?.run().await
}
