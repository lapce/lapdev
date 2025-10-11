use anyhow::{anyhow, Result};
use clap::Parser;
use tracing::info;
use tracing_subscriber;
use uuid::Uuid;

mod server;

use server::DevboxProxyServer;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Lapdev API server URL for WebSocket tunnel
    #[arg(long, env = "LAPDEV_API_URL")]
    api_url: String,

    /// Lapdev environment ID
    #[arg(long, env = "LAPDEV_ENVIRONMENT_ID")]
    environment_id: Option<String>,

    /// Lapdev environment auth token
    #[arg(long, env = "LAPDEV_ENVIRONMENT_AUTH_TOKEN")]
    environment_auth_token: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing subscriber
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_target(false)
        .init();

    let args = Args::parse();

    info!("Starting Lapdev Kube Devbox Proxy");
    info!("API URL: {}", args.api_url);

    // Validate that environment ID and auth token are present
    let environment_id = args.environment_id.ok_or_else(|| {
        anyhow!("LAPDEV_ENVIRONMENT_ID environment variable is required")
    })?;
    let environment_auth_token = args.environment_auth_token.ok_or_else(|| {
        anyhow!("LAPDEV_ENVIRONMENT_AUTH_TOKEN environment variable is required")
    })?;

    info!("Environment ID: {}", environment_id);

    // Parse environment ID as UUID
    let env_id = Uuid::parse_str(&environment_id).map_err(|e| {
        anyhow!("Failed to parse environment_id as UUID: {}", e)
    })?;

    let server = DevboxProxyServer::new(
        args.api_url,
        env_id,
        environment_auth_token,
    )
    .await?;

    info!("Devbox proxy server initialized successfully");

    server.run().await?;

    Ok(())
}
