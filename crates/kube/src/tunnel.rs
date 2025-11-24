use lapdev_tunnel::TunnelClient;
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
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
    clients: Arc<RwLock<HashMap<Uuid, BTreeMap<u64, Arc<TunnelClient>>>>>,
    generation_counter: AtomicU64,
}

impl TunnelRegistry {
    pub fn new() -> Self {
        Self {
            metrics: Arc::new(RwLock::new(HashMap::new())),
            clients: Arc::new(RwLock::new(HashMap::new())),
            generation_counter: AtomicU64::new(1),
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

    fn next_generation(&self) -> u64 {
        self.generation_counter.fetch_add(1, Ordering::SeqCst)
    }

    pub async fn register_client(&self, cluster_id: Uuid, client: Arc<TunnelClient>) -> u64 {
        let generation = self.next_generation();
        {
            let mut metrics = self.metrics.write().await;
            metrics
                .entry(cluster_id)
                .or_insert_with(TunnelMetrics::default);
        }

        let mut clients = self.clients.write().await;
        let cluster_clients = clients.entry(cluster_id).or_insert_with(BTreeMap::new);
        let replaced = cluster_clients
            .insert(generation, Arc::clone(&client))
            .is_some();
        if replaced {
            tracing::warn!(
                "Overwrote existing tunnel client entry for cluster {} at generation {}; this should be rare",
                cluster_id,
                generation
            );
        }
        tracing::info!(
            "Registered tunnel client for cluster {}; generation {}; active entries {}",
            cluster_id,
            generation,
            cluster_clients.len()
        );
        generation
    }

    pub async fn remove_client(&self, cluster_id: Uuid, generation: u64) {
        let mut removed = false;
        let mut cluster_empty_after = false;
        {
            let mut clients = self.clients.write().await;
            match clients.get_mut(&cluster_id) {
                Some(cluster_clients) => {
                    removed = cluster_clients.remove(&generation).is_some();
                    if removed {
                        tracing::info!(
                            "Removed tunnel client for cluster {} with generation {}; remaining {}",
                            cluster_id,
                            generation,
                            cluster_clients.len()
                        );
                        if cluster_clients.is_empty() {
                            clients.remove(&cluster_id);
                            cluster_empty_after = true;
                        }
                    } else {
                        let latest_generation = cluster_clients.keys().next_back().copied();
                        tracing::info!(
                            "Skip removing tunnel client for cluster {} due to generation mismatch (requested {}, latest {:?})",
                            cluster_id,
                            generation,
                            latest_generation
                        );
                    }
                }
                None => {
                    tracing::info!(
                        "No tunnel clients registered for cluster {}; requested removal generation {}",
                        cluster_id,
                        generation
                    );
                }
            }
        }
        if removed && cluster_empty_after {
            let mut metrics = self.metrics.write().await;
            metrics.remove(&cluster_id);
        }
    }

    pub async fn get_client(&self, cluster_id: Uuid) -> Option<Arc<TunnelClient>> {
        let clients = self.clients.read().await;
        clients.get(&cluster_id).and_then(|cluster_clients| {
            cluster_clients
                .values()
                .rev()
                .find(|client| !client.is_closed())
                .cloned()
        })
    }
}
