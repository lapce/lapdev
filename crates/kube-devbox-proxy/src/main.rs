use anyhow::{anyhow, Result};
use clap::Parser;
use tracing::info;
use tracing_subscriber;
use uuid::Uuid;

mod branch_config;
mod rpc_server;
mod server;

use server::DevboxProxyServer;

pub const DEFAULT_KUBE_MANAGER_RPC_HOST: &str = "lapdev-kube-manager";
pub const DEFAULT_KUBE_MANAGER_RPC_PORT: u16 = 7771;

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

    /// Kube-manager RPC host
    #[arg(long, env = "KUBE_MANAGER_RPC_HOST", default_value = DEFAULT_KUBE_MANAGER_RPC_HOST)]
    kube_manager_host: String,

    /// Kube-manager RPC port
    #[arg(long, env = "KUBE_MANAGER_RPC_PORT", default_value_t = DEFAULT_KUBE_MANAGER_RPC_PORT)]
    kube_manager_port: u16,

    /// Whether this is a shared environment (needs RPC connection)
    #[arg(long, env = "IS_SHARED_ENVIRONMENT", default_value_t = false)]
    is_shared: bool,
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
    let environment_id = args
        .environment_id
        .ok_or_else(|| anyhow!("LAPDEV_ENVIRONMENT_ID environment variable is required"))?;
    let environment_auth_token = args
        .environment_auth_token
        .ok_or_else(|| anyhow!("LAPDEV_ENVIRONMENT_AUTH_TOKEN environment variable is required"))?;

    info!("Environment ID: {}", environment_id);

    // Parse environment ID as UUID
    let env_id = Uuid::parse_str(&environment_id)
        .map_err(|e| anyhow!("Failed to parse environment_id as UUID: {}", e))?;

    let server = DevboxProxyServer::new(args.api_url, env_id, environment_auth_token).await?;

    info!("Devbox proxy server initialized successfully");

    // For shared environments, also connect to kube-manager RPC
    if args.is_shared {
        info!("Shared environment detected, connecting to kube-manager RPC");
        let kube_manager_addr = format!("{}:{}", args.kube_manager_host, args.kube_manager_port);
        info!("Connecting to kube-manager at {}", kube_manager_addr);

        let branch_config = branch_config::BranchConfig::new(args.api_url.clone());
        let rpc_server = rpc_server::DevboxProxyRpcServer::new(branch_config);

        // Spawn RPC connection in background
        let env_id_clone = env_id;
        tokio::spawn(async move {
            loop {
                match connect_to_kube_manager(
                    &kube_manager_addr,
                    env_id_clone,
                    rpc_server.clone(),
                )
                .await
                {
                    Ok(_) => {
                        info!("RPC connection to kube-manager closed, reconnecting in 5s...");
                    }
                    Err(e) => {
                        tracing::error!(
                            "Failed to connect to kube-manager: {}, retrying in 5s...",
                            e
                        );
                    }
                }
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        });
    }

    server.run().await?;

    Ok(())
}

async fn connect_to_kube_manager(
    addr: &str,
    environment_id: Uuid,
    rpc_server: rpc_server::DevboxProxyRpcServer,
) -> Result<()> {
    use futures_util::StreamExt;
    use lapdev_kube_rpc::{DevboxProxyManagerRpc, DevboxProxyManagerRpcClient, DevboxProxyRpc};
    use lapdev_rpc::spawn_twoway;
    use tarpc::server::{BaseChannel, Channel};

    info!("Establishing RPC connection to kube-manager at {}", addr);

    let transport = tarpc::serde_transport::tcp::connect(addr, tarpc::tokio_serde::formats::Bincode::default)
        .await?;

    let (server_chan, client_chan, _) = spawn_twoway(transport);

    // Create RPC client to call kube-manager
    let manager_client = DevboxProxyManagerRpcClient::new(
        tarpc::client::Config::default(),
        client_chan,
    )
    .spawn();

    info!("Registering devbox-proxy with kube-manager for environment {}", environment_id);

    // Register with kube-manager
    manager_client
        .register_devbox_proxy(tarpc::context::current(), environment_id)
        .await
        .map_err(|e| anyhow!("RPC call failed: {}", e))?
        .map_err(|e| anyhow!("Registration failed: {}", e))?;

    info!("Successfully registered with kube-manager");

    // Start serving RPC requests from kube-manager
    BaseChannel::with_defaults(server_chan)
        .execute(rpc_server.serve())
        .for_each(|response| async move {
            tokio::spawn(response);
        })
        .await;

    Ok(())
}
