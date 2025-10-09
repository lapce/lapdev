use std::{collections::HashMap, sync::Arc};

use anyhow::{anyhow, Result};
use futures::{SinkExt, StreamExt};
use lapdev_common::kube::KUBE_CLUSTER_TOKEN_HEADER;
use lapdev_kube_rpc::{
    ClientTunnelFrame, ClientTunnelMessage, ServerTunnelFrame, ServerTunnelMessage,
    TunnelEstablishmentResponse, TunnelStatus,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use uuid::Uuid;

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

    /// Connect to data plane WebSocket endpoint
    async fn connect_data_plane_websocket(
        endpoint: &str,
        auth_token: &str,
    ) -> Result<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    > {
        use tokio_tungstenite::connect_async;

        // Convert HTTP(S) endpoint to WebSocket URL
        let ws_url = if endpoint.starts_with("https://") {
            endpoint.replace("https://", "wss://")
        } else if endpoint.starts_with("http://") {
            endpoint.replace("http://", "ws://")
        } else {
            format!("ws://{}", endpoint)
        };

        tracing::info!("Connecting data plane WebSocket to: {}", ws_url);

        // Create WebSocket request with auth header
        let mut request = ws_url.into_client_request()?;
        request.headers_mut().insert(
            KUBE_CLUSTER_TOKEN_HEADER,
            auth_token
                .parse()
                .map_err(|e| anyhow!("Invalid auth token: {}", e))?,
        );

        // Connect to WebSocket
        let (ws_stream, response) = connect_async(request)
            .await
            .map_err(|e| anyhow!("Failed to connect to data plane WebSocket: {}", e))?;

        tracing::info!(
            "Data plane WebSocket connected, response: {}",
            response.status()
        );

        Ok(ws_stream)
    }

    /// Establish tunnel connection to controller
    pub async fn establish_tunnel(
        &self,
        controller_endpoint: String,
        auth_token: String,
    ) -> Result<TunnelEstablishmentResponse> {
        tracing::info!("Establishing tunnel to controller: {}", controller_endpoint);

        let tunnel_id = format!("tunnel_{}", uuid::Uuid::new_v4());
        let websocket_endpoint = format!("{}/tunnel", controller_endpoint.trim_end_matches('/'));

        // Establish data plane WebSocket connection
        let data_plane_endpoint =
            format!("{}/data-plane", controller_endpoint.trim_end_matches('/'));
        let data_plane_websocket =
            Self::connect_data_plane_websocket(&data_plane_endpoint, &auth_token).await?;

        // Create tunnel client
        let tunnel_client = TunnelClient {
            tunnel_id: tunnel_id.clone(),
            websocket_endpoint: websocket_endpoint.clone(),
            connected_at: chrono::Utc::now(),
            is_connected: true,
            active_connections: 0,
            total_connections: 0,
            bytes_transferred: 0,
            data_plane_websocket: Some(Arc::new(tokio::sync::Mutex::new(data_plane_websocket))),
        };

        // Store the tunnel client
        {
            let mut client_guard = self.tunnel_client.write().await;
            *client_guard = Some(tunnel_client);
        }

        tracing::info!("Tunnel established successfully: {}", tunnel_id);

        Ok(TunnelEstablishmentResponse {
            success: true,
            tunnel_id,
            websocket_endpoint,
            error_message: None,
        })
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

    /// Handle incoming TCP tunnel requests from KubeController
    pub async fn handle_tunnel_message(
        &self,
        message: ServerTunnelMessage,
        active_tcp_connections: &mut HashMap<String, tokio::net::tcp::OwnedWriteHalf>,
    ) -> Result<()> {
        let client_guard = self.tunnel_client.read().await;
        let client = client_guard
            .as_ref()
            .ok_or_else(|| anyhow!("No tunnel client available"))?;

        match message {
            ServerTunnelMessage::OpenConnection {
                tunnel_id,
                target_host,
                target_port,
                protocol_hint: _,
            } => {
                tracing::info!(
                    "Opening TCP connection to {}:{} for tunnel {}",
                    target_host,
                    target_port,
                    tunnel_id
                );

                // Open TCP connection to target service in cluster
                match tokio::net::TcpStream::connect((target_host.as_str(), target_port)).await {
                    Ok(tcp_stream) => {
                        let connection_id = format!("{}:{}", target_host, target_port);
                        let connection_id_clone = connection_id.clone();

                        // Split TCP stream into reader and writer
                        let (tcp_reader, tcp_writer) = tcp_stream.into_split();

                        // Store the TCP writer for sending data to the service
                        active_tcp_connections.insert(connection_id.clone(), tcp_writer);

                        // Spawn task to read from TCP and send back through WebSocket
                        let ws_clone = client.data_plane_websocket.clone();
                        let tunnel_id_clone = tunnel_id.clone();
                        tokio::spawn(async move {
                            Self::handle_tcp_reader(
                                tcp_reader,
                                ws_clone,
                                tunnel_id_clone,
                                connection_id_clone,
                            )
                            .await;
                        });

                        // Send success response through data plane WebSocket
                        if let Some(ref ws) = client.data_plane_websocket {
                            let response_msg = ClientTunnelMessage::ConnectionOpened {
                                tunnel_id: tunnel_id.clone(),
                                local_addr: connection_id,
                            };
                            let frame = ClientTunnelFrame {
                                message: response_msg,
                                timestamp: chrono::Utc::now(),
                                message_id: Uuid::new_v4(),
                            };

                            if let Ok(data) = frame.serialize() {
                                let mut ws_guard = ws.lock().await;
                                let _ = futures::SinkExt::send(
                                    &mut *ws_guard,
                                    tokio_tungstenite::tungstenite::Message::Binary(data.into()),
                                )
                                .await;
                            }
                        }

                        tracing::info!(
                            "TCP connection opened successfully for tunnel {}",
                            tunnel_id
                        );
                    }
                    Err(e) => {
                        tracing::error!("Failed to open TCP connection: {}", e);

                        // Send failure response through data plane WebSocket
                        if let Some(ref ws) = client.data_plane_websocket {
                            let response_msg = ClientTunnelMessage::ConnectionFailed {
                                tunnel_id,
                                error: e.to_string(),
                                error_code: lapdev_kube_rpc::TunnelErrorCode::ConnectionRefused,
                            };
                            let frame = ClientTunnelFrame {
                                message: response_msg,
                                timestamp: chrono::Utc::now(),
                                message_id: Uuid::new_v4(),
                            };

                            if let Ok(data) = frame.serialize() {
                                let mut ws_guard = ws.lock().await;
                                let _ = futures::SinkExt::send(
                                    &mut *ws_guard,
                                    tokio_tungstenite::tungstenite::Message::Binary(data.into()),
                                )
                                .await;
                            }
                        }
                    }
                }
            }
            ServerTunnelMessage::Data {
                tunnel_id,
                payload,
                sequence_num: _,
            } => {
                // Forward data to TCP connection
                tracing::debug!(
                    "Forwarding {} bytes to TCP connection for tunnel {}",
                    payload.len(),
                    tunnel_id
                );

                // Find the appropriate TCP connection and write data
                if let Some((connection_id, tcp_writer)) = active_tcp_connections.iter_mut().next()
                {
                    match tcp_writer.write_all(&payload).await {
                        Ok(_) => {
                            tracing::debug!(
                                "Successfully forwarded {} bytes to TCP connection {}",
                                payload.len(),
                                connection_id
                            );
                        }
                        Err(e) => {
                            tracing::error!(
                                "Failed to write to TCP connection {}: {}",
                                connection_id,
                                e
                            );
                            // Remove the failed connection
                            let connection_id_clone = connection_id.clone();
                            active_tcp_connections.remove(&connection_id_clone);
                        }
                    }
                }
            }
            ServerTunnelMessage::CloseConnection { tunnel_id, reason } => {
                tracing::info!(
                    "Closing TCP connection for tunnel {} (reason: {:?})",
                    tunnel_id,
                    reason
                );

                // Remove and close TCP connection
                active_tcp_connections.clear(); // Simplified - would normally close specific connection

                // Send close confirmation
                if let Some(ref ws) = client.data_plane_websocket {
                    let response_msg = ClientTunnelMessage::ConnectionClosed {
                        tunnel_id,
                        bytes_transferred: 0, // Would track actual bytes in production
                    };
                    let frame = ClientTunnelFrame {
                        message: response_msg,
                        timestamp: chrono::Utc::now(),
                        message_id: Uuid::new_v4(),
                    };

                    if let Ok(data) = frame.serialize() {
                        let mut ws_guard = ws.lock().await;
                        let _ = futures::SinkExt::send(
                            &mut *ws_guard,
                            tokio_tungstenite::tungstenite::Message::Binary(data.into()),
                        )
                        .await;
                    }
                }
            }
            ServerTunnelMessage::Ping { timestamp } => {
                // Respond with pong
                if let Some(ref ws) = client.data_plane_websocket {
                    let response_msg = ClientTunnelMessage::Pong { timestamp };
                    let frame = ClientTunnelFrame {
                        message: response_msg,
                        timestamp: chrono::Utc::now(),
                        message_id: Uuid::new_v4(),
                    };

                    if let Ok(data) = frame.serialize() {
                        let mut ws_guard = ws.lock().await;
                        let _ = futures::SinkExt::send(
                            &mut *ws_guard,
                            tokio_tungstenite::tungstenite::Message::Binary(data.into()),
                        )
                        .await;
                    }
                }
            }
            _ => {
                tracing::warn!("Unhandled tunnel message type: {:?}", message);
            }
        }

        Ok(())
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

    /// Handle TCP reader - read from TCP and send back through WebSocket
    async fn handle_tcp_reader(
        mut tcp_reader: tokio::net::tcp::OwnedReadHalf,
        ws_clone: Option<
            Arc<
                tokio::sync::Mutex<
                    tokio_tungstenite::WebSocketStream<
                        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
                    >,
                >,
            >,
        >,
        tunnel_id: String,
        connection_id: String,
    ) {
        if let Some(ref ws) = ws_clone {
            let mut buffer = vec![0u8; 8192];
            let mut sequence_num = 0u32;

            loop {
                match tcp_reader.read(&mut buffer).await {
                    Ok(0) => {
                        // EOF - TCP connection closed
                        tracing::info!("TCP connection {} closed (EOF)", connection_id);
                        break;
                    }
                    Ok(n) => {
                        // Read n bytes, send them back through WebSocket
                        let data = buffer[..n].to_vec();
                        tracing::debug!("Read {} bytes from TCP connection {}", n, connection_id);

                        let response_msg = ClientTunnelMessage::Data {
                            tunnel_id: tunnel_id.clone(),
                            payload: data,
                            sequence_num: Some(sequence_num),
                        };
                        sequence_num = sequence_num.wrapping_add(1);

                        let frame = ClientTunnelFrame {
                            message: response_msg,
                            timestamp: chrono::Utc::now(),
                            message_id: Uuid::new_v4(),
                        };

                        if let Ok(frame_data) = frame.serialize() {
                            let mut ws_guard = ws.lock().await;
                            if let Err(e) = futures::SinkExt::send(
                                &mut *ws_guard,
                                tokio_tungstenite::tungstenite::Message::Binary(frame_data.into()),
                            )
                            .await
                            {
                                tracing::error!("Failed to send data through WebSocket: {}", e);
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            "Failed to read from TCP connection {}: {}",
                            connection_id,
                            e
                        );
                        break;
                    }
                }
            }

            // Send connection closed message
            let close_msg = ClientTunnelMessage::CloseConnection {
                tunnel_id: tunnel_id.clone(),
                reason: lapdev_kube_rpc::CloseReason::ServerRequest,
            };
            let frame = ClientTunnelFrame {
                message: close_msg,
                timestamp: chrono::Utc::now(),
                message_id: Uuid::new_v4(),
            };

            if let Ok(frame_data) = frame.serialize() {
                let mut ws_guard = ws.lock().await;
                let _ = futures::SinkExt::send(
                    &mut *ws_guard,
                    tokio_tungstenite::tungstenite::Message::Binary(frame_data.into()),
                )
                .await;
            }
        }

        tracing::info!(
            "TCP reader task completed for connection: {}",
            connection_id
        );
    }

    /// Handle a single tunnel connection cycle
    async fn handle_tunnel_connection_cycle(&self) -> Result<()> {
        tracing::info!("Attempting to establish tunnel connection...");

        let (stream, _) = tokio_tungstenite::connect_async(self.tunnel_request.clone()).await?;

        tracing::info!("Tunnel WebSocket connection established");

        // For tunnel operations, we don't need RPC setup like KubeManager
        // Instead, we handle tunnel messages directly
        let (_ws_sender, mut ws_receiver) = stream.split();

        // Create local active TCP connections for this connection cycle
        let mut active_tcp_connections: HashMap<String, tokio::net::tcp::OwnedWriteHalf> =
            HashMap::new();

        // Keep the connection alive and handle incoming tunnel messages
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(tokio_tungstenite::tungstenite::Message::Binary(data)) => {
                    // Try to deserialize as TunnelFrame
                    if let Ok(frame) = ServerTunnelFrame::deserialize(&data) {
                        tracing::debug!("Received tunnel message: {:?}", frame.message);

                        // Handle the tunnel message
                        if let Err(e) = self
                            .handle_tunnel_message(frame.message, &mut active_tcp_connections)
                            .await
                        {
                            tracing::error!("Failed to handle tunnel message: {}", e);
                        }
                    } else {
                        tracing::warn!("Failed to deserialize tunnel frame");
                    }
                }
                Ok(tokio_tungstenite::tungstenite::Message::Close(_)) => {
                    tracing::info!("Tunnel connection closed by server");
                    break;
                }
                Ok(_) => {
                    // Ignore other message types (text, ping, pong)
                }
                Err(e) => {
                    tracing::error!("Tunnel WebSocket error: {}", e);
                    break;
                }
            }
        }

        Ok(())
    }
}
