use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;
use uuid::Uuid;

use lapdev_db::api::DbApi;
use lapdev_kube::server::KubeClusterServer;
use lapdev_kube::tunnel::TunnelRegistry;

// Submodules
mod app_catalog;
mod cluster;
mod deployment;
mod environment;
mod preview_url;
mod service;
pub mod validation;
mod workload;
mod workload_yaml_cleaner;
pub mod yaml_parser;

// Re-exports
pub use validation::*;
pub use yaml_parser::*;

pub(crate) enum EnvironmentNamespaceKind {
    Personal,
    Shared,
    Branch,
}

#[derive(Clone)]
pub struct KubeController {
    // KubeManager connections per cluster
    pub kube_cluster_servers: Arc<RwLock<HashMap<Uuid, Vec<KubeClusterServer>>>>,
    // Tunnel registry for preview URL functionality
    pub tunnel_registry: Arc<TunnelRegistry>,
    // Database API
    pub db: DbApi,
}

impl KubeController {
    pub fn new(db: DbApi) -> Self {
        Self {
            kube_cluster_servers: Arc::new(RwLock::new(HashMap::new())),
            tunnel_registry: Arc::new(TunnelRegistry::new()),
            db,
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
