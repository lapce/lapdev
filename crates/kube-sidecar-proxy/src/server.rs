use crate::{
    config::{DevboxConnection, RouteDecision, RoutingTable, SidecarSettings},
    connection_registry::ConnectionRegistry,
    error::Result,
    http2_proxy::handle_http2_proxy,
    otel_routing::{determine_routing_target, extract_routing_context},
    protocol_detector::{detect_protocol, ProtocolDetectionResult, ProtocolType},
    rpc::SidecarProxyRpcServer,
};
use anyhow::{anyhow, Context};
use futures::StreamExt;
use lapdev_common::kube::{
    ProxyPortRoute, SIDECAR_PROXY_MANAGER_ADDR_ENV_VAR, SIDECAR_PROXY_PORT_ROUTES_ENV_VAR,
    SIDECAR_PROXY_WORKLOAD_ENV_VAR,
};
use lapdev_kube_rpc::{http_parser, SidecarProxyManagerRpcClient, SidecarProxyRpc};
use lapdev_rpc::spawn_twoway;
use std::{collections::HashSet, env, io, net::SocketAddr, str::FromStr, sync::Arc};
use tarpc::server::{BaseChannel, Channel};
use tokio::{
    io::{copy_bidirectional, AsyncRead, AsyncWrite},
    net::{TcpListener, TcpStream},
    sync::{mpsc, watch, RwLock},
    time::{sleep, Duration, MissedTickBehavior},
};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

const SIDECAR_HEARTBEAT_INTERVAL_SECS: u64 = 30;

/// Main sidecar proxy server
#[derive(Clone)]
pub struct SidecarProxyServer {
    workload_id: Uuid,
    sidecar_proxy_manager_addr: String,
    settings: Arc<SidecarSettings>,
    routing_table: Arc<RwLock<RoutingTable>>,
    connection_registry: Arc<ConnectionRegistry>,
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

        let raw_workload_id =
            env::var(SIDECAR_PROXY_WORKLOAD_ENV_VAR).map_err(|err| match err {
                env::VarError::NotPresent => {
                    anyhow!(format!(
                        "{SIDECAR_PROXY_WORKLOAD_ENV_VAR} env var must be set"
                    ))
                }
                env::VarError::NotUnicode(_) => anyhow!(format!(
                    "{SIDECAR_PROXY_WORKLOAD_ENV_VAR} must be valid UTF-8"
                )),
            })?;

        let workload_id = Uuid::from_str(&raw_workload_id).map_err(|_| {
            anyhow!(format!(
                "{SIDECAR_PROXY_WORKLOAD_ENV_VAR} isn't a valid uuid"
            ))
        })?;

        info!(
            workload_id = %workload_id,
            namespace = namespace
                .as_deref()
                .unwrap_or("unknown"),
            pod_name = pod_name
                .as_deref()
                .unwrap_or("unknown"),
            "Starting sidecar proxy workload"
        );

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

        let mut routing_table = RoutingTable::default();

        match env::var(SIDECAR_PROXY_PORT_ROUTES_ENV_VAR) {
            Ok(raw_routes) => {
                let routes: Vec<ProxyPortRoute> =
                    serde_json::from_str(&raw_routes).with_context(|| {
                        format!(
                            "failed to parse {} as JSON array of proxy routes",
                            SIDECAR_PROXY_PORT_ROUTES_ENV_VAR
                        )
                    })?;

                if routes.is_empty() {
                    warn!(
                        "{} provided but contained no routes; sidecar will fall back to default port",
                        SIDECAR_PROXY_PORT_ROUTES_ENV_VAR
                    );
                } else {
                    info!(
                        "Loaded {} proxy port routes from {}",
                        routes.len(),
                        SIDECAR_PROXY_PORT_ROUTES_ENV_VAR
                    );
                }

                routing_table.set_port_routes(routes);
            }
            Err(env::VarError::NotPresent) => {
                debug!(
                    "{} not set; using default listener port {}",
                    SIDECAR_PROXY_PORT_ROUTES_ENV_VAR,
                    listen_addr.port()
                );
            }
            Err(env::VarError::NotUnicode(_)) => {
                return Err(
                    anyhow!("{} must be valid UTF-8", SIDECAR_PROXY_PORT_ROUTES_ENV_VAR).into(),
                );
            }
        }

        let server = Self {
            workload_id,
            sidecar_proxy_manager_addr,
            settings: Arc::new(settings),
            routing_table: Arc::new(RwLock::new(routing_table)),
            connection_registry: Arc::new(ConnectionRegistry::default()),
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

        // Determine which ports we should listen on.
        let listen_addr = self.settings.as_ref().listen_addr;
        let listen_ip = listen_addr.ip();
        let default_port = listen_addr.port();

        let initial_ports: Vec<u16> = {
            let table = self.routing_table.read().await;
            let mut ports: HashSet<u16> = HashSet::new();
            ports.insert(default_port);
            ports.extend(table.port_routes().map(|route| route.proxy_port));
            ports.into_iter().collect()
        };

        if initial_ports.is_empty() {
            return Err(anyhow!("no listener ports configured for sidecar proxy").into());
        }

        let (listener_err_tx, mut listener_err_rx) =
            mpsc::unbounded_channel::<(SocketAddr, String)>();
        let mut listener_handles = Vec::new();

        for port in initial_ports {
            let addr = SocketAddr::new(listen_ip, port);
            let listener = TcpListener::bind(addr).await?;
            info!("Sidecar proxy listening on: {}", addr);

            let routing_table = Arc::clone(&self.routing_table);
            let rpc_client = Arc::clone(&self.rpc_client);
            let settings = Arc::clone(&self.settings);
            let connection_registry = Arc::clone(&self.connection_registry);
            let err_tx = listener_err_tx.clone();

            let handle = tokio::spawn(async move {
                if let Err(err) = run_listener(
                    listener,
                    addr,
                    settings,
                    routing_table,
                    rpc_client,
                    connection_registry,
                )
                .await
                {
                    let _ = err_tx.send((addr, err.to_string()));
                }
            });

            listener_handles.push(handle);
        }

        drop(listener_err_tx);

        let mut server = Box::pin(async move {
            if let Some((addr, err)) = listener_err_rx.recv().await {
                error!("Listener {} failed: {}", addr, err);
            }
        });

        tokio::select! {
            _ = &mut server => {
                info!("Server shut down gracefully");
            }
            _ = shutdown_signal => {
                info!("Shutdown signal received");
            }
        }

        for handle in listener_handles {
            handle.abort();
            let _ = handle.await;
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
            Arc::clone(&self.connection_registry),
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

        let register_result = rpc_server.register_sidecar_proxy().await;
        if let Err(err) = register_result {
            rpc_server_task.abort();
            let _ = rpc_server_task.await;
            return Err(err.into());
        }

        info!("RPC client connected to kube manager");

        let (hb_shutdown_tx, hb_shutdown_rx) = watch::channel(false);
        let mut heartbeat_task = Box::pin(run_manager_heartbeat(
            hb_shutdown_rx,
            rpc_server.manager_client(),
            self.workload_id,
            self.settings.as_ref().environment_id,
        ));

        tokio::pin!(rpc_server_task);

        tokio::select! {
            res = &mut rpc_server_task => {
                let _ = hb_shutdown_tx.send(true);
                if let Err(join_err) = heartbeat_task.await {
                    warn!("Heartbeat task ended: {}", join_err);
                }
                res.map_err(|e| anyhow!("sidecar RPC server task failed: {}", e))?;
            }
            res = &mut heartbeat_task => {
                let _ = hb_shutdown_tx.send(true);
                rpc_server_task.abort();
                let _ = rpc_server_task.await;
                match res {
                    Ok(()) => return Err(anyhow!("Sidecar heartbeat stopped unexpectedly").into()),
                    Err(err) => return Err(err.into()),
                }
            }
        }

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

async fn run_listener(
    listener: TcpListener,
    listener_addr: SocketAddr,
    settings: Arc<SidecarSettings>,
    routing_table: Arc<RwLock<RoutingTable>>,
    rpc_client: Arc<RwLock<Option<SidecarProxyManagerRpcClient>>>,
    connection_registry: Arc<ConnectionRegistry>,
) -> io::Result<()> {
    loop {
        match listener.accept().await {
            Ok((inbound_stream, client_addr)) => {
                debug!(
                    "Accepted connection on {} from {}",
                    listener_addr, client_addr
                );
                let routing_table = Arc::clone(&routing_table);
                let rpc_client = Arc::clone(&rpc_client);
                let settings = Arc::clone(&settings);
                let connection_registry = Arc::clone(&connection_registry);

                tokio::spawn(async move {
                    if let Err(e) = handle_connection(
                        inbound_stream,
                        client_addr,
                        settings,
                        routing_table,
                        rpc_client,
                        connection_registry,
                    )
                    .await
                    {
                        if is_route_update_error(&e) {
                            debug!(
                                "Connection from {} closed due to routing update",
                                client_addr
                            );
                        } else {
                            error!("Error handling connection from {}: {}", client_addr, e);
                        }
                    }
                });
            }
            Err(e) => match e.kind() {
                io::ErrorKind::ConnectionAborted
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::Interrupted => {
                    warn!(
                        "Transient accept error on listener {}: {}",
                        listener_addr, e
                    );
                    continue;
                }
                _ => {
                    if let Some(raw_os_error) = e.raw_os_error() {
                        const EMFILE: i32 = 24;
                        const WSAEMFILE: i32 = 10024;

                        if raw_os_error == EMFILE || raw_os_error == WSAEMFILE {
                            error!(
                                "File descriptor limit hit on listener {}: {} (errno={}), backing off before retrying",
                                listener_addr, e, raw_os_error
                            );
                            sleep(Duration::from_millis(100)).await;
                            continue;
                        }
                    }

                    error!(
                        "Failed to accept connection on listener {}: {}",
                        listener_addr, e
                    );
                    return Err(e);
                }
            },
        }
    }
}

async fn run_manager_heartbeat(
    mut shutdown_rx: watch::Receiver<bool>,
    client: SidecarProxyManagerRpcClient,
    workload_id: Uuid,
    environment_id: Uuid,
) -> anyhow::Result<()> {
    let mut interval = tokio::time::interval(Duration::from_secs(SIDECAR_HEARTBEAT_INTERVAL_SECS));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = shutdown_rx.changed(), if *shutdown_rx.borrow() => {
                return Ok(());
            }
            _ = interval.tick() => {
                match client
                    .heartbeat(tarpc::context::current(), workload_id, environment_id)
                    .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => return Err(anyhow!("Heartbeat rejected by kube-manager: {}", err)),
                    Err(err) => return Err(anyhow!("Heartbeat RPC failed: {}", err)),
                }
            }
        }
    }
}

/// Handle a single TCP connection by extracting original destination and forwarding
async fn handle_connection(
    mut inbound_stream: TcpStream,
    client_addr: SocketAddr,
    settings: Arc<SidecarSettings>,
    routing_table: Arc<RwLock<RoutingTable>>,
    rpc_client: Arc<RwLock<Option<SidecarProxyManagerRpcClient>>>,
    connection_registry: Arc<ConnectionRegistry>,
) -> io::Result<()> {
    let local_addr = inbound_stream.local_addr()?;
    let proxy_port = local_addr.port();

    debug!(
        "Connection from {} accepted on proxy port {}",
        client_addr, proxy_port
    );

    // Detect protocol by reading initial data with a bounded timeout
    let detection = detect_protocol(&mut inbound_stream, Some(Duration::from_secs(10))).await;
    let detection = match detection {
        Ok(result) => result,
        Err(e) => {
            warn!("Failed to detect protocol for {}: {}", client_addr, e);
            return Err(e);
        }
    };

    let ProtocolDetectionResult {
        protocol: protocol_type,
        buffer: initial_data,
        timed_out,
    } = detection;

    if timed_out {
        debug!(
            "Protocol detection for {} timed out after 10s; falling back to {:?}",
            client_addr, protocol_type
        );
    }

    match protocol_type {
        ProtocolType::Http { .. } => {
            handle_http_proxy(
                inbound_stream,
                client_addr,
                proxy_port,
                initial_data,
                Arc::clone(&settings),
                Arc::clone(&routing_table),
                Arc::clone(&rpc_client),
                Arc::clone(&connection_registry),
            )
            .await
        }
        ProtocolType::Http2 => {
            handle_http2_proxy(
                inbound_stream,
                client_addr,
                proxy_port,
                initial_data,
                settings,
                Arc::clone(&routing_table),
                Arc::clone(&rpc_client),
                Arc::clone(&connection_registry),
            )
            .await
        }
        ProtocolType::Tcp => {
            let (service_port, decision) = {
                let table = routing_table.read().await;
                let service_port = table.service_port_for_proxy(proxy_port);
                let decision = table.resolve_tcp(service_port);
                (service_port, decision)
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
                    let shutdown_rx = connection_registry.subscribe(None).await;
                    let metadata = connection.metadata();
                    info!(
                        "TCP {} via proxy port {} (service {}) intercepted by Devbox (intercept_id={}, workload_id={}, target_port={})",
                        client_addr,
                        proxy_port,
                        service_port,
                        metadata.intercept_id,
                        metadata.workload_id,
                        target_port
                    );
                    handle_devbox_tunnel(
                        inbound_stream,
                        client_addr,
                        service_port,
                        initial_data,
                        connection,
                        target_port,
                        shutdown_rx,
                    )
                    .await
                }
                RouteDecision::DefaultLocal { target_port } => {
                    let shutdown_rx = connection_registry.subscribe(None).await;
                    let local_target = SocketAddr::new("127.0.0.1".parse().unwrap(), target_port);
                    info!(
                        "Proxying TCP from {} (proxy port {}, service {}) to {}",
                        client_addr, proxy_port, service_port, local_target
                    );
                    handle_tcp_proxy(inbound_stream, local_target, initial_data, shutdown_rx).await
                }
                RouteDecision::BranchService { .. } => {
                    let shutdown_rx = connection_registry.subscribe(None).await;
                    let local_target = SocketAddr::new("127.0.0.1".parse().unwrap(), service_port);
                    info!(
                        "TCP connection from {} on proxy port {} matched branch service route unexpectedly; proxying to local {}",
                        client_addr, proxy_port, local_target
                    );
                    handle_tcp_proxy(inbound_stream, local_target, initial_data, shutdown_rx).await
                }
            }
        }
    }
}

/// Handle HTTP proxying with OpenTelemetry header parsing and intelligent routing
async fn handle_http_proxy(
    mut inbound_stream: TcpStream,
    client_addr: SocketAddr,
    proxy_port: u16,
    initial_data: Vec<u8>,
    _settings: Arc<SidecarSettings>,
    routing_table: Arc<RwLock<RoutingTable>>,
    _rpc_client: Arc<RwLock<Option<SidecarProxyManagerRpcClient>>>,
    connection_registry: Arc<ConnectionRegistry>,
) -> io::Result<()> {
    // Try to parse the HTTP request from the buffered data that protocol detection accumulated
    let (http_request, _body_start) =
        match http_parser::parse_http_request_from_buffer(&initial_data) {
            Ok(parsed) => parsed,
            Err(e) => {
                warn!(
                    "Failed to parse HTTP request: {}, falling back to TCP proxy",
                    e
                );
                let fallback_port = {
                    let table = routing_table.read().await;
                    let service_port = table.service_port_for_proxy(proxy_port);
                    table.target_port_for_service(service_port)
                };
                let fallback_target = SocketAddr::new("127.0.0.1".parse().unwrap(), fallback_port);
                let shutdown_rx = connection_registry.subscribe(None).await;
                return handle_tcp_proxy(
                    inbound_stream,
                    fallback_target,
                    initial_data,
                    shutdown_rx,
                )
                .await;
            }
        };

    // Extract OpenTelemetry and routing context from headers
    let routing_context = extract_routing_context(&http_request.headers);

    let branch_id = routing_context.lapdev_environment_id;
    let (service_port, decision) = {
        let table = routing_table.read().await;
        let service_port = table.service_port_for_proxy(proxy_port);
        let decision = table.resolve_http(service_port, branch_id);
        (service_port, decision)
    };

    let mut fallback_port = service_port;

    match decision {
        RouteDecision::BranchService { service } => {
            if let Some(service_name) = service.service_name_for_port(service_port) {
                info!(
                    "HTTP {} {} routing to branch {:?} service {}:{}",
                    http_request.method, http_request.path, branch_id, service_name, service_port
                );

                let shutdown_rx = connection_registry.subscribe(branch_id).await;
                if let Err(err) = proxy_branch_stream(
                    &mut inbound_stream,
                    service_name,
                    service_port,
                    &initial_data,
                    shutdown_rx,
                )
                .await
                {
                    if is_route_update_error(&err) {
                        return Err(err);
                    }
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
                    service_port, branch_id
                );
            }
        }
        RouteDecision::BranchDevbox {
            connection,
            target_port,
        } => {
            let metadata = connection.metadata();
            info!(
                "HTTP {} {} intercepted by branch devbox (env {:?}, intercept_id={}, workload_id={}, target_port={})",
                http_request.method,
                http_request.path,
                branch_id,
                metadata.intercept_id,
                metadata.workload_id,
                target_port
            );
            let shutdown_rx = connection_registry.subscribe(branch_id).await;
            return handle_devbox_tunnel(
                inbound_stream,
                client_addr,
                service_port,
                initial_data,
                connection,
                target_port,
                shutdown_rx,
            )
            .await;
        }
        RouteDecision::DefaultDevbox {
            connection,
            target_port,
        } => {
            let metadata = connection.metadata();
            info!(
                "HTTP {} {} intercepted by shared devbox (proxy port {}, service port {}, intercept_id={}, workload_id={}, target_port={})",
                http_request.method,
                http_request.path,
                proxy_port,
                service_port,
                metadata.intercept_id,
                metadata.workload_id,
                target_port
            );
            let shutdown_rx = connection_registry.subscribe(None).await;
            return handle_devbox_tunnel(
                inbound_stream,
                client_addr,
                service_port,
                initial_data,
                connection,
                target_port,
                shutdown_rx,
            )
            .await;
        }
        RouteDecision::DefaultLocal { target_port } => {
            fallback_port = target_port;
        }
    }

    let fallback_target = SocketAddr::new("127.0.0.1".parse().unwrap(), fallback_port);

    // Determine routing target based on headers for fallback logging
    let routing_target = determine_routing_target(&routing_context, service_port);
    info!(
        "HTTP {} {} -> {} (routing: {}, trace_id: {:?})",
        http_request.method,
        http_request.path,
        fallback_target,
        routing_target.get_metadata(),
        routing_context.trace_context.trace_id
    );

    let shutdown_rx = connection_registry.subscribe(None).await;
    proxy_stream(
        &mut inbound_stream,
        fallback_target,
        &initial_data,
        shutdown_rx,
    )
    .await
}

async fn proxy_stream(
    inbound_stream: &mut TcpStream,
    target: SocketAddr,
    initial_data: &[u8],
    shutdown_rx: watch::Receiver<bool>,
) -> io::Result<()> {
    let mut outbound_stream = TcpStream::connect(target).await?;

    if !initial_data.is_empty() {
        tokio::io::AsyncWriteExt::write_all(&mut outbound_stream, initial_data).await?;
    }

    match bidirectional_with_shutdown(inbound_stream, &mut outbound_stream, shutdown_rx).await {
        Ok((bytes_tx, bytes_rx)) => {
            debug!(
                "HTTP connection completed via {}: {} bytes tx, {} bytes rx",
                target, bytes_tx, bytes_rx
            );
        }
        Err(err) if is_route_update_error(&err) => {
            debug!(
                "HTTP connection via {} closed due to routing update",
                target
            );
            return Err(err);
        }
        Err(e) => {
            debug!("HTTP connection via {} ended: {}", target, e);
            return Err(e);
        }
    }

    Ok(())
}

async fn proxy_branch_stream(
    inbound_stream: &mut TcpStream,
    service_name: &str,
    port: u16,
    initial_data: &[u8],
    shutdown_rx: watch::Receiver<bool>,
) -> io::Result<()> {
    let mut outbound_stream = TcpStream::connect((service_name, port)).await?;

    if !initial_data.is_empty() {
        tokio::io::AsyncWriteExt::write_all(&mut outbound_stream, initial_data).await?;
    }

    match bidirectional_with_shutdown(inbound_stream, &mut outbound_stream, shutdown_rx).await {
        Ok((bytes_tx, bytes_rx)) => {
            debug!(
                "HTTP connection completed via branch service {}:{}: {} bytes tx, {} bytes rx",
                service_name, port, bytes_tx, bytes_rx
            );
        }
        Err(err) if is_route_update_error(&err) => {
            debug!(
                "HTTP connection via branch service {}:{} closed due to routing update",
                service_name, port
            );
            return Err(err);
        }
        Err(e) => {
            debug!(
                "HTTP connection via branch service {}:{} ended: {}",
                service_name, port, e
            );
            return Err(e);
        }
    }

    Ok(())
}

/// Handle TCP proxying (raw byte forwarding)
async fn handle_tcp_proxy(
    mut inbound_stream: TcpStream,
    local_target: SocketAddr,
    initial_data: Vec<u8>,
    shutdown_rx: watch::Receiver<bool>,
) -> io::Result<()> {
    // Connect to the local service
    let mut outbound_stream = TcpStream::connect(&local_target).await?;

    // Send the initial data we read for protocol detection
    if !initial_data.is_empty() {
        tokio::io::AsyncWriteExt::write_all(&mut outbound_stream, &initial_data).await?;
    }

    // Start bidirectional copying for the rest of the connection
    match bidirectional_with_shutdown(&mut inbound_stream, &mut outbound_stream, shutdown_rx).await
    {
        Ok((bytes_tx, bytes_rx)) => {
            debug!(
                "TCP connection completed: {} bytes tx, {} bytes rx",
                bytes_tx, bytes_rx
            );
        }
        Err(err) if is_route_update_error(&err) => {
            debug!(
                "TCP connection via {} closed due to routing update",
                local_target
            );
            return Err(err);
        }
        Err(e) => {
            debug!("TCP connection ended: {}", e);
            return Err(e);
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
    service_port: u16,
    initial_data: Vec<u8>,
    connection: Arc<DevboxConnection>,
    target_port: u16,
    shutdown_rx: watch::Receiver<bool>,
) -> io::Result<()> {
    let metadata = connection.metadata();

    info!(
        "Routing {} (service port {}) through devbox tunnel (intercept_id={}, workload_id={}, target_port={})",
        client_addr, service_port, metadata.intercept_id, metadata.workload_id, target_port
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
            connection.clear_client().await;
            return Err(io::Error::from(err));
        }
    };

    if !initial_data.is_empty() {
        tokio::io::AsyncWriteExt::write_all(&mut devbox_stream, &initial_data).await?;
    }

    match bidirectional_with_shutdown(&mut inbound_stream, &mut devbox_stream, shutdown_rx).await {
        Ok((bytes_tx, bytes_rx)) => {
            info!(
                "Devbox tunnel completed: {} bytes sent, {} bytes received (intercept_id={})",
                bytes_tx, bytes_rx, metadata.intercept_id
            );
        }
        Err(err) if is_route_update_error(&err) => {
            info!(
                "Devbox tunnel for intercept_id={} closed due to routing update",
                metadata.intercept_id
            );
            return Err(err);
        }
        Err(err) => {
            warn!(
                "Devbox tunnel error for intercept_id={}: {}",
                metadata.intercept_id, err
            );
            connection.clear_client().await;
            return Err(io::Error::from(err));
        }
    }

    if let Err(err) = tokio::io::AsyncWriteExt::shutdown(&mut devbox_stream).await {
        debug!("Failed to shutdown tunnel stream cleanly: {}", err);
    }

    Ok(())
}

async fn bidirectional_with_shutdown<Inbound, Outbound>(
    inbound_stream: &mut Inbound,
    outbound_stream: &mut Outbound,
    mut shutdown_rx: watch::Receiver<bool>,
) -> io::Result<(u64, u64)>
where
    Inbound: AsyncRead + AsyncWrite + Unpin + ?Sized,
    Outbound: AsyncRead + AsyncWrite + Unpin + ?Sized,
{
    let copy_future = copy_bidirectional(inbound_stream, outbound_stream);
    tokio::pin!(copy_future);
    tokio::select! {
        res = &mut copy_future => res,
        recv = shutdown_rx.changed() => {
            match recv {
                Ok(_) => Err(route_update_error()),
                Err(_) => Err(route_update_error()),
            }
        }
    }
}

fn route_update_error() -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionAborted, RouteUpdateError)
}

fn is_route_update_error(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::ConnectionAborted
        && err
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<RouteUpdateError>())
            .is_some()
}

#[derive(Debug)]
struct RouteUpdateError;

impl std::fmt::Display for RouteUpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "connection closed due to routing update")
    }
}

impl std::error::Error for RouteUpdateError {}
