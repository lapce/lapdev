use std::{collections::HashMap, sync::Arc};
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;

use crate::environment_events::EnvironmentLifecycleEvent;
use lapdev_db::api::DbApi;
use lapdev_kube::server::KubeClusterServer;
use lapdev_kube::tunnel::TunnelRegistry;
use tracing::{debug, warn};

// Submodules
mod app_catalog;
mod cluster;
mod deployment;
mod environment;
mod preview_url;
mod resources;
mod service;
pub mod validation;
mod workload;
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
    // Broadcast channel for environment lifecycle events
    pub environment_events: broadcast::Sender<EnvironmentLifecycleEvent>,
}

impl KubeController {
    pub fn new(
        db: DbApi,
        environment_events: broadcast::Sender<EnvironmentLifecycleEvent>,
    ) -> Self {
        Self {
            kube_cluster_servers: Arc::new(RwLock::new(HashMap::new())),
            tunnel_registry: Arc::new(TunnelRegistry::new()),
            db,
            environment_events,
        }
    }

    pub async fn get_random_kube_cluster_server(
        &self,
        cluster_id: Uuid,
    ) -> Option<KubeClusterServer> {
        let servers = self.kube_cluster_servers.read().await;
        servers.get(&cluster_id)?.last().cloned()
    }

    pub async fn refresh_cluster_namespace_watches(&self, cluster_id: Uuid) {
        let namespaces = match self.db.get_cluster_catalog_namespaces(cluster_id).await {
            Ok(namespaces) => namespaces,
            Err(err) => {
                warn!(
                    cluster_id = %cluster_id,
                    error = ?err,
                    "Failed to load namespaces for watch configuration"
                );
                return;
            }
        };

        let servers = self.kube_cluster_servers.read().await;
        let Some(cluster_servers) = servers.get(&cluster_id) else {
            debug!(
                cluster_id = %cluster_id,
                "No connected KubeManager instances; skipping namespace watch refresh"
            );
            return;
        };

        for server in cluster_servers {
            if let Err(err) = server
                .send_namespace_watch_configuration(namespaces.clone())
                .await
            {
                warn!(
                    cluster_id = %cluster_id,
                    error = ?err,
                    "Failed to send namespace watch configuration to KubeManager"
                );
            }
        }
    }
}
