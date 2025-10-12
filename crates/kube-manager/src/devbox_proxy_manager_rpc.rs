use lapdev_kube_rpc::{DevboxProxyManagerRpc, DevboxProxyRpcClient};
use tracing::info;
use uuid::Uuid;

use crate::devbox_proxy_manager::DevboxProxyManager;

#[derive(Clone)]
pub struct DevboxProxyManagerRpcServer {
    manager: DevboxProxyManager,
    rpc_client: DevboxProxyRpcClient,
}

impl DevboxProxyManagerRpcServer {
    pub(crate) fn new(
        manager: DevboxProxyManager,
        rpc_client: DevboxProxyRpcClient,
    ) -> Self {
        Self {
            manager,
            rpc_client,
        }
    }
}

impl DevboxProxyManagerRpc for DevboxProxyManagerRpcServer {
    async fn register_devbox_proxy(
        self,
        _context: ::tarpc::context::Context,
        environment_id: Uuid,
    ) -> Result<(), String> {
        info!(
            "Registering devbox-proxy for environment {}",
            environment_id
        );

        self.manager
            .devbox_proxies
            .write()
            .await
            .insert(environment_id, self.rpc_client.clone());

        Ok(())
    }

    async fn heartbeat(
        self,
        _context: ::tarpc::context::Context,
        environment_id: Uuid,
    ) -> Result<(), String> {
        // TODO: Update last_heartbeat timestamp
        tracing::debug!("Heartbeat from devbox-proxy for environment {}", environment_id);
        Ok(())
    }
}
