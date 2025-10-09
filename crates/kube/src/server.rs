use lapdev_common::kube::KubeClusterInfo;
use lapdev_db::api::DbApi;
use lapdev_kube_rpc::{KubeClusterRpc, KubeManagerRpcClient};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::tunnel::TunnelRegistry;

/// KubeClusterServer is the central server where
/// KubeManager and KubeCli connects to
#[derive(Clone)]
pub struct KubeClusterServer {
    cluster_id: Uuid,
    pub rpc_client: KubeManagerRpcClient,
    db: DbApi,
    kube_cluster_servers: Arc<RwLock<HashMap<Uuid, Vec<KubeClusterServer>>>>,
    tunnel_registry: Arc<TunnelRegistry>,
}

impl KubeClusterServer {
    pub fn new(
        cluster_id: Uuid,
        client: KubeManagerRpcClient,
        db: DbApi,
        kube_cluster_servers: Arc<RwLock<HashMap<Uuid, Vec<KubeClusterServer>>>>,
        tunnel_registry: Arc<TunnelRegistry>,
    ) -> Self {
        Self {
            cluster_id,
            rpc_client: client,
            db,
            kube_cluster_servers,
            tunnel_registry,
        }
    }

    pub fn cluster_id(&self) -> Uuid {
        self.cluster_id
    }

    pub async fn register(&self) {
        let mut servers = self.kube_cluster_servers.write().await;
        servers
            .entry(self.cluster_id)
            .or_insert_with(Vec::new)
            .push(self.clone());
        tracing::info!(
            "Registered KubeClusterServer for cluster {}",
            self.cluster_id
        );
    }

    pub async fn unregister(&self) {
        let mut servers = self.kube_cluster_servers.write().await;
        if let Some(cluster_servers) = servers.get_mut(&self.cluster_id) {
            // Remove servers with matching cluster_id (in case there are multiple connections)
            let initial_len = cluster_servers.len();
            cluster_servers.retain(|s| s.cluster_id != self.cluster_id || !std::ptr::eq(s, self));

            if cluster_servers.len() < initial_len {
                tracing::info!(
                    "Unregistered KubeClusterServer for cluster {}",
                    self.cluster_id
                );
            }

            // Remove empty entries
            if cluster_servers.is_empty() {
                servers.remove(&self.cluster_id);
            }
        }
    }
}

impl KubeClusterRpc for KubeClusterServer {
    async fn report_cluster_info(
        self,
        _context: ::tarpc::context::Context,
        cluster_info: KubeClusterInfo,
    ) -> Result<(), String> {
        tracing::info!(
            "Received cluster info for cluster {}: {:?}",
            self.cluster_id,
            cluster_info
        );

        // Verify cluster exists, error out if not found
        let _cluster = self
            .db
            .get_kube_cluster(self.cluster_id)
            .await
            .map_err(|e| format!("Database error: {}", e))?
            .ok_or_else(|| format!("Cluster {} not found", self.cluster_id))?;

        // Update cluster info in database
        let status_str = format!("{:?}", cluster_info.status);
        self.db
            .update_kube_cluster_info(
                self.cluster_id,
                Some(cluster_info.cluster_version),
                Some(status_str),
                cluster_info.region,
            )
            .await
            .map_err(|e| format!("Failed to update cluster info: {}", e))?;

        tracing::info!(
            "Successfully updated cluster {} info in database",
            self.cluster_id
        );

        Ok(())
    }

    async fn tunnel_heartbeat(self, _context: ::tarpc::context::Context) -> Result<(), String> {
        tracing::debug!("Received heartbeat from cluster {}", self.cluster_id);

        self.tunnel_registry.update_heartbeat(self.cluster_id).await
    }

    async fn report_tunnel_metrics(
        self,
        _context: ::tarpc::context::Context,
        active_connections: u32,
        bytes_transferred: u64,
        connection_count: u64,
        connection_errors: u64,
    ) -> Result<(), String> {
        tracing::debug!(
            "Received tunnel metrics from cluster {}: connections={}, bytes={}, errors={}",
            self.cluster_id,
            active_connections,
            bytes_transferred,
            connection_errors
        );

        self.tunnel_registry
            .update_metrics(
                self.cluster_id,
                active_connections,
                bytes_transferred,
                connection_count,
                connection_errors,
            )
            .await
    }
}
