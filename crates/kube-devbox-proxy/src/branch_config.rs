use lapdev_kube_rpc::BranchEnvironmentInfo;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use uuid::Uuid;

struct BranchTunnelInfo {
    info: BranchEnvironmentInfo,
    tunnel_task: Option<JoinHandle<()>>,
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
            "Adding branch environment {} (namespace: {}) - tunnel will be started on-demand",
            environment_id,
            branch.namespace
        );

        let mut branches = self.branches.write().await;
        branches.insert(
            environment_id,
            BranchTunnelInfo {
                info: branch,
                tunnel_task: None, // No tunnel started yet
            },
        );
    }

    pub async fn remove_branch(&self, environment_id: Uuid) {
        let mut branches = self.branches.write().await;
        if let Some(branch_tunnel) = branches.remove(&environment_id) {
            tracing::info!("Removing branch environment {}", environment_id);
            // Abort the tunnel task if it's running
            if let Some(task) = branch_tunnel.tunnel_task {
                task.abort();
            }
        } else {
            tracing::warn!(
                "Attempted to remove non-existent branch environment {}",
                environment_id
            );
        }
    }

    pub async fn start_branch_tunnel(&self, environment_id: Uuid) -> Result<(), String> {
        let mut branches = self.branches.write().await;

        let branch_tunnel = branches
            .get_mut(&environment_id)
            .ok_or_else(|| format!("Branch environment {} not found", environment_id))?;

        if branch_tunnel.tunnel_task.is_some() {
            return Err(format!(
                "Tunnel for branch {} is already running",
                environment_id
            ));
        }

        tracing::info!("Starting tunnel for branch environment {}", environment_id);

        // Spawn a task to maintain WebSocket tunnel for this branch environment
        let api_url = self.api_url.clone();
        let auth_token = branch_tunnel.info.auth_token.clone();
        let tunnel_task = tokio::spawn(async move {
            crate::rpc_server::DevboxProxyRpcServer::maintain_tunnel(
                api_url,
                environment_id,
                auth_token,
            )
            .await;
        });

        branch_tunnel.tunnel_task = Some(tunnel_task);
        Ok(())
    }

    pub async fn stop_branch_tunnel(&self, environment_id: Uuid) -> Result<(), String> {
        let mut branches = self.branches.write().await;

        let branch_tunnel = branches
            .get_mut(&environment_id)
            .ok_or_else(|| format!("Branch environment {} not found", environment_id))?;

        if let Some(task) = branch_tunnel.tunnel_task.take() {
            tracing::info!("Stopping tunnel for branch environment {}", environment_id);
            task.abort();
            Ok(())
        } else {
            Err(format!(
                "Tunnel for branch {} is not running",
                environment_id
            ))
        }
    }

    pub async fn get_branch_tunnel_status(&self, environment_id: Uuid) -> Result<bool, String> {
        let branches = self.branches.read().await;

        let branch_tunnel = branches
            .get(&environment_id)
            .ok_or_else(|| format!("Branch environment {} not found", environment_id))?;

        Ok(branch_tunnel.tunnel_task.is_some())
    }

    pub async fn list_branches(&self) -> Vec<BranchEnvironmentInfo> {
        let branches = self.branches.read().await;
        branches.values().map(|b| b.info.clone()).collect()
    }

    pub async fn get_branch(&self, environment_id: Uuid) -> Option<BranchEnvironmentInfo> {
        let branches = self.branches.read().await;
        branches.get(&environment_id).map(|b| b.info.clone())
    }
}
