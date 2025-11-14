use anyhow::{Context, Result};
use chrono::Utc;
use colored::Colorize;
use futures::StreamExt;
use lapdev_common::devbox::{DirectTunnelConfig, DirectTunnelCredential};
use lapdev_devbox_rpc::{DevboxClientRpc, DevboxSessionRpcClient};
use lapdev_rpc::spawn_twoway;
use lapdev_tunnel::direct::DirectEndpoint;
use lapdev_tunnel::{
    run_tunnel_server, websocket_serde_transport, RelayEndpoint, TunnelClient, TunnelError,
};
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::Arc,
    time::Duration,
};
use tarpc::server::{BaseChannel, Channel};
use tokio::{
    signal,
    sync::{mpsc, oneshot, Mutex, RwLock},
    task::JoinHandle,
    time::{sleep, timeout, MissedTickBehavior},
};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use uuid::Uuid;

use crate::{
    auth,
    devbox::dns::{
        is_tcp_protocol, HostsManager, ServiceBridge, ServiceBridgeStartReport, ServiceEndpoint,
        SyntheticIpAllocator,
    },
};

const DEVBOX_HEARTBEAT_INTERVAL_SECS: u64 = 30;
const DEVBOX_HEARTBEAT_TIMEOUT_SECS: u64 = 10;

#[derive(Debug)]
enum HeartbeatEvent {
    Failed(String),
    TimedOut,
}

/// Execute the devbox connect command
pub async fn execute(api_host: &str) -> Result<()> {
    let tunnel_manager = DevboxTunnelManager::new(api_host)
        .await
        .map_err(|err| anyhow::anyhow!("Failed to initialize Devbox tunnel manager: {}", err))?;

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
    tunnel_manager: DevboxTunnelManager,
}

impl DevboxClientRpcServer {
    fn new(
        shutdown_tx: mpsc::UnboundedSender<ShutdownSignal>,
        env_change_tx: mpsc::UnboundedSender<Option<lapdev_devbox_rpc::DevboxEnvironmentInfo>>,
        tunnel_manager: DevboxTunnelManager,
    ) -> Self {
        Self {
            shutdown_tx,
            env_change_tx,
            tunnel_manager,
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

    async fn request_direct_config(
        self,
        _context: tarpc::context::Context,
        user_id: Uuid,
        stun_observed_addr: Option<SocketAddr>,
    ) -> Result<Option<DirectTunnelConfig>, String> {
        self.tunnel_manager
            .build_direct_channel_config(user_id, stun_observed_addr)
            .await
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
    active_environment: Arc<Mutex<Option<Uuid>>>,
    direct_endpoint: Arc<DirectEndpoint>,
    /// Shared tunnel client for DNS service bridge
    tunnel_client: Arc<RwLock<Option<Arc<TunnelClient>>>>,
    /// Service bridge for DNS resolution
    service_bridge: Arc<ServiceBridge>,
    /// Hosts file manager
    hosts_manager: Arc<HostsManager>,
    /// IP allocator
    ip_allocator: Arc<Mutex<SyntheticIpAllocator>>,
    client_token: Arc<RwLock<Option<String>>>,
    client_user_id: Arc<RwLock<Option<Uuid>>>,
}

impl DevboxTunnelManager {
    async fn new(api_host: impl Into<String>) -> Result<Self, TunnelError> {
        let api_host = api_host.into();
        let api_host = api_host.trim().trim_end_matches('/').to_string();
        let ws_base = format!("wss://{}", api_host);
        let tunnel_client = Arc::new(RwLock::new(None));
        let direct_endpoint = Arc::new(DirectEndpoint::bind().await?);

        {
            let direct_endpoint = direct_endpoint.clone();
            tokio::spawn(async move {
                direct_endpoint.start_tunnel_server().await;
            });
        }

        Ok(Self {
            api_host: Arc::new(api_host),
            ws_base: Arc::new(ws_base),
            intercept_task: Arc::new(Mutex::new(None)),
            client_task: Arc::new(Mutex::new(None)),
            active_environment: Arc::new(Mutex::new(None)),
            direct_endpoint,
            tunnel_client: tunnel_client.clone(),
            service_bridge: Arc::new(ServiceBridge::new(tunnel_client)),
            hosts_manager: Arc::new(HostsManager::new()),
            ip_allocator: Arc::new(Mutex::new(SyntheticIpAllocator::new())),
            client_token: Arc::new(RwLock::new(None)),
            client_user_id: Arc::new(RwLock::new(None)),
        })
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
        // Set up bidirectional RPC over the websocket stream
        let transport =
            websocket_serde_transport(stream, tarpc::tokio_serde::formats::Bincode::default());
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

        // Create RPC server (for server to call us)
        let client_rpc_server =
            DevboxClientRpcServer::new(shutdown_tx.clone(), env_change_tx.clone(), self.clone());

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

        self.ensure_client(&rpc_client, token, session_info.user_id)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to start client tunnel: {}", e))?;

        if let Err(err) = self
            .ensure_intercept(&rpc_client, token, session_info.user_id)
            .await
        {
            tracing::error!(
                user_id = %session_info.user_id,
                "Failed to start intercept tunnel: {}",
                err
            );
        }

        {
            let rpc_client = rpc_client.clone();
            let env_change_tx = env_change_tx.clone();
            tokio::spawn(async move {
                let result = rpc_client
                    .get_active_environment(tarpc::context::current())
                    .await;
                match result {
                    Ok(Ok(env)) => {
                        if env_change_tx.send(env).is_err() {
                            tracing::debug!(
                                "Failed to deliver initial active environment; receiver dropped"
                            );
                        }
                    }
                    Ok(Err(err)) => {
                        eprintln!(
                            "{} Failed to fetch active environment: {}",
                            "⚠".yellow(),
                            err
                        );
                    }
                    Err(err) => {
                        eprintln!(
                            "{} Failed to fetch active environment: {}",
                            "⚠".yellow(),
                            err
                        );
                    }
                }
            });
        }

        if is_first_connection {
            println!("{}", "\n✓ Connected and ready".green());
            println!("{}", "  Press Ctrl+C to disconnect".dimmed());
        } else {
            println!("{}", "✓ Reconnected successfully".green());
        }

        let mut server_task = server_task;
        let mut should_exit = false;
        let (heartbeat_event_tx, mut heartbeat_event_rx) =
            mpsc::unbounded_channel::<HeartbeatEvent>();
        let (mut heartbeat_task, heartbeat_cancel_tx) = Self::spawn_heartbeat_task(
            rpc_client.clone(),
            session_info.user_id,
            heartbeat_event_tx,
        );

        loop {
            tokio::select! {
                heartbeat_event = heartbeat_event_rx.recv() => {
                    self.handle_heartbeat_event(session_info.user_id, heartbeat_event);
                    break;
                }
                res = &mut server_task => {
                    self.handle_server_task_result(res);
                    break;
                }
                _ = signal::ctrl_c() => {
                    println!("\n{}", "Received Ctrl+C, disconnecting...".yellow());
                    should_exit = true;
                    break;
                }
                maybe_signal = shutdown_rx.recv() => {
                    if self.handle_shutdown_signal(maybe_signal) {
                        should_exit = true;
                    }
                    break;
                }
                notification = env_change_rx.recv() => {
                    match notification {
                        Some(env) => {
                            tracing::info!("Event loop received environment change notification");
                            self
                                .handle_environment_notification(env, &rpc_client, &session_info)
                                .await;
                        }
                        None => {
                            tracing::warn!("Environment change channel closed unexpectedly");
                            break;
                        }
                    }
                }
            }
        }

        if !server_task.is_finished() {
            server_task.abort();
        }
        if !heartbeat_task.is_finished() {
            let _ = heartbeat_cancel_tx.send(());
        }
        if let Err(err) = heartbeat_task.await {
            tracing::warn!("Heartbeat task join error: {}", err);
        }

        // Return true if we should exit completely, false to retry connection
        Ok(should_exit)
    }

    fn handle_heartbeat_event(&self, user_id: Uuid, event: Option<HeartbeatEvent>) {
        match event {
            Some(HeartbeatEvent::Failed(err)) => {
                tracing::warn!(
                    user_id = %user_id,
                    error = %err,
                    "Devbox session heartbeat failed"
                );
                eprintln!(
                    "{} Connection heartbeat lost ({}), reconnecting...",
                    "⚠".yellow(),
                    err
                );
            }
            Some(HeartbeatEvent::TimedOut) => {
                tracing::warn!(
                    user_id = %user_id,
                    timeout_secs = DEVBOX_HEARTBEAT_TIMEOUT_SECS,
                    "Devbox session heartbeat timed out"
                );
                eprintln!(
                    "{} Connection heartbeat timed out, reconnecting...",
                    "⚠".yellow()
                );
            }
            None => {
                tracing::warn!(
                    user_id = %user_id,
                    "Heartbeat channel closed unexpectedly"
                );
                eprintln!("{} Heartbeat channel closed, reconnecting...", "⚠".yellow());
            }
        }
    }

    fn handle_server_task_result(&self, result: Result<(), tokio::task::JoinError>) {
        match result {
            Ok(()) => {
                tracing::info!("RPC server task completed normally");
            }
            Err(err) => {
                tracing::warn!("RPC server task failed: {}", err);
            }
        }
    }

    fn handle_shutdown_signal(&self, signal: Option<ShutdownSignal>) -> bool {
        match signal {
            Some(ShutdownSignal::Displaced(device)) => {
                println!(
                    "\n{} Session displaced by new login from: {}",
                    "⚠".yellow(),
                    device.bright_white()
                );
                true
            }
            None => {
                tracing::warn!("Shutdown channel closed unexpectedly");
                false
            }
        }
    }

    async fn handle_environment_notification(
        &self,
        environment: Option<lapdev_devbox_rpc::DevboxEnvironmentInfo>,
        rpc_client: &DevboxSessionRpcClient,
        session_info: &lapdev_devbox_rpc::DevboxSessionInfo,
    ) {
        {
            *self.active_environment.lock().await = environment.as_ref().map(|e| e.environment_id);
        }

        match environment {
            Some(env) => {
                self.process_active_environment(env, rpc_client, session_info)
                    .await;
            }
            None => {
                self.process_environment_cleared().await;
            }
        }
    }

    async fn process_active_environment(
        &self,
        env: lapdev_devbox_rpc::DevboxEnvironmentInfo,
        rpc_client: &DevboxSessionRpcClient,
        session_info: &lapdev_devbox_rpc::DevboxSessionInfo,
    ) {
        println!(
            "  Active environment: {} / {}",
            env.cluster_name.bright_white(),
            env.namespace.cyan()
        );

        self.print_device_intercepts(env.environment_id, rpc_client, session_info)
            .await;

        tracing::info!("Setting up DNS for environment {}", env.environment_id);
        match self
            .setup_dns_for_environment(env.environment_id, rpc_client)
            .await
        {
            Ok(()) => tracing::info!("DNS setup completed successfully"),
            Err(err) => eprintln!("{} Failed to refresh DNS: {}", "⚠".yellow(), err),
        }

        match self.restart_client_tunnel(rpc_client).await {
            Ok(()) => {
                tracing::info!(
                    environment_id = %env.environment_id,
                    "Client tunnel restarted after environment change"
                );
            }
            Err(err) => {
                tracing::warn!(
                    environment_id = %env.environment_id,
                    error = %err,
                    "Failed to restart client tunnel after environment change"
                );
            }
        }
    }

    async fn process_environment_cleared(&self) {
        println!("{} No active environment selected", "ℹ".blue());
        tracing::info!("Clearing DNS entries");
        println!("{} Clearing DNS entries...", "🔧".cyan());
        self.cleanup_dns().await;
    }

    async fn print_device_intercepts(
        &self,
        env_id: Uuid,
        rpc_client: &DevboxSessionRpcClient,
        session_info: &lapdev_devbox_rpc::DevboxSessionInfo,
    ) {
        match rpc_client
            .list_workload_intercepts(tarpc::context::current(), env_id)
            .await
        {
            Ok(Ok(intercepts)) => {
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
            Ok(Err(err)) => {
                eprintln!("{} Failed to list intercepts: {}", "⚠".yellow(), err);
            }
            Err(err) => {
                eprintln!("{} Failed to list intercepts: {}", "⚠".yellow(), err);
            }
        }
    }

    /// Register a newly connected tunnel client and update shared state
    async fn set_tunnel_client(&self, client: Arc<TunnelClient>) {
        {
            let mut guard = self.tunnel_client.write().await;
            *guard = Some(Arc::clone(&client));
        }
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

    fn spawn_heartbeat_task(
        rpc_client: DevboxSessionRpcClient,
        user_id: Uuid,
        event_tx: mpsc::UnboundedSender<HeartbeatEvent>,
    ) -> (JoinHandle<()>, oneshot::Sender<()>) {
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            Self::heartbeat_loop(rpc_client, user_id, event_tx, cancel_rx).await;
        });
        (task, cancel_tx)
    }

    async fn heartbeat_loop(
        rpc_client: DevboxSessionRpcClient,
        user_id: Uuid,
        event_tx: mpsc::UnboundedSender<HeartbeatEvent>,
        mut cancel_rx: oneshot::Receiver<()>,
    ) {
        let mut heartbeat_interval =
            tokio::time::interval(Duration::from_secs(DEVBOX_HEARTBEAT_INTERVAL_SECS));
        heartbeat_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = &mut cancel_rx => {
                    tracing::debug!(user_id = %user_id, "Heartbeat loop cancelled");
                    break;
                }
                _ = heartbeat_interval.tick() => {
                    let heartbeat_result = timeout(
                        Duration::from_secs(DEVBOX_HEARTBEAT_TIMEOUT_SECS),
                        Self::send_session_heartbeat(&rpc_client)
                    )
                    .await;

                    match heartbeat_result {
                        Ok(Ok(())) => {}
                        Ok(Err(err)) => {
                            let _ = event_tx.send(HeartbeatEvent::Failed(err));
                            break;
                        }
                        Err(_) => {
                            let _ = event_tx.send(HeartbeatEvent::TimedOut);
                            break;
                        }
                    }
                }
            }
        }
    }
}

struct TunnelTask {
    kind: TunnelKind,
    intercept_id: Option<Uuid>,
    shutdown: oneshot::Sender<()>,
    handle: JoinHandle<()>,
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
    async fn ensure_intercept(
        &self,
        rpc_client: &DevboxSessionRpcClient,
        token: &str,
        user_id: Uuid,
    ) -> Result<(), String> {
        let mut guard = self.intercept_task.lock().await;
        if guard.is_some() {
            return Ok(());
        }

        let task = self.spawn_tunnel_task(rpc_client, TunnelKind::Intercept, token, user_id, None);

        *guard = Some(task);
        Ok(())
    }

    async fn ensure_client(
        &self,
        rpc_client: &DevboxSessionRpcClient,
        token: &str,
        user_id: Uuid,
    ) -> Result<(), String> {
        let mut guard = self.client_task.lock().await;
        if guard.is_some() {
            return Ok(());
        }

        {
            let mut stored_token = self.client_token.write().await;
            *stored_token = Some(token.to_string());
        }

        {
            let mut stored_user = self.client_user_id.write().await;
            *stored_user = Some(user_id);
        }

        let task = self.spawn_tunnel_task(rpc_client, TunnelKind::Client, token, user_id, None);
        *guard = Some(task);
        Ok(())
    }

    async fn stop_client(&self) {
        if let Some(task) = self.client_task.lock().await.take() {
            Self::stop_task(task, "Devbox client tunnel").await;
        }
    }

    async fn restart_client_tunnel(
        &self,
        rpc_client: &DevboxSessionRpcClient,
    ) -> Result<(), String> {
        self.stop_client().await;
        let token = { self.client_token.read().await.clone() };
        let user_id = { self.client_user_id.read().await.clone() };
        match (token, user_id) {
            (Some(token), Some(user_id)) => self.ensure_client(rpc_client, &token, user_id).await,
            _ => Ok(()),
        }
    }

    async fn build_direct_channel_config(
        &self,
        user_id: Uuid,
        stun_observed_addr: Option<SocketAddr>,
    ) -> Result<Option<DirectTunnelConfig>, String> {
        let active_user = { self.client_user_id.read().await.clone() };
        let Some(current_user) = active_user else {
            return Err("No active devbox client session available".to_string());
        };

        if current_user != user_id {
            return Err("Requesting user does not match active session".to_string());
        }

        let Some(server_observed_addr) = self.direct_endpoint.observed_addr() else {
            tracing::warn!("Direct endpoint has no STUN observed address; cannot provide config");
            return Ok(None);
        };

        let token = Uuid::new_v4().to_string();
        let expires_at = Utc::now() + chrono::Duration::minutes(2);
        self.direct_endpoint
            .credential()
            .insert(token.clone(), expires_at)
            .await;

        let config = DirectTunnelConfig {
            credential: DirectTunnelCredential { token, expires_at },
            server_certificate: Some(self.direct_endpoint.server_certificate().to_vec()),
            stun_observed_addr: Some(server_observed_addr),
        };

        if let Some(addr) = stun_observed_addr {
            let direct_endpoint = self.direct_endpoint.clone();
            tokio::spawn(async move {
                if let Err(err) = direct_endpoint.send_probe(addr).await {
                    tracing::warn!(
                        %addr,
                        error = %err,
                        "Failed to send probe packet to requesting sidecar"
                    );
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let _ = direct_endpoint.send_probe(addr).await;
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let _ = direct_endpoint.send_probe(addr).await;
            });
        }

        Ok(Some(config))
    }

    async fn stop_intercept(&self) {
        if let Some(task) = self.intercept_task.lock().await.take() {
            Self::stop_task(task, "Devbox intercept tunnel").await;
        }
    }

    async fn shutdown(&self) {
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
        rpc_client: &DevboxSessionRpcClient,
        kind: TunnelKind,
        token: &str,
        user_id: Uuid,
        intercept_id: Option<Uuid>,
    ) -> TunnelTask {
        let manager = self.clone();

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let token = token.to_string();
        let rpc_client = rpc_client.clone();
        let handle = tokio::spawn(async move {
            manager
                .run_tunnel_loop(&rpc_client, kind, token, user_id, intercept_id, shutdown_rx)
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
        rpc_client: &DevboxSessionRpcClient,
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
                result = self.connect_and_run_tunnel(rpc_client, kind, &ws_url, &token) => {
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
        rpc_client: &DevboxSessionRpcClient,
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

        if matches!(kind, TunnelKind::Client) {
            let stun_observed_addr = self.direct_endpoint.observed_addr();
            let direct_config = {
                let active_environment = { *self.active_environment.lock().await };
                if let Some(environment_id) = active_environment {
                    rpc_client
                        .request_direct_client_config(
                            tarpc::context::current(),
                            environment_id,
                            stun_observed_addr,
                        )
                        .await
                        .ok()
                        .and_then(|r| r.ok())
                        .flatten()
                } else {
                    None
                }
            };
            let (client, mode) = TunnelClient::connect_with_direct_or_relay::<_, _, _, _>(
                direct_config.as_ref(),
                &self.direct_endpoint,
                || async { Ok(stream) },
                |err| {
                    tracing::warn!(
                        error = %err,
                        "Direct devbox client tunnel attempt failed; falling back to relay"
                    );
                },
            )
            .await?;
            let client = Arc::new(client);
            match mode {
                lapdev_tunnel::TunnelMode::Direct => {
                    tracing::info!("Devbox client tunnel using direct QUIC transport");
                }
                lapdev_tunnel::TunnelMode::Relay => {
                    tracing::info!("Devbox client tunnel using API relay transport");
                }
            }

            self.set_tunnel_client(Arc::clone(&client)).await;

            client.closed().await;
            tracing::info!("Tunnel client closed for DNS service bridge");

            self.clear_tunnel_client(&client).await;
            Ok(())
        } else {
            let transport = RelayEndpoint::client_connection(stream).await?;
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
