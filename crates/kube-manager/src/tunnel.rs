use anyhow::Result;
use lapdev_tunnel::{direct::QuicTransport, run_tunnel_server, RelayEndpoint};

/// Manages the lifecycle of the background tunnel connection used for preview URLs.
#[derive(Clone)]
pub struct TunnelManager {
    tunnel_request: tokio_tungstenite::tungstenite::http::Request<()>,
    auth_token: String,
}

impl TunnelManager {
    pub fn new(
        tunnel_request: tokio_tungstenite::tungstenite::http::Request<()>,
        auth_token: String,
    ) -> Self {
        Self {
            tunnel_request,
            auth_token,
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

        let connection = RelayEndpoint::client_connection(stream).await?;
        run_tunnel_server(connection).await?;

        tracing::info!("Tunnel server session ended");
        Ok(())
    }
}
