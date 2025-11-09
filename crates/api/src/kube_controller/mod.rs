use std::{
    collections::{BTreeMap, HashMap},
    sync::{atomic::AtomicU64, Arc},
};
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;

use crate::environment_events::EnvironmentLifecycleEvent;
use lapdev_db::api::DbApi;
use lapdev_kube::{server::KubeClusterServer, tunnel::TunnelRegistry};
use lapdev_common::devbox::DirectChannelConfig;
use lapdev_kube_rpc::{DevboxRouteConfig, ProxyBranchRouteConfig};
use tracing::{debug, warn};

// Submodules
mod app_catalog;
mod cluster;
pub(crate) mod container_images;
mod deployment;
mod environment;
mod preview_url;
mod resources;
mod service;
pub mod validation;
mod workload;
pub mod yaml_parser;

// Re-exports
pub use self::container_images::CONTAINER_IMAGE_TAG;
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
    pub kube_cluster_servers: Arc<RwLock<HashMap<Uuid, BTreeMap<u64, KubeClusterServer>>>>,
    pub kube_cluster_server_generation: Arc<AtomicU64>,
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
            kube_cluster_server_generation: Arc::new(AtomicU64::new(1)),
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
        servers
            .get(&cluster_id)?
            .iter()
            .next_back()
            .map(|(_, server)| server.clone())
    }

    pub async fn refresh_cluster_namespace_watches(&self, cluster_id: Uuid) {
        let namespaces = match self.db.get_cluster_watch_namespaces(cluster_id).await {
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

        for server in cluster_servers.values() {
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

    pub async fn set_devbox_routes(
        &self,
        cluster_id: Uuid,
        environment_id: Uuid,
        routes: HashMap<Uuid, DevboxRouteConfig>,
    ) -> Result<(), String> {
        let servers = self.kube_cluster_servers.read().await;
        let Some(cluster_servers) = servers.get(&cluster_id) else {
            return Err(format!(
                "No connected KubeManager instances for cluster {}",
                cluster_id
            ));
        };

        let mut last_err: Option<String> = None;
        for server in cluster_servers.values() {
            match server
                .set_devbox_routes(environment_id, routes.clone())
                .await
            {
                Ok(()) => return Ok(()),
                Err(err) => last_err = Some(err),
            }
        }

        Err(last_err.unwrap_or_else(|| {
            "Failed to dispatch set_devbox_routes to any KubeManager instances".to_string()
        }))
    }

    pub async fn set_devbox_route(
        &self,
        cluster_id: Uuid,
        environment_id: Uuid,
        route: DevboxRouteConfig,
    ) -> Result<(), String> {
        let servers = self.kube_cluster_servers.read().await;
        let Some(cluster_servers) = servers.get(&cluster_id) else {
            return Err(format!(
                "No connected KubeManager instances for cluster {}",
                cluster_id
            ));
        };

        let mut last_err: Option<String> = None;
        for server in cluster_servers.values() {
            match server.set_devbox_route(environment_id, route.clone()).await {
                Ok(()) => return Ok(()),
                Err(err) => last_err = Some(err),
            }
        }

        Err(last_err.unwrap_or_else(|| {
            "Failed to dispatch set_devbox_route to any KubeManager instances".to_string()
        }))
    }

    pub async fn remove_devbox_route(
        &self,
        cluster_id: Uuid,
        environment_id: Uuid,
        workload_id: Uuid,
        branch_environment_id: Option<Uuid>,
    ) -> Result<(), String> {
        let servers = self.kube_cluster_servers.read().await;
        let Some(cluster_servers) = servers.get(&cluster_id) else {
            return Err(format!(
                "No connected KubeManager instances for cluster {}",
                cluster_id
            ));
        };

        let mut last_err: Option<String> = None;
        for server in cluster_servers.values() {
            match server
                .remove_devbox_route(environment_id, workload_id, branch_environment_id)
                .await
            {
                Ok(()) => return Ok(()),
                Err(err) => last_err = Some(err),
            }
        }

        Err(last_err.unwrap_or_else(|| {
            "Failed to dispatch remove_devbox_route to any KubeManager instances".to_string()
        }))
    }

    pub async fn clear_devbox_routes(
        &self,
        cluster_id: Uuid,
        environment_id: Uuid,
        branch_environment_id: Option<Uuid>,
    ) -> Result<(), String> {
        let servers = self.kube_cluster_servers.read().await;
        let Some(cluster_servers) = servers.get(&cluster_id) else {
            return Err(format!(
                "No connected KubeManager instances for cluster {}",
                cluster_id
            ));
        };

        let mut last_err: Option<String> = None;
        for server in cluster_servers.values() {
            match server
                .clear_devbox_routes(environment_id, branch_environment_id)
                .await
            {
                Ok(()) => return Ok(()),
                Err(err) => last_err = Some(err),
            }
        }

        Err(last_err.unwrap_or_else(|| {
            "Failed to dispatch clear_devbox_routes to any KubeManager instances".to_string()
        }))
    }

    pub async fn request_devbox_direct_config(
        &self,
        cluster_id: Uuid,
        user_id: Uuid,
        environment_id: Uuid,
        namespace: String,
    ) -> Result<Option<DirectChannelConfig>, String> {
        let servers = self.kube_cluster_servers.read().await;
        let Some(cluster_servers) = servers.get(&cluster_id) else {
            return Err(format!(
                "No connected KubeManager instances for cluster {}",
                cluster_id
            ));
        };

        let mut last_err: Option<String> = None;
        for server in cluster_servers.values() {
            match server
                .get_devbox_direct_config(user_id, environment_id, namespace.clone())
                .await
            {
                Ok(config) => return Ok(config),
                Err(err) => last_err = Some(err),
            }
        }

        Err(last_err.unwrap_or_else(|| {
            "Failed to dispatch get_devbox_direct_config to any KubeManager instances".to_string()
        }))
    }

    pub async fn update_branch_service_route(
        &self,
        cluster_id: Uuid,
        base_environment_id: Uuid,
        workload_id: Uuid,
        route: ProxyBranchRouteConfig,
    ) -> Result<(), String> {
        let servers = self.kube_cluster_servers.read().await;
        let Some(cluster_servers) = servers.get(&cluster_id) else {
            return Err(format!(
                "No connected KubeManager instances for cluster {}",
                cluster_id
            ));
        };

        let mut last_err: Option<String> = None;
        for server in cluster_servers.values() {
            match server
                .rpc_client
                .update_branch_service_route(
                    tarpc::context::current(),
                    base_environment_id,
                    workload_id,
                    route.clone(),
                )
                .await
            {
                Ok(Ok(())) => return Ok(()),
                Ok(Err(err)) => last_err = Some(err),
                Err(err) => last_err = Some(format!("{}", err)),
            }
        }

        Err(last_err.unwrap_or_else(|| {
            "Failed to dispatch update_branch_service_route to any KubeManager instances"
                .to_string()
        }))
    }

    pub async fn remove_branch_service_route(
        &self,
        cluster_id: Uuid,
        base_environment_id: Uuid,
        workload_id: Uuid,
        branch_environment_id: Uuid,
    ) -> Result<(), String> {
        let servers = self.kube_cluster_servers.read().await;
        let Some(cluster_servers) = servers.get(&cluster_id) else {
            return Err(format!(
                "No connected KubeManager instances for cluster {}",
                cluster_id
            ));
        };

        let mut last_err: Option<String> = None;
        for server in cluster_servers.values() {
            match server
                .rpc_client
                .remove_branch_service_route(
                    tarpc::context::current(),
                    base_environment_id,
                    workload_id,
                    branch_environment_id,
                )
                .await
            {
                Ok(Ok(())) => return Ok(()),
                Ok(Err(err)) => last_err = Some(err),
                Err(err) => last_err = Some(format!("{}", err)),
            }
        }

        Err(last_err.unwrap_or_else(|| {
            "Failed to dispatch remove_branch_service_route to any KubeManager instances"
                .to_string()
        }))
    }
}
