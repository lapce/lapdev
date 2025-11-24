use anyhow::Result;
use futures::StreamExt;
use lapdev_kube_rpc::{DevboxProxyManagerRpc, DevboxProxyRpcClient};
use lapdev_rpc::spawn_twoway;
use std::{collections::HashMap, sync::Arc};
use tarpc::server::{BaseChannel, Channel};
use tokio::sync::RwLock;
use tracing::{error, info};
use uuid::Uuid;

use crate::devbox_proxy_manager_rpc::DevboxProxyManagerRpcServer;

pub const DEFAULT_DEVBOX_PROXY_MANAGER_PORT: u16 = 7771;
pub const DEVBOX_PROXY_MANAGER_PORT_ENV_VAR: &str = "LAPDEV_DEVBOX_PROXY_MANAGER_PORT";

#[derive(Debug, Clone)]
pub struct DevboxProxyInfo {
    pub environment_id: Uuid,
    pub last_heartbeat: std::time::Instant,
}

#[derive(Clone)]
pub struct DevboxProxyManager {
    pub(crate) devbox_proxies: Arc<RwLock<HashMap<Uuid, DevboxProxyRpcClient>>>,
}

impl DevboxProxyManager {
    pub(crate) async fn new() -> Result<Self> {
        let port = std::env::var(DEVBOX_PROXY_MANAGER_PORT_ENV_VAR)
            .ok()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(DEFAULT_DEVBOX_PROXY_MANAGER_PORT);
        let bind_addr = format!("0.0.0.0:{port}");
        info!("Starting TCP server for devbox proxies on: {}", bind_addr);

        let mut listener = tarpc::serde_transport::tcp::listen(
            ("0.0.0.0", port),
            tarpc::tokio_serde::formats::Bincode::default,
        )
        .await?;
        info!("TCP server listening on: {}", bind_addr);

        let m = Self {
            devbox_proxies: Arc::new(RwLock::new(HashMap::new())),
        };

        {
            let m = m.clone();
            tokio::spawn(async move {
                while let Some(conn) = listener.next().await {
                    if let Ok(conn) = conn {
                        let m = m.clone();
                        tokio::spawn(async move {
                            let (server_chan, client_chan, _) = spawn_twoway(conn);
                            let rpc_client = DevboxProxyRpcClient::new(
                                tarpc::client::Config::default(),
                                client_chan,
                            )
                            .spawn();
                            let rpc_server =
                                DevboxProxyManagerRpcServer::new(m.clone(), rpc_client);
                            BaseChannel::with_defaults(server_chan)
                                .execute(rpc_server.serve())
                                .for_each(|resp| async move {
                                    tokio::spawn(resp);
                                })
                                .await;
                        });
                    }
                }
                error!("Devbox proxy manager TCP connection stopped");
            });
        }

        Ok(m)
    }

    pub async fn get_proxy_client(&self, environment_id: Uuid) -> Option<DevboxProxyRpcClient> {
        let proxies = self.devbox_proxies.read().await;
        proxies.get(&environment_id).cloned()
    }
}
