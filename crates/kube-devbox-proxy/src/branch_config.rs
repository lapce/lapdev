use lapdev_kube_rpc::BranchEnvironmentInfo;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use uuid::Uuid;

struct BranchTunnelInfo {
    info: BranchEnvironmentInfo,
    tunnel_task: JoinHandle<()>,
}

/// Stores configuration for branch environments that share this devbox-proxy
#[derive(Clone)]
pub struct BranchConfig {
    branches: Arc<RwLock<HashMap<Uuid, BranchTunnelInfo>>>,
    api_url: String,
}

impl BranchConfig {
    pub fn new(api_url: String) -> Self {
        Self {
            branches: Arc::new(RwLock::new(HashMap::new())),
            api_url,
        }
    }

    pub async fn add_branch(&self, branch: BranchEnvironmentInfo) {
        let environment_id = branch.environment_id;
        tracing::info!(
            "Adding branch environment {} (namespace: {})",
            environment_id,
            branch.namespace
        );

        // Spawn a task to maintain WebSocket tunnel for this branch environment
        let api_url = self.api_url.clone();
        let auth_token = branch.auth_token.clone();
        let tunnel_task = tokio::spawn(async move {
            Self::maintain_branch_tunnel(api_url, environment_id, auth_token).await;
        });

        let mut branches = self.branches.write().await;
        branches.insert(
            environment_id,
            BranchTunnelInfo {
                info: branch,
                tunnel_task,
            },
        );
    }

    pub async fn remove_branch(&self, environment_id: Uuid) {
        let mut branches = self.branches.write().await;
        if let Some(branch_tunnel) = branches.remove(&environment_id) {
            tracing::info!("Removing branch environment {}", environment_id);
            // Abort the tunnel task
            branch_tunnel.tunnel_task.abort();
        } else {
            tracing::warn!(
                "Attempted to remove non-existent branch environment {}",
                environment_id
            );
        }
    }

    pub async fn list_branches(&self) -> Vec<BranchEnvironmentInfo> {
        let branches = self.branches.read().await;
        branches.values().map(|b| b.info.clone()).collect()
    }

    pub async fn get_branch(&self, environment_id: Uuid) -> Option<BranchEnvironmentInfo> {
        let branches = self.branches.read().await;
        branches.get(&environment_id).map(|b| b.info.clone())
    }

    async fn maintain_branch_tunnel(api_url: String, environment_id: Uuid, auth_token: String) {
        use futures_util::StreamExt;
        use tokio_tungstenite::{connect_async, tungstenite::Message};

        tracing::info!(
            "Starting tunnel maintenance for branch environment {}",
            environment_id
        );

        loop {
            // Build WebSocket URL
            let ws_url = format!(
                "{}/kube/devbox-proxy/tunnel/{}",
                api_url
                    .replace("http://", "ws://")
                    .replace("https://", "wss://"),
                environment_id
            );

            tracing::info!(
                "Connecting branch {} tunnel to WebSocket: {}",
                environment_id,
                ws_url
            );

            // Create custom headers with auth token
            let request = match http::Request::builder()
                .uri(&ws_url)
                .header("X-Lapdev-Environment-Token", &auth_token)
                .body(())
            {
                Ok(req) => req,
                Err(e) => {
                    tracing::error!(
                        "Failed to build WebSocket request for branch {}: {}",
                        environment_id,
                        e
                    );
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    continue;
                }
            };

            // Connect to WebSocket
            match connect_async(request).await {
                Ok((ws_stream, response)) => {
                    tracing::info!(
                        "Branch {} WebSocket tunnel connected, status: {:?}",
                        environment_id,
                        response.status()
                    );

                    let (_ws_sender, mut ws_receiver) = ws_stream.split();

                    // Handle incoming messages
                    while let Some(msg) = ws_receiver.next().await {
                        match msg {
                            Ok(Message::Binary(data)) => {
                                tracing::debug!(
                                    "Branch {} received {} bytes from WebSocket",
                                    environment_id,
                                    data.len()
                                );
                                // Data from devbox will be forwarded to in-cluster services
                            }
                            Ok(Message::Close(_)) => {
                                tracing::info!(
                                    "Branch {} WebSocket connection closed by server",
                                    environment_id
                                );
                                break;
                            }
                            Ok(Message::Ping(_)) => {
                                tracing::debug!("Branch {} received ping", environment_id);
                            }
                            Ok(Message::Pong(_)) => {
                                tracing::debug!("Branch {} received pong", environment_id);
                            }
                            Ok(Message::Text(text)) => {
                                tracing::debug!(
                                    "Branch {} received text message: {}",
                                    environment_id,
                                    text
                                );
                            }
                            Err(e) => {
                                tracing::error!(
                                    "Branch {} WebSocket error: {}",
                                    environment_id,
                                    e
                                );
                                break;
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to connect branch {} WebSocket tunnel: {}",
                        environment_id,
                        e
                    );
                }
            }

            tracing::info!(
                "Branch {} tunnel disconnected, reconnecting in 5s...",
                environment_id
            );
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        }
    }
}
