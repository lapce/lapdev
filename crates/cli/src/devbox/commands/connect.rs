use anyhow::{Context, Result};
use colored::Colorize;
use futures::StreamExt;
use lapdev_devbox_rpc::{DevboxClientRpc, DevboxSessionRpcClient, StartInterceptRequest};
use lapdev_rpc::spawn_twoway;
use tarpc::server::{BaseChannel, Channel};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_util::codec::LengthDelimitedCodec;
use uuid::Uuid;

use crate::{auth, devbox::websocket_transport::WebSocketTransport};

/// Execute the devbox connect command
pub async fn execute(api_url: &str) -> Result<()> {
    // Load token from keychain
    let token = auth::get_token(api_url)
        .context("Failed to load authentication token. Please run 'lapdev login' first.")?;

    println!("{}", "🔌 Connecting to Lapdev devbox...".cyan());

    // Construct WebSocket URL
    let ws_url = api_url
        .replace("https://", "wss://")
        .replace("http://", "ws://");
    let ws_url = format!("{}/kube/devbox/rpc", ws_url);

    tracing::info!("Connecting to: {}", ws_url);

    // Create WebSocket request with authentication
    let mut request = ws_url
        .into_client_request()
        .context("Failed to create WebSocket request")?;

    // Add authentication header
    let headers = request.headers_mut();
    headers.insert(
        "Authorization",
        format!("Bearer {}", token)
            .parse()
            .context("Failed to parse authorization header")?,
    );

    // TODO: Add client version header
    // headers.insert("X-Lapdev-Client-Version", env!("CARGO_PKG_VERSION").parse()?);

    // Connect WebSocket
    let (stream, _response) = tokio_tungstenite::connect_async(request)
        .await
        .context("Failed to establish WebSocket connection")?;

    println!("{}", "✓ WebSocket connection established".green());

    // Create transport layer
    let trans = WebSocketTransport::new(stream);
    let io = LengthDelimitedCodec::builder().new_framed(trans);

    // Set up bidirectional RPC
    let transport =
        tarpc::serde_transport::new(io, tarpc::tokio_serde::formats::Bincode::default());
    let (server_chan, client_chan, _abort_handle) = spawn_twoway(transport);

    // Create RPC client (for calling server methods)
    let rpc_client =
        DevboxSessionRpcClient::new(tarpc::client::Config::default(), client_chan).spawn();

    // Create RPC server (for server to call us)
    let client_rpc_server = DevboxClientRpcServer::new();

    // Spawn the RPC server task
    let server_task = tokio::spawn(async move {
        tracing::info!("Starting DevboxClientRpc server...");
        BaseChannel::with_defaults(server_chan)
            .execute(client_rpc_server.serve())
            .for_each(|resp| async move {
                tokio::spawn(resp);
            })
            .await;
        tracing::info!("DevboxClientRpc server stopped");
    });

    // Call whoami to get session info
    match rpc_client
        .whoami(tarpc::context::current())
        .await
        .context("RPC call failed")?
    {
        Ok(session_info) => {
            println!(
                "{} Connected as {} ({})",
                "✓".green(),
                session_info.email.bright_white().bold(),
                session_info.device_name.cyan()
            );
            println!(
                "  Session expires: {}",
                session_info
                    .expires_at
                    .format("%Y-%m-%d %H:%M:%S UTC")
                    .to_string()
                    .dimmed()
            );
        }
        Err(e) => {
            eprintln!("{} Failed to get session info: {}", "✗".red(), e);
            return Err(anyhow::anyhow!("Authentication failed: {}", e));
        }
    }

    // TODO: Get active environment
    // TODO: List current intercepts and replay them
    // TODO: Handle Ctrl+C for graceful shutdown
    // TODO: Add reconnection logic

    println!("{}", "\n👂 Listening for intercept requests...".cyan());
    println!("{}", "Press Ctrl+C to disconnect".dimmed());

    // Wait for server task
    server_task.await.context("Server task failed")?;

    Ok(())
}

/// RPC server implementation for the CLI
/// This handles incoming calls from the server
#[derive(Clone)]
struct DevboxClientRpcServer {
    // TODO: Add intercept manager state
}

impl DevboxClientRpcServer {
    fn new() -> Self {
        Self {}
    }
}

impl DevboxClientRpc for DevboxClientRpcServer {
    async fn start_intercept(
        self,
        _context: tarpc::context::Context,
        intercept: StartInterceptRequest,
    ) -> Result<(), String> {
        println!(
            "{} Starting intercept for workload: {}/{}",
            "→".cyan(),
            intercept.namespace,
            intercept.workload_name
        );

        for mapping in &intercept.port_mappings {
            println!(
                "  {} localhost:{} → {}:{}",
                "↔".dimmed(),
                mapping.local_port,
                intercept.workload_name,
                mapping.workload_port
            );
        }

        // TODO: Implement actual port forwarding
        // For now, just acknowledge receipt
        tracing::info!(
            "Received start_intercept request for {}/{} with {} port mappings",
            intercept.namespace,
            intercept.workload_name,
            intercept.port_mappings.len()
        );

        Ok(())
    }

    async fn stop_intercept(
        self,
        _context: tarpc::context::Context,
        intercept_id: Uuid,
    ) -> Result<(), String> {
        println!("{} Stopping intercept: {}", "✗".yellow(), intercept_id);

        // TODO: Implement actual cleanup
        tracing::info!("Received stop_intercept request for {}", intercept_id);

        Ok(())
    }

    async fn session_displaced(self, _context: tarpc::context::Context, new_device_name: String) {
        println!(
            "\n{} Session displaced by new login from: {}",
            "⚠".yellow(),
            new_device_name.bright_white()
        );
        println!("{}", "Disconnecting...".dimmed());

        // TODO: Gracefully shutdown
        tracing::warn!("Session displaced by: {}", new_device_name);
    }

    async fn ping(self, _context: tarpc::context::Context) -> Result<(), String> {
        tracing::trace!("Received ping");
        Ok(())
    }
}
