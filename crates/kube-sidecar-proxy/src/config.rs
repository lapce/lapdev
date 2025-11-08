use http::header::HeaderName;
use lapdev_common::kube::{ProxyPortRoute, KUBE_ENVIRONMENT_TOKEN_HEADER_LOWER};
use lapdev_tunnel::{
    TunnelClient, TunnelError, TunnelTcpStream, WebSocketTransport as TunnelWebSocketTransport,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, io, net::SocketAddr, sync::Arc};
use tokio::sync::{OnceCell, RwLock};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use uuid::Uuid;

/// Immutable settings for the sidecar proxy determined at boot time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarSettings {
    /// Address the sidecar listens on for iptables redirected traffic.
    pub listen_addr: SocketAddr,
    /// Namespace the sidecar is running in.
    pub namespace: Option<String>,
    /// Pod name for identification/logging.
    pub pod_name: Option<String>,
    /// Lapdev environment identifier for this workload.
    pub environment_id: Uuid,
    /// Lapdev environment scoped auth token used for RPC calls.
    pub environment_auth_token: String,
    /// Health check configuration for readiness/liveness.
    pub health_check: HealthCheckConfig,
    /// Metrics export configuration.
    pub metrics: MetricsConfig,
}

impl SidecarSettings {
    pub fn new(
        listen_addr: SocketAddr,
        namespace: Option<String>,
        pod_name: Option<String>,
        environment_id: Uuid,
        environment_auth_token: String,
    ) -> Self {
        Self {
            listen_addr,
            namespace,
            pod_name,
            environment_id,
            environment_auth_token,
            health_check: HealthCheckConfig::default(),
            metrics: MetricsConfig::default(),
        }
    }
}

/// Mutable routing state shared between RPC handlers and the proxy loop.
pub struct RoutingTable {
    pub branch_routes: HashMap<Uuid, BranchRoute>,
    pub default_route: DefaultRoute,
    pub port_routes: HashMap<u16, ProxyPortRoute>,
}

impl Default for RoutingTable {
    fn default() -> Self {
        Self {
            branch_routes: HashMap::new(),
            default_route: DefaultRoute::default(),
            port_routes: HashMap::new(),
        }
    }
}

impl RoutingTable {
    pub fn resolve_http(&self, port: u16, branch_id: Option<Uuid>) -> RouteDecision {
        if let Some(branch_id) = branch_id {
            if let Some(route) = self.branch_routes.get(&branch_id) {
                match &route.mode {
                    BranchMode::Devbox(connection) => {
                        return RouteDecision::BranchDevbox {
                            connection: connection.clone(),
                            target_port: connection.resolve_target_port(port),
                        };
                    }
                    BranchMode::Service => {
                        return RouteDecision::BranchService {
                            service: route.service.clone(),
                        };
                    }
                }
            }
        }

        if let DefaultRoute::Devbox(connection) = &self.default_route {
            return RouteDecision::DefaultDevbox {
                connection: connection.clone(),
                target_port: connection.resolve_target_port(port),
            };
        }

        RouteDecision::DefaultLocal {
            target_port: self.target_port_for_service(port),
        }
    }

    pub fn resolve_tcp(&self, port: u16) -> RouteDecision {
        if let DefaultRoute::Devbox(connection) = &self.default_route {
            return RouteDecision::DefaultDevbox {
                connection: connection.clone(),
                target_port: connection.resolve_target_port(port),
            };
        }

        RouteDecision::DefaultLocal {
            target_port: self.target_port_for_service(port),
        }
    }

    pub async fn replace_branch_routes(
        &mut self,
        routes: impl IntoIterator<Item = (Uuid, BranchServiceRoute)>,
    ) {
        let mut old_routes = std::mem::take(&mut self.branch_routes);
        let mut new_routes = HashMap::new();

        for (env_id, service_route) in routes.into_iter() {
            if let Some(existing) = old_routes.remove(&env_id) {
                let mode = existing.mode;
                new_routes.insert(
                    env_id,
                    BranchRoute {
                        service: service_route,
                        mode,
                    },
                );
            } else {
                new_routes.insert(
                    env_id,
                    BranchRoute {
                        service: service_route,
                        mode: BranchMode::Service,
                    },
                );
            }
        }

        self.branch_routes = new_routes;
    }

    pub fn set_branch_devbox(
        &mut self,
        branch_id: &Uuid,
        connection: Arc<DevboxConnection>,
    ) -> bool {
        let Some(entry) = self.branch_routes.get_mut(branch_id) else {
            return false;
        };

        entry.mode = BranchMode::Devbox(connection);
        true
    }

    pub fn clear_branch_devboxes(&mut self) {
        for route in self.branch_routes.values_mut() {
            if matches!(route.mode, BranchMode::Devbox(_)) {
                route.mode = BranchMode::Service;
            }
        }
    }

    pub fn remove_branch_devbox(&mut self, branch_id: &Uuid) -> bool {
        let Some(route) = self.branch_routes.get_mut(branch_id) else {
            return false;
        };

        if matches!(route.mode, BranchMode::Devbox(_)) {
            route.mode = BranchMode::Service;
            true
        } else {
            false
        }
    }

    pub async fn upsert_branch_service_route(
        &mut self,
        branch_id: Uuid,
        service_route: BranchServiceRoute,
    ) {
        if let Some(existing) = self.branch_routes.remove(&branch_id) {
            let mode = existing.mode;
            self.branch_routes.insert(
                branch_id,
                BranchRoute {
                    service: service_route,
                    mode,
                },
            );
        } else {
            self.branch_routes.insert(
                branch_id,
                BranchRoute {
                    service: service_route,
                    mode: BranchMode::Service,
                },
            );
        }
    }

    pub async fn remove_branch_service_route(&mut self, branch_id: &Uuid) -> bool {
        self.branch_routes.remove(branch_id).is_some()
    }

    pub fn remove_branch_devbox_by_intercept(&mut self, intercept_id: &Uuid) -> Option<Uuid> {
        for (branch_id, route) in self.branch_routes.iter_mut() {
            if let BranchMode::Devbox(connection) = &route.mode {
                if connection.metadata().intercept_id == *intercept_id {
                    route.mode = BranchMode::Service;
                    return Some(*branch_id);
                }
            }
        }
        None
    }

    pub fn set_default_devbox(&mut self, connection: Arc<DevboxConnection>) {
        self.default_route = DefaultRoute::Devbox(connection);
    }

    pub fn clear_default_devbox(&mut self) {
        self.default_route = DefaultRoute::Local;
    }

    pub fn default_devbox(&self) -> Option<Arc<DevboxConnection>> {
        match &self.default_route {
            DefaultRoute::Devbox(connection) => Some(connection.clone()),
            DefaultRoute::Local => None,
        }
    }

    pub fn remove_default_devbox_by_intercept(&mut self, intercept_id: &Uuid) -> bool {
        match &self.default_route {
            DefaultRoute::Devbox(connection)
                if connection.metadata().intercept_id == *intercept_id =>
            {
                self.default_route = DefaultRoute::Local;
                true
            }
            _ => false,
        }
    }

    pub fn set_port_routes(
        &mut self,
        routes: impl IntoIterator<Item = ProxyPortRoute>,
    ) -> &mut Self {
        self.port_routes.clear();
        for route in routes {
            self.port_routes.insert(route.proxy_port, route);
        }
        self
    }

    pub fn upsert_port_route(&mut self, route: ProxyPortRoute) -> Option<ProxyPortRoute> {
        self.port_routes.insert(route.proxy_port, route)
    }

    pub fn remove_port_route(&mut self, proxy_port: u16) -> Option<ProxyPortRoute> {
        self.port_routes.remove(&proxy_port)
    }

    pub fn port_route(&self, proxy_port: u16) -> Option<&ProxyPortRoute> {
        self.port_routes.get(&proxy_port)
    }

    pub fn port_routes(&self) -> impl Iterator<Item = &ProxyPortRoute> {
        self.port_routes.values()
    }

    pub fn service_port_for_proxy(&self, proxy_port: u16) -> u16 {
        self.port_routes
            .get(&proxy_port)
            .map(|route| route.service_port)
            .unwrap_or(proxy_port)
    }

    pub fn target_port_for_service(&self, service_port: u16) -> u16 {
        self.port_routes
            .values()
            .find(|route| route.service_port == service_port)
            .map(|route| route.target_port)
            .unwrap_or(service_port)
    }
}

#[derive(Clone)]
pub struct BranchRoute {
    pub service: BranchServiceRoute,
    pub mode: BranchMode,
}

#[derive(Clone)]
pub struct BranchServiceRoute {
    pub service_names: HashMap<u16, String>,
    pub headers: HashMap<String, String>,
    pub requires_auth: bool,
    pub access_level: AccessLevel,
    pub timeout_ms: Option<u64>,
}

impl BranchServiceRoute {
    pub fn new_service(port: u16, service_name: String) -> Self {
        let mut service_names = HashMap::new();
        service_names.insert(port, service_name);
        Self {
            service_names,
            headers: HashMap::new(),
            requires_auth: true,
            access_level: AccessLevel::Personal,
            timeout_ms: None,
        }
    }

    pub fn service_name_for_port(&self, port: u16) -> Option<&str> {
        self.service_names.get(&port).map(String::as_str)
    }
}

#[derive(Clone)]
pub enum BranchMode {
    Service,
    Devbox(Arc<DevboxConnection>),
}

pub enum DefaultRoute {
    Local,
    Devbox(Arc<DevboxConnection>),
}

impl Default for DefaultRoute {
    fn default() -> Self {
        DefaultRoute::Local
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevboxRouteMetadata {
    pub intercept_id: Uuid,
    pub workload_id: Uuid,
    pub auth_token: String,
    pub websocket_url: String,
    pub path_pattern: String,
    pub port_mappings: HashMap<u16, u16>,
    pub created_at_epoch_seconds: Option<i64>,
    pub expires_at_epoch_seconds: Option<i64>,
}

impl DevboxRouteMetadata {
    pub fn resolve_target_port(&self, original_port: u16) -> u16 {
        self.port_mappings
            .get(&original_port)
            .copied()
            .unwrap_or(original_port)
    }
}

pub struct DevboxConnection {
    metadata: DevboxRouteMetadata,
    client: RwLock<OnceCell<Arc<TunnelClient>>>,
}

impl DevboxConnection {
    pub fn new(metadata: DevboxRouteMetadata) -> Self {
        Self {
            metadata,
            client: RwLock::new(OnceCell::new()),
        }
    }

    pub fn metadata(&self) -> &DevboxRouteMetadata {
        &self.metadata
    }

    pub fn resolve_target_port(&self, original_port: u16) -> u16 {
        self.metadata.resolve_target_port(original_port)
    }

    pub async fn connect_tcp_stream(
        &self,
        target_host: &str,
        target_port: u16,
    ) -> io::Result<TunnelTcpStream> {
        let client = self.ensure_client().await.map_err(io::Error::from)?;

        match client
            .connect_tcp(target_host.to_string(), target_port)
            .await
        {
            Ok(stream) => Ok(stream),
            Err(TunnelError::ConnectionClosed) => {
                self.clear_client().await;
                let client = self.ensure_client().await.map_err(io::Error::from)?;
                client
                    .connect_tcp(target_host.to_string(), target_port)
                    .await
                    .map_err(io::Error::from)
            }
            Err(err) => Err(io::Error::from(err)),
        }
    }

    pub async fn clear_client(&self) {
        self.client.write().await.take();
    }

    async fn ensure_client(&self) -> Result<Arc<TunnelClient>, TunnelError> {
        if let Some(client) = self.client.read().await.get() {
            return Ok(client.clone());
        }

        let client = self
            .client
            .read()
            .await
            .get_or_try_init(|| async {
                let client = self.create_client().await?;
                Ok::<Arc<TunnelClient>, TunnelError>(Arc::new(client))
            })
            .await?
            .clone();

        Ok(client.clone())
    }

    async fn create_client(&self) -> Result<TunnelClient, TunnelError> {
        let mut request = self
            .metadata
            .websocket_url
            .clone()
            .into_client_request()
            .map_err(tunnel_transport_error)?;

        let env_header_value = self
            .metadata
            .auth_token
            .parse()
            .map_err(tunnel_transport_error)?;
        request.headers_mut().insert(
            HeaderName::from_static(KUBE_ENVIRONMENT_TOKEN_HEADER_LOWER),
            env_header_value,
        );

        let (stream, _) = connect_async(request)
            .await
            .map_err(tunnel_transport_error)?;

        let transport = TunnelWebSocketTransport::new(stream);
        Ok(TunnelClient::connect(transport))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AccessLevel {
    /// Only accessible by the owner with authentication
    Personal,
    /// Accessible by organization members with authentication
    Shared,
    /// Accessible by anyone without authentication
    Public,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckConfig {
    pub path: String,
    pub interval_seconds: u64,
    pub timeout_ms: u64,
    pub failure_threshold: u32,
}

impl Default for HealthCheckConfig {
    fn default() -> Self {
        Self {
            path: "/health".to_string(),
            interval_seconds: 30,
            timeout_ms: 5000,
            failure_threshold: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsConfig {
    pub enabled: bool,
    pub path: String,
    pub port: Option<u16>,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: "/metrics".to_string(),
            port: None,
        }
    }
}

pub enum RouteDecision {
    BranchService {
        service: BranchServiceRoute,
    },
    BranchDevbox {
        connection: Arc<DevboxConnection>,
        target_port: u16,
    },
    DefaultDevbox {
        connection: Arc<DevboxConnection>,
        target_port: u16,
    },
    DefaultLocal {
        target_port: u16,
    },
}

/// Kubernetes annotations used for configuration
pub struct ProxyAnnotations;

impl ProxyAnnotations {
    /// Annotation for specifying target service
    pub const TARGET_SERVICE: &'static str = "lapdev.io/proxy-target-service";

    /// Annotation for specifying target port
    pub const TARGET_PORT: &'static str = "lapdev.io/proxy-target-port";

    /// Annotation for routing rules (JSON)
    pub const ROUTING_RULES: &'static str = "lapdev.io/proxy-routing-rules";

    /// Annotation for access level
    pub const ACCESS_LEVEL: &'static str = "lapdev.io/proxy-access-level";

    /// Annotation for enabling/disabling authentication
    pub const REQUIRE_AUTH: &'static str = "lapdev.io/proxy-require-auth";

    /// Annotation for custom headers (JSON)
    pub const CUSTOM_HEADERS: &'static str = "lapdev.io/proxy-custom-headers";

    /// Annotation for timeout in milliseconds
    pub const TIMEOUT_MS: &'static str = "lapdev.io/proxy-timeout-ms";
}

fn tunnel_transport_error<E>(err: E) -> TunnelError
where
    E: std::fmt::Display,
{
    TunnelError::Transport(io::Error::new(io::ErrorKind::Other, err.to_string()))
}
