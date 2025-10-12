use anyhow::{Context, Result};
use lapdev_tunnel::TunnelClient;
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::Arc,
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::RwLock,
    task::JoinHandle,
};
use tracing::{debug, error, info, warn};

use super::ServiceEndpoint;

/// Bridges TCP connections from synthetic IPs to services via tunnel
pub struct ServiceBridge {
    /// Maps synthetic IP:port to service endpoint info
    endpoints: Arc<RwLock<HashMap<SocketAddr, ServiceEndpoint>>>,
    /// Listener tasks for each synthetic IP
    listeners: Arc<RwLock<Vec<JoinHandle<()>>>>,
    /// Tunnel client for connecting to services
    tunnel_client: Arc<RwLock<Option<Arc<TunnelClient>>>>,
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
    pub async fn start(&self, endpoints: Vec<ServiceEndpoint>) -> Result<()> {
        if self.tunnel_client.read().await.is_none() {
            anyhow::bail!("Tunnel client not set");
        }

        // Stop existing listeners
        self.stop().await;

        // Update endpoint mappings
        let mut endpoint_map = self.endpoints.write().await;
        endpoint_map.clear();
        for endpoint in &endpoints {
            let addr = SocketAddr::new(endpoint.synthetic_ip, endpoint.port);
            endpoint_map.insert(addr, endpoint.clone());
        }
        drop(endpoint_map);

        // Start listener for each unique IP:port combination
        let tunnel_client = self.tunnel_client.read().await.clone().unwrap();
        let mut tasks = Vec::new();
        for endpoint in endpoints {
            let addr = SocketAddr::new(endpoint.synthetic_ip, endpoint.port);
            match self.spawn_listener(addr, Arc::clone(&tunnel_client)).await {
                Ok(task) => tasks.push(task),
                Err(e) => {
                    error!("Failed to start listener on {}: {}", addr, e);
                    // Continue with other listeners
                }
            }
        }

        let num_endpoints = tasks.len();
        *self.listeners.write().await = tasks;

        info!("Service bridge started with {} endpoints", num_endpoints);
        Ok(())
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
    async fn spawn_listener(&self, addr: SocketAddr, tunnel_client: Arc<TunnelClient>) -> Result<JoinHandle<()>> {
        let listener = TcpListener::bind(addr)
            .await
            .with_context(|| format!("Failed to bind to {}", addr))?;

        let endpoints = Arc::clone(&self.endpoints);

        info!("Started listener on {}", addr);

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

impl Drop for ServiceBridge {
    fn drop(&mut self) {
        let listeners = self.listeners.clone();
        tokio::spawn(async move {
            let mut listeners = listeners.write().await;
            for task in listeners.drain(..) {
                task.abort();
            }
        });
    }
}

/// Handle a single TCP connection by proxying it through the tunnel
async fn handle_connection(
    mut local_stream: TcpStream,
    local_addr: SocketAddr,
    endpoints: Arc<RwLock<HashMap<SocketAddr, ServiceEndpoint>>>,
    tunnel_client: Arc<TunnelClient>,
) -> Result<()> {
    // Look up the service endpoint
    let endpoint = {
        let endpoints = endpoints.read().await;
        endpoints
            .get(&local_addr)
            .cloned()
            .context(format!("No endpoint found for {}", local_addr))?
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
