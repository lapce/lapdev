use lapdev_kube_rpc::{ClientTunnelMessage, ServerTunnelMessage};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, RwLock};
use uuid::Uuid;

pub type ServerTunnelMessageSender = mpsc::UnboundedSender<ServerTunnelMessage>;

#[derive(Debug)]
pub enum TunnelResponse {
    ConnectionOpened {
        tunnel_id: String,
        local_addr: String,
    },
    ConnectionFailed {
        tunnel_id: String,
        error: String,
    },
    Data {
        tunnel_id: String,
        payload: Vec<u8>,
    },
    ConnectionClosed {
        tunnel_id: String,
        bytes_transferred: u64,
    },
}

pub type TunnelResponseSender = oneshot::Sender<TunnelResponse>;
pub type TunnelDataSender = mpsc::UnboundedSender<Vec<u8>>;

#[derive(Debug, Clone)]
pub struct TunnelMetrics {
    pub active_connections: u32,
    pub total_connections: u64,
    pub bytes_transferred: u64,
    pub connection_errors: u64,
}

pub struct TunnelRegistry {
    tunnel_metrics: Arc<RwLock<HashMap<Uuid, TunnelMetrics>>>,
    tunnel_senders: Arc<RwLock<HashMap<Uuid, ServerTunnelMessageSender>>>, // cluster_id -> message sender
    // Pending requests waiting for responses from KubeManager
    pending_responses: Arc<RwLock<HashMap<String, TunnelResponseSender>>>, // tunnel_id -> response sender
    // Active tunnel data channels for streaming HTTP responses
    tunnel_data_channels: Arc<RwLock<HashMap<String, TunnelDataSender>>>, // tunnel_id -> data sender
}

impl TunnelRegistry {
    pub fn new() -> Self {
        Self {
            tunnel_metrics: Arc::new(RwLock::new(HashMap::new())),
            tunnel_senders: Arc::new(RwLock::new(HashMap::new())),
            pending_responses: Arc::new(RwLock::new(HashMap::new())),
            tunnel_data_channels: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn update_heartbeat(&self, cluster_id: Uuid) -> Result<(), String> {
        Ok(())
    }

    pub async fn register_tunnel_sender(
        &self,
        cluster_id: Uuid,
        sender: ServerTunnelMessageSender,
    ) {
        let mut senders = self.tunnel_senders.write().await;
        senders.insert(cluster_id, sender);
        tracing::info!(
            "Registered data plane message sender for cluster: {}",
            cluster_id
        );
    }

    pub async fn update_metrics(
        &self,
        cluster_id: Uuid,
        active_connections: u32,
        bytes_transferred: u64,
        connection_count: u64,
        connection_errors: u64,
    ) -> Result<(), String> {
        let mut metrics = self.tunnel_metrics.write().await;
        if let Some(tunnel_metrics) = metrics.get_mut(&cluster_id) {
            tunnel_metrics.active_connections = active_connections;
            tunnel_metrics.bytes_transferred += bytes_transferred;
            tunnel_metrics.total_connections += connection_count;
            tunnel_metrics.connection_errors += connection_errors;
            Ok(())
        } else {
            Err(format!(
                "Tunnel metrics not found for cluster: {}",
                cluster_id
            ))
        }
    }

    pub async fn remove_tunnel(&self, cluster_id: Uuid) {
        let mut metrics = self.tunnel_metrics.write().await;
        let mut senders = self.tunnel_senders.write().await;
        metrics.remove(&cluster_id);
        senders.remove(&cluster_id);
        tracing::info!("Removed tunnel for cluster: {}", cluster_id);
    }

    /// Send a message through the data plane channel
    pub async fn send_tunnel_message(
        &self,
        cluster_id: Uuid,
        message: ServerTunnelMessage,
    ) -> Result<(), String> {
        let senders = self.tunnel_senders.read().await;
        let sender = senders
            .get(&cluster_id)
            .ok_or_else(|| format!("No data plane connection for cluster: {}", cluster_id))?;

        sender
            .send(message)
            .map_err(|e| format!("Failed to send message through channel: {}", e))?;

        Ok(())
    }

    pub async fn get_tunnel_sender(&self, cluster_id: Uuid) -> Option<ServerTunnelMessageSender> {
        let senders = self.tunnel_senders.read().await;
        senders.get(&cluster_id).cloned()
    }

    /// Send a tunnel message and wait for the response
    pub async fn send_tunnel_message_with_response(
        &self,
        cluster_id: Uuid,
        message: ServerTunnelMessage,
        timeout: Duration,
    ) -> Result<TunnelResponse, String> {
        let tunnel_id = match &message {
            ServerTunnelMessage::OpenConnection { tunnel_id, .. } => tunnel_id.clone(),
            _ => return Err("Only OpenConnection messages support responses".to_string()),
        };

        // Create response channel
        let (response_tx, response_rx) = oneshot::channel();

        // Register the response channel
        {
            let mut pending = self.pending_responses.write().await;
            pending.insert(tunnel_id.clone(), response_tx);
        }

        // Send the message
        if let Err(e) = self.send_tunnel_message(cluster_id, message).await {
            // Clean up on send failure
            let mut pending = self.pending_responses.write().await;
            pending.remove(&tunnel_id);
            return Err(e);
        }

        // Wait for response with timeout
        let response = tokio::time::timeout(timeout, response_rx)
            .await
            .map_err(|_| {
                // Clean up on timeout
                let pending = self.pending_responses.clone();
                let tunnel_id_clone = tunnel_id.clone();
                tokio::spawn(async move {
                    let mut pending = pending.write().await;
                    pending.remove(&tunnel_id_clone);
                });
                format!("Timeout waiting for response for tunnel {}", tunnel_id)
            })?
            .map_err(|_| "Response channel closed".to_string())?;

        Ok(response)
    }

    /// Handle incoming tunnel message from KubeManager
    pub async fn handle_client_message(&self, message: ClientTunnelMessage) {
        match message {
            ClientTunnelMessage::ConnectionOpened {
                tunnel_id,
                local_addr,
            } => {
                if let Some(sender) = self.pending_responses.write().await.remove(&tunnel_id) {
                    let _ = sender.send(TunnelResponse::ConnectionOpened {
                        tunnel_id,
                        local_addr,
                    });
                }
            }
            ClientTunnelMessage::ConnectionFailed {
                tunnel_id, error, ..
            } => {
                if let Some(sender) = self.pending_responses.write().await.remove(&tunnel_id) {
                    let _ = sender.send(TunnelResponse::ConnectionFailed { tunnel_id, error });
                }
            }
            ClientTunnelMessage::Data {
                tunnel_id, payload, ..
            } => {
                // Send data to the appropriate data channel
                let data_channels = self.tunnel_data_channels.read().await;
                if let Some(data_sender) = data_channels.get(&tunnel_id) {
                    let _ = data_sender.send(payload);
                }
            }
            ClientTunnelMessage::ConnectionClosed {
                tunnel_id,
                bytes_transferred,
            } => {
                // Clean up data channel
                self.tunnel_data_channels.write().await.remove(&tunnel_id);
                // Notify if there's a pending response
                if let Some(sender) = self.pending_responses.write().await.remove(&tunnel_id) {
                    let _ = sender.send(TunnelResponse::ConnectionClosed {
                        tunnel_id,
                        bytes_transferred,
                    });
                }
            }
            _ => {
                // Handle other message types as needed
                tracing::debug!("Received unhandled client tunnel message: {:?}", message);
            }
        }
    }

    /// Register a data channel for receiving streaming data
    pub async fn register_data_channel(&self, tunnel_id: String, sender: TunnelDataSender) {
        let mut channels = self.tunnel_data_channels.write().await;
        channels.insert(tunnel_id.clone(), sender);
        tracing::debug!("Registered data channel for tunnel: {}", tunnel_id);
    }

    /// Clean up tunnel resources
    pub async fn cleanup_tunnel(&self, tunnel_id: &str) {
        let mut pending = self.pending_responses.write().await;
        let mut channels = self.tunnel_data_channels.write().await;
        pending.remove(tunnel_id);
        channels.remove(tunnel_id);
        tracing::debug!("Cleaned up tunnel resources for: {}", tunnel_id);
    }
}
