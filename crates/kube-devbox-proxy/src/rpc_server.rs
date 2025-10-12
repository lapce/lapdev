use lapdev_kube_rpc::{BranchEnvironmentInfo, DevboxProxyRpc};
use tracing::info;
use uuid::Uuid;

use crate::branch_config::BranchConfig;

#[derive(Clone)]
pub struct DevboxProxyRpcServer {
    branch_config: BranchConfig,
}

impl DevboxProxyRpcServer {
    pub fn new(branch_config: BranchConfig) -> Self {
        Self { branch_config }
    }
}

impl DevboxProxyRpc for DevboxProxyRpcServer {
    async fn heartbeat(self, _context: ::tarpc::context::Context) -> Result<(), String> {
        tracing::debug!("Received heartbeat from kube-manager");
        Ok(())
    }

    async fn add_branch_environment(
        self,
        _context: ::tarpc::context::Context,
        branch: BranchEnvironmentInfo,
    ) -> Result<(), String> {
        info!(
            "Adding branch environment {} (namespace: {})",
            branch.environment_id, branch.namespace
        );

        self.branch_config.add_branch(branch).await;
        Ok(())
    }

    async fn remove_branch_environment(
        self,
        _context: ::tarpc::context::Context,
        environment_id: Uuid,
    ) -> Result<(), String> {
        info!("Removing branch environment {}", environment_id);

        self.branch_config.remove_branch(environment_id).await;
        Ok(())
    }

    async fn list_branch_environments(
        self,
        _context: ::tarpc::context::Context,
    ) -> Result<Vec<BranchEnvironmentInfo>, String> {
        let branches = self.branch_config.list_branches().await;
        info!("Listing {} branch environments", branches.len());
        Ok(branches)
    }
}
