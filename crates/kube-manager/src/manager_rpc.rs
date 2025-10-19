use anyhow::Result;
use lapdev_common::kube::{
    KubeNamespaceInfo, KubeWorkload, KubeWorkloadKind, KubeWorkloadList, PaginationParams,
};
use lapdev_kube_rpc::{
    KubeClusterRpcClient, KubeManagerRpc, KubeRawWorkloadYaml, KubeWorkloadsWithResources,
    NamespacedResourceRequest, NamespacedResourceResponse, TunnelStatus, WorkloadIdentifier,
};
use uuid::Uuid;

use crate::manager::KubeManager;

#[derive(Clone)]
pub struct KubeManagerRpcServer {
    manager: KubeManager,
    rpc_client: KubeClusterRpcClient,
}

impl KubeManagerRpcServer {
    pub(crate) fn new(manager: KubeManager, rpc_client: KubeClusterRpcClient) -> Self {
        Self {
            manager,
            rpc_client,
        }
    }

    pub(crate) async fn report_cluster_info(&self) -> Result<()> {
        let cluster_info = self.manager.collect_cluster_info().await?;
        tracing::info!("Reporting cluster info: {:?}", cluster_info);

        match self
            .rpc_client
            .report_cluster_info(tarpc::context::current(), cluster_info)
            .await
        {
            Ok(_) => tracing::info!("Successfully reported cluster info"),
            Err(e) => tracing::error!("RPC call failed: {}", e),
        }

        Ok(())
    }
}

impl KubeManagerRpc for KubeManagerRpcServer {
    async fn get_workloads(
        self,
        _context: ::tarpc::context::Context,
        namespace: Option<String>,
        workload_kind_filter: Option<KubeWorkloadKind>,
        include_system_workloads: bool,
        pagination: Option<PaginationParams>,
    ) -> Result<KubeWorkloadList, String> {
        match self
            .manager
            .collect_workloads(
                namespace,
                workload_kind_filter,
                include_system_workloads,
                pagination,
            )
            .await
        {
            Ok(workloads) => {
                tracing::info!(
                    "Successfully collected {} workloads",
                    workloads.workloads.len()
                );
                Ok(workloads)
            }
            Err(e) => {
                tracing::error!("Failed to collect workloads: {e}");
                Err(format!("Failed to collect workloads: {e}"))
            }
        }
    }

    async fn get_workload_details(
        self,
        _context: ::tarpc::context::Context,
        name: String,
        namespace: String,
    ) -> Result<Option<KubeWorkload>, String> {
        match self
            .manager
            .get_workload_by_name(name.clone(), namespace.clone())
            .await
        {
            Ok(workload) => {
                if workload.is_some() {
                    tracing::info!("Successfully found workload: {namespace}/{name}");
                } else {
                    tracing::warn!("Workload not found: {namespace}/{name}");
                }
                Ok(workload)
            }
            Err(e) => {
                tracing::error!("Failed to get workload details for {namespace}/{name}: {e}");
                Err(format!("Failed to get workload details: {e}"))
            }
        }
    }

    async fn get_namespaces(
        self,
        _context: ::tarpc::context::Context,
    ) -> Result<Vec<KubeNamespaceInfo>, String> {
        match self.manager.collect_namespaces().await {
            Ok(namespaces) => {
                tracing::info!("Successfully collected {} namespaces", namespaces.len());
                Ok(namespaces)
            }
            Err(e) => {
                tracing::error!("Failed to collect namespaces: {e}");
                Err(format!("Failed to collect namespaces: {e}"))
            }
        }
    }

    async fn deploy_workload_yaml(
        self,
        _context: ::tarpc::context::Context,
        environment_id: uuid::Uuid,
        environment_auth_token: String,
        namespace: String,
        workloads_with_resources: KubeWorkloadsWithResources,
        labels: std::collections::HashMap<String, String>,
    ) -> Result<(), String> {
        match self
            .manager
            .apply_workloads_with_resources(
                Some(environment_id),
                environment_auth_token,
                namespace.clone(),
                workloads_with_resources,
                labels,
            )
            .await
        {
            Ok(()) => {
                tracing::info!("Successfully deployed workloads to namespace: {namespace}");
                Ok(())
            }
            Err(e) => {
                tracing::error!("Failed to deploy workloads to namespace {namespace}: {e}");
                Err(format!("Failed to deploy workloads: {e}"))
            }
        }
    }

    async fn get_workloads_raw_yaml(
        self,
        _context: ::tarpc::context::Context,
        workloads: Vec<WorkloadIdentifier>,
    ) -> Result<Vec<KubeRawWorkloadYaml>, String> {
        let mut result = Vec::with_capacity(workloads.len());

        for workload in workloads {
            match self
                .manager
                .get_raw_workload_yaml(&workload.name, &workload.namespace, &workload.kind)
                .await
            {
                Ok(yaml) => {
                    result.push(KubeRawWorkloadYaml {
                        name: workload.name,
                        namespace: workload.namespace,
                        kind: workload.kind,
                        workload_yaml: yaml,
                    });
                }
                Err(e) => {
                    let msg = format!(
                        "Failed to fetch raw YAML for {}/{}: {e}",
                        workload.namespace, workload.name
                    );
                    tracing::error!("{}", msg);
                    return Err(msg);
                }
            }
        }

        Ok(result)
    }

    async fn get_namespaced_resources(
        self,
        _context: ::tarpc::context::Context,
        requests: Vec<NamespacedResourceRequest>,
    ) -> Result<Vec<NamespacedResourceResponse>, String> {
        match self.manager.fetch_namespaced_resources(requests).await {
            Ok(resources) => Ok(resources),
            Err(e) => {
                tracing::error!("Failed to fetch namespaced resources: {e}");
                Err(format!("Failed to fetch namespaced resources: {e}"))
            }
        }
    }

    async fn update_workload_containers(
        self,
        _context: ::tarpc::context::Context,
        environment_id: Uuid,
        environment_auth_token: String,
        workload_id: uuid::Uuid,
        name: String,
        namespace: String,
        kind: lapdev_common::kube::KubeWorkloadKind,
        containers: Vec<lapdev_common::kube::KubeContainerInfo>,
        labels: std::collections::HashMap<String, String>,
    ) -> Result<(), String> {
        tracing::info!(
            "Starting atomic update for workload {}/{} (id: {})",
            namespace,
            name,
            workload_id
        );

        match self
            .manager
            .update_workload_containers_atomic(
                environment_id,
                environment_auth_token,
                workload_id,
                name.clone(),
                namespace.clone(),
                kind,
                containers,
                labels,
            )
            .await
        {
            Ok(()) => {
                tracing::info!(
                    "Successfully updated workload containers for {}/{}",
                    namespace,
                    name
                );
                Ok(())
            }
            Err(e) => {
                tracing::error!(
                    "Failed to update workload containers for {}/{}: {}",
                    namespace,
                    name,
                    e
                );
                Err(format!("Failed to update workload containers: {}", e))
            }
        }
    }

    async fn create_branch_workload(
        self,
        _context: ::tarpc::context::Context,
        base_workload_id: uuid::Uuid,
        base_workload_name: String,
        branch_environment_id: uuid::Uuid,
        branch_environment_auth_token: String,
        namespace: String,
        kind: lapdev_common::kube::KubeWorkloadKind,
        containers: Vec<lapdev_common::kube::KubeContainerInfo>,
        labels: std::collections::HashMap<String, String>,
    ) -> Result<(), String> {
        tracing::info!(
            "Creating branch deployment for environment '{}' based on '{}' in namespace '{}' (base id: {})",
            branch_environment_id, base_workload_name, namespace, base_workload_id
        );

        match self
            .manager
            .create_branch_workload_atomic(
                base_workload_id,
                base_workload_name.clone(),
                branch_environment_id,
                branch_environment_auth_token,
                namespace.clone(),
                kind,
                containers,
                labels,
            )
            .await
        {
            Ok(()) => {
                tracing::info!(
                    "Successfully created branch deployment for environment '{}' based on '{}/{}' (id: {})",
                    branch_environment_id, namespace, base_workload_name, base_workload_id
                );
                Ok(())
            }
            Err(e) => {
                tracing::error!(
                    "Failed to create branch deployment for environment '{}' based on '{}/{}': {}",
                    branch_environment_id,
                    namespace,
                    base_workload_name,
                    e
                );
                Err(format!("Failed to create branch deployment: {}", e))
            }
        }
    }

    async fn destroy_environment(
        self,
        _context: ::tarpc::context::Context,
        environment_id: Uuid,
        namespace: String,
    ) -> Result<(), String> {
        tracing::info!(
            "Received request to destroy environment {} in namespace {}",
            environment_id,
            namespace
        );

        match self
            .manager
            .destroy_environment(environment_id, &namespace)
            .await
        {
            Ok(()) => Ok(()),
            Err(e) => {
                tracing::error!(
                    "Failed to destroy environment {} in namespace {}: {}",
                    environment_id,
                    namespace,
                    e
                );
                Err(format!("Failed to destroy environment: {}", e))
            }
        }
    }

    async fn get_tunnel_status(
        self,
        _context: ::tarpc::context::Context,
    ) -> Result<TunnelStatus, String> {
        self.manager
            .get_tunnel_status()
            .await
            .map_err(|e| format!("Failed to get tunnel status: {}", e))
    }

    async fn close_tunnel_connection(
        self,
        _context: ::tarpc::context::Context,
        tunnel_id: String,
    ) -> Result<(), String> {
        tracing::info!("Closing tunnel connection: {}", tunnel_id);

        self.manager
            .close_tunnel_connection(tunnel_id)
            .await
            .map_err(|e| format!("Failed to close tunnel connection: {}", e))
    }

    async fn add_branch_environment(
        self,
        _context: ::tarpc::context::Context,
        base_environment_id: Uuid,
        branch: lapdev_kube_rpc::BranchEnvironmentInfo,
    ) -> Result<(), String> {
        tracing::info!(
            "Adding branch environment {} to devbox-proxy for base environment {}",
            branch.environment_id,
            base_environment_id
        );

        // Get the devbox-proxy RPC client for the base environment
        let proxy_client = self
            .manager
            .devbox_proxy_manager
            .get_proxy_client(base_environment_id)
            .await
            .ok_or_else(|| {
                format!(
                    "No devbox-proxy registered for base environment {}",
                    base_environment_id
                )
            })?;

        // Forward the request to the devbox-proxy
        proxy_client
            .add_branch_environment(tarpc::context::current(), branch)
            .await
            .map_err(|e| format!("RPC call to devbox-proxy failed: {}", e))?
    }

    async fn remove_branch_environment(
        self,
        _context: ::tarpc::context::Context,
        base_environment_id: Uuid,
        branch_environment_id: Uuid,
    ) -> Result<(), String> {
        tracing::info!(
            "Removing branch environment {} from devbox-proxy for base environment {}",
            branch_environment_id,
            base_environment_id
        );

        // Get the devbox-proxy RPC client for the base environment
        let proxy_client = self
            .manager
            .devbox_proxy_manager
            .get_proxy_client(base_environment_id)
            .await
            .ok_or_else(|| {
                format!(
                    "No devbox-proxy registered for base environment {}",
                    base_environment_id
                )
            })?;

        // Forward the request to the devbox-proxy
        proxy_client
            .remove_branch_environment(tarpc::context::current(), branch_environment_id)
            .await
            .map_err(|e| format!("RPC call to devbox-proxy failed: {}", e))?
    }
}
