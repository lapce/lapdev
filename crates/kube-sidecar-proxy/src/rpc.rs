use anyhow::Result;
use lapdev_kube_rpc::{SidecarProxyManagerRpcClient, SidecarProxyRpc};
use uuid::Uuid;

#[derive(Clone)]
pub(crate) struct SidecarProxyRpcServer {
    workload_id: Uuid,
    rpc_client: SidecarProxyManagerRpcClient,
}

impl SidecarProxyRpcServer {
    pub(crate) fn new(workload_id: Uuid, rpc_client: SidecarProxyManagerRpcClient) -> Self {
        Self {
            workload_id,
            rpc_client,
        }
    }

    pub(crate) async fn register_sidecar_proxy(&self) -> Result<()> {
        let _ = self
            .rpc_client
            .register_sidecar_proxy(tarpc::context::current(), self.workload_id)
            .await?;
        Ok(())
    }
}

impl SidecarProxyRpc for SidecarProxyRpcServer {
    async fn heartbeat(self, context: ::tarpc::context::Context) -> Result<(), String> {
        todo!()
    }
}
