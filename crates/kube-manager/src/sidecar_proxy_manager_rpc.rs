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
        self.manager.sidecar_proxies.write().await.insert(
            environment_id,
            crate::sidecar_proxy_manager::SidecarProxyRegistration {
                workload_id,
                rpc_client: self.rpc_client.clone(),
            },
        );

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

    async fn request_devbox_tunnel(
        self,
        _context: ::tarpc::context::Context,
        intercept_id: Uuid,
        source_addr: String,
        target_port: u16,
    ) -> Result<lapdev_kube_rpc::DevboxTunnelInfo, String> {
        // TODO: Full implementation:
        // 1. Validate the intercept_id exists and is active
        // 2. Generate a unique tunnel_id
        // 3. Call API to register the tunnel (so API knows to expect sidecar WebSocket)
        // 4. Return WebSocket URL + auth token for sidecar to connect directly to API
        // 5. API will broker data between sidecar WebSocket ↔ devbox WebSocket
        //
        // New architecture (hybrid control/data plane):
        //   Control: Sidecar → Kube-Manager (RPC) → API (setup)
        //   Data:    Sidecar → API (WebSocket) → Devbox (direct streaming)

        tracing::info!(
            "Requesting devbox tunnel for intercept_id={}, source_addr={}, target_port={}",
            intercept_id,
            source_addr,
            target_port
        );

        // For now, return a placeholder with the expected structure
        Err("request_devbox_tunnel not yet fully implemented - need to integrate with TunnelRegistry in API".to_string())
    }
}
