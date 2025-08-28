use anyhow::{anyhow, Result};
use futures::StreamExt;
use lapdev_common::kube::{
    DEFAULT_SIDECAR_PROXY_MANAGER_PORT, SIDECAR_PROXY_MANAGER_ADDR_ENV_VAR,
    SIDECAR_PROXY_MANAGER_PORT_ENV_VAR,
};
use lapdev_kube_rpc::{SidecarProxyManagerRpc, SidecarProxyRpcClient};
use lapdev_rpc::spawn_twoway;
use std::{collections::HashMap, sync::Arc};
use tarpc::server::{BaseChannel, Channel};
use tokio::sync::RwLock;
use tracing::{error, info};
use uuid::Uuid;

use crate::sidecar_proxy_manager_rpc::SidecarProxyManagerRpcServer;

#[derive(Debug, Clone)]
pub struct SidecarProxyInfo {
    pub pod_name: String,
    pub namespace: String,
    pub environment_id: Option<String>,
    pub last_heartbeat: std::time::Instant,
    pub metrics: SidecarProxyMetrics,
}

#[derive(Debug, Clone, Default)]
pub struct SidecarProxyMetrics {
    pub request_count: u64,
    pub byte_count: u64,
    pub active_connections: u32,
}

#[derive(Clone)]
pub struct SidecarProxyManager {
    pub(crate) sidecar_proxies: Arc<RwLock<HashMap<Uuid, SidecarProxyManagerRpcServer>>>,
}

impl SidecarProxyManager {
    pub(crate) async fn new() -> Result<Self> {
        let sidecar_proxy_manager_addr = std::env::var(SIDECAR_PROXY_MANAGER_ADDR_ENV_VAR)
            .map_err(|_| anyhow!("can't find LAPDEV_SIDECAR_PROXY_MANAGER_ADDR env var"))?;

        // Parse the URL to extract the port
        let port = std::env::var(SIDECAR_PROXY_MANAGER_PORT_ENV_VAR)
            .ok()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(DEFAULT_SIDECAR_PROXY_MANAGER_PORT);
        let bind_addr = format!("0.0.0.0:{port}");
        info!("Starting TCP server for sidecar proxies on: {}", bind_addr);

        let mut listener = tarpc::serde_transport::tcp::listen(
            ("0.0.0.0", port),
            tarpc::tokio_serde::formats::Bincode::default,
        )
        .await?;
        info!("TCP server listening on: {}", bind_addr);

        let m = Self {
            sidecar_proxies: Arc::new(RwLock::new(HashMap::new())),
        };

        {
            let m = m.clone();
            tokio::spawn(async move {
                while let Some(conn) = listener.next().await {
                    if let Ok(conn) = conn {
                        let m = m.clone();
                        tokio::spawn(async move {
                            let (server_chan, client_chan, _) = spawn_twoway(conn);
                            let rpc_client = SidecarProxyRpcClient::new(
                                tarpc::client::Config::default(),
                                client_chan,
                            )
                            .spawn();
                            let rpc_server =
                                SidecarProxyManagerRpcServer::new(m.clone(), rpc_client);
                            BaseChannel::with_defaults(server_chan)
                                .execute(rpc_server.serve())
                                .for_each(|resp| async move {
                                    tokio::spawn(resp);
                                })
                                .await;
                        });
                    }
                }
                error!("TCP connection stopped");
            });
        }

        Ok(m)
    }
}
