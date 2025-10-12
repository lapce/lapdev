use crate::{
    config::ProxyConfig,
    discovery::ServiceDiscovery,
    error::{Result, SidecarProxyError},
    original_dest::get_original_destination,
    otel_routing::{determine_routing_target, extract_routing_context},
    protocol_detector::{detect_protocol, ProtocolType},
    rpc::SidecarProxyRpcServer,
};
use anyhow::anyhow;
use futures::StreamExt;
use http::header::AUTHORIZATION;
use kube::Client;
use lapdev_common::kube::{SIDECAR_PROXY_MANAGER_ADDR_ENV_VAR, SIDECAR_PROXY_WORKLOAD_ENV_VAR};
use lapdev_kube_rpc::{http_parser, SidecarProxyManagerRpcClient, SidecarProxyRpc};
use lapdev_rpc::spawn_twoway;
use std::{collections::HashMap, io, net::SocketAddr, str::FromStr, sync::Arc};
use tarpc::server::{BaseChannel, Channel};
use tokio::{
    io::{copy_bidirectional, AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Mutex, RwLock},
};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tracing::{debug, error, info, warn};
use lapdev_tunnel::{
    TunnelClient, TunnelError, TunnelTcpStream, WebSocketTransport as TunnelWebSocketTransport,
};
use uuid::Uuid;

/// Main sidecar proxy server
#[derive(Clone)]
pub struct SidecarProxyServer {
    workload_id: Uuid,
    sidecar_proxy_manager_addr: String,
    listen_addr: SocketAddr,
    discovery: Arc<ServiceDiscovery>,
    config: Arc<RwLock<ProxyConfig>>,
    /// RPC client to kube-manager (None until connection established)
    rpc_client: Arc<RwLock<Option<SidecarProxyManagerRpcClient>>>,
    devbox_tunnel_manager: Arc<DevboxTunnelManager>,
}

impl SidecarProxyServer {
    /// Create a new sidecar proxy server
    pub async fn new(
        listen_addr: SocketAddr,
        default_target: SocketAddr,
        namespace: Option<String>,
        pod_name: Option<String>,
        environment_id: String,
        environment_auth_token: String,
    ) -> Result<Self> {
        let sidecar_proxy_manager_addr = std::env::var(SIDECAR_PROXY_MANAGER_ADDR_ENV_VAR)
            .map_err(|_| {
                anyhow!(format!(
                    "can't find {SIDECAR_PROXY_MANAGER_ADDR_ENV_VAR} env var"
                ))
            })?;

        let workload_id = std::env::var(SIDECAR_PROXY_WORKLOAD_ENV_VAR).map_err(|_| {
            anyhow!(format!(
                "can't find {SIDECAR_PROXY_WORKLOAD_ENV_VAR} env var"
            ))
        })?;
        let workload_id = Uuid::from_str(&workload_id).map_err(|_| {
            anyhow!(format!(
                "{SIDECAR_PROXY_MANAGER_ADDR_ENV_VAR} isn't a valid uuid"
            ))
        })?;

        // Parse environment ID - now required
        let env_id = Uuid::parse_str(&environment_id)
            .map_err(|e| anyhow!("Failed to parse environment_id as UUID: {}", e))?;

        // Create initial configuration
        let config = ProxyConfig {
            listen_addr,
            default_target,
            namespace: namespace.clone(),
            pod_name: pod_name.clone(),
            environment_id: Some(env_id),
            environment_auth_token: Some(environment_auth_token),
            ..Default::default()
        };

        // Initialize Kubernetes client
        let client = Client::try_default()
            .await
            .map_err(|e| SidecarProxyError::Kubernetes(e))?;

        // Create service discovery
        let discovery = Arc::new(ServiceDiscovery::new(
            client,
            namespace.clone(),
            pod_name.clone(),
            env_id,
            config.clone(),
        ));

        let config = Arc::new(RwLock::new(config));

        let server = Self {
            workload_id,
            sidecar_proxy_manager_addr,
            listen_addr,
            discovery,
            config,
            rpc_client: Arc::new(RwLock::new(None)),
            devbox_tunnel_manager: Arc::new(DevboxTunnelManager::new()),
        };

        Ok(server)
    }

    /// Start the server with graceful shutdown support
    pub async fn serve_with_graceful_shutdown<F>(self, shutdown_signal: F) -> Result<()>
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        // Start RPC connection to kube manager
        {
            let server = self.clone();
            tokio::spawn(async move {
                server.connect_rpc().await;
            });
        }

        // Start service discovery in the background
        let discovery_clone = Arc::clone(&self.discovery);
        let discovery_task = tokio::spawn(async move {
            if let Err(e) = discovery_clone.start_watching().await {
                error!("Service discovery error: {}", e);
            }
        });

        // Start configuration updates handler
        let config_clone = Arc::clone(&self.config);
        let mut config_rx = self.discovery.subscribe();
        let config_task = tokio::spawn(async move {
            while let Ok(new_config) = config_rx.recv().await {
                info!("Received configuration update");
                {
                    let mut config = config_clone.write().await;
                    *config = new_config;
                }
            }
        });

        // Create TCP listener for iptables-redirected connections
        let listener = TcpListener::bind(&self.listen_addr).await?;
        info!("Sidecar proxy listening on: {}", self.listen_addr);

        // Handle connections
        let config_for_server = Arc::clone(&self.config);
        let rpc_client_for_server = Arc::clone(&self.rpc_client);
        let tunnel_manager_for_server = Arc::clone(&self.devbox_tunnel_manager);
        let server = async move {
            loop {
                match listener.accept().await {
                    Ok((inbound_stream, client_addr)) => {
                        debug!("Accepted connection from {}", client_addr);
                        let config = Arc::clone(&config_for_server);
                        let rpc_client = Arc::clone(&rpc_client_for_server);
                        let tunnel_manager = Arc::clone(&tunnel_manager_for_server);

                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(
                                inbound_stream,
                                client_addr,
                                config,
                                rpc_client,
                                tunnel_manager,
                            )
                            .await
                            {
                                error!("Error handling connection from {}: {}", client_addr, e);
                            }
                        });
                    }
                    Err(e) => {
                        error!("Failed to accept connection: {}", e);
                        break;
                    }
                }
            }
        };

        // Wait for either the server to complete or tasks to fail
        tokio::select! {
            _ = server => {
                info!("Server shut down gracefully");
            }
            result = discovery_task => {
                match result {
                    Ok(_) => info!("Service discovery task completed"),
                    Err(e) => error!("Service discovery task error: {}", e),
                }
            }
            result = config_task => {
                match result {
                    Ok(_) => info!("Configuration task completed"),
                    Err(e) => error!("Configuration task error: {}", e),
                }
            }
        }

        Ok(())
    }

    async fn connect_rpc(&self) {
        loop {
            match self.handle_rpc_connection_cycle().await {
                Ok(_) => {
                    warn!("Connection to kube manager exited, retrying in 5 seconds...",);
                }
                Err(e) => {
                    warn!(
                        "Connection to kube manager failed: {}, retrying in 5 seconds...",
                        e
                    );
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }

    async fn handle_rpc_connection_cycle(&self) -> Result<()> {
        info!(
            "Attempting to connect to sidecar proxy manager rpc at: {}",
            self.sidecar_proxy_manager_addr,
        );

        // Connect via TCP
        let conn = tarpc::serde_transport::tcp::connect(
            &self.sidecar_proxy_manager_addr,
            tarpc::tokio_serde::formats::Bincode::default,
        )
        .await?;

        debug!(
            "TCP connection established to {}",
            self.sidecar_proxy_manager_addr
        );

        // Create length-delimited codec for message framing
        let (server_chan, client_chan, _) = spawn_twoway(conn);

        // Create RPC client
        let rpc_client =
            SidecarProxyManagerRpcClient::new(tarpc::client::Config::default(), client_chan)
                .spawn();

        // Store RPC client for use by connection handlers
        {
            let mut client_lock = self.rpc_client.write().await;
            *client_lock = Some(rpc_client.clone());
        }

        let rpc_server =
            SidecarProxyRpcServer::new(self.workload_id, rpc_client, Arc::clone(&self.config));
        let rpc_server_clone = rpc_server.clone();
        let rpc_server_task = tokio::spawn(async move {
            BaseChannel::with_defaults(server_chan)
                .execute(rpc_server_clone.serve())
                .for_each(|resp| async move {
                    tokio::spawn(resp);
                })
                .await;
        });

        let _ = rpc_server.register_sidecar_proxy().await;

        info!("RPC client connected to kube manager");

        let _ = rpc_server_task.await;

        // Clear RPC client when connection ends
        {
            let mut client_lock = self.rpc_client.write().await;
            *client_lock = None;
        }

        Ok(())
    }

    /// Start the server (blocking)
    pub async fn serve(self) -> Result<()> {
        // Use a never-completing future as the shutdown signal
        let shutdown_signal = std::future::pending::<()>();
        self.serve_with_graceful_shutdown(shutdown_signal).await
    }

    /// Get the current configuration
    pub async fn get_config(&self) -> ProxyConfig {
        self.config.read().await.clone()
    }

    /// Get the service discovery instance
    pub fn discovery(&self) -> &Arc<ServiceDiscovery> {
        &self.discovery
    }
}

/// Handle a single TCP connection by extracting original destination and forwarding
async fn handle_connection(
    mut inbound_stream: TcpStream,
    client_addr: SocketAddr,
    config: Arc<RwLock<ProxyConfig>>,
    rpc_client: Arc<RwLock<Option<SidecarProxyManagerRpcClient>>>,
    tunnel_manager: Arc<DevboxTunnelManager>,
) -> io::Result<()> {
    // Extract the original destination from the iptables-redirected connection
    let original_dest = match get_original_destination(&inbound_stream) {
        Ok(dest) => dest,
        Err(e) => {
            error!(
                "Failed to get original destination for {}: {}",
                client_addr, e
            );
            return Err(e);
        }
    };

    debug!("Original destination: {} -> {}", client_addr, original_dest);

    // Check if there's a DevboxTunnel route for this destination
    if let Some(devbox_route) = check_devbox_tunnel_route(&config, original_dest.port()).await {
        info!(
            "Devbox intercept detected for port {}: intercept_id={}, routing to devbox",
            original_dest.port(),
            devbox_route.intercept_id
        );
        return handle_devbox_tunnel(
            inbound_stream,
            client_addr,
            original_dest,
            devbox_route,
            rpc_client,
            tunnel_manager,
        )
        .await;
    }

    // Convert to local address (same port, localhost)
    let local_target = SocketAddr::new("127.0.0.1".parse().unwrap(), original_dest.port());

    // Detect protocol by reading initial data
    let (protocol_type, initial_data) = match detect_protocol(&mut inbound_stream).await {
        Ok(result) => result,
        Err(e) => {
            warn!("Failed to detect protocol for {}: {}", client_addr, e);
            return Err(e);
        }
    };

    match protocol_type {
        ProtocolType::Http { method, path } => {
            info!(
                "Detected HTTP {} {} from {} -> {} (local: {})",
                method, path, client_addr, original_dest, local_target
            );
            handle_http_proxy(inbound_stream, original_dest.port(), initial_data).await
        }
        ProtocolType::Tcp => {
            info!(
                "Proxying TCP from {} -> {} (local: {})",
                client_addr, original_dest, local_target
            );
            handle_tcp_proxy(inbound_stream, local_target, initial_data).await
        }
    }
}

/// Handle HTTP proxying with OpenTelemetry header parsing and intelligent routing
async fn handle_http_proxy(
    mut inbound_stream: TcpStream,
    original_port: u16,
    mut initial_data: Vec<u8>,
) -> io::Result<()> {
    // Try to parse the HTTP request, reading more data if needed
    let (http_request, _body_start) = match http_parser::parse_complete_http_request(
        &mut inbound_stream,
        &mut initial_data,
    )
    .await
    {
        Ok(parsed) => parsed,
        Err(e) => {
            warn!(
                "Failed to parse HTTP request: {}, falling back to TCP proxy",
                e
            );
            let local_target = SocketAddr::new("127.0.0.1".parse().unwrap(), original_port);
            return handle_tcp_proxy(inbound_stream, local_target, initial_data).await;
        }
    };

    // Extract OpenTelemetry and routing context from headers
    let routing_context = extract_routing_context(&http_request.headers);

    // Determine routing target based on headers
    let routing_target = determine_routing_target(&routing_context, original_port);

    // Get the actual target address
    let local_target = SocketAddr::new("127.0.0.1".parse().unwrap(), routing_target.get_port());

    // Log the routing decision
    info!(
        "HTTP {} {} -> {} (routing: {}, trace_id: {:?})",
        http_request.method,
        http_request.path,
        local_target,
        routing_target.get_metadata(),
        routing_context.trace_context.trace_id
    );

    // Connect to the local service
    let mut outbound_stream = TcpStream::connect(&local_target).await?;

    // Send the initial data we read for protocol detection
    if !initial_data.is_empty() {
        tokio::io::AsyncWriteExt::write_all(&mut outbound_stream, &initial_data).await?;
    }

    // Start bidirectional copying for the rest of the connection
    match copy_bidirectional(&mut inbound_stream, &mut outbound_stream).await {
        Ok((bytes_tx, bytes_rx)) => {
            debug!(
                "HTTP connection completed: {} bytes tx, {} bytes rx",
                bytes_tx, bytes_rx
            );
        }
        Err(e) => {
            debug!("HTTP connection ended: {}", e);
        }
    }

    Ok(())
}

/// Handle TCP proxying (raw byte forwarding)
async fn handle_tcp_proxy(
    mut inbound_stream: TcpStream,
    local_target: SocketAddr,
    initial_data: Vec<u8>,
) -> io::Result<()> {
    // Connect to the local service
    let mut outbound_stream = TcpStream::connect(&local_target).await?;

    // Send the initial data we read for protocol detection
    if !initial_data.is_empty() {
        tokio::io::AsyncWriteExt::write_all(&mut outbound_stream, &initial_data).await?;
    }

    // Start bidirectional copying for the rest of the connection
    match copy_bidirectional(&mut inbound_stream, &mut outbound_stream).await {
        Ok((bytes_tx, bytes_rx)) => {
            debug!(
                "TCP connection completed: {} bytes tx, {} bytes rx",
                bytes_tx, bytes_rx
            );
        }
        Err(e) => {
            debug!("TCP connection ended: {}", e);
        }
    }

    Ok(())
}

/// DevboxTunnel route information
#[derive(Debug, Clone)]
struct DevboxRouteInfo {
    intercept_id: Uuid,
    session_id: Uuid,
    target_port: u16,
    auth_token: String,
}

/// Check if there's a DevboxTunnel route for the given port
async fn check_devbox_tunnel_route(
    config: &Arc<RwLock<ProxyConfig>>,
    port: u16,
) -> Option<DevboxRouteInfo> {
    use crate::config::RouteTarget;

    let config = config.read().await;

    // Check all routes for DevboxTunnel targeting this port
    for route in &config.routes {
        if let RouteTarget::DevboxTunnel {
            intercept_id,
            session_id,
            target_port,
            auth_token,
        } = &route.target
        {
            // Match if the original destination port corresponds to this intercept
            // In a real implementation, you might also check the path pattern
            if *target_port == port {
                return Some(DevboxRouteInfo {
                    intercept_id: *intercept_id,
                    session_id: *session_id,
                    target_port: *target_port,
                    auth_token: auth_token.clone(),
                });
            }
        }
    }

    None
}

struct DevboxTunnelManager {
    clients: Mutex<HashMap<Uuid, Arc<TunnelClient>>>,
}

impl DevboxTunnelManager {
    fn new() -> Self {
        Self {
            clients: Mutex::new(HashMap::new()),
        }
    }

    async fn connect_tcp_stream(
        &self,
        session_id: Uuid,
        websocket_url: &str,
        auth_token: &str,
        target_host: &str,
        target_port: u16,
    ) -> Result<TunnelTcpStream, TunnelError> {
        let client = self
            .ensure_client(session_id, websocket_url, auth_token)
            .await?;

        match client
            .connect_tcp(target_host.to_string(), target_port)
            .await
        {
            Ok(stream) => Ok(stream),
            Err(TunnelError::ConnectionClosed) => {
                self.remove_client(session_id).await;
                let client = self
                    .ensure_client(session_id, websocket_url, auth_token)
                    .await?;
                client
                    .connect_tcp(target_host.to_string(), target_port)
                    .await
            }
            Err(err) => Err(err),
        }
    }

    async fn ensure_client(
        &self,
        session_id: Uuid,
        websocket_url: &str,
        auth_token: &str,
    ) -> Result<Arc<TunnelClient>, TunnelError> {
        if let Some(existing) = self.clients.lock().await.get(&session_id) {
            return Ok(existing.clone());
        }

        let new_client = Arc::new(self.create_client(websocket_url, auth_token).await?);

        let mut clients = self.clients.lock().await;
        match clients.entry(session_id) {
            std::collections::hash_map::Entry::Occupied(entry) => Ok(entry.get().clone()),
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(new_client.clone());
                Ok(new_client)
            }
        }
    }

    async fn create_client(
        &self,
        websocket_url: &str,
        auth_token: &str,
    ) -> Result<TunnelClient, TunnelError> {
        let mut request = websocket_url
            .into_client_request()
            .map_err(tunnel_transport_error)?;

        let header = format!("Bearer {}", auth_token)
            .parse()
            .map_err(tunnel_transport_error)?;
        request.headers_mut().insert(AUTHORIZATION, header);

        let (stream, _) = connect_async(request)
            .await
            .map_err(tunnel_transport_error)?;

        let transport = TunnelWebSocketTransport::new(stream);
        Ok(TunnelClient::connect(transport))
    }

    async fn remove_client(&self, session_id: Uuid) {
        let mut clients = self.clients.lock().await;
        clients.remove(&session_id);
    }
}

fn tunnel_transport_error<E>(err: E) -> TunnelError
where
    E: std::fmt::Display,
{
    TunnelError::Transport(io::Error::new(io::ErrorKind::Other, err.to_string()))
}

/// Handle a connection that should be routed through a devbox tunnel
///
/// Architecture: Hybrid control/data plane
///   Control: Sidecar → Kube-Manager (RPC) → API (setup tunnel)
///   Data:    Sidecar → API (WebSocket) ↔ Devbox (direct streaming)
async fn handle_devbox_tunnel(
    mut inbound_stream: TcpStream,
    client_addr: SocketAddr,
    original_dest: SocketAddr,
    devbox_route: DevboxRouteInfo,
    rpc_client: Arc<RwLock<Option<SidecarProxyManagerRpcClient>>>,
    tunnel_manager: Arc<DevboxTunnelManager>,
) -> io::Result<()> {
    info!(
        "Routing {} -> {} through devbox tunnel (intercept_id={}, session_id={})",
        client_addr, original_dest, devbox_route.intercept_id, devbox_route.session_id
    );

    // Get RPC client
    let client = {
        let lock = rpc_client.read().await;
        lock.clone()
    };

    let client = match client {
        Some(c) => c,
        None => {
            error!("RPC client not available - cannot establish devbox tunnel");
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "RPC client not connected to kube-manager",
            ));
        }
    };

    // Step 1: Call RPC to get tunnel info (control plane)
    info!(
        "Requesting tunnel setup from kube-manager for intercept_id={}",
        devbox_route.intercept_id
    );
    let tunnel_info = client
        .request_devbox_tunnel(
            tarpc::context::current(),
            devbox_route.intercept_id,
            client_addr.to_string(),
            devbox_route.target_port,
        )
        .await
        .map_err(|e| {
            error!("Failed to request devbox tunnel: {}", e);
            io::Error::new(io::ErrorKind::Other, format!("RPC error: {}", e))
        })?
        .map_err(|e| {
            error!("Kube-manager rejected tunnel request: {}", e);
            io::Error::new(io::ErrorKind::PermissionDenied, e)
        })?;

    info!(
        "Tunnel setup successful: tunnel_id={}, connecting to {}",
        tunnel_info.tunnel_id, tunnel_info.websocket_url
    );

    let mut devbox_stream = match tunnel_manager
        .connect_tcp_stream(
            devbox_route.session_id,
            &tunnel_info.websocket_url,
            &tunnel_info.auth_token,
            "127.0.0.1",
            devbox_route.target_port,
        )
        .await
    {
        Ok(stream) => stream,
        Err(err) => {
            error!(
                "Failed to establish tunnel stream for intercept {}: {}",
                devbox_route.intercept_id, err
            );
            return Err(io::Error::from(err));
        }
    };

    match copy_bidirectional(&mut inbound_stream, &mut devbox_stream).await {
        Ok((bytes_tx, bytes_rx)) => {
            info!(
                "Devbox tunnel completed: {} bytes sent, {} bytes received (intercept_id={})",
                bytes_tx, bytes_rx, devbox_route.intercept_id
            );
        }
        Err(err) => {
            warn!(
                "Devbox tunnel error for intercept_id={}: {}",
                devbox_route.intercept_id, err
            );
            tunnel_manager.remove_client(devbox_route.session_id).await;
            return Err(err);
        }
    }

    if let Err(err) = tokio::io::AsyncWriteExt::shutdown(&mut devbox_stream).await {
        debug!("Failed to shutdown tunnel stream cleanly: {}", err);
    }

    Ok(())
}
