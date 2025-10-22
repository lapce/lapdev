use anyhow::Result;
use futures::StreamExt;
use lapdev_common::kube::{DEFAULT_SIDECAR_PROXY_MANAGER_PORT, SIDECAR_PROXY_MANAGER_PORT_ENV_VAR};
use lapdev_kube_rpc::{KubeClusterRpcClient, SidecarProxyManagerRpc, SidecarProxyRpcClient};
use lapdev_rpc::spawn_twoway;
use std::{collections::HashMap, sync::Arc};
use tarpc::server::{BaseChannel, Channel};
use tokio::sync::RwLock;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::sidecar_proxy_manager_rpc::SidecarProxyManagerRpcServer;

#[derive(Clone)]
pub struct SidecarProxyManager {
    pub(crate) sidecar_proxies: Arc<RwLock<HashMap<Uuid, SidecarProxyRegistration>>>,
    kube_cluster_rpc_client: Arc<RwLock<Option<KubeClusterRpcClient>>>,
}

#[derive(Clone)]
pub(crate) struct SidecarProxyRegistration {
    pub workload_id: Uuid,
    pub rpc_client: SidecarProxyRpcClient,
}

impl SidecarProxyManager {
    pub(crate) async fn new() -> Result<Self> {
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
            kube_cluster_rpc_client: Arc::new(RwLock::new(None)),
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

    pub async fn set_service_routes_if_registered(&self, environment_id: Uuid) -> Result<()> {
        let registration = {
            let map = self.sidecar_proxies.read().await;
            map.get(&environment_id).cloned()
        };

        let Some(registration) = registration else {
            return Ok(());
        };

        let cluster_client = {
            let guard = self.kube_cluster_rpc_client.read().await;
            guard.clone()
        };

        let Some(cluster_client) = cluster_client else {
            warn!(
                "Cluster RPC client unavailable; skipping route sync for environment {}",
                environment_id
            );
            return Ok(());
        };

        let routes = match cluster_client
            .list_branch_service_routes(
                tarpc::context::current(),
                environment_id,
                registration.workload_id,
            )
            .await
        {
            Ok(Ok(routes)) => routes,
            Ok(Err(err)) => {
                return Err(anyhow::anyhow!(
                    "API rejected route request for environment {}: {}",
                    environment_id,
                    err
                ));
            }
            Err(err) => {
                return Err(anyhow::anyhow!(
                    "Failed to fetch routes for environment {}: {}",
                    environment_id,
                    err
                ));
            }
        };

        if let Err(e) = registration
            .rpc_client
            .set_service_routes(tarpc::context::current(), routes)
            .await
        {
            return Err(anyhow::anyhow!("Failed to push service routes: {}", e));
        }

        Ok(())
    }

    pub async fn set_cluster_rpc_client(&self, client: KubeClusterRpcClient) {
        let mut guard = self.kube_cluster_rpc_client.write().await;
        *guard = Some(client);
    }

    pub async fn clear_cluster_rpc_client(&self) {
        let mut guard = self.kube_cluster_rpc_client.write().await;
        *guard = None;
    }
}
