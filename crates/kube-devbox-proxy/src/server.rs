use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info};
use uuid::Uuid;

pub struct DevboxProxyServer {
    api_url: String,
    environment_id: Uuid,
    environment_auth_token: String,
}

impl DevboxProxyServer {
    pub async fn new(
        api_url: String,
        environment_id: Uuid,
        environment_auth_token: String,
    ) -> Result<Self> {
        Ok(Self {
            api_url,
            environment_id,
            environment_auth_token,
        })
    }

    pub async fn run(self) -> Result<()> {
        // Maintain WebSocket tunnel connection with auto-reconnect
        loop {
            info!("Connecting to API WebSocket tunnel at {}", self.api_url);
            match Self::connect_tunnel(
                &self.api_url,
                self.environment_id,
                &self.environment_auth_token,
            )
            .await
            {
                Ok(_) => {
                    info!("WebSocket tunnel connection closed, reconnecting...");
                }
                Err(e) => {
                    error!(
                        "Failed to connect WebSocket tunnel: {}, retrying in 5s...",
                        e
                    );
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        }
    }

    async fn connect_tunnel(
        api_url: &str,
        environment_id: Uuid,
        environment_auth_token: &str,
    ) -> Result<()> {
        // Build WebSocket URL: ws://api/kube/devbox-proxy/tunnel/{environment_id}
        let ws_url = format!(
            "{}/kube/devbox-proxy/tunnel/{}",
            api_url
                .replace("http://", "ws://")
                .replace("https://", "wss://"),
            environment_id
        );

        info!("Connecting to WebSocket tunnel: {}", ws_url);

        // Create custom headers with auth token
        let request = http::Request::builder()
            .uri(&ws_url)
            .header("X-Lapdev-Environment-Token", environment_auth_token)
            .body(())
            .map_err(|e| anyhow!("Failed to build request: {}", e))?;

        // Connect to WebSocket
        let (ws_stream, response) = connect_async(request).await?;
        info!(
            "WebSocket tunnel connected, status: {:?}",
            response.status()
        );

        let (_ws_sender, mut ws_receiver) = ws_stream.split();

        info!("WebSocket tunnel established, waiting for messages...");

        // Handle incoming messages from the API
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(Message::Binary(data)) => {
                    debug!("Received {} bytes from WebSocket", data.len());
                    // Data from devbox will be forwarded to in-cluster services
                    // This will be implemented when we add the actual proxying logic
                }
                Ok(Message::Close(_)) => {
                    info!("WebSocket connection closed by server");
                    break;
                }
                Ok(Message::Ping(_)) => {
                    debug!("Received ping");
                }
                Ok(Message::Pong(_)) => {
                    debug!("Received pong");
                }
                Ok(Message::Text(text)) => {
                    debug!("Received text message: {}", text);
                }
                Err(e) => {
                    error!("WebSocket error: {}", e);
                    break;
                }
                _ => {}
            }
        }

        Ok(())
    }
}
