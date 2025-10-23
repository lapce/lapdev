use anyhow::Result;
use futures::StreamExt;
use lapdev_common::kube::{DEFAULT_SIDECAR_PROXY_MANAGER_PORT, SIDECAR_PROXY_MANAGER_PORT_ENV_VAR};
use lapdev_kube_rpc::{
    DevboxRouteConfig, KubeClusterRpcClient, ProxyBranchRouteConfig, SidecarProxyManagerRpc,
    SidecarProxyRpcClient,
};
use lapdev_rpc::spawn_twoway;
use std::{collections::HashMap, sync::Arc};
use tarpc::server::{BaseChannel, Channel};
use tokio::sync::RwLock;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::sidecar_proxy_manager_rpc::SidecarProxyManagerRpcServer;

#[derive(Clone)]
pub struct SidecarProxyManager {
    pub(crate) sidecar_proxies: Arc<RwLock<HashMap<Uuid, HashMap<Uuid, SidecarProxyRpcClient>>>>,
    kube_cluster_rpc_client: Arc<RwLock<Option<KubeClusterRpcClient>>>,
}

impl SidecarProxyManager {
    pub(crate) async fn new() -> Result<Self> {
        // Parse the URL to extract the port
        let port = std::env::var(SIDECAR_PROXY_MANAGER_PORT_ENV_VAR)
            .ok()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(DEFAULT_SIDECAR_PROXY_MANAGER_PORT);
        let bind_addr = format!("0.0.0.0:{port}");
        info!("Starting TCP server for sidecar proxies on: {}", bind_addr);

        let mut listener = tarpc::serde_transport::tcp::listen(
            ("0.0.0.0", port),
            tarpc::tokio_serde::formats::Bincode::default,
        )
        .await?;
        info!("TCP server listening on: {}", bind_addr);

        let m = Self {
            sidecar_proxies: Arc::new(RwLock::new(HashMap::new())),
            kube_cluster_rpc_client: Arc::new(RwLock::new(None)),
        };

        {
            let m = m.clone();
            tokio::spawn(async move {
                while let Some(conn) = listener.next().await {
                    if let Ok(conn) = conn {
                        let m = m.clone();
                        tokio::spawn(async move {
                            let (server_chan, client_chan, _) = spawn_twoway(conn);
                            let rpc_client = SidecarProxyRpcClient::new(
                                tarpc::client::Config::default(),
                                client_chan,
                            )
                            .spawn();
                            let rpc_server =
                                SidecarProxyManagerRpcServer::new(m.clone(), rpc_client);
                            BaseChannel::with_defaults(server_chan)
                                .execute(rpc_server.serve())
                                .for_each(|resp| async move {
                                    tokio::spawn(resp);
                                })
                                .await;
                        });
                    }
                }
                error!("TCP connection stopped");
            });
        }

        Ok(m)
    }

    pub async fn set_service_routes_if_registered(&self, environment_id: Uuid) -> Result<()> {
        let connections = {
            let map = self.sidecar_proxies.read().await;
            map.get(&environment_id).cloned()
        };

        let Some(connections) = connections else {
            return Ok(());
        };

        let cluster_client = {
            let guard = self.kube_cluster_rpc_client.read().await;
            guard.clone()
        };

        let Some(cluster_client) = cluster_client else {
            warn!(
                "Cluster RPC client unavailable; skipping route sync for environment {}",
                environment_id
            );
            return Ok(());
        };

        for (workload_id, client) in connections {
            let routes = match cluster_client
                .clone()
                .list_branch_service_routes(tarpc::context::current(), environment_id, workload_id)
                .await
            {
                Ok(Ok(routes)) => routes,
                Ok(Err(err)) => {
                    return Err(anyhow::anyhow!(
                        "API rejected route request for environment {} workload {}: {}",
                        environment_id,
                        workload_id,
                        err
                    ));
                }
                Err(err) => {
                    return Err(anyhow::anyhow!(
                        "Failed to fetch routes for environment {} workload {}: {}",
                        environment_id,
                        workload_id,
                        err
                    ));
                }
            };

            if let Err(e) = client
                .set_service_routes(tarpc::context::current(), routes)
                .await
            {
                return Err(anyhow::anyhow!(
                    "Failed to push service routes to workload {}: {}",
                    workload_id,
                    e
                ));
            }
        }

        Ok(())
    }

    pub async fn set_devbox_routes(
        &self,
        environment_id: Uuid,
        mut routes: HashMap<Uuid, DevboxRouteConfig>,
    ) -> Result<(), String> {
        let connections = {
            let map = self.sidecar_proxies.read().await;
            map.get(&environment_id).cloned()
        };

        let Some(connections) = connections else {
            warn!(
                "No sidecar proxy registered for environment {} when sending devbox routes",
                environment_id
            );
            return Ok(());
        };

        for (workload_id, client) in connections {
            if let Some(route) = routes.remove(&workload_id) {
                if let Err(e) = client
                    .set_devbox_route(tarpc::context::current(), route)
                    .await
                {
                    return Err(format!(
                        "Failed to send devbox routes to sidecar (workload {}): {}",
                        workload_id, e
                    ));
                }
            } else if let Err(e) = client.stop_devbox(tarpc::context::current(), None).await {
                return Err(format!(
                    "Failed to clear devbox routes for workload {}: {}",
                    workload_id, e
                ));
            }
        }

        for workload_id in routes.into_keys() {
            warn!(
                "No sidecar proxy registered for workload {} when sending devbox route",
                workload_id
            );
        }

        Ok(())
    }

    pub async fn upsert_branch_service_route(
        &self,
        base_environment_id: Uuid,
        workload_id: Uuid,
        route: ProxyBranchRouteConfig,
    ) -> Result<(), String> {
        let connections = {
            let map = self.sidecar_proxies.read().await;
            map.get(&base_environment_id).cloned()
        };

        let Some(connections) = connections else {
            warn!(
                "No sidecar proxy registered for environment {} when updating branch service route",
                base_environment_id
            );
            return Ok(());
        };

        let Some(client) = connections.get(&workload_id) else {
            warn!(
                "No sidecar proxy registered for workload {} in environment {} when updating branch service route",
                workload_id, base_environment_id
            );
            return Ok(());
        };

        if let Err(err) = client
            .upsert_branch_service_route(tarpc::context::current(), route)
            .await
        {
            return Err(format!(
                "Failed to update branch service route for workload {}: {}",
                workload_id, err
            ));
        }

        Ok(())
    }

    pub async fn remove_branch_service_route(
        &self,
        base_environment_id: Uuid,
        workload_id: Uuid,
        branch_environment_id: Uuid,
    ) -> Result<(), String> {
        let connections = {
            let map = self.sidecar_proxies.read().await;
            map.get(&base_environment_id).cloned()
        };

        let Some(connections) = connections else {
            return Ok(());
        };

        let Some(client) = connections.get(&workload_id) else {
            return Ok(());
        };

        if let Err(err) = client
            .remove_branch_service_route(tarpc::context::current(), branch_environment_id)
            .await
        {
            return Err(format!(
                "Failed to remove branch service route for workload {} branch {}: {}",
                workload_id, branch_environment_id, err
            ));
        }

        Ok(())
    }

    pub async fn clear_devbox_routes(
        &self,
        environment_id: Uuid,
        branch_environment_id: Option<Uuid>,
    ) -> Result<(), String> {
        let connections = {
            let map = self.sidecar_proxies.read().await;
            map.get(&environment_id).cloned()
        };

        let Some(connections) = connections else {
            return Ok(());
        };

        for (workload_id, client) in connections {
            if let Err(err) = client
                .stop_devbox(tarpc::context::current(), branch_environment_id)
                .await
            {
                return Err(format!(
                    "Failed to clear devbox routes for workload {}: {}",
                    workload_id, err
                ));
            }
        }

        Ok(())
    }

    pub async fn set_cluster_rpc_client(&self, client: KubeClusterRpcClient) {
        let mut guard = self.kube_cluster_rpc_client.write().await;
        *guard = Some(client);
    }

    pub async fn clear_cluster_rpc_client(&self) {
        let mut guard = self.kube_cluster_rpc_client.write().await;
        *guard = None;
    }
}
