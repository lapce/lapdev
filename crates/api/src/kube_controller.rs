use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;
use uuid::Uuid;
use lapdev_kube::server::KubeClusterServer;

#[derive(Clone)]
pub struct KubeController {
    // KubeManager connections per cluster
    pub kube_cluster_servers: Arc<RwLock<HashMap<Uuid, Vec<KubeClusterServer>>>>,
}

impl KubeController {
    pub fn new() -> Self {
        Self {
            kube_cluster_servers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn get_random_kube_cluster_server(
        &self,
        cluster_id: Uuid,
    ) -> Option<KubeClusterServer> {
        let servers = self.kube_cluster_servers.read().await;
        servers.get(&cluster_id)?.last().cloned()
    }
}
