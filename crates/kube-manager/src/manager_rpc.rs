use anyhow::{anyhow, Result};
use lapdev_common::kube::{
    KubeNamespaceInfo, KubeWorkload, KubeWorkloadKind, KubeWorkloadList, PaginationParams,
};
use lapdev_kube_rpc::{
    DevboxRouteConfig, KubeClusterRpcClient, KubeManagerRpc, KubeRawWorkloadYaml,
    KubeWorkloadsWithResources, NamespacedResourceRequest, NamespacedResourceResponse,
    ProxyBranchRouteConfig, WorkloadIdentifier,
};
use std::collections::HashMap;
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

        let _ = self
            .rpc_client
            .report_cluster_info(tarpc::context::current(), cluster_info)
            .await
            .map_err(|e| {
                tracing::error!("RPC call failed: {}", e);
                anyhow!("report_cluster_info RPC failed: {e}")
            })?;

        tracing::info!("Successfully reported cluster info");

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
        namespace: String,
        workloads_with_resources: KubeWorkloadsWithResources,
        labels: std::collections::HashMap<String, String>,
    ) -> Result<(), String> {
        match self
            .manager
            .apply_workloads_with_resources(namespace.clone(), workloads_with_resources, labels)
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

    async fn set_devbox_routes(
        self,
        _context: ::tarpc::context::Context,
        environment_id: Uuid,
        routes: HashMap<Uuid, DevboxRouteConfig>,
    ) -> Result<(), String> {
        self.manager.set_devbox_routes(environment_id, routes).await
    }

    async fn update_branch_service_route(
        self,
        _context: ::tarpc::context::Context,
        base_environment_id: Uuid,
        workload_id: Uuid,
        route: ProxyBranchRouteConfig,
    ) -> Result<(), String> {
        self.manager
            .update_branch_service_route(base_environment_id, workload_id, route)
            .await
    }

    async fn remove_branch_service_route(
        self,
        _context: ::tarpc::context::Context,
        base_environment_id: Uuid,
        workload_id: Uuid,
        branch_environment_id: Uuid,
    ) -> Result<(), String> {
        self.manager
            .remove_branch_service_route(base_environment_id, workload_id, branch_environment_id)
            .await
    }

    async fn clear_devbox_routes(
        self,
        _context: ::tarpc::context::Context,
        environment_id: Uuid,
        branch_environment_id: Option<Uuid>,
    ) -> Result<(), String> {
        self.manager
            .clear_devbox_routes(environment_id, branch_environment_id)
            .await
    }

    async fn refresh_branch_service_routes(
        self,
        _context: ::tarpc::context::Context,
        environment_id: Uuid,
    ) -> Result<(), String> {
        match self
            .manager
            .refresh_branch_service_routes(environment_id)
            .await
        {
            Ok(()) => Ok(()),
            Err(e) => {
                tracing::warn!(
                    "Failed to refresh branch service routes for environment {}: {}",
                    environment_id,
                    e
                );
                Err(format!("Failed to refresh branch service routes: {}", e))
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

    async fn configure_watches(
        self,
        _context: ::tarpc::context::Context,
        namespaces: Vec<String>,
    ) -> Result<(), String> {
        self.manager
            .configure_namespace_watches(namespaces)
            .await
            .map_err(|e| {
                tracing::error!("Failed to configure namespace watches: {e}");
                format!("Failed to configure namespace watches: {e}")
            })
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
        let Some(proxy_client) = self
            .manager
            .devbox_proxy_manager
            .get_proxy_client(base_environment_id)
            .await
        else {
            // if the devbox proxy doesn't connect, we don't need to do anything
            return Ok(());
        };

        // Forward the request to the devbox-proxy
        proxy_client
            .remove_branch_environment(tarpc::context::current(), branch_environment_id)
            .await
            .map_err(|e| format!("RPC call to devbox-proxy failed: {}", e))?
    }
}
