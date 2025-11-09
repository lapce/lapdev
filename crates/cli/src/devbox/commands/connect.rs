use anyhow::{Context, Result};
use colored::Colorize;
use futures::StreamExt;
use lapdev_common::devbox::{DirectCandidateSet, DirectChannelConfig};
use lapdev_devbox_rpc::{DevboxClientRpc, DevboxSessionRpcClient};
use lapdev_rpc::spawn_twoway;
use lapdev_tunnel::direct::{collect_candidates, CandidateOptions, DirectQuicServer};
use lapdev_tunnel::WebSocketTransport;
use lapdev_tunnel::{
    run_tunnel_server, TunnelClient, TunnelError, WebSocketTransport as TunnelWebSocketTransport,
};
use std::time::Duration;
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::Arc,
};
use tarpc::server::{BaseChannel, Channel};
use tokio::{
    signal,
    sync::{mpsc, oneshot, Mutex, RwLock},
    task::JoinHandle,
    time::{sleep, MissedTickBehavior},
};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_util::codec::LengthDelimitedCodec;
use uuid::Uuid;

use crate::{
    auth,
    devbox::dns::{
        is_tcp_protocol, HostsManager, ServiceBridge, ServiceBridgeStartReport, ServiceEndpoint,
        SyntheticIpAllocator,
    },
};

const DEVBOX_HEARTBEAT_INTERVAL_SECS: u64 = 30;

/// Execute the devbox connect command
pub async fn execute(api_host: &str) -> Result<()> {
    let tunnel_manager = DevboxTunnelManager::new(api_host);

    // Load token from keychain, or prompt for login if not found
    let token = match auth::get_token(tunnel_manager.api_host()) {
        Ok(token) => token,
        Err(_) => {
            // No token found, run login flow
            println!(
                "{}",
                "No authentication token found. Starting login...".yellow()
            );
            println!();

            // Run login
            super::login::execute(tunnel_manager.api_host(), None).await?;

            // Retrieve the newly stored token
            auth::get_token(tunnel_manager.api_host())
                .context("Failed to load authentication token after login")?
        }
    };

    // Check hosts file permissions early and warn user
    tunnel_manager.check_and_warn_permissions();

    let mut backoff = Duration::from_secs(1);
    let mut is_first_connection = true;

    {
        let tunnel_manager = tunnel_manager.clone();
        loop {
            match tunnel_manager
                .connect_and_run_rpc(&token, is_first_connection)
                .await
            {
                Ok(should_exit) => {
                    if should_exit {
                        // Clean exit (Ctrl+C or session displaced)
                        break;
                    }
                    // Otherwise fall through to retry
                    tracing::info!("RPC connection closed, will retry...");
                }
                Err(e) => {
                    if is_first_connection {
                        // On first connection, fail fast for auth errors
                        return Err(e);
                    }
                    eprintln!("{} Connection error: {}", "⚠".yellow(), e);
                }
            }

            is_first_connection = false;

            // Wait before reconnecting
            println!(
                "{} Reconnecting in {} seconds...",
                "🔄".cyan(),
                backoff.as_secs()
            );
            tokio::select! {
                _ = signal::ctrl_c() => {
                    println!("\n{}", "Received Ctrl+C, disconnecting...".yellow());
                    break;
                }
                _ = sleep(backoff) => {
                    backoff = (backoff.saturating_mul(2)).min(Duration::from_secs(30));
                }
            }
        }
    }

    tunnel_manager.cleanup_dns().await;
    tunnel_manager.shutdown().await;

    Ok(())
}

/// RPC server implementation for the CLI
/// This handles incoming calls from the server
#[derive(Clone)]
struct DevboxClientRpcServer {
    shutdown_tx: mpsc::UnboundedSender<ShutdownSignal>,
    env_change_tx: mpsc::UnboundedSender<Option<lapdev_devbox_rpc::DevboxEnvironmentInfo>>,
}

impl DevboxClientRpcServer {
    fn new(
        shutdown_tx: mpsc::UnboundedSender<ShutdownSignal>,
        env_change_tx: mpsc::UnboundedSender<Option<lapdev_devbox_rpc::DevboxEnvironmentInfo>>,
    ) -> Self {
        Self {
            shutdown_tx,
            env_change_tx,
        }
    }
}

impl DevboxClientRpc for DevboxClientRpcServer {
    async fn session_displaced(self, _context: tarpc::context::Context, new_device_name: String) {
        let shutdown_tx = self.shutdown_tx;

        let _ = shutdown_tx.send(ShutdownSignal::Displaced(new_device_name.clone()));

        tracing::warn!("Session displaced by: {}", new_device_name);
    }

    async fn environment_changed(
        self,
        _context: tarpc::context::Context,
        environment: Option<lapdev_devbox_rpc::DevboxEnvironmentInfo>,
    ) {
        tracing::info!("environment_changed RPC handler called");
        let env_change_tx = self.env_change_tx;

        if let Some(ref env) = environment {
            println!(
                "\n{} Environment changed to: {} / {}",
                "🔄".cyan(),
                env.cluster_name.bright_white(),
                env.namespace.cyan()
            );
            tracing::info!(
                "Active environment changed to: {} ({})",
                env.environment_id,
                env.namespace
            );
        } else {
            println!("\n{} Active environment cleared", "🔄".cyan());
            tracing::info!("Active environment cleared");
        }

        match env_change_tx.send(environment.clone()) {
            Ok(_) => {
                tracing::info!("Successfully sent environment change to main loop");
            }
            Err(e) => {
                tracing::error!("Failed to send environment change to main loop: {:?}", e);
            }
        }
    }

    async fn ping(self, _context: tarpc::context::Context) -> Result<(), String> {
        tracing::trace!("Received ping");
        Ok(())
    }
}

enum ShutdownSignal {
    Displaced(String),
}

#[derive(Clone)]
struct DevboxTunnelManager {
    api_host: Arc<String>,
    ws_base: Arc<String>,
    intercept_task: Arc<Mutex<Option<TunnelTask>>>,
    client_task: Arc<Mutex<Option<TunnelTask>>>,
    direct_server: Arc<Mutex<Option<DirectServerHandle>>>,
    /// Shared tunnel client for DNS service bridge
    tunnel_client: Arc<RwLock<Option<Arc<TunnelClient>>>>,
    /// Service bridge for DNS resolution
    service_bridge: Arc<ServiceBridge>,
    /// Hosts file manager
    hosts_manager: Arc<HostsManager>,
    /// IP allocator
    ip_allocator: Arc<Mutex<SyntheticIpAllocator>>,
}

impl DevboxTunnelManager {
    fn new(api_host: impl Into<String>) -> Self {
        let api_host = api_host.into();
        let api_host = api_host.trim().trim_end_matches('/').to_string();
        let ws_base = format!("wss://{}", api_host);

        Self {
            api_host: Arc::new(api_host),
            ws_base: Arc::new(ws_base),
            intercept_task: Arc::new(Mutex::new(None)),
            client_task: Arc::new(Mutex::new(None)),
            direct_server: Arc::new(Mutex::new(None)),
            tunnel_client: Arc::new(RwLock::new(None)),
            service_bridge: Arc::new(ServiceBridge::new()),
            hosts_manager: Arc::new(HostsManager::new()),
            ip_allocator: Arc::new(Mutex::new(SyntheticIpAllocator::new())),
        }
    }

    fn api_host(&self) -> &str {
        self.api_host.as_str()
    }

    fn ws_base(&self) -> &str {
        self.ws_base.as_str()
    }

    async fn connect_and_run_rpc(&self, token: &str, is_first_connection: bool) -> Result<bool> {
        if is_first_connection {
            println!("{}", "🔌 Connecting to Lapdev devbox...".cyan());
        } else {
            println!("{}", "🔌 Reconnecting to Lapdev devbox...".cyan());
        }

        // Construct WebSocket URL
        let ws_url = format!("{}/api/v1/kube/devbox/rpc", self.ws_base());

        tracing::info!("Connecting to: {}", ws_url);

        // Create WebSocket request with authentication
        let mut request = ws_url
            .into_client_request()
            .context("Failed to create WebSocket request")?;

        // Add authentication header
        let headers = request.headers_mut();
        headers.insert(
            "Authorization",
            format!("Bearer {}", token)
                .parse()
                .context("Failed to parse authorization header")?,
        );

        // TODO: Add client version header
        // headers.insert("X-Lapdev-Client-Version", env!("CARGO_PKG_VERSION").parse()?);

        // Connect WebSocket
        let (stream, _response) = tokio_tungstenite::connect_async(request)
            .await
            .context("Failed to establish WebSocket connection")?;

        println!("{}", "✓ WebSocket connection established".green());

        // Create transport layer
        let trans = WebSocketTransport::new(stream);
        let io = LengthDelimitedCodec::builder().new_framed(trans);

        // Set up bidirectional RPC
        let transport =
            tarpc::serde_transport::new(io, tarpc::tokio_serde::formats::Bincode::default());
        let (server_chan, client_chan, _abort_handle) = spawn_twoway(transport);

        // Create channels for this connection
        let (shutdown_tx, mut shutdown_rx) = mpsc::unbounded_channel::<ShutdownSignal>();
        let (env_change_tx, mut env_change_rx) =
            mpsc::unbounded_channel::<Option<lapdev_devbox_rpc::DevboxEnvironmentInfo>>();

        // Create RPC client (for calling server methods)
        let rpc_client =
            DevboxSessionRpcClient::new(tarpc::client::Config::default(), client_chan).spawn();

        // Call whoami to get session info
        let session_info = match rpc_client
            .whoami(tarpc::context::current())
            .await
            .context("RPC call failed")?
        {
            Ok(info) => {
                println!(
                    "{} Connected as {} ({})",
                    "✓".green(),
                    info.email.bright_white().bold(),
                    info.device_name.cyan()
                );
                println!(
                    "  Session expires: {}",
                    info.expires_at
                        .format("%Y-%m-%d %H:%M:%S UTC")
                        .to_string()
                        .dimmed()
                );
                info
            }
            Err(e) => {
                eprintln!("{} Failed to get session info: {}", "✗".red(), e);
                return Err(anyhow::anyhow!("Authentication failed: {}", e));
            }
        };

        let direct_port = self
            .ensure_direct_server()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to start direct server: {}", e))?;
        let mut candidate_opts = CandidateOptions::default();
        candidate_opts.preferred_port = Some(direct_port);
        let discovery = collect_candidates(&candidate_opts);
        match rpc_client
            .publish_direct_candidates(tarpc::context::current(), discovery.candidates)
            .await
        {
            Ok(Ok(config)) => {
                self.configure_direct_server(&config).await;
                tracing::info!(
                    detected_interface = ?discovery.diagnostics.detected_interface,
                    relay_candidates = discovery.diagnostics.relay_candidates,
                    "Direct channel negotiation initialized"
                );
            }
            Ok(Err(err)) => {
                tracing::warn!("Failed to publish direct candidates: {}", err);
            }
            Err(err) => {
                tracing::warn!("Direct candidate RPC transport error: {}", err);
            }
        }

        // Create RPC server (for server to call us)
        let client_rpc_server =
            DevboxClientRpcServer::new(shutdown_tx.clone(), env_change_tx.clone());

        // Spawn the RPC server task
        let server_task = tokio::spawn(async move {
            tracing::info!("Starting DevboxClientRpc server...");
            BaseChannel::with_defaults(server_chan)
                .execute(client_rpc_server.serve())
                .for_each(|resp| async move {
                    tokio::spawn(resp);
                })
                .await;
            tracing::info!("DevboxClientRpc server stopped");
        });

        self.ensure_client(token, session_info.user_id)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to start client tunnel: {}", e))?;

        if let Err(err) = self.ensure_intercept(token, session_info.user_id).await {
            tracing::error!(
                user_id = %session_info.user_id,
                "Failed to start intercept tunnel: {}",
                err
            );
        }

        // Attempt to rehydrate active environment and intercepts
        let active_environment = match rpc_client
            .get_active_environment(tarpc::context::current())
            .await
            .context("RPC call failed")?
        {
            Ok(Some(env)) => {
                println!(
                    "  Active environment: {} / {}",
                    env.cluster_name.bright_white(),
                    env.namespace.cyan()
                );
                Some(env)
            }
            Ok(None) => {
                println!("{} No active environment selected", "ℹ".blue());
                None
            }
            Err(err) => {
                eprintln!(
                    "{} Failed to fetch active environment: {}",
                    "⚠".yellow(),
                    err
                );
                None
            }
        };

        if let Some(env) = active_environment.clone() {
            match rpc_client
                .list_workload_intercepts(tarpc::context::current(), env.environment_id)
                .await
                .context("RPC call failed")?
            {
                Ok(intercepts) => {
                    for intercept in intercepts {
                        if intercept.device_name != session_info.device_name {
                            continue;
                        }

                        println!(
                            "  {} Intercept active for {}/{} ({} port(s))",
                            "↻".green(),
                            intercept.namespace.bright_white(),
                            intercept.workload_name.cyan(),
                            intercept.port_mappings.len()
                        );
                    }
                }
                Err(err) => {
                    eprintln!("{} Failed to list intercepts: {}", "⚠".yellow(), err);
                }
            }

            // Set up DNS for the active environment
            if let Err(e) = self
                .setup_dns_for_environment(env.environment_id, &rpc_client)
                .await
            {
                eprintln!("{} Failed to set up DNS: {}", "⚠".yellow(), e);
            }
        }

        if is_first_connection {
            println!("{}", "\n✓ Connected and ready".green());
            println!("{}", "  Press Ctrl+C to disconnect".dimmed());
        } else {
            println!("{}", "✓ Reconnected successfully".green());
        }

        let mut server_task = server_task;
        let mut should_exit = false;
        let mut heartbeat_interval =
            tokio::time::interval(Duration::from_secs(DEVBOX_HEARTBEAT_INTERVAL_SECS));
        heartbeat_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = heartbeat_interval.tick() => {
                    if let Err(err) = Self::send_session_heartbeat(&rpc_client).await {
                        tracing::warn!(
                            user_id = %session_info.user_id,
                            "Devbox session heartbeat failed: {}",
                            err
                        );
                        eprintln!("{} Connection heartbeat lost, reconnecting...", "⚠".yellow());
                        break;
                    }
                }
                res = &mut server_task => {
                    match res {
                        Ok(()) => {
                            tracing::info!("RPC server task completed normally");
                        }
                        Err(e) => {
                            tracing::warn!("RPC server task failed: {}", e);
                        }
                    }
                    // Connection closed, retry
                    break;
                }
                _ = signal::ctrl_c() => {
                    println!("\n{}", "Received Ctrl+C, disconnecting...".yellow());
                    should_exit = true;
                    break;
                }
                maybe_signal = shutdown_rx.recv() => {
                    if let Some(signal) = maybe_signal {
                        match signal {
                            ShutdownSignal::Displaced(device) => {
                                println!(
                                    "\n{} Session displaced by new login from: {}",
                                    "⚠".yellow(),
                                    device.bright_white()
                                );
                                should_exit = true;
                            }
                        }
                    }
                    break;
                }
                Some(env) = env_change_rx.recv() => {
                    tracing::info!("Event loop received environment change notification");
                    match env {
                        Some(env_info) => {
                            tracing::info!("Setting up DNS for environment {}", env_info.environment_id);
                            if let Err(e) = self.setup_dns_for_environment(env_info.environment_id, &rpc_client).await {
                                eprintln!("{} Failed to refresh DNS: {}", "⚠".yellow(), e);
                            } else {
                                tracing::info!("DNS setup completed successfully");
                            }
                        }
                        None => {
                            tracing::info!("Clearing DNS entries");
                            println!("{} Clearing DNS entries...", "🔧".cyan());
                            self.cleanup_dns().await;
                        }
                    }
                    // Continue the loop instead of exiting
                }
            }
        }

        if !server_task.is_finished() {
            server_task.abort();
        }

        // Return true if we should exit completely, false to retry connection
        Ok(should_exit)
    }

    /// Get the tunnel client for making connections
    async fn get_tunnel_client(&self) -> Option<Arc<TunnelClient>> {
        self.tunnel_client.read().await.clone()
    }

    /// Register a newly connected tunnel client and update shared state
    async fn set_tunnel_client(&self, client: Arc<TunnelClient>) {
        {
            let mut guard = self.tunnel_client.write().await;
            *guard = Some(Arc::clone(&client));
        }
        // Propagate the new client to the service bridge so future listeners use it
        self.service_bridge.set_tunnel_client(client).await;
    }

    /// Clear the stored tunnel client if it matches the provided instance
    async fn clear_tunnel_client(&self, client: &Arc<TunnelClient>) {
        let mut guard = self.tunnel_client.write().await;
        if guard
            .as_ref()
            .map_or(false, |current| Arc::ptr_eq(current, client))
        {
            *guard = None;
        }
    }

    /// Check hosts file permissions and warn user if insufficient
    fn check_and_warn_permissions(&self) {
        if !self.hosts_manager.check_permissions() {
            eprintln!();
            eprintln!(
                "{} {}",
                "⚠".yellow(),
                "Insufficient permissions to modify hosts file"
                    .yellow()
                    .bold()
            );
            eprintln!(
                "  Service DNS resolution will not work without write access to the hosts file."
            );
            eprintln!();
            if cfg!(unix) {
                eprintln!("  To fix this, run the command with sudo:");
                eprintln!("    {}", "sudo -E lapdev devbox connect".bright_white());
            } else if cfg!(windows) {
                eprintln!("  To fix this, run the command as Administrator:");
                eprintln!("    Right-click the terminal and select 'Run as Administrator'");
            }
            eprintln!();
            eprintln!(
                "  Alternatively, you can manually add entries to the hosts file when prompted."
            );
            eprintln!();
        }
    }

    /// Set up DNS for an environment by fetching services and configuring hosts/bridge
    async fn setup_dns_for_environment(
        &self,
        env_id: Uuid,
        rpc_client: &DevboxSessionRpcClient,
    ) -> Result<()> {
        println!("{} Setting up DNS for services...", "🔧".cyan());

        // Check hosts file permissions upfront
        if !self.hosts_manager.check_permissions() {
            eprintln!("{} No write permission for hosts file", "⚠".yellow());
            eprintln!(
                "  DNS will not work until you grant permissions or manually update the hosts file."
            );
            if cfg!(unix) {
                eprintln!("  Try running with: sudo -E lapdev devbox connect");
            } else if cfg!(windows) {
                eprintln!("  Try running as Administrator");
            }
            eprintln!();
        }

        // Fetch services for the environment
        let services = match rpc_client
            .list_services(tarpc::context::current(), env_id)
            .await
            .context("RPC call failed")?
        {
            Ok(services) => services,
            Err(e) => {
                eprintln!("{} Failed to fetch services: {}", "⚠".yellow(), e);
                return Ok(());
            }
        };

        if services.is_empty() {
            println!("{} No services found in environment", "ℹ".blue());
            return Ok(());
        }

        // Allocate synthetic IPs and create endpoints
        let mut bridge_endpoints = Vec::new();
        let mut hosts_endpoints = Vec::new();
        let mut seen_services = HashSet::new();
        let mut skipped_services = HashSet::new();
        let mut skipped_endpoints = 0usize;
        let mut skipped_non_tcp: HashMap<String, Vec<(u16, String)>> = HashMap::new();
        let mut allocator = self.ip_allocator.lock().await;
        allocator.clear();

        for service in &services {
            for port in &service.ports {
                if !is_tcp_protocol(&port.protocol) {
                    skipped_non_tcp
                        .entry(format!("{}.{}", service.name, service.namespace))
                        .or_default()
                        .push((port.port, port.protocol.clone()));
                    continue;
                }

                let service_key = format!("{}.{}", service.name, service.namespace);
                if let Some(ip) = allocator.allocate(&service.name, &service.namespace) {
                    let endpoint =
                        ServiceEndpoint::new(service, port.port, port.protocol.clone(), ip);

                    if seen_services.insert(service_key) {
                        hosts_endpoints.push(endpoint.clone());
                    }

                    bridge_endpoints.push(endpoint);
                } else {
                    skipped_endpoints += 1;
                    if skipped_services.insert(service_key.clone()) {
                        tracing::warn!(
                            service = %service.name,
                            namespace = %service.namespace,
                            "Synthetic IP pool exhausted; skipping service"
                        );
                    }
                }
            }
        }
        drop(allocator);

        if !skipped_non_tcp.is_empty() {
            let skipped_port_count: usize = skipped_non_tcp.values().map(|ports| ports.len()).sum();
            let service_count = skipped_non_tcp.len();
            eprintln!(
                "{} Skipped {} non-TCP port(s) across {} service(s); UDP forwarding is not supported.",
                "ℹ".blue(),
                skipped_port_count,
                service_count
            );
            for (service, ports) in skipped_non_tcp.iter() {
                let formatted_ports: Vec<String> = ports
                    .iter()
                    .map(|(port, protocol)| format!("{}/{}", port, protocol.to_uppercase()))
                    .collect();
                eprintln!("    {}: {}", service, formatted_ports.join(", "));
            }
        }

        println!(
            "  {} service endpoint(s) allocated ({} host entry/entries)",
            bridge_endpoints.len(),
            hosts_endpoints.len()
        );

        if !skipped_services.is_empty() {
            eprintln!(
                "{} Synthetic IP pool exhausted. Skipped {} endpoint(s) across {} service(s).",
                "⚠".yellow(),
                skipped_endpoints,
                skipped_services.len()
            );
            eprintln!(
                "  Active connections may be incomplete. Please try reducing DNS scope or filing a support ticket."
            );
        }

        let bridge_report: ServiceBridgeStartReport =
            self.service_bridge.start(bridge_endpoints).await;

        if !bridge_report.failed.is_empty() {
            eprintln!(
                "{} Failed to bind {} endpoint(s). These services will remain unresolved locally:",
                "⚠".yellow(),
                bridge_report.failed.len()
            );
            for (endpoint, err) in &bridge_report.failed {
                eprintln!(
                    "    {}/{}:{} ({}) - {}",
                    endpoint.namespace,
                    endpoint.service_name,
                    endpoint.port,
                    endpoint.protocol,
                    err
                );
            }
        }

        if !bridge_report.started.is_empty() {
            println!(
                "{} Service bridge started ({} endpoint(s) active)",
                "✓".green(),
                bridge_report.started.len()
            );
        } else {
            println!(
                "{} Service bridge not running; no endpoints could be activated",
                "⚠".yellow()
            );
        }

        let active_service_keys: HashSet<String> = bridge_report
            .started
            .iter()
            .map(|endpoint| format!("{}.{}", endpoint.service_name, endpoint.namespace))
            .collect();

        let mut effective_hosts = hosts_endpoints;
        effective_hosts.retain(|endpoint| {
            let key = format!("{}.{}", endpoint.service_name, endpoint.namespace);
            active_service_keys.contains(&key)
        });

        if effective_hosts.len() < active_service_keys.len() {
            tracing::warn!(
                filtered_services = active_service_keys.len() - effective_hosts.len(),
                "Filtered out host entries for services without active listeners"
            );
        }

        // Update hosts file to reflect active listeners only
        match self.hosts_manager.write_entries(&effective_hosts).await {
            Ok(()) => {
                if effective_hosts.is_empty() {
                    println!(
                        "{} No hosts entries written (no active listeners)",
                        "ℹ".blue()
                    );
                } else {
                    println!(
                        "{} Hosts file updated with {} service entr{}",
                        "✓".green(),
                        effective_hosts.len(),
                        if effective_hosts.len() == 1 {
                            "y"
                        } else {
                            "ies"
                        }
                    );
                }
            }
            Err(e) => {
                eprintln!("{} Failed to update hosts file: {}", "⚠".yellow(), e);
                if !effective_hosts.is_empty() {
                    self.hosts_manager
                        .print_manual_instructions(&effective_hosts);
                }
            }
        }

        Ok(())
    }

    /// Cleanup DNS entries
    async fn cleanup_dns(&self) {
        if let Err(e) = self.hosts_manager.remove_entries().await {
            tracing::warn!("Failed to clean up hosts file: {}", e);
        }
        self.service_bridge.stop().await;
        self.ip_allocator.lock().await.clear();
    }

    async fn send_session_heartbeat(rpc_client: &DevboxSessionRpcClient) -> Result<(), String> {
        match rpc_client.heartbeat(tarpc::context::current()).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(err)) => Err(err),
            Err(err) => Err(err.to_string()),
        }
    }

    async fn ensure_direct_server(&self) -> Result<u16, String> {
        if let Some(port) = {
            let guard = self.direct_server.lock().await;
            guard.as_ref().map(|handle| handle.port())
        } {
            return Ok(port);
        }

        let handle = Self::start_direct_server_task().await?;
        let port = handle.port();
        let mut guard = self.direct_server.lock().await;
        *guard = Some(handle);
        Ok(port)
    }

    async fn configure_direct_server(&self, config: &DirectChannelConfig) {
        let credential = {
            let guard = self.direct_server.lock().await;
            guard.as_ref().map(|handle| Arc::clone(&handle.credential))
        };

        if let Some(store) = credential {
            let mut guard = store.write().await;
            *guard = Some(config.credential.token.clone());
        }
    }

    async fn stop_direct_server(&self) {
        let handle = {
            let mut guard = self.direct_server.lock().await;
            guard.take()
        };

        if let Some(handle) = handle {
            handle.shutdown().await;
        }
    }

    async fn start_direct_server_task() -> Result<DirectServerHandle, String> {
        let bind_addr = SocketAddr::from(([0, 0, 0, 0], 0));
        let server = Arc::new(
            DirectQuicServer::bind(bind_addr)
                .await
                .map_err(|e| e.to_string())?,
        );
        let port = server.local_addr().map_err(|e| e.to_string())?.port();
        let credential = server.credential_handle();
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

        let server_task = Arc::clone(&server);
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        server_task.close();
                        break;
                    }
                    result = server_task.accept() => {
                        match result {
                            Ok(transport) => {
                                tokio::spawn(async move {
                                    if let Err(err) = run_tunnel_server(transport).await {
                                        tracing::warn!(
                                            error = %err,
                                            "Direct tunnel server error"
                                        );
                                    }
                                });
                            }
                            Err(err) => {
                                tracing::warn!(
                                    error = %err,
                                    "Failed to accept direct tunnel connection"
                                );
                            }
                        }
                    }
                }
            }
        });

        Ok(DirectServerHandle {
            credential,
            server,
            port,
            shutdown: Some(shutdown_tx),
            task,
        })
    }
}

struct TunnelTask {
    kind: TunnelKind,
    intercept_id: Option<Uuid>,
    shutdown: oneshot::Sender<()>,
    handle: JoinHandle<()>,
}

struct DirectServerHandle {
    credential: Arc<RwLock<Option<String>>>,
    server: Arc<DirectQuicServer>,
    port: u16,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl DirectServerHandle {
    fn port(&self) -> u16 {
        self.port
    }

    async fn set_token(&self, token: String) {
        let mut guard = self.credential.write().await;
        *guard = Some(token);
    }

    async fn shutdown(mut self) {
        self.server.close();
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if !self.task.is_finished() {
            self.task.abort();
        }
    }
}

#[derive(Copy, Clone)]
enum TunnelKind {
    Intercept,
    Client,
}

impl TunnelKind {
    fn path(self) -> &'static str {
        match self {
            TunnelKind::Intercept => "intercept",
            TunnelKind::Client => "client",
        }
    }

    fn as_str(self) -> &'static str {
        self.path()
    }
}

impl DevboxTunnelManager {
    async fn ensure_intercept(&self, token: &str, user_id: Uuid) -> Result<(), String> {
        let mut guard = self.intercept_task.lock().await;
        if guard.is_some() {
            return Ok(());
        }

        let task = self.spawn_tunnel_task(TunnelKind::Intercept, token, user_id, None);

        *guard = Some(task);
        Ok(())
    }

    async fn ensure_client(&self, token: &str, user_id: Uuid) -> Result<(), String> {
        let mut guard = self.client_task.lock().await;
        if guard.is_some() {
            return Ok(());
        }

        let task = self.spawn_tunnel_task(TunnelKind::Client, token, user_id, None);
        *guard = Some(task);
        Ok(())
    }

    async fn stop_client(&self) {
        if let Some(task) = self.client_task.lock().await.take() {
            Self::stop_task(task, "Devbox client tunnel").await;
        }
    }

    async fn stop_intercept(&self) {
        if let Some(task) = self.intercept_task.lock().await.take() {
            Self::stop_task(task, "Devbox intercept tunnel").await;
        }
    }

    async fn shutdown(&self) {
        self.stop_direct_server().await;
        self.stop_intercept().await;
        self.stop_client().await;
    }

    async fn stop_task(task: TunnelTask, context: &str) {
        let TunnelTask {
            kind,
            intercept_id,
            shutdown,
            handle,
        } = task;
        let _ = shutdown.send(());
        if let Err(err) = handle.await {
            tracing::warn!(
                tunnel_kind = kind.as_str(),
                intercept_id = intercept_id.map(|id| id.to_string()),
                error = %err,
                "{} task exited with error",
                context
            );
        }
    }

    fn spawn_tunnel_task(
        &self,
        kind: TunnelKind,
        token: &str,
        user_id: Uuid,
        intercept_id: Option<Uuid>,
    ) -> TunnelTask {
        let manager = self.clone();

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let token = token.to_string();
        let handle = tokio::spawn(async move {
            manager
                .run_tunnel_loop(kind, token, user_id, intercept_id, shutdown_rx)
                .await;
        });

        TunnelTask {
            kind,
            intercept_id,
            shutdown: shutdown_tx,
            handle,
        }
    }
}

impl DevboxTunnelManager {
    async fn run_tunnel_loop(
        &self,
        kind: TunnelKind,
        token: String,
        user_id: Uuid,
        intercept_id: Option<Uuid>,
        mut shutdown_rx: oneshot::Receiver<()>,
    ) {
        let ws_url = format!(
            "{}/api/v1/kube/devbox/tunnel/{}/{}",
            self.ws_base(),
            kind.path(),
            user_id
        );
        let intercept_id_str = intercept_id.map(|id| id.to_string());

        let mut backoff = Duration::from_secs(1);

        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    tracing::info!(
                        %user_id,
                        tunnel_kind = kind.as_str(),
                        intercept_id = intercept_id_str.as_deref(),
                        "Devbox tunnel shutdown signal received"
                    );
                    break;
                }
                result = self.connect_and_run_tunnel(kind, &ws_url, &token) => {
                    match result {
                        Ok(()) => {
                            tracing::info!(
                                %user_id,
                                intercept_id = intercept_id_str.as_deref(),
                                "Devbox {} tunnel closed gracefully", kind.as_str()
                            );
                            backoff = Duration::from_secs(1);
                        }
                        Err(err) => {
                            tracing::warn!(
                                %user_id,
                                intercept_id = intercept_id_str.as_deref(),
                                "Devbox {} tunnel disconnected: {}", kind.as_str(),
                                err
                            );
                            backoff = (backoff.saturating_mul(2)).min(Duration::from_secs(30));
                        }
                    }

                    tokio::select! {
                        _ = &mut shutdown_rx => {
                            tracing::info!(
                                %user_id,
                                intercept_id = intercept_id_str.as_deref(),
                                "Devbox {} tunnel shutdown signal received", kind.as_str()
                            );
                            break;
                        }
                        _ = sleep(backoff) => {}
                    }
                }
            }
        }
    }

    async fn connect_and_run_tunnel(
        &self,
        kind: TunnelKind,
        ws_url: &str,
        token: &str,
    ) -> Result<(), TunnelError> {
        let mut request = ws_url
            .into_client_request()
            .map_err(tunnel_transport_error)?;

        let header = format!("Bearer {}", token)
            .parse()
            .map_err(tunnel_transport_error)?;
        request
            .headers_mut()
            .insert(http::header::AUTHORIZATION, header);

        let (stream, _) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(tunnel_transport_error)?;

        tracing::info!("Devbox {} tunnel connected: {}", kind.as_str(), ws_url);

        let transport = TunnelWebSocketTransport::new(stream);

        if matches!(kind, TunnelKind::Client) {
            let client = Arc::new(TunnelClient::connect(transport));
            tracing::info!("Tunnel client exposed for DNS service bridge");

            self.set_tunnel_client(Arc::clone(&client)).await;

            client.closed().await;
            tracing::info!("Tunnel client closed for DNS service bridge");

            self.clear_tunnel_client(&client).await;
            Ok(())
        } else {
            run_tunnel_server(transport).await
        }
    }
}

fn tunnel_transport_error<E>(err: E) -> TunnelError
where
    E: std::fmt::Display,
{
    TunnelError::Transport(std::io::Error::new(
        std::io::ErrorKind::Other,
        err.to_string(),
    ))
}
