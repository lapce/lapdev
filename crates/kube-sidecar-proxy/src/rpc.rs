use anyhow::Result;
use lapdev_kube_rpc::{
    DevboxRouteConfig, ProxyRouteConfig, SidecarProxyManagerRpcClient, SidecarProxyRpc,
};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::{AccessLevel, ProxyConfig, RouteConfig, RouteTarget};

#[derive(Clone)]
pub(crate) struct SidecarProxyRpcServer {
    workload_id: Uuid,
    environment_id: Uuid,
    namespace: String,
    rpc_client: SidecarProxyManagerRpcClient,
    config: Arc<RwLock<ProxyConfig>>,
}

impl SidecarProxyRpcServer {
    pub(crate) fn new(
        workload_id: Uuid,
        environment_id: Uuid,
        namespace: String,
        rpc_client: SidecarProxyManagerRpcClient,
        config: Arc<RwLock<ProxyConfig>>,
    ) -> Self {
        Self {
            workload_id,
            environment_id,
            namespace,
            rpc_client,
            config,
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
        routes: Vec<ProxyRouteConfig>,
    ) -> Result<(), String> {
        let mut config = self.config.write().await;

        // Retain existing DevboxTunnel routes, remove all other service routes
        config
            .routes
            .retain(|route| matches!(route.target, RouteTarget::DevboxTunnel { .. }));

        for route in routes {
            config.routes.push(RouteConfig {
                path: route.path.clone(),
                target: RouteTarget::Service {
                    name: route.service_name.clone(),
                    port: route.port,
                },
                branch_environment_id: route.branch_environment_id,
                headers: std::collections::HashMap::new(),
                timeout_ms: None,
                requires_auth: true,
                access_level: AccessLevel::Personal,
            });
        }

        info!(
            "Updated service routes; total routes: {}",
            config.routes.len()
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

        let mut config = self.config.write().await;

        // Check if route already exists
        for existing_route in &config.routes {
            if let RouteTarget::DevboxTunnel { intercept_id, .. } = &existing_route.target {
                if intercept_id == &route.intercept_id {
                    warn!(
                        "Devbox route for intercept_id={} already exists, replacing",
                        route.intercept_id
                    );
                    // Remove old route before adding new one
                    config.routes.retain(|r| {
                        if let RouteTarget::DevboxTunnel { intercept_id, .. } = &r.target {
                            intercept_id != &route.intercept_id
                        } else {
                            true
                        }
                    });
                    break;
                }
            }
        }

        // Add new DevboxTunnel route
        let new_route = RouteConfig {
            path: route.path_pattern.clone(),
            target: RouteTarget::DevboxTunnel {
                intercept_id: route.intercept_id,
                session_id: route.session_id,
                target_port: route.target_port,
                auth_token: route.auth_token,
            },
            branch_environment_id: None,
            headers: std::collections::HashMap::new(),
            timeout_ms: Some(30000),
            requires_auth: true,
            access_level: AccessLevel::Personal,
        };

        config.routes.push(new_route);

        info!(
            "Successfully added devbox route for intercept_id={}. Total routes: {}",
            route.intercept_id,
            config.routes.len()
        );

        Ok(true)
    }

    async fn remove_devbox_route(
        self,
        _context: ::tarpc::context::Context,
        intercept_id: Uuid,
    ) -> Result<bool, String> {
        info!("Removing devbox route: intercept_id={}", intercept_id);

        let mut config = self.config.write().await;
        let before_count = config.routes.len();

        // Remove routes matching the intercept_id
        config.routes.retain(|route| {
            if let RouteTarget::DevboxTunnel {
                intercept_id: id, ..
            } = &route.target
            {
                id != &intercept_id
            } else {
                true
            }
        });

        let after_count = config.routes.len();
        let removed = before_count > after_count;

        if removed {
            info!(
                "Successfully removed devbox route for intercept_id={}. Routes remaining: {}",
                intercept_id, after_count
            );
        } else {
            warn!(
                "No devbox route found for intercept_id={} to remove",
                intercept_id
            );
        }

        Ok(removed)
    }

    async fn list_devbox_routes(
        self,
        _context: ::tarpc::context::Context,
    ) -> Result<Vec<DevboxRouteConfig>, String> {
        let config = self.config.read().await;
        let mut routes = Vec::new();

        for route in &config.routes {
            if let RouteTarget::DevboxTunnel {
                intercept_id,
                session_id,
                target_port,
                auth_token,
            } = &route.target
            {
                routes.push(DevboxRouteConfig {
                    intercept_id: *intercept_id,
                    session_id: *session_id,
                    target_port: *target_port,
                    auth_token: auth_token.clone(),
                    path_pattern: route.path.clone(),
                });
            }
        }

        info!("Listed {} devbox routes", routes.len());
        Ok(routes)
    }
}
