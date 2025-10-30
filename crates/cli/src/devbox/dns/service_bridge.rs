use anyhow::{Context, Result};
use lapdev_tunnel::TunnelClient;
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::RwLock,
    task::JoinHandle,
};
use tracing::{debug, error, info, warn};

use super::{is_tcp_protocol, ServiceEndpoint};

/// Bridges TCP connections from synthetic IPs to services via tunnel
pub struct ServiceBridge {
    /// Maps synthetic IP:port to service endpoint info
    endpoints: Arc<RwLock<HashMap<SocketAddr, ServiceEndpoint>>>,
    /// Listener tasks for each synthetic IP
    listeners: Arc<RwLock<Vec<JoinHandle<()>>>>,
    /// Tunnel client for connecting to services
    tunnel_client: Arc<RwLock<Option<Arc<TunnelClient>>>>,
}

#[derive(Debug, Default)]
pub struct ServiceBridgeStartReport {
    pub started: Vec<ServiceEndpoint>,
    pub failed: Vec<(ServiceEndpoint, anyhow::Error)>,
}

impl ServiceBridge {
    pub fn new() -> Self {
        Self {
            endpoints: Arc::new(RwLock::new(HashMap::new())),
            listeners: Arc::new(RwLock::new(Vec::new())),
            tunnel_client: Arc::new(RwLock::new(None)),
        }
    }

    /// Set the tunnel client to use for connections
    pub async fn set_tunnel_client(&self, client: Arc<TunnelClient>) {
        *self.tunnel_client.write().await = Some(client);
    }

    /// Start listening on synthetic IPs for the given service endpoints
    pub async fn start(&self, endpoints: Vec<ServiceEndpoint>) -> ServiceBridgeStartReport {
        // Stop existing listeners
        self.stop().await;

        let mut tcp_endpoints = Vec::new();
        let mut skipped = Vec::new();

        for endpoint in endpoints {
            if is_tcp_protocol(&endpoint.protocol) {
                tcp_endpoints.push(endpoint);
            } else {
                skipped.push(endpoint);
            }
        }

        if !skipped.is_empty() {
            for endpoint in &skipped {
                warn!(
                    service = %endpoint.service_name,
                    namespace = %endpoint.namespace,
                    port = endpoint.port,
                    protocol = %endpoint.protocol,
                    "Skipping non-TCP service endpoint; protocol not supported"
                );
            }
        }

        {
            // Clear any existing endpoint mappings before rebuilding
            let mut endpoint_map = self.endpoints.write().await;
            endpoint_map.clear();
        }

        // Start listener for each unique IP:port combination
        let tunnel_client = Arc::clone(&self.tunnel_client);
        let mut tasks = Vec::new();
        let mut started = Vec::new();
        let mut failed = Vec::new();

        for endpoint in tcp_endpoints {
            let addr = SocketAddr::new(endpoint.synthetic_ip, endpoint.port);
            match self
                .spawn_listener(addr, endpoint.clone(), Arc::clone(&tunnel_client))
                .await
            {
                Ok(task) => {
                    {
                        let mut endpoint_map = self.endpoints.write().await;
                        endpoint_map.insert(addr, endpoint.clone());
                    }
                    tasks.push(task);
                    started.push(endpoint);
                }
                Err(e) => {
                    error!("Failed to start listener on {}: {}", addr, e);
                    failed.push((endpoint, e));
                }
            }
        }

        *self.listeners.write().await = tasks;

        let num_started = started.len();
        info!("Service bridge started with {} endpoints", num_started);

        ServiceBridgeStartReport { started, failed }
    }

    /// Stop all listeners
    pub async fn stop(&self) {
        let mut listeners = self.listeners.write().await;
        for task in listeners.drain(..) {
            task.abort();
        }
        info!("Service bridge stopped");
    }

    /// Spawn a TCP listener for a specific address
    async fn spawn_listener(
        &self,
        addr: SocketAddr,
        endpoint: ServiceEndpoint,
        tunnel_client: Arc<RwLock<Option<Arc<TunnelClient>>>>,
    ) -> Result<JoinHandle<()>> {
        let listener = TcpListener::bind(addr)
            .await
            .with_context(|| format!("Failed to bind to {}", addr))?;

        let endpoints = Arc::clone(&self.endpoints);

        info!(
            "Started listener on {} for {}/{} (port {})",
            addr, endpoint.namespace, endpoint.service_name, endpoint.port
        );

        let task = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, peer_addr)) => {
                        debug!("Accepted connection from {} to {}", peer_addr, addr);

                        let endpoints = Arc::clone(&endpoints);
                        let tunnel_client = Arc::clone(&tunnel_client);

                        tokio::spawn(async move {
                            if let Err(e) =
                                handle_connection(stream, addr, endpoints, tunnel_client).await
                            {
                                error!("Connection handler error: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        error!("Failed to accept connection on {}: {}", addr, e);
                    }
                }
            }
        });

        Ok(task)
    }
}

/// Handle a single TCP connection by proxying it through the tunnel
async fn handle_connection(
    mut local_stream: TcpStream,
    local_addr: SocketAddr,
    endpoints: Arc<RwLock<HashMap<SocketAddr, ServiceEndpoint>>>,
    tunnel_client: Arc<RwLock<Option<Arc<TunnelClient>>>>,
) -> Result<()> {
    // Look up the service endpoint
    let endpoint = {
        let endpoints = endpoints.read().await;
        endpoints
            .get(&local_addr)
            .cloned()
            .context(format!("No endpoint found for {}", local_addr))?
    };

    let tunnel_client = {
        let guard = tunnel_client.read().await;
        guard
            .as_ref()
            .cloned()
            .context("Tunnel client not available")?
    };

    debug!(
        "Connecting to service: {} ({}:{})",
        endpoint.fqdn, endpoint.fqdn, endpoint.port
    );

    // Connect through tunnel
    let tunnel_stream = tunnel_client
        .connect_tcp(&endpoint.fqdn, endpoint.port)
        .await
        .context("Failed to establish tunnel connection")?;

    debug!("Tunnel established to {}", endpoint.fqdn);

    // Proxy bytes bidirectionally
    match tokio::io::copy_bidirectional(&mut local_stream, &mut Box::pin(tunnel_stream)).await {
        Ok((to_remote, to_local)) => {
            debug!(
                "Connection closed: {} bytes to remote, {} bytes to local",
                to_remote, to_local
            );
        }
        Err(e) => {
            warn!("Proxy error: {}", e);
        }
    }

    Ok(())
}
