use lapdev_kube_rpc::{SidecarProxyManagerRpc, SidecarProxyRpcClient};
use uuid::Uuid;

use crate::sidecar_proxy_manager::SidecarProxyManager;

#[derive(Clone)]
pub struct SidecarProxyManagerRpcServer {
    manager: SidecarProxyManager,
    _rpc_client: SidecarProxyRpcClient,
}

impl SidecarProxyManagerRpcServer {
    pub(crate) fn new(manager: SidecarProxyManager, rpc_client: SidecarProxyRpcClient) -> Self {
        Self {
            manager,
            _rpc_client: rpc_client,
        }
    }
}

impl SidecarProxyManagerRpc for SidecarProxyManagerRpcServer {
    async fn heartbeat(self, _context: ::tarpc::context::Context) -> Result<(), String> {
        todo!()
    }

    async fn register_sidecar_proxy(
        self,
        _context: ::tarpc::context::Context,
        workload_id: Uuid,
    ) -> Result<(), String> {
        self.manager
            .sidecar_proxies
            .write()
            .await
            .insert(workload_id, self.clone());
        Ok(())
    }

    async fn report_routing_metrics(
        self,
        _context: ::tarpc::context::Context,
        _request_count: u64,
        _byte_count: u64,
        _active_connections: u32,
    ) -> Result<(), String> {
        todo!()
    }
}
