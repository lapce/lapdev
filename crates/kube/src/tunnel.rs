use lapdev_tunnel::TunnelClient;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Default)]
pub struct TunnelMetrics {
    pub active_connections: u32,
    pub total_connections: u64,
    pub bytes_transferred: u64,
    pub connection_errors: u64,
}

pub struct TunnelRegistry {
    metrics: Arc<RwLock<HashMap<Uuid, TunnelMetrics>>>,
    clients: Arc<RwLock<HashMap<Uuid, Arc<TunnelClient>>>>,
}

impl TunnelRegistry {
    pub fn new() -> Self {
        Self {
            metrics: Arc::new(RwLock::new(HashMap::new())),
            clients: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn update_heartbeat(&self, _cluster_id: Uuid) -> Result<(), String> {
        Ok(())
    }

    pub async fn update_metrics(
        &self,
        cluster_id: Uuid,
        active_connections: u32,
        bytes_transferred: u64,
        connection_count: u64,
        connection_errors: u64,
    ) -> Result<(), String> {
        let mut guard = self.metrics.write().await;
        let entry = guard
            .entry(cluster_id)
            .or_insert_with(TunnelMetrics::default);
        entry.active_connections = active_connections;
        entry.bytes_transferred = entry.bytes_transferred.saturating_add(bytes_transferred);
        entry.total_connections = entry.total_connections.saturating_add(connection_count);
        entry.connection_errors = entry.connection_errors.saturating_add(connection_errors);
        Ok(())
    }

    pub async fn register_client(&self, cluster_id: Uuid, client: Arc<TunnelClient>) {
        {
            let mut metrics = self.metrics.write().await;
            metrics
                .entry(cluster_id)
                .or_insert_with(TunnelMetrics::default);
        }

        let mut clients = self.clients.write().await;
        clients.insert(cluster_id, client);
        tracing::info!("Registered tunnel client for cluster {}", cluster_id);
    }

    pub async fn remove_client(&self, cluster_id: Uuid) {
        {
            let mut clients = self.clients.write().await;
            clients.remove(&cluster_id);
        }
        {
            let mut metrics = self.metrics.write().await;
            metrics.remove(&cluster_id);
        }
        tracing::info!("Removed tunnel client for cluster {}", cluster_id);
    }

    pub async fn get_client(&self, cluster_id: Uuid) -> Option<Arc<TunnelClient>> {
        let clients = self.clients.read().await;
        clients.get(&cluster_id).cloned()
    }
}
