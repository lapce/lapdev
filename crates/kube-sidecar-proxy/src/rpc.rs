use anyhow::Result;
use lapdev_kube_rpc::{
    DevboxRouteConfig, ProxyBranchRouteConfig, ProxyRouteAccessLevel, SidecarProxyManagerRpcClient,
    SidecarProxyRpc,
};
use lapdev_tunnel::direct::DirectEndpoint;
use std::{collections::HashSet, sync::Arc};
use tokio::sync::RwLock;
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    config::{
        AccessLevel, BranchServiceRoute, DevboxConnection, DevboxRouteMetadata, RoutingTable,
    },
    connection_registry::ConnectionRegistry,
};

#[derive(Clone)]
pub(crate) struct SidecarProxyRpcServer {
    workload_id: Uuid,
    environment_id: Uuid,
    namespace: String,
    rpc_client: SidecarProxyManagerRpcClient,
    routing_table: Arc<RwLock<RoutingTable>>,
    connection_registry: Arc<ConnectionRegistry>,
    direct_endpoint: Arc<DirectEndpoint>,
}

impl SidecarProxyRpcServer {
    pub(crate) fn new(
        workload_id: Uuid,
        environment_id: Uuid,
        namespace: String,
        rpc_client: SidecarProxyManagerRpcClient,
        routing_table: Arc<RwLock<RoutingTable>>,
        connection_registry: Arc<ConnectionRegistry>,
        direct_endpoint: Arc<DirectEndpoint>,
    ) -> Self {
        Self {
            workload_id,
            environment_id,
            namespace,
            rpc_client,
            routing_table,
            connection_registry,
            direct_endpoint,
        }
    }

    pub(crate) fn manager_client(&self) -> SidecarProxyManagerRpcClient {
        self.rpc_client.clone()
    }

    pub(crate) async fn register_sidecar_proxy(&self) -> Result<()> {
        let _ = self
            .rpc_client
            .register_sidecar_proxy(
                tarpc::context::current(),
                self.workload_id,
                self.environment_id,
                self.namespace.clone(),
            )
            .await?;
        Ok(())
    }
}

impl SidecarProxyRpc for SidecarProxyRpcServer {
    async fn heartbeat(self, _context: ::tarpc::context::Context) -> Result<(), String> {
        // TODO: Implement heartbeat logic
        // - Report health status
        // - Update last_seen timestamp
        // - Return metrics
        Ok(())
    }

    async fn set_service_routes(
        self,
        _context: ::tarpc::context::Context,
        routes: Vec<ProxyBranchRouteConfig>,
    ) -> Result<(), String> {
        let mut routing_table = self.routing_table.write().await;
        let previous_branches: HashSet<Uuid> =
            routing_table.branch_routes.keys().copied().collect();
        let mut updates: Vec<(Uuid, BranchServiceRoute)> = Vec::new();
        let mut devbox_overrides: Vec<(Uuid, Arc<DevboxConnection>)> = Vec::new();

        for route in &routes {
            let branch_id = route.branch_environment_id;

            if !route.service_names.is_empty() {
                let service_route = BranchServiceRoute {
                    service_names: route.service_names.clone(),
                    headers: route.headers.clone(),
                    requires_auth: route.requires_auth,
                    access_level: access_level_from_proxy(route.access_level),
                    timeout_ms: route.timeout_ms,
                };
                updates.push((branch_id, service_route));
            }

            if let Some(devbox) = route.devbox_route.clone() {
                let connection = devbox_connection_from_config(
                    devbox,
                    Arc::clone(&self.direct_endpoint),
                    self.rpc_client.clone(),
                    branch_id,
                );
                devbox_overrides.push((branch_id, connection));
            }
        }

        routing_table.replace_branch_routes(updates).await;

        for (branch_id, devbox) in devbox_overrides {
            if !routing_table.set_branch_devbox(&branch_id, devbox) {
                warn!(
                    "Received devbox override for unknown branch environment {}",
                    branch_id
                );
            }
        }

        info!(
            "Updated branch routes; total routes: {}",
            routing_table.branch_routes.len()
        );

        let mut updated_branches: HashSet<Uuid> = routes
            .iter()
            .map(|route| route.branch_environment_id)
            .collect();
        for branch in previous_branches {
            if !updated_branches.contains(&branch) {
                updated_branches.insert(branch);
            }
        }
        drop(routing_table);
        self.connection_registry
            .shutdown_branches(updated_branches.into_iter().map(Some))
            .await;

        Ok(())
    }

    async fn set_devbox_route(
        self,
        _context: ::tarpc::context::Context,
        route: DevboxRouteConfig,
    ) -> Result<(), String> {
        if route.workload_id != self.workload_id {
            warn!(
                "Ignoring devbox route for workload {} on sidecar workload {}",
                route.workload_id, self.workload_id
            );
            return Ok(());
        }

        let target_environment_id = route.branch_environment_id.unwrap_or(self.environment_id);
        let devbox_connection = devbox_connection_from_config(
            route.clone(),
            Arc::clone(&self.direct_endpoint),
            self.rpc_client.clone(),
            target_environment_id,
        );

        let mut routing_table = self.routing_table.write().await;

        match route.branch_environment_id {
            Some(branch_id) => {
                if routing_table.set_branch_devbox(&branch_id, devbox_connection.clone()) {
                    info!(
                        "Attached devbox route to branch {} (intercept_id={})",
                        branch_id, route.intercept_id
                    );
                } else {
                    warn!(
                        "Received branch devbox route for unknown environment {}",
                        branch_id
                    );
                }
            }
            None => {
                routing_table.set_default_devbox(devbox_connection.clone());
                info!(
                    "Registered default devbox route (intercept_id={})",
                    route.intercept_id
                );
            }
        }

        drop(routing_table);
        let key = route.branch_environment_id;
        self.connection_registry.shutdown_branch(key).await;

        Ok(())
    }

    async fn stop_devbox(
        self,
        _context: ::tarpc::context::Context,
        branch_environment: Option<Uuid>,
    ) -> Result<(), String> {
        let mut routing_table = self.routing_table.write().await;

        match branch_environment {
            Some(branch_id) => {
                if routing_table.remove_branch_devbox(&branch_id) {
                    info!(
                        "Cleared devbox route for branch environment {} on workload {}",
                        branch_id, self.workload_id
                    );
                } else {
                    info!(
                        "No devbox route to clear for branch environment {} on workload {}",
                        branch_id, self.workload_id
                    );
                }
            }
            None => {
                routing_table.clear_default_devbox();
                info!(
                    "Cleared default devbox route for workload {}",
                    self.workload_id
                );
            }
        }

        drop(routing_table);
        self.connection_registry
            .shutdown_branch(branch_environment)
            .await;

        Ok(())
    }

    async fn upsert_branch_service_route(
        self,
        _context: ::tarpc::context::Context,
        route: ProxyBranchRouteConfig,
    ) -> Result<(), String> {
        let branch_id = route.branch_environment_id;
        if route.service_names.is_empty() {
            return Err("Missing service_names for branch service route update".to_string());
        }

        let service_route = BranchServiceRoute {
            service_names: route.service_names.clone(),
            headers: route.headers.clone(),
            requires_auth: route.requires_auth,
            access_level: access_level_from_proxy(route.access_level),
            timeout_ms: route.timeout_ms,
        };

        let devbox_override = route.devbox_route.map(|devbox| {
            devbox_connection_from_config(
                devbox,
                Arc::clone(&self.direct_endpoint),
                self.rpc_client.clone(),
                branch_id,
            )
        });

        let mut routing_table = self.routing_table.write().await;
        routing_table
            .upsert_branch_service_route(branch_id, service_route)
            .await;

        if let Some(connection) = devbox_override {
            routing_table.set_branch_devbox(&branch_id, connection);
        }

        info!(
            "Updated branch service route for branch {} on workload {}",
            branch_id, self.workload_id
        );

        drop(routing_table);
        self.connection_registry
            .shutdown_branch(Some(branch_id))
            .await;

        Ok(())
    }

    async fn remove_branch_service_route(
        self,
        _context: ::tarpc::context::Context,
        branch_environment_id: Uuid,
    ) -> Result<(), String> {
        let mut routing_table = self.routing_table.write().await;
        if routing_table
            .remove_branch_service_route(&branch_environment_id)
            .await
        {
            info!(
                "Removed branch service route for branch {} on workload {}",
                branch_environment_id, self.workload_id
            );
        } else {
            info!(
                "No branch service route found for branch {} on workload {}",
                branch_environment_id, self.workload_id
            );
        }

        drop(routing_table);
        self.connection_registry
            .shutdown_branch(Some(branch_environment_id))
            .await;

        Ok(())
    }
}

fn devbox_connection_from_config(
    route: DevboxRouteConfig,
    direct_endpoint: Arc<DirectEndpoint>,
    rpc_client: SidecarProxyManagerRpcClient,
    target_environment_id: Uuid,
) -> Arc<DevboxConnection> {
    Arc::new(DevboxConnection::new(
        DevboxRouteMetadata {
            intercept_id: route.intercept_id,
            workload_id: route.workload_id,
            auth_token: route.auth_token,
            websocket_url: route.websocket_url,
            path_pattern: route.path_pattern,
            port_mappings: route.port_mappings,
            created_at_epoch_seconds: route.created_at_epoch_seconds,
            expires_at_epoch_seconds: route.expires_at_epoch_seconds,
        },
        direct_endpoint,
        rpc_client,
        target_environment_id,
    ))
}

fn access_level_from_proxy(level: ProxyRouteAccessLevel) -> AccessLevel {
    match level {
        ProxyRouteAccessLevel::Personal => AccessLevel::Personal,
        ProxyRouteAccessLevel::Shared => AccessLevel::Shared,
        ProxyRouteAccessLevel::Public => AccessLevel::Public,
    }
}
