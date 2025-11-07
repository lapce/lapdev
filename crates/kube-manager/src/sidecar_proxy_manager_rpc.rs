use lapdev_kube_rpc::{SidecarProxyManagerRpc, SidecarProxyRpcClient};
use std::{net::SocketAddr, sync::Arc};
use tarpc::context::Context;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::sidecar_proxy_manager::SidecarProxyManager;

#[derive(Clone)]
pub struct SidecarProxyManagerRpcServer {
    manager: SidecarProxyManager,
    pub(crate) rpc_client: SidecarProxyRpcClient,
    connection_generation: Arc<RwLock<Option<u64>>>,
    environment: Arc<RwLock<Option<Uuid>>>,
    workload: Arc<RwLock<Option<Uuid>>>,
    peer_addr: Option<SocketAddr>,
}

impl SidecarProxyManagerRpcServer {
    pub(crate) fn new(
        manager: SidecarProxyManager,
        rpc_client: SidecarProxyRpcClient,
        peer_addr: Option<SocketAddr>,
    ) -> Self {
        Self {
            manager,
            rpc_client,
            connection_generation: Arc::new(RwLock::new(None)),
            environment: Arc::new(RwLock::new(None)),
            workload: Arc::new(RwLock::new(None)),
            peer_addr,
        }
    }
}

impl SidecarProxyManagerRpc for SidecarProxyManagerRpcServer {
    async fn heartbeat(
        self,
        _context: Context,
        workload_id: Uuid,
        environment_id: Uuid,
    ) -> Result<(), String> {
        if self
            .manager
            .record_sidecar_heartbeat(environment_id, workload_id)
            .await
        {
            Ok(())
        } else {
            Err("sidecar proxy not registered".to_string())
        }
    }

    async fn register_sidecar_proxy(
        self,
        _context: Context,
        workload_id: Uuid,
        environment_id: Uuid,
        _namespace: String,
    ) -> Result<(), String> {
        {
            *self.environment.write().await = Some(environment_id);
            *self.workload.write().await = Some(workload_id);
        }

        let generation = self
            .manager
            .register_sidecar(environment_id, workload_id, self.rpc_client.clone())
            .await;

        {
            let mut guard = self.connection_generation.write().await;
            *guard = Some(generation);
        }

        self.manager
            .set_service_routes_if_registered(environment_id)
            .await
            .map_err(|e| e.to_string())?;

        Ok(())
    }

    async fn report_routing_metrics(
        self,
        _context: Context,
        _request_count: u64,
        _byte_count: u64,
        _active_connections: u32,
    ) -> Result<(), String> {
        Ok(())
    }
}
