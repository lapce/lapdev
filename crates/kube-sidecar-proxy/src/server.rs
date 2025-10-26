use crate::{
    config::{BranchServiceRoute, DevboxConnection, RouteDecision, RoutingTable, SidecarSettings},
    error::Result,
    original_dest::get_original_destination,
    otel_routing::{determine_routing_target, extract_routing_context},
    protocol_detector::{detect_protocol, ProtocolType},
    rpc::SidecarProxyRpcServer,
};
use anyhow::anyhow;
use futures::StreamExt;
use lapdev_common::kube::{SIDECAR_PROXY_MANAGER_ADDR_ENV_VAR, SIDECAR_PROXY_WORKLOAD_ENV_VAR};
use lapdev_kube_rpc::{http_parser, SidecarProxyManagerRpcClient, SidecarProxyRpc};
use lapdev_rpc::spawn_twoway;
use std::{io, net::SocketAddr, str::FromStr, sync::Arc};
use tarpc::server::{BaseChannel, Channel};
use tokio::{
    io::copy_bidirectional,
    net::{TcpListener, TcpStream},
    sync::RwLock,
};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Main sidecar proxy server
#[derive(Clone)]
pub struct SidecarProxyServer {
    workload_id: Uuid,
    sidecar_proxy_manager_addr: String,
    settings: Arc<SidecarSettings>,
    routing_table: Arc<RwLock<RoutingTable>>,
    /// RPC client to kube-manager (None until connection established)
    rpc_client: Arc<RwLock<Option<SidecarProxyManagerRpcClient>>>,
}

impl SidecarProxyServer {
    /// Create a new sidecar proxy server
    pub async fn new(
        listen_addr: SocketAddr,
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

        let workload_id = match std::env::var(SIDECAR_PROXY_WORKLOAD_ENV_VAR) {
            Ok(value) => Uuid::from_str(&value).map_err(|_| {
                anyhow!(format!(
                    "{SIDECAR_PROXY_WORKLOAD_ENV_VAR} isn't a valid uuid"
                ))
            })?,
            Err(_) => {
                warn!(
                    "{} not set, defaulting workload identifier to environment id",
                    SIDECAR_PROXY_WORKLOAD_ENV_VAR
                );
                Uuid::parse_str(&environment_id).map_err(|e| {
                    anyhow!(format!("Failed to parse environment_id as UUID: {}", e))
                })?
            }
        };

        // Parse environment ID - now required
        let env_id = Uuid::parse_str(&environment_id)
            .map_err(|e| anyhow!("Failed to parse environment_id as UUID: {}", e))?;

        let settings = SidecarSettings::new(
            listen_addr,
            namespace.clone(),
            pod_name.clone(),
            env_id,
            environment_auth_token,
        );

        let server = Self {
            workload_id,
            sidecar_proxy_manager_addr,
            settings: Arc::new(settings),
            routing_table: Arc::new(RwLock::new(RoutingTable::default())),
            rpc_client: Arc::new(RwLock::new(None)),
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

        // Create TCP listener for iptables-redirected connections
        let listen_addr = self.settings.as_ref().listen_addr;
        let listener = TcpListener::bind(&listen_addr).await?;
        info!("Sidecar proxy listening on: {}", listen_addr);

        // Handle connections
        let routing_table_for_server = Arc::clone(&self.routing_table);
        let rpc_client_for_server = Arc::clone(&self.rpc_client);
        let server = async move {
            loop {
                match listener.accept().await {
                    Ok((inbound_stream, client_addr)) => {
                        debug!("Accepted connection from {}", client_addr);
                        let routing_table = Arc::clone(&routing_table_for_server);
                        let rpc_client = Arc::clone(&rpc_client_for_server);

                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(
                                inbound_stream,
                                client_addr,
                                routing_table,
                                rpc_client,
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
        tokio::pin!(server);

        tokio::select! {
            _ = &mut server => {
                info!("Server shut down gracefully");
            }
            _ = shutdown_signal => {
                info!("Shutdown signal received");
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

        let namespace = self
            .settings
            .as_ref()
            .namespace
            .clone()
            .unwrap_or_else(|| "default".to_string());
        let rpc_server = SidecarProxyRpcServer::new(
            self.workload_id,
            self.settings.as_ref().environment_id,
            namespace,
            rpc_client,
            Arc::clone(&self.routing_table),
        );
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
}

/// Handle a single TCP connection by extracting original destination and forwarding
async fn handle_connection(
    mut inbound_stream: TcpStream,
    client_addr: SocketAddr,
    routing_table: Arc<RwLock<RoutingTable>>,
    _rpc_client: Arc<RwLock<Option<SidecarProxyManagerRpcClient>>>,
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

    // Detect protocol by reading initial data
    let (protocol_type, initial_data) = match detect_protocol(&mut inbound_stream).await {
        Ok(result) => result,
        Err(e) => {
            warn!("Failed to detect protocol for {}: {}", client_addr, e);
            return Err(e);
        }
    };

    match protocol_type {
        ProtocolType::Http { .. } => {
            handle_http_proxy(
                inbound_stream,
                client_addr,
                original_dest,
                initial_data,
                Arc::clone(&routing_table),
                Arc::clone(&_rpc_client),
            )
            .await
        }
        ProtocolType::Tcp => {
            let decision = {
                let table = routing_table.read().await;
                table.resolve_tcp(original_dest.port())
            };

            match decision {
                RouteDecision::DefaultDevbox {
                    connection,
                    target_port,
                }
                | RouteDecision::BranchDevbox {
                    connection,
                    target_port,
                } => {
                    let metadata = connection.metadata();
                    info!(
                        "TCP {} -> {} intercepted by Devbox (intercept_id={}, session_id={}, target_port={})",
                        client_addr,
                        original_dest,
                        metadata.intercept_id,
                        metadata.session_id,
                        target_port
                    );
                    handle_devbox_tunnel(
                        inbound_stream,
                        client_addr,
                        original_dest,
                        initial_data,
                        connection,
                        target_port,
                    )
                    .await
                }
                _ => {
                    let local_target =
                        SocketAddr::new("127.0.0.1".parse().unwrap(), original_dest.port());
                    info!(
                        "Proxying TCP from {} -> {} (local: {})",
                        client_addr, original_dest, local_target
                    );
                    handle_tcp_proxy(inbound_stream, local_target, initial_data).await
                }
            }
        }
    }
}

/// Handle HTTP proxying with OpenTelemetry header parsing and intelligent routing
async fn handle_http_proxy(
    mut inbound_stream: TcpStream,
    client_addr: SocketAddr,
    original_dest: SocketAddr,
    mut initial_data: Vec<u8>,
    routing_table: Arc<RwLock<RoutingTable>>,
    _rpc_client: Arc<RwLock<Option<SidecarProxyManagerRpcClient>>>,
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
            let fallback_target =
                SocketAddr::new("127.0.0.1".parse().unwrap(), original_dest.port());
            return handle_tcp_proxy(inbound_stream, fallback_target, initial_data).await;
        }
    };

    // Extract OpenTelemetry and routing context from headers
    let routing_context = extract_routing_context(&http_request.headers);
    let fallback_target = SocketAddr::new("127.0.0.1".parse().unwrap(), original_dest.port());

    let branch_id = routing_context.lapdev_environment_id;
    let decision = {
        let table = routing_table.read().await;
        table.resolve_http(original_dest.port(), branch_id)
    };

    match decision {
        RouteDecision::BranchService { service } => {
            let port = original_dest.port();
            if let Some(service_name) = service.service_name_for_port(port) {
                info!(
                    "HTTP {} {} routing to branch {:?} service {}:{}",
                    http_request.method, http_request.path, branch_id, service_name, port
                );

                if let Err(err) =
                    proxy_branch_stream(&mut inbound_stream, service_name, port, &initial_data)
                        .await
                {
                    warn!(
                        "Branch route {} for env {:?} failed: {}; falling back to shared target",
                        service_name, branch_id, err
                    );
                } else {
                    return Ok(());
                }
            } else {
                warn!(
                    "Missing branch service mapping for port {} in env {:?}; falling back to shared target",
                    port, branch_id
                );
            }
        }
        RouteDecision::BranchDevbox {
            connection,
            target_port,
        } => {
            let metadata = connection.metadata();
            info!(
                "HTTP {} {} intercepted by branch devbox (env {:?}, intercept_id={}, session_id={}, target_port={})",
                http_request.method,
                http_request.path,
                branch_id,
                metadata.intercept_id,
                metadata.session_id,
                target_port
            );
            return handle_devbox_tunnel(
                inbound_stream,
                client_addr,
                original_dest,
                initial_data,
                connection,
                target_port,
            )
            .await;
        }
        RouteDecision::DefaultDevbox {
            connection,
            target_port,
        } => {
            let metadata = connection.metadata();
            info!(
                "HTTP {} {} intercepted by shared devbox (port {}, intercept_id={}, session_id={}, target_port={})",
                http_request.method,
                http_request.path,
                original_dest.port(),
                metadata.intercept_id,
                metadata.session_id,
                target_port
            );
            return handle_devbox_tunnel(
                inbound_stream,
                client_addr,
                original_dest,
                initial_data,
                connection,
                target_port,
            )
            .await;
        }
        RouteDecision::DefaultLocal => {}
    }

    // Determine routing target based on headers for fallback logging
    let routing_target = determine_routing_target(&routing_context, original_dest.port());
    info!(
        "HTTP {} {} -> {} (routing: {}, trace_id: {:?})",
        http_request.method,
        http_request.path,
        fallback_target,
        routing_target.get_metadata(),
        routing_context.trace_context.trace_id
    );

    proxy_stream(&mut inbound_stream, fallback_target, &initial_data).await
}

async fn proxy_stream(
    inbound_stream: &mut TcpStream,
    target: SocketAddr,
    initial_data: &[u8],
) -> io::Result<()> {
    let mut outbound_stream = TcpStream::connect(target).await?;

    if !initial_data.is_empty() {
        tokio::io::AsyncWriteExt::write_all(&mut outbound_stream, initial_data).await?;
    }

    match copy_bidirectional(inbound_stream, &mut outbound_stream).await {
        Ok((bytes_tx, bytes_rx)) => {
            debug!(
                "HTTP connection completed via {}: {} bytes tx, {} bytes rx",
                target, bytes_tx, bytes_rx
            );
        }
        Err(e) => {
            debug!("HTTP connection via {} ended: {}", target, e);
        }
    }

    Ok(())
}

async fn proxy_branch_stream(
    inbound_stream: &mut TcpStream,
    service_name: &str,
    port: u16,
    initial_data: &[u8],
) -> io::Result<()> {
    let mut outbound_stream = TcpStream::connect((service_name, port)).await?;

    if !initial_data.is_empty() {
        tokio::io::AsyncWriteExt::write_all(&mut outbound_stream, initial_data).await?;
    }

    match copy_bidirectional(inbound_stream, &mut outbound_stream).await {
        Ok((bytes_tx, bytes_rx)) => {
            debug!(
                "HTTP connection completed via branch service {}:{}: {} bytes tx, {} bytes rx",
                service_name, port, bytes_tx, bytes_rx
            );
        }
        Err(e) => {
            debug!(
                "HTTP connection via branch service {}:{} ended: {}",
                service_name, port, e
            );
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

/// Handle a connection that should be routed through a devbox tunnel
///
/// Architecture:
///   Sidecar opens a websocket directly to the Lapdev API using the intercept session token,
///   then proxies data between the cluster workload and the developer's machine over that tunnel.
async fn handle_devbox_tunnel(
    mut inbound_stream: TcpStream,
    client_addr: SocketAddr,
    original_dest: SocketAddr,
    mut initial_data: Vec<u8>,
    connection: Arc<DevboxConnection>,
    target_port: u16,
) -> io::Result<()> {
    let metadata = connection.metadata();

    info!(
        "Routing {} -> {} through devbox tunnel (intercept_id={}, session_id={}, target_port={})",
        client_addr, original_dest, metadata.intercept_id, metadata.session_id, target_port
    );

    info!(
        "Connecting to devbox tunnel websocket for intercept_id={} at {}",
        metadata.intercept_id, metadata.websocket_url
    );

    let mut devbox_stream = match connection
        .connect_tcp_stream("127.0.0.1", target_port)
        .await
    {
        Ok(stream) => stream,
        Err(err) => {
            error!(
                "Failed to establish tunnel stream for intercept {}: {}",
                metadata.intercept_id, err
            );
            connection.clear_client();
            return Err(io::Error::from(err));
        }
    };

    if !initial_data.is_empty() {
        tokio::io::AsyncWriteExt::write_all(&mut devbox_stream, &initial_data).await?;
    }

    match copy_bidirectional(&mut inbound_stream, &mut devbox_stream).await {
        Ok((bytes_tx, bytes_rx)) => {
            info!(
                "Devbox tunnel completed: {} bytes sent, {} bytes received (intercept_id={})",
                bytes_tx, bytes_rx, metadata.intercept_id
            );
        }
        Err(err) => {
            warn!(
                "Devbox tunnel error for intercept_id={}: {}",
                metadata.intercept_id, err
            );
            connection.clear_client();
            return Err(io::Error::from(err));
        }
    }

    if let Err(err) = tokio::io::AsyncWriteExt::shutdown(&mut devbox_stream).await {
        debug!("Failed to shutdown tunnel stream cleanly: {}", err);
    }

    Ok(())
}
