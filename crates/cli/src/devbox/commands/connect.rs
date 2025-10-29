use anyhow::{Context, Result};
use colored::Colorize;
use futures::StreamExt;
use lapdev_devbox_rpc::{DevboxClientRpc, DevboxSessionRpcClient, StartInterceptRequest};
use lapdev_rpc::spawn_twoway;
use lapdev_tunnel::WebSocketTransport;
use lapdev_tunnel::{
    run_tunnel_server, TunnelClient, TunnelError, WebSocketTransport as TunnelWebSocketTransport,
};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tarpc::server::{BaseChannel, Channel};
use tokio::{
    signal,
    sync::{mpsc, oneshot, Mutex, RwLock},
    task::JoinHandle,
    time::sleep,
};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_util::codec::LengthDelimitedCodec;
use uuid::Uuid;

use crate::{
    auth,
    devbox::dns::{HostsManager, ServiceBridge, ServiceEndpoint, SyntheticIpAllocator},
};

/// Execute the devbox connect command
pub async fn execute(api_url: &str) -> Result<()> {
    // Load token from keychain, or prompt for login if not found
    let token = match auth::get_token(api_url) {
        Ok(token) => token,
        Err(_) => {
            // No token found, run login flow
            println!(
                "{}",
                "No authentication token found. Starting login...".yellow()
            );
            println!();

            // Run login
            super::login::execute(api_url, None).await?;

            // Retrieve the newly stored token
            auth::get_token(api_url).context("Failed to load authentication token after login")?
        }
    };

    let tunnel_manager = Arc::new(DevboxTunnelManager::new());

    // Check hosts file permissions early and warn user
    tunnel_manager.check_and_warn_permissions();

    let mut backoff = Duration::from_secs(1);
    let mut is_first_connection = true;

    loop {
        match connect_and_run_rpc(
            api_url,
            &token,
            Arc::clone(&tunnel_manager),
            is_first_connection,
        )
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

    tunnel_manager.cleanup_dns().await;
    tunnel_manager.shutdown().await;

    Ok(())
}

async fn connect_and_run_rpc(
    api_url: &str,
    token: &str,
    tunnel_manager: Arc<DevboxTunnelManager>,
    is_first_connection: bool,
) -> Result<bool> {
    if is_first_connection {
        println!("{}", "🔌 Connecting to Lapdev devbox...".cyan());
    } else {
        println!("{}", "🔌 Reconnecting to Lapdev devbox...".cyan());
    }

    // Construct WebSocket URL
    let ws_url = api_url
        .replace("https://", "wss://")
        .replace("http://", "ws://");
    let ws_url = format!("{}/api/v1/kube/devbox/rpc", ws_url);

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

    // Create RPC server (for server to call us)
    let client_rpc_server = DevboxClientRpcServer::new(
        shutdown_tx.clone(),
        env_change_tx.clone(),
        Arc::clone(&tunnel_manager),
        api_url.to_string(),
        token.to_string(),
        session_info.session_id,
    );

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

    tunnel_manager
        .ensure_client(api_url, token, session_info.session_id)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start client tunnel: {}", e))?;

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

                    if let Err(err) = tunnel_manager
                        .ensure_intercept(
                            api_url,
                            token,
                            session_info.session_id,
                            intercept.intercept_id,
                        )
                        .await
                    {
                        tracing::error!(
                            workload_id = %intercept.workload_id,
                            intercept_id = %intercept.intercept_id,
                            "Failed to ensure intercept tunnel during rehydrate: {}",
                            err
                        );
                    }
                }
            }
            Err(err) => {
                eprintln!("{} Failed to list intercepts: {}", "⚠".yellow(), err);
            }
        }

        // Set up DNS for the active environment
        if let Err(e) = tunnel_manager
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

    loop {
        tokio::select! {
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
                        if let Err(e) = tunnel_manager.setup_dns_for_environment(env_info.environment_id, &rpc_client).await {
                            eprintln!("{} Failed to refresh DNS: {}", "⚠".yellow(), e);
                        } else {
                            tracing::info!("DNS setup completed successfully");
                        }
                    }
                    None => {
                        tracing::info!("Clearing DNS entries");
                        println!("{} Clearing DNS entries...", "🔧".cyan());
                        tunnel_manager.cleanup_dns().await;
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

/// RPC server implementation for the CLI
/// This handles incoming calls from the server
#[derive(Clone)]
struct DevboxClientRpcServer {
    shutdown_tx: mpsc::UnboundedSender<ShutdownSignal>,
    env_change_tx: mpsc::UnboundedSender<Option<lapdev_devbox_rpc::DevboxEnvironmentInfo>>,
    tunnel_manager: Arc<DevboxTunnelManager>,
    api_url: String,
    token: String,
    session_id: Uuid,
}

impl DevboxClientRpcServer {
    fn new(
        shutdown_tx: mpsc::UnboundedSender<ShutdownSignal>,
        env_change_tx: mpsc::UnboundedSender<Option<lapdev_devbox_rpc::DevboxEnvironmentInfo>>,
        tunnel_manager: Arc<DevboxTunnelManager>,
        api_url: String,
        token: String,
        session_id: Uuid,
    ) -> Self {
        Self {
            shutdown_tx,
            env_change_tx,
            tunnel_manager,
            api_url,
            token,
            session_id,
        }
    }
}

impl DevboxClientRpc for DevboxClientRpcServer {
    async fn start_intercept(
        self,
        _context: tarpc::context::Context,
        intercept: StartInterceptRequest,
    ) -> Result<(), String> {
        let tunnel_manager = self.tunnel_manager.clone();
        let api_url = self.api_url.clone();
        let token = self.token.clone();
        let session_id = self.session_id;

        println!(
            "{} Starting intercept for workload: {}/{}",
            "→".cyan(),
            intercept.namespace,
            intercept.workload_name
        );

        if let Err(err) = tunnel_manager
            .ensure_intercept(&api_url, &token, session_id, intercept.intercept_id)
            .await
        {
            tracing::error!(
                workload_id = %intercept.workload_id,
                intercept_id = %intercept.intercept_id,
                "Failed to start intercept tunnel: {}",
                err
            );
        } else {
            tracing::info!(
                workload_id = %intercept.workload_id,
                intercept_id = %intercept.intercept_id,
                "Intercept {} acknowledged by CLI",
                intercept.intercept_id
            );
        }
        Ok(())
    }

    async fn stop_intercept(
        self,
        _context: tarpc::context::Context,
        intercept_id: Uuid,
    ) -> Result<(), String> {
        println!("{} Received stop intercept: {}", "✗".yellow(), intercept_id);
        let tunnel_manager = self.tunnel_manager.clone();
        tunnel_manager.stop_intercept_by_id(intercept_id).await;
        tracing::info!("Intercept {} stop acknowledged by CLI", intercept_id);
        Ok(())
    }

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
        let tunnel_manager = self.tunnel_manager.clone();
        tunnel_manager.stop_all_intercepts().await;

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

struct DevboxTunnelManager {
    intercepts: Mutex<InterceptState>,
    client_task: Mutex<Option<TunnelTask>>,
    /// Shared tunnel client for DNS service bridge
    tunnel_client: Arc<RwLock<Option<Arc<TunnelClient>>>>,
    /// Service bridge for DNS resolution
    service_bridge: Arc<ServiceBridge>,
    /// Hosts file manager
    hosts_manager: Arc<HostsManager>,
    /// IP allocator
    ip_allocator: Arc<Mutex<SyntheticIpAllocator>>,
}

#[derive(Default)]
struct InterceptState {
    tasks: HashMap<Uuid, TunnelTask>,
}

impl DevboxTunnelManager {
    fn new() -> Self {
        Self {
            intercepts: Mutex::new(InterceptState::default()),
            client_task: Mutex::new(None),
            tunnel_client: Arc::new(RwLock::new(None)),
            service_bridge: Arc::new(ServiceBridge::new()),
            hosts_manager: Arc::new(HostsManager::new()),
            ip_allocator: Arc::new(Mutex::new(SyntheticIpAllocator::new())),
        }
    }

    /// Get the tunnel client for making connections
    async fn get_tunnel_client(&self) -> Option<Arc<TunnelClient>> {
        self.tunnel_client.read().await.clone()
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
        // Wait for tunnel client to be available
        let tunnel_client = loop {
            if let Some(client) = self.get_tunnel_client().await {
                break client;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        };

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
        let mut endpoints = Vec::new();
        let mut allocator = self.ip_allocator.lock().await;

        for service in &services {
            for port in &service.ports {
                if let Some(ip) = allocator.allocate(&service.name, &service.namespace, port.port) {
                    endpoints.push(ServiceEndpoint::new(
                        service,
                        port.port,
                        port.protocol.clone(),
                        ip,
                    ));
                }
            }
        }
        drop(allocator);

        println!("  {} service endpoint(s) allocated", endpoints.len());

        // Update hosts file
        match self.hosts_manager.write_entries(&endpoints) {
            Ok(()) => {
                println!("{} Hosts file updated", "✓".green());
            }
            Err(e) => {
                eprintln!("{} Failed to update hosts file: {}", "⚠".yellow(), e);
                self.hosts_manager.print_manual_instructions(&endpoints);
                // Still continue to start the service bridge - user might update manually
            }
        }

        // Start service bridge
        self.service_bridge.set_tunnel_client(tunnel_client).await;
        if let Err(e) = self.service_bridge.start(endpoints).await {
            eprintln!("{} Failed to start service bridge: {}", "⚠".yellow(), e);
        } else {
            println!("{} Service bridge started", "✓".green());
        }

        Ok(())
    }

    /// Cleanup DNS entries
    async fn cleanup_dns(&self) {
        if let Err(e) = self.hosts_manager.remove_entries() {
            tracing::warn!("Failed to clean up hosts file: {}", e);
        }
        self.service_bridge.stop().await;
        self.ip_allocator.lock().await.clear();
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
        api_url: &str,
        token: &str,
        session_id: Uuid,
        intercept_id: Uuid,
    ) -> Result<(), String> {
        let mut state = self.intercepts.lock().await;
        if state.tasks.contains_key(&intercept_id) {
            return Ok(());
        }

        let task = spawn_tunnel_task(
            TunnelKind::Intercept,
            api_url.trim_end_matches('/').to_string(),
            token.to_string(),
            session_id,
            Some(intercept_id),
            None,
        );

        state.tasks.insert(intercept_id, task);
        Ok(())
    }

    async fn stop_intercept_by_id(&self, intercept_id: Uuid) {
        let task = self.intercepts.lock().await.tasks.remove(&intercept_id);

        if let Some(task) = task {
            Self::stop_task(task, "Devbox intercept tunnel").await;
        }
    }

    async fn stop_all_intercepts(&self) {
        let tasks: Vec<TunnelTask> = {
            let mut state = self.intercepts.lock().await;
            state.tasks.drain().map(|(_, task)| task).collect()
        };

        for task in tasks {
            Self::stop_task(task, "Devbox intercept tunnel").await;
        }
    }

    async fn ensure_client(
        &self,
        api_url: &str,
        token: &str,
        session_id: Uuid,
    ) -> Result<(), String> {
        let mut guard = self.client_task.lock().await;
        if guard.is_some() {
            return Ok(());
        }

        let task = spawn_tunnel_task(
            TunnelKind::Client,
            api_url.trim_end_matches('/').to_string(),
            token.to_string(),
            session_id,
            None,
            Some(Arc::clone(&self.tunnel_client)), // Share client tunnel for DNS
        );
        *guard = Some(task);
        Ok(())
    }

    async fn stop_client(&self) {
        if let Some(task) = self.client_task.lock().await.take() {
            Self::stop_task(task, "Devbox client tunnel").await;
        }
    }

    async fn shutdown(&self) {
        self.stop_all_intercepts().await;
        if let Some(task) = self.client_task.lock().await.take() {
            Self::stop_task(task, "Devbox client tunnel").await;
        }
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
}

fn spawn_tunnel_task(
    kind: TunnelKind,
    api_url: String,
    token: String,
    session_id: Uuid,
    intercept_id: Option<Uuid>,
    tunnel_client_slot: Option<Arc<RwLock<Option<Arc<TunnelClient>>>>>,
) -> TunnelTask {
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let handle = tokio::spawn(run_tunnel_loop(
        kind,
        api_url,
        token,
        session_id,
        intercept_id,
        shutdown_rx,
        tunnel_client_slot,
    ));

    TunnelTask {
        kind,
        intercept_id,
        shutdown: shutdown_tx,
        handle,
    }
}

async fn run_tunnel_loop(
    kind: TunnelKind,
    api_url: String,
    token: String,
    session_id: Uuid,
    intercept_id: Option<Uuid>,
    mut shutdown_rx: oneshot::Receiver<()>,
    tunnel_client_slot: Option<Arc<RwLock<Option<Arc<TunnelClient>>>>>,
) {
    let ws_base = api_url
        .replace("https://", "wss://")
        .replace("http://", "ws://");
    let ws_url = match (kind, intercept_id) {
        (TunnelKind::Intercept, _) => format!(
            "{}/api/v1/kube/devbox/tunnel/{}/{}",
            ws_base.trim_end_matches('/'),
            kind.path(),
            session_id
        ),
        _ => format!(
            "{}/api/v1/kube/devbox/tunnel/{}/{}",
            ws_base.trim_end_matches('/'),
            kind.path(),
            session_id
        ),
    };
    let intercept_id_str = intercept_id.map(|id| id.to_string());

    let mut backoff = Duration::from_secs(1);

    loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                tracing::info!(
                    %session_id,
                    tunnel_kind = kind.as_str(),
                    intercept_id = intercept_id_str.as_deref(),
                    "Devbox tunnel shutdown signal received"
                );
                break;
            }
            result = connect_and_run_tunnel(&ws_url, &token, tunnel_client_slot.as_ref()) => {
                match result {
                    Ok(()) => {
                        tracing::info!(
                            %session_id,
                            tunnel_kind = kind.as_str(),
                            intercept_id = intercept_id_str.as_deref(),
                            "Devbox tunnel closed gracefully"
                        );
                        backoff = Duration::from_secs(1);
                    }
                    Err(err) => {
                        tracing::warn!(
                            %session_id,
                            tunnel_kind = kind.as_str(),
                            intercept_id = intercept_id_str.as_deref(),
                            "Devbox tunnel disconnected: {}",
                            err
                        );
                        backoff = (backoff.saturating_mul(2)).min(Duration::from_secs(30));
                    }
                }

                tokio::select! {
                    _ = &mut shutdown_rx => {
                        tracing::info!(
                            %session_id,
                            tunnel_kind = kind.as_str(),
                            intercept_id = intercept_id_str.as_deref(),
                            "Devbox tunnel shutdown signal received"
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
    ws_url: &str,
    token: &str,
    tunnel_client_slot: Option<&Arc<RwLock<Option<Arc<TunnelClient>>>>>,
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

    tracing::info!("Devbox tunnel connected: {}", ws_url);

    let transport = TunnelWebSocketTransport::new(stream);

    // If this tunnel should expose a client, create it and store it
    if let Some(slot) = tunnel_client_slot {
        let client = Arc::new(TunnelClient::connect(transport));
        *slot.write().await = Some(Arc::clone(&client));
        tracing::info!("Tunnel client exposed for DNS service bridge");

        // Keep the client alive until we're asked to shut down
        // The client's internal tasks will handle the actual tunneling
        tokio::time::sleep(Duration::from_secs(u64::MAX)).await;
        Ok(())
    } else {
        // Regular tunnel server mode (for intercept tunnel)
        run_tunnel_server(transport).await
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
