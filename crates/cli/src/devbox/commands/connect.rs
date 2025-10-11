use anyhow::{Context, Result};
use colored::Colorize;
use futures::StreamExt;
use lapdev_devbox_rpc::{DevboxClientRpc, DevboxSessionRpcClient, StartInterceptRequest};
use lapdev_rpc::spawn_twoway;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tarpc::server::{BaseChannel, Channel};
use tokio::{
    net::TcpListener,
    signal,
    sync::{mpsc, oneshot, Mutex},
    task::JoinHandle,
    time::sleep,
};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_util::codec::LengthDelimitedCodec;
use tunnel::{run_tunnel_server, TunnelError, WebSocketTransport as TunnelWebSocketTransport};
use uuid::Uuid;

use crate::{auth, devbox::websocket_transport::WebSocketTransport};

/// Execute the devbox connect command
pub async fn execute(api_url: &str) -> Result<()> {
    // Load token from keychain
    let token = auth::get_token(api_url)
        .context("Failed to load authentication token. Please run 'lapdev login' first.")?;

    println!("{}", "🔌 Connecting to Lapdev devbox...".cyan());

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

    let intercept_manager = Arc::new(InterceptManager::new());
    let (shutdown_tx, mut shutdown_rx) = mpsc::unbounded_channel::<ShutdownSignal>();

    // Create RPC client (for calling server methods)
    let rpc_client =
        DevboxSessionRpcClient::new(tarpc::client::Config::default(), client_chan).spawn();

    // Create RPC server (for server to call us)
    let client_rpc_server =
        DevboxClientRpcServer::new(intercept_manager.clone(), shutdown_tx.clone());

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

    intercept_manager
        .ensure_tunnel(api_url, &token, session_info.session_id)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start devbox tunnel: {}", e))?;

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

                    let request = StartInterceptRequest {
                        intercept_id: intercept.intercept_id,
                        workload_id: intercept.workload_id,
                        workload_name: intercept.workload_name.clone(),
                        namespace: intercept.namespace.clone(),
                        port_mappings: intercept.port_mappings.clone(),
                    };

                    match intercept_manager.start(request).await {
                        Ok(port_count) => {
                            println!(
                                "  {} Rehydrated intercept for {}/{} ({} port(s))",
                                "↻".green(),
                                intercept.namespace.bright_white(),
                                intercept.workload_name.cyan(),
                                port_count
                            );
                        }
                        Err(err) => {
                            eprintln!(
                                "{} Failed to rehydrate intercept {}: {}",
                                "✗".red(),
                                intercept.intercept_id,
                                err
                            );
                        }
                    }
                }
            }
            Err(err) => {
                eprintln!("{} Failed to list intercepts: {}", "⚠".yellow(), err);
            }
        }
    }

    println!("{}", "\n👂 Listening for intercept requests...".cyan());
    println!("{}", "Press Ctrl+C to disconnect".dimmed());

    let mut server_task = server_task;

    tokio::select! {
        res = &mut server_task => {
            res.context("Devbox RPC server task failed")?;
        }
        _ = signal::ctrl_c() => {
            println!("\n{}", "Received Ctrl+C, disconnecting...".yellow());
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
                    }
                }
            }
        }
    }

    intercept_manager.stop_all().await;
    intercept_manager.shutdown_tunnel().await;

    if !server_task.is_finished() {
        server_task.abort();
    }

    Ok(())
}

/// RPC server implementation for the CLI
/// This handles incoming calls from the server
#[derive(Clone)]
struct DevboxClientRpcServer {
    manager: Arc<InterceptManager>,
    shutdown_tx: mpsc::UnboundedSender<ShutdownSignal>,
}

impl DevboxClientRpcServer {
    fn new(
        manager: Arc<InterceptManager>,
        shutdown_tx: mpsc::UnboundedSender<ShutdownSignal>,
    ) -> Self {
        Self {
            manager,
            shutdown_tx,
        }
    }
}

impl DevboxClientRpc for DevboxClientRpcServer {
    async fn start_intercept(
        self,
        _context: tarpc::context::Context,
        intercept: StartInterceptRequest,
    ) -> Result<(), String> {
        println!(
            "{} Starting intercept for workload: {}/{}",
            "→".cyan(),
            intercept.namespace,
            intercept.workload_name
        );

        for mapping in &intercept.port_mappings {
            println!(
                "  {} localhost:{} → {}:{}",
                "↔".dimmed(),
                mapping.local_port,
                intercept.workload_name,
                mapping.workload_port
            );
        }

        let mapping_count = intercept.port_mappings.len();

        tracing::info!(
            "Applying intercept {} with {} port mappings",
            intercept.intercept_id,
            mapping_count
        );

        let manager = self.manager;
        let port_count = manager.start(intercept).await?;

        println!(
            "  {} Intercept listeners active on {} port(s)",
            "✓".green(),
            port_count
        );

        Ok(())
    }

    async fn stop_intercept(
        self,
        _context: tarpc::context::Context,
        intercept_id: Uuid,
    ) -> Result<(), String> {
        println!("{} Stopping intercept: {}", "✗".yellow(), intercept_id);
        let manager = self.manager;
        manager.stop(intercept_id).await
    }

    async fn session_displaced(self, _context: tarpc::context::Context, new_device_name: String) {
        let manager = self.manager;
        let shutdown_tx = self.shutdown_tx;

        manager.stop_all().await;
        let _ = shutdown_tx.send(ShutdownSignal::Displaced(new_device_name.clone()));

        tracing::warn!("Session displaced by: {}", new_device_name);
    }

    async fn ping(self, _context: tarpc::context::Context) -> Result<(), String> {
        tracing::trace!("Received ping");
        Ok(())
    }
}

enum ShutdownSignal {
    Displaced(String),
}

struct InterceptManager {
    intercepts: Mutex<HashMap<Uuid, InterceptHandle>>,
    tunnel: DevboxTunnelManager,
}

impl InterceptManager {
    fn new() -> Self {
        Self {
            intercepts: Mutex::new(HashMap::new()),
            tunnel: DevboxTunnelManager::default(),
        }
    }

    async fn start(&self, intercept: StartInterceptRequest) -> Result<usize, String> {
        let previous = {
            let mut intercepts = self.intercepts.lock().await;
            intercepts.remove(&intercept.intercept_id)
        };

        if let Some(handle) = previous {
            handle.shutdown().await;
        }

        let mut listeners = Vec::new();
        for mapping in &intercept.port_mappings {
            match ListenerHandle::new(
                intercept.intercept_id,
                &intercept.workload_name,
                &intercept.namespace,
                mapping,
            )
            .await
            {
                Ok(listener) => listeners.push(listener),
                Err(err) => {
                    for listener in listeners {
                        listener.shutdown().await;
                    }
                    return Err(err);
                }
            }
        }

        let listener_count = listeners.len();

        let mut intercepts = self.intercepts.lock().await;
        intercepts.insert(
            intercept.intercept_id,
            InterceptHandle {
                intercept,
                listeners,
            },
        );

        Ok(listener_count)
    }

    async fn stop(&self, intercept_id: Uuid) -> Result<(), String> {
        let handle = {
            let mut intercepts = self.intercepts.lock().await;
            intercepts.remove(&intercept_id)
        };

        if let Some(handle) = handle {
            handle.shutdown().await;
        }

        Ok(())
    }

    async fn stop_all(&self) {
        let handles = {
            let mut intercepts = self.intercepts.lock().await;
            intercepts
                .drain()
                .map(|(_, handle)| handle)
                .collect::<Vec<_>>()
        };

        for handle in handles {
            handle.shutdown().await;
        }
    }

    async fn ensure_tunnel(
        &self,
        api_url: &str,
        token: &str,
        session_id: Uuid,
    ) -> Result<(), String> {
        self.tunnel.ensure_running(api_url, token, session_id).await
    }

    async fn shutdown_tunnel(&self) {
        self.tunnel.shutdown().await;
    }
}

#[derive(Default)]
struct DevboxTunnelManager {
    state: Mutex<Option<TunnelTask>>,
}

struct TunnelTask {
    shutdown: oneshot::Sender<()>,
    handle: JoinHandle<()>,
}

impl DevboxTunnelManager {
    async fn ensure_running(
        &self,
        api_url: &str,
        token: &str,
        session_id: Uuid,
    ) -> Result<(), String> {
        let mut state = self.state.lock().await;
        if state.is_some() {
            return Ok(());
        }

        let api_url = api_url.trim_end_matches('/').to_string();
        let token = token.to_string();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let handle = tokio::spawn(run_tunnel_loop(api_url, token, session_id, shutdown_rx));
        *state = Some(TunnelTask {
            shutdown: shutdown_tx,
            handle,
        });

        Ok(())
    }

    async fn shutdown(&self) {
        let task = {
            let mut state = self.state.lock().await;
            state.take()
        };

        if let Some(task) = task {
            let _ = task.shutdown.send(());
            if let Err(err) = task.handle.await {
                tracing::warn!("Devbox tunnel task exited with error: {}", err);
            }
        }
    }
}

struct InterceptHandle {
    listeners: Vec<ListenerHandle>,
}

impl InterceptHandle {
    async fn shutdown(self) {
        for listener in self.listeners {
            listener.shutdown().await;
        }
    }
}

struct ListenerHandle {
    local_port: u16,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl ListenerHandle {
    async fn new(
        intercept_id: Uuid,
        workload_name: &str,
        namespace: &str,
        mapping: &lapdev_devbox_rpc::PortMapping,
    ) -> Result<Self, String> {
        let local_port = mapping.local_port;
        let workload_port = mapping.workload_port;

        let listener = TcpListener::bind(("127.0.0.1", local_port))
            .await
            .map_err(|err| format!("Failed to bind local port {}: {}", local_port, err))?;

        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

        let workload_name = workload_name.to_string();
        let namespace = namespace.to_string();

        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        tracing::debug!(intercept_id = %intercept_id, port = local_port, "Shutting down intercept listener");
                        break;
                    }
                    accept_result = listener.accept() => {
                        match accept_result {
                            Ok((stream, addr)) => {
                                tracing::info!(
                                    intercept_id = %intercept_id,
                                    local_port,
                                    workload_port,
                                    client = %addr,
                                    "Accepted connection for {}/{}",
                                    namespace,
                                    workload_name
                                );
                                drop(stream);
                            }
                            Err(err) => {
                                tracing::error!(
                                    intercept_id = %intercept_id,
                                    port = local_port,
                                    error = %err,
                                    "Intercept listener accept failed"
                                );
                                break;
                            }
                        }
                    }
                }
            }
        });

        Ok(Self {
            local_port,
            shutdown: Some(shutdown_tx),
            task,
        })
    }

    async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }

        if let Err(err) = self.task.await {
            tracing::debug!(port = self.local_port, error = %err, "Listener task exited with error");
        }
    }
}

async fn run_tunnel_loop(
    api_url: String,
    token: String,
    session_id: Uuid,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    let ws_base = api_url
        .replace("https://", "wss://")
        .replace("http://", "ws://");
    let ws_url = format!(
        "{}/api/v1/kube/devbox/tunnel/{}",
        ws_base.trim_end_matches('/'),
        session_id
    );

    let mut backoff = Duration::from_secs(1);

    loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                tracing::info!(%session_id, "Devbox tunnel shutdown signal received");
                break;
            }
            result = connect_and_run_tunnel(&ws_url, &token) => {
                match result {
                    Ok(()) => {
                        tracing::info!(%session_id, "Devbox tunnel closed gracefully");
                        backoff = Duration::from_secs(1);
                    }
                    Err(err) => {
                        tracing::warn!(%session_id, "Devbox tunnel disconnected: {}", err);
                        backoff = (backoff.saturating_mul(2)).min(Duration::from_secs(30));
                    }
                }

                tokio::select! {
                    _ = &mut shutdown_rx => {
                        tracing::info!(%session_id, "Devbox tunnel shutdown signal received");
                        break;
                    }
                    _ = sleep(backoff) => {}
                }
            }
        }
    }
}

async fn connect_and_run_tunnel(ws_url: &str, token: &str) -> Result<(), TunnelError> {
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
    run_tunnel_server(transport).await
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
