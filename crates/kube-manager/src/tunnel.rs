use std::sync::Arc;

use anyhow::{anyhow, Result};
use lapdev_common::kube::KUBE_CLUSTER_TOKEN_HEADER;
use lapdev_kube_rpc::{TunnelEstablishmentResponse, TunnelStatus};
use lapdev_tunnel::{run_tunnel_server, WebSocketTransport as TunnelWebSocketTransport};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
/// Tunnel client for managing data plane WebSocket connection
#[derive(Debug)]
pub struct TunnelClient {
    pub tunnel_id: String,
    pub websocket_endpoint: String,
    pub connected_at: chrono::DateTime<chrono::Utc>,
    pub is_connected: bool,
    pub active_connections: u32,
    pub total_connections: u64,
    pub bytes_transferred: u64,
    // Data plane WebSocket connection for tunnel messages
    pub data_plane_websocket: Option<
        Arc<
            tokio::sync::Mutex<
                tokio_tungstenite::WebSocketStream<
                    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
                >,
            >,
        >,
    >,
}

/// Tunnel manager for handling WebSocket tunnel connections
#[derive(Clone)]
pub struct TunnelManager {
    tunnel_request: tokio_tungstenite::tungstenite::http::Request<()>,
    tunnel_client: Arc<tokio::sync::RwLock<Option<TunnelClient>>>,
}

impl TunnelManager {
    pub fn new(tunnel_request: tokio_tungstenite::tungstenite::http::Request<()>) -> Self {
        Self {
            tunnel_request,
            tunnel_client: Arc::new(tokio::sync::RwLock::new(None)),
        }
    }

    /// Get current tunnel status
    pub async fn get_tunnel_status(&self) -> Result<TunnelStatus> {
        let client_guard = self.tunnel_client.read().await;

        match client_guard.as_ref() {
            Some(client) => Ok(TunnelStatus {
                tunnel_id: Some(client.tunnel_id.clone()),
                is_connected: client.is_connected,
                connected_at: Some(client.connected_at),
                last_heartbeat: Some(chrono::Utc::now()),
                active_connections: client.active_connections,
                total_connections: client.total_connections,
                bytes_transferred: client.bytes_transferred,
            }),
            None => Ok(TunnelStatus {
                tunnel_id: None,
                is_connected: false,
                connected_at: None,
                last_heartbeat: None,
                active_connections: 0,
                total_connections: 0,
                bytes_transferred: 0,
            }),
        }
    }

    /// Close tunnel connection
    pub async fn close_tunnel_connection(&self, tunnel_id: String) -> Result<()> {
        tracing::info!("Closing tunnel connection: {}", tunnel_id);

        let mut client_guard = self.tunnel_client.write().await;

        match client_guard.as_mut() {
            Some(client) if client.tunnel_id == tunnel_id => {
                client.is_connected = false;
                tracing::info!("Tunnel connection closed: {}", tunnel_id);
                Ok(())
            }
            Some(_) => Err(anyhow!("Tunnel ID mismatch")),
            None => Err(anyhow!("No tunnel connection found")),
        }
    }

    /// Start the tunnel manager connection cycle
    pub async fn start_tunnel_cycle(&self) -> Result<()> {
        loop {
            match self.handle_tunnel_connection_cycle().await {
                Ok(_) => {
                    tracing::warn!("Tunnel connection cycle completed, will retry in 5 seconds...");
                }
                Err(e) => {
                    tracing::warn!(
                        "Tunnel connection cycle failed: {}, retrying in 5 seconds...",
                        e
                    );
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }

    /// Handle a single tunnel connection cycle
    async fn handle_tunnel_connection_cycle(&self) -> Result<()> {
        tracing::info!("Attempting to establish tunnel connection...");

        let (stream, _) = tokio_tungstenite::connect_async(self.tunnel_request.clone()).await?;

        tracing::info!("Tunnel WebSocket connection established");

        let transport = TunnelWebSocketTransport::new(stream);
        run_tunnel_server(transport).await?;

        tracing::info!("Tunnel server session ended");
        Ok(())
    }
}
