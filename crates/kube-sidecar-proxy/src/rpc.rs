use anyhow::Result;
use lapdev_kube_rpc::{
    DevboxRouteConfig, ProxyBranchRouteConfig, ProxyRouteAccessLevel, SidecarProxyManagerRpcClient,
    SidecarProxyRpc,
};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::{AccessLevel, BranchMode, BranchServiceRoute, DevboxRoute, RoutingTable};

#[derive(Clone)]
pub(crate) struct SidecarProxyRpcServer {
    workload_id: Uuid,
    environment_id: Uuid,
    namespace: String,
    rpc_client: SidecarProxyManagerRpcClient,
    routing_table: Arc<RwLock<RoutingTable>>,
}

impl SidecarProxyRpcServer {
    pub(crate) fn new(
        workload_id: Uuid,
        environment_id: Uuid,
        namespace: String,
        rpc_client: SidecarProxyManagerRpcClient,
        routing_table: Arc<RwLock<RoutingTable>>,
    ) -> Self {
        Self {
            workload_id,
            environment_id,
            namespace,
            rpc_client,
            routing_table,
        }
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
        let mut updates: Vec<(Uuid, BranchServiceRoute)> = Vec::new();
        let mut devbox_overrides: Vec<(Uuid, DevboxRoute)> = Vec::new();

        for route in routes {
            let branch_id = route.branch_environment_id;

            if let Some(service_name) = route.service_name.clone() {
                let service_route = BranchServiceRoute {
                    service_name,
                    headers: route.headers.clone(),
                    requires_auth: route.requires_auth,
                    access_level: access_level_from_proxy(route.access_level),
                    timeout_ms: route.timeout_ms,
                };
                updates.push((branch_id, service_route));
            }

            if let Some(devbox) = route.devbox_route.clone() {
                devbox_overrides.push((branch_id, devbox_route_from_config(devbox)));
            }
        }

        routing_table.replace_branch_routes(updates);

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

        Ok(())
    }

    async fn add_devbox_route(
        self,
        _context: ::tarpc::context::Context,
        route: DevboxRouteConfig,
    ) -> Result<bool, String> {
        info!(
            "Adding devbox route: intercept_id={}, session_id={}, target_port={}, path={}",
            route.intercept_id, route.session_id, route.target_port, route.path_pattern
        );

        let mut routing_table = self.routing_table.write().await;
        let devbox_route = devbox_route_from_config(route.clone());

        if let Some(branch_id) = route.branch_environment_id {
            if routing_table.set_branch_devbox(&branch_id, devbox_route.clone()) {
                info!(
                    "Attached devbox route to branch {} (intercept_id={})",
                    branch_id, route.intercept_id
                );
            } else {
                warn!(
                    "Received branch devbox route for unknown environment {}",
                    branch_id
                );
                return Ok(false);
            }
        } else {
            routing_table.set_default_devbox(devbox_route.clone());
            info!(
                "Registered default devbox route (intercept_id={}, default_target_port={})",
                route.intercept_id, route.target_port
            );
        }

        Ok(true)
    }

    async fn remove_devbox_route(
        self,
        _context: ::tarpc::context::Context,
        intercept_id: Uuid,
    ) -> Result<bool, String> {
        info!("Removing devbox route: intercept_id={}", intercept_id);

        let mut routing_table = self.routing_table.write().await;

        if let Some(branch_id) = routing_table.remove_branch_devbox_by_intercept(&intercept_id) {
            info!(
                "Removed branch devbox route for environment {} (intercept_id={})",
                branch_id, intercept_id
            );
            return Ok(true);
        }

        if routing_table.remove_default_devbox_by_intercept(&intercept_id) {
            info!(
                "Removed default devbox route (intercept_id={})",
                intercept_id
            );
            return Ok(true);
        }

        warn!(
            "No devbox route found for intercept_id={} to remove",
            intercept_id
        );

        Ok(false)
    }

    async fn list_devbox_routes(
        self,
        _context: ::tarpc::context::Context,
    ) -> Result<Vec<DevboxRouteConfig>, String> {
        let routing_table = self.routing_table.read().await;
        let mut routes = Vec::new();

        if let Some(route) = routing_table.default_devbox() {
            routes.push(devbox_route_config_from_route(route, None));
        }

        for (branch_id, branch_route) in &routing_table.branch_routes {
            if let crate::config::BranchMode::Devbox(devbox) = &branch_route.mode {
                routes.push(devbox_route_config_from_route(devbox, Some(*branch_id)));
            }
        }

        info!("Listed {} devbox routes", routes.len());
        Ok(routes)
    }
}

fn devbox_route_from_config(route: DevboxRouteConfig) -> DevboxRoute {
    DevboxRoute {
        intercept_id: route.intercept_id,
        session_id: route.session_id,
        target_port: route.target_port,
        auth_token: route.auth_token,
        websocket_url: route.websocket_url,
        path_pattern: route.path_pattern,
        port_mappings: route.port_mappings,
        created_at_epoch_seconds: route.created_at_epoch_seconds,
        expires_at_epoch_seconds: route.expires_at_epoch_seconds,
    }
}

fn devbox_route_config_from_route(
    route: &DevboxRoute,
    branch_id: Option<Uuid>,
) -> DevboxRouteConfig {
    DevboxRouteConfig {
        intercept_id: route.intercept_id,
        session_id: route.session_id,
        target_port: route.target_port,
        auth_token: route.auth_token.clone(),
        websocket_url: route.websocket_url.clone(),
        path_pattern: route.path_pattern.clone(),
        branch_environment_id: branch_id,
        created_at_epoch_seconds: route.created_at_epoch_seconds,
        expires_at_epoch_seconds: route.expires_at_epoch_seconds,
        port_mappings: route.port_mappings.clone(),
    }
}

fn access_level_from_proxy(level: ProxyRouteAccessLevel) -> AccessLevel {
    match level {
        ProxyRouteAccessLevel::Personal => AccessLevel::Personal,
        ProxyRouteAccessLevel::Shared => AccessLevel::Shared,
        ProxyRouteAccessLevel::Public => AccessLevel::Public,
    }
}
