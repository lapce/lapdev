use anyhow::Result;
use futures::StreamExt;
use lapdev_common::{
    devbox::DirectTunnelConfig,
    kube::{DEFAULT_SIDECAR_PROXY_MANAGER_PORT, SIDECAR_PROXY_MANAGER_PORT_ENV_VAR},
};
use lapdev_kube_rpc::{
    DevboxRouteConfig, KubeClusterRpcClient, ProxyBranchRouteConfig, SidecarProxyManagerRpc,
    SidecarProxyRpcClient,
};
use lapdev_rpc::spawn_twoway;
use std::{
    collections::{BTreeMap, HashMap},
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use tarpc::server::{BaseChannel, Channel};
use tokio::sync::RwLock;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::sidecar_proxy_manager_rpc::SidecarProxyManagerRpcServer;

#[derive(Clone)]
pub struct SidecarProxyManager {
    pub(crate) sidecar_proxies:
        Arc<RwLock<HashMap<Uuid, HashMap<Uuid, BTreeMap<u64, SidecarProxyRpcClient>>>>>,
    kube_cluster_rpc_client: Arc<RwLock<Option<KubeClusterRpcClient>>>,
    generation_counter: Arc<AtomicU64>,
    devbox_route_snapshots: Arc<RwLock<HashMap<Uuid, HashMap<Uuid, DevboxRouteConfig>>>>,
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
            generation_counter: Arc::new(AtomicU64::new(1)),
            devbox_route_snapshots: Arc::new(RwLock::new(HashMap::new())),
        };

        {
            let manager = m.clone();
            tokio::spawn(async move {
                while let Some(conn) = listener.next().await {
                    if let Ok(conn) = conn {
                        let manager = manager.clone();
                        tokio::spawn(async move {
                            let peer_addr = conn.peer_addr().ok();
                            let (server_chan, client_chan, _) = spawn_twoway(conn);
                            let rpc_client = SidecarProxyRpcClient::new(
                                tarpc::client::Config::default(),
                                client_chan,
                            )
                            .spawn();
                            let rpc_server = SidecarProxyManagerRpcServer::new(
                                manager.clone(),
                                rpc_client,
                                peer_addr,
                            );
                            let server_clone = rpc_server.clone();
                            BaseChannel::with_defaults(server_chan)
                                .execute(server_clone.serve())
                                .for_each(|resp| async move {
                                    tokio::spawn(resp);
                                })
                                .await;
                            rpc_server.handle_disconnect().await;
                        });
                    }
                }
                error!("TCP connection stopped");
            });
        }

        Ok(m)
    }

    fn next_generation(&self) -> u64 {
        self.generation_counter.fetch_add(1, Ordering::SeqCst)
    }

    pub async fn register_sidecar(
        &self,
        environment_id: Uuid,
        workload_id: Uuid,
        client: SidecarProxyRpcClient,
    ) -> u64 {
        let mut map = self.sidecar_proxies.write().await;
        let generation = self.next_generation();
        map.entry(environment_id)
            .or_default()
            .entry(workload_id)
            .or_insert_with(BTreeMap::new)
            .insert(generation, client);
        generation
    }

    pub async fn remove_sidecar(&self, environment_id: Uuid, workload_id: Uuid, generation: u64) {
        let mut map = self.sidecar_proxies.write().await;
        if let Some(workloads) = map.get_mut(&environment_id) {
            if let Some(entries) = workloads.get_mut(&workload_id) {
                let removed = entries.remove(&generation).is_some();
                if entries.is_empty() {
                    workloads.remove(&workload_id);
                }
                if removed && workloads.is_empty() {
                    map.remove(&environment_id);
                }
            }
        }
    }

    pub async fn latest_clients_for_environment(
        &self,
        environment_id: Uuid,
    ) -> Option<HashMap<Uuid, SidecarProxyRpcClient>> {
        let map = self.sidecar_proxies.read().await;
        map.get(&environment_id).map(|workloads| {
            workloads
                .iter()
                .filter_map(|(workload_id, entries)| {
                    entries
                        .iter()
                        .next_back()
                        .map(|(_, client)| (*workload_id, client.clone()))
                })
                .collect()
        })
    }

    pub async fn record_sidecar_heartbeat(&self, environment_id: Uuid, workload_id: Uuid) -> bool {
        let map = self.sidecar_proxies.read().await;
        map.get(&environment_id)
            .and_then(|workloads| workloads.get(&workload_id))
            .map(|entries| !entries.is_empty())
            .unwrap_or(false)
    }

    pub async fn set_service_routes_if_registered(&self, environment_id: Uuid) -> Result<()> {
        let Some(connections) = self.latest_clients_for_environment(environment_id).await else {
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
        self.snapshot_devbox_routes_for_environment(environment_id, routes.clone())
            .await;

        let Some(connections) = self.latest_clients_for_environment(environment_id).await else {
            warn!(
                "No sidecar proxy registered for environment {} when sending devbox routes",
                environment_id
            );
            return Ok(());
        };

        for (workload_id, client) in connections {
            let client = client.clone();
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

    pub async fn set_devbox_route(
        &self,
        environment_id: Uuid,
        route: DevboxRouteConfig,
    ) -> Result<(), String> {
        self.upsert_devbox_route_snapshot(environment_id, route.clone())
            .await;

        let Some(connections) = self.latest_clients_for_environment(environment_id).await else {
            warn!(
                "No sidecar proxy registered for environment {} when sending devbox route",
                environment_id
            );
            return Ok(());
        };

        let Some(client) = connections.get(&route.workload_id) else {
            warn!(
                "No sidecar proxy registered for workload {} when sending devbox route",
                route.workload_id
            );
            return Ok(());
        };

        let workload_id = route.workload_id;

        if let Err(err) = client
            .clone()
            .set_devbox_route(tarpc::context::current(), route)
            .await
        {
            return Err(format!(
                "Failed to send devbox route to workload {}: {}",
                workload_id, err
            ));
        }

        Ok(())
    }

    pub async fn remove_devbox_route(
        &self,
        environment_id: Uuid,
        workload_id: Uuid,
        branch_environment_id: Option<Uuid>,
    ) -> Result<(), String> {
        let removed = self
            .remove_devbox_route_snapshot(
                environment_id,
                workload_id,
                branch_environment_id.clone(),
            )
            .await;

        if !removed {
            return Ok(());
        }

        let Some(connections) = self.latest_clients_for_environment(environment_id).await else {
            return Ok(());
        };

        let Some(client) = connections.get(&workload_id) else {
            return Ok(());
        };

        if let Err(err) = client
            .clone()
            .stop_devbox(tarpc::context::current(), branch_environment_id)
            .await
        {
            return Err(format!(
                "Failed to clear devbox route for workload {}: {}",
                workload_id, err
            ));
        }

        Ok(())
    }

    pub async fn upsert_branch_service_route(
        &self,
        base_environment_id: Uuid,
        workload_id: Uuid,
        route: ProxyBranchRouteConfig,
    ) -> Result<(), String> {
        let Some(connections) = self
            .latest_clients_for_environment(base_environment_id)
            .await
        else {
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
        let Some(connections) = self
            .latest_clients_for_environment(base_environment_id)
            .await
        else {
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
        self.prune_devbox_route_snapshot(environment_id, branch_environment_id)
            .await;

        let Some(connections) = self.latest_clients_for_environment(environment_id).await else {
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

    pub async fn request_direct_config(
        &self,
        environment_id: Uuid,
        stun_observed_addr: Option<SocketAddr>,
    ) -> Result<Option<DirectTunnelConfig>, String> {
        let cluster_client = {
            let guard = self.kube_cluster_rpc_client.read().await;
            guard.clone()
        };

        let Some(client) = cluster_client else {
            return Err("Cluster RPC client unavailable; cannot request direct config".to_string());
        };

        match client
            .request_direct_config(
                tarpc::context::current(),
                environment_id,
                stun_observed_addr,
            )
            .await
        {
            Ok(Ok(config)) => Ok(config),
            Ok(Err(err)) => Err(err),
            Err(err) => Err(format!(
                "Failed to request direct config from KubeCluster: {}",
                err
            )),
        }
    }

    async fn snapshot_devbox_routes_for_environment(
        &self,
        environment_id: Uuid,
        routes: HashMap<Uuid, DevboxRouteConfig>,
    ) {
        let mut guard = self.devbox_route_snapshots.write().await;
        guard.insert(environment_id, routes);
    }

    async fn upsert_devbox_route_snapshot(&self, environment_id: Uuid, route: DevboxRouteConfig) {
        let mut guard = self.devbox_route_snapshots.write().await;
        guard
            .entry(environment_id)
            .or_default()
            .insert(route.workload_id, route);
    }

    async fn remove_devbox_route_snapshot(
        &self,
        environment_id: Uuid,
        workload_id: Uuid,
        branch_environment_id: Option<Uuid>,
    ) -> bool {
        let mut guard = self.devbox_route_snapshots.write().await;
        let Some(routes) = guard.get_mut(&environment_id) else {
            return false;
        };

        let should_remove = routes
            .get(&workload_id)
            .map(|route| route.branch_environment_id == branch_environment_id)
            .unwrap_or(false);

        if should_remove {
            routes.remove(&workload_id);
        }

        if routes.is_empty() {
            guard.remove(&environment_id);
        }

        should_remove
    }

    async fn devbox_route_for_workload(
        &self,
        environment_id: Uuid,
        workload_id: Uuid,
    ) -> Option<DevboxRouteConfig> {
        let guard = self.devbox_route_snapshots.read().await;
        guard
            .get(&environment_id)
            .and_then(|routes| routes.get(&workload_id).cloned())
    }

    async fn prune_devbox_route_snapshot(
        &self,
        environment_id: Uuid,
        branch_environment_id: Option<Uuid>,
    ) {
        let mut guard = self.devbox_route_snapshots.write().await;
        let Some(routes) = guard.get_mut(&environment_id) else {
            return;
        };

        match branch_environment_id {
            Some(branch_id) => {
                routes.retain(|_, route| route.branch_environment_id != Some(branch_id));
            }
            None => {
                routes.retain(|_, route| route.branch_environment_id.is_some());
            }
        }

        if routes.is_empty() {
            guard.remove(&environment_id);
        }
    }

    pub async fn replay_devbox_route(
        &self,
        environment_id: Uuid,
        workload_id: Uuid,
    ) -> Result<(), String> {
        let Some(route) = self
            .devbox_route_for_workload(environment_id, workload_id)
            .await
        else {
            return Ok(());
        };

        let Some(connections) = self.latest_clients_for_environment(environment_id).await else {
            return Ok(());
        };

        let Some(client) = connections.get(&workload_id) else {
            return Ok(());
        };

        client
            .clone()
            .set_devbox_route(tarpc::context::current(), route)
            .await
            .map_err(|e| {
                format!(
                    "Failed to replay devbox route for workload {} (RPC error): {}",
                    workload_id, e
                )
            })?
            .map_err(|e| {
                format!(
                    "Failed to replay devbox route for workload {}: {}",
                    workload_id, e
                )
            })
    }
}
