use lapdev_kube_rpc::{BranchEnvironmentInfo, DevboxProxyRpc};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::info;
use uuid::Uuid;

use crate::branch_config::BranchConfig;

#[derive(Clone)]
pub struct DevboxProxyRpcServer {
    branch_config: BranchConfig,
    is_shared: bool,
    api_url: String,
    environment_id: Uuid,
    environment_auth_token: String,
    base_tunnel_task: Arc<RwLock<Option<JoinHandle<()>>>>,
}

impl DevboxProxyRpcServer {
    pub fn new(
        branch_config: BranchConfig,
        is_shared: bool,
        api_url: String,
        environment_id: Uuid,
        environment_auth_token: String,
    ) -> Self {
        Self {
            branch_config,
            is_shared,
            api_url,
            environment_id,
            environment_auth_token,
            base_tunnel_task: Arc::new(RwLock::new(None)),
        }
    }
}

impl DevboxProxyRpc for DevboxProxyRpcServer {
    async fn heartbeat(self, _context: ::tarpc::context::Context) -> Result<(), String> {
        tracing::debug!("Received heartbeat from kube-manager");
        Ok(())
    }

    async fn add_branch_environment(
        self,
        _context: ::tarpc::context::Context,
        branch: BranchEnvironmentInfo,
    ) -> Result<(), String> {
        if !self.is_shared {
            return Err(
                "add_branch_environment is only supported for shared environments".to_string(),
            );
        }

        info!(
            "Adding branch environment {} (namespace: {})",
            branch.environment_id, branch.namespace
        );

        self.branch_config.add_branch(branch).await;
        Ok(())
    }

    async fn remove_branch_environment(
        self,
        _context: ::tarpc::context::Context,
        environment_id: Uuid,
    ) -> Result<(), String> {
        if !self.is_shared {
            return Err(
                "remove_branch_environment is only supported for shared environments".to_string(),
            );
        }

        info!("Removing branch environment {}", environment_id);

        self.branch_config.remove_branch(environment_id).await;
        Ok(())
    }

    async fn list_branch_environments(
        self,
        _context: ::tarpc::context::Context,
    ) -> Result<Vec<BranchEnvironmentInfo>, String> {
        if !self.is_shared {
            return Err(
                "list_branch_environments is only supported for shared environments".to_string(),
            );
        }

        let branches = self.branch_config.list_branches().await;
        info!("Listing {} branch environments", branches.len());
        Ok(branches)
    }

    async fn start_tunnel(
        self,
        _context: ::tarpc::context::Context,
        environment_id: Option<Uuid>,
    ) -> Result<(), String> {
        match environment_id {
            None => {
                // Start base tunnel for personal environment
                if self.is_shared {
                    return Err(
                        "Base tunnel (None) is only supported for personal environments"
                            .to_string(),
                    );
                }

                let mut task_lock = self.base_tunnel_task.write().await;

                if task_lock.is_some() {
                    return Err("Base tunnel is already running".to_string());
                }

                info!(
                    "Starting base tunnel for personal environment {}",
                    self.environment_id
                );

                // Spawn a task to maintain the base tunnel
                let api_url = self.api_url.clone();
                let environment_id = self.environment_id;
                let auth_token = self.environment_auth_token.clone();

                let tunnel_task = tokio::spawn(async move {
                    Self::maintain_tunnel(api_url, environment_id, auth_token).await;
                });

                *task_lock = Some(tunnel_task);
                Ok(())
            }
            Some(branch_env_id) => {
                // Start branch tunnel for shared environment
                if !self.is_shared {
                    return Err(
                        "Branch tunnel (Some) is only supported for shared environments"
                            .to_string(),
                    );
                }

                info!("Starting tunnel for branch environment {}", branch_env_id);

                self.branch_config.start_branch_tunnel(branch_env_id).await
            }
        }
    }

    async fn stop_tunnel(
        self,
        _context: ::tarpc::context::Context,
        environment_id: Option<Uuid>,
    ) -> Result<(), String> {
        match environment_id {
            None => {
                // Stop base tunnel for personal environment
                if self.is_shared {
                    return Err(
                        "Base tunnel (None) is only supported for personal environments"
                            .to_string(),
                    );
                }

                let mut task_lock = self.base_tunnel_task.write().await;

                if let Some(task) = task_lock.take() {
                    info!(
                        "Stopping base tunnel for personal environment {}",
                        self.environment_id
                    );
                    task.abort();
                    Ok(())
                } else {
                    Err("Base tunnel is not running".to_string())
                }
            }
            Some(branch_env_id) => {
                // Stop branch tunnel for shared environment
                if !self.is_shared {
                    return Err(
                        "Branch tunnel (Some) is only supported for shared environments"
                            .to_string(),
                    );
                }

                info!("Stopping tunnel for branch environment {}", branch_env_id);

                self.branch_config.stop_branch_tunnel(branch_env_id).await
            }
        }
    }

    async fn get_tunnel_status(
        self,
        _context: ::tarpc::context::Context,
        environment_id: Option<Uuid>,
    ) -> Result<bool, String> {
        match environment_id {
            None => {
                // Get base tunnel status for personal environment
                let task_lock = self.base_tunnel_task.read().await;
                Ok(task_lock.is_some())
            }
            Some(branch_env_id) => {
                // Get branch tunnel status for shared environment
                self.branch_config
                    .get_branch_tunnel_status(branch_env_id)
                    .await
            }
        }
    }
}

impl DevboxProxyRpcServer {
    pub(crate) async fn maintain_tunnel(api_url: String, environment_id: Uuid, auth_token: String) {
        use lapdev_tunnel::{run_tunnel_server, WebSocketTransport};
        use tokio_tungstenite::connect_async;

        tracing::info!(
            "Starting tunnel maintenance for environment {}",
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
                "Connecting tunnel for {} to WebSocket: {}",
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
                        "Failed to build WebSocket request for environment {}: {}",
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
                        "Environment {} WebSocket tunnel connected, status: {:?}",
                        environment_id,
                        response.status()
                    );

                    // Wrap WebSocket in transport adapter
                    let transport = WebSocketTransport::new(ws_stream);

                    // Run the tunnel server - this handles all TCP connections
                    match run_tunnel_server(transport).await {
                        Ok(()) => {
                            tracing::info!(
                                "Environment {} tunnel server exited gracefully",
                                environment_id
                            );
                        }
                        Err(e) => {
                            tracing::error!(
                                "Environment {} tunnel server error: {}",
                                environment_id,
                                e
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to connect environment {} WebSocket tunnel: {}",
                        environment_id,
                        e
                    );
                }
            }

            tracing::info!(
                "Environment {} tunnel disconnected, reconnecting in 5s...",
                environment_id
            );
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        }
    }
}
