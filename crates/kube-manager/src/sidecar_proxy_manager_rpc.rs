use lapdev_kube_rpc::{SidecarProxyManagerRpc, SidecarProxyRpcClient};
use uuid::Uuid;

use crate::sidecar_proxy_manager::SidecarProxyManager;

#[derive(Clone)]
pub struct SidecarProxyManagerRpcServer {
    manager: SidecarProxyManager,
    pub(crate) rpc_client: SidecarProxyRpcClient,
}

impl SidecarProxyManagerRpcServer {
    pub(crate) fn new(manager: SidecarProxyManager, rpc_client: SidecarProxyRpcClient) -> Self {
        Self {
            manager,
            rpc_client,
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
        environment_id: Uuid,
        _namespace: String,
    ) -> Result<(), String> {
        {
            let mut map = self.manager.sidecar_proxies.write().await;
            map.entry(environment_id)
                .or_default()
                .insert(workload_id, self.rpc_client.clone());
        }

        self.manager
            .set_service_routes_if_registered(environment_id)
            .await
            .map_err(|e| e.to_string())?;

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
