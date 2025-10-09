use anyhow::Result;
use lapdev_common::kube::{
    KubeAppCatalogWorkload, KubeNamespaceInfo, KubeWorkload, KubeWorkloadKind, KubeWorkloadList,
    PaginationParams,
};
use lapdev_kube_rpc::{
    KubeClusterRpcClient, KubeManagerRpc, KubeWorkloadsWithResources, TunnelEstablishmentResponse,
    TunnelStatus, WorkloadIdentifier,
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

    async fn get_workloads_yaml(
        self,
        _context: ::tarpc::context::Context,
        workloads: Vec<KubeAppCatalogWorkload>,
    ) -> Result<KubeWorkloadsWithResources, String> {
        match self
            .manager
            .retrieve_workloads_yaml(workloads.clone())
            .await
        {
            Ok(workloads_with_resources) => {
                tracing::info!(
                    "Successfully retrieved {} workloads with {} services, {} configmaps, {} secrets",
                    workloads_with_resources.workloads.len(),
                    workloads_with_resources.services.len(),
                    workloads_with_resources.configmaps.len(),
                    workloads_with_resources.secrets.len()
                );
                Ok(workloads_with_resources)
            }
            Err(e) => {
                tracing::error!("Failed to retrieve workloads YAML: {}", e);
                Err(format!("Failed to retrieve workloads YAML: {}", e))
            }
        }
    }

    async fn get_workload_yaml(
        self,
        _context: ::tarpc::context::Context,
        workload: KubeAppCatalogWorkload,
    ) -> Result<lapdev_kube_rpc::KubeWorkloadYaml, String> {
        match self
            .manager
            .retrieve_single_workload_yaml(workload.clone())
            .await
        {
            Ok(workload_yaml) => {
                tracing::info!(
                    "Successfully retrieved workload YAML for {}/{}",
                    workload.namespace,
                    workload.name
                );
                Ok(workload_yaml)
            }
            Err(e) => {
                tracing::error!(
                    "Failed to retrieve workload YAML for {}/{}: {}",
                    workload.namespace,
                    workload.name,
                    e
                );
                Err(format!("Failed to retrieve workload YAML: {}", e))
            }
        }
    }

    async fn deploy_workload_yaml(
        self,
        _context: ::tarpc::context::Context,
        environment_id: uuid::Uuid,
        namespace: String,
        workloads_with_resources: KubeWorkloadsWithResources,
        labels: std::collections::HashMap<String, String>,
    ) -> Result<(), String> {
        match self
            .manager
            .apply_workloads_with_resources(
                Some(environment_id),
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

    async fn get_workloads_details(
        self,
        _context: ::tarpc::context::Context,
        workloads: Vec<WorkloadIdentifier>,
    ) -> Result<Vec<lapdev_common::kube::KubeWorkloadDetails>, String> {
        let mut details = Vec::new();

        for workload in workloads {
            match self
                .manager
                .get_workload_resource_details(&workload.name, &workload.namespace, &workload.kind)
                .await
            {
                Ok(containers) => {
                    details.push(lapdev_common::kube::KubeWorkloadDetails {
                        name: workload.name,
                        namespace: workload.namespace,
                        kind: workload.kind,
                        containers,
                    });
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to get workload details for {}/{}: {}",
                        workload.namespace,
                        workload.name,
                        e
                    );
                    return Err(format!(
                        "Failed to get workload details for {}/{}: {}",
                        workload.namespace, workload.name, e
                    ));
                }
            }
        }

        tracing::info!(
            "Successfully retrieved details for {} workloads",
            details.len()
        );
        Ok(details)
    }

    async fn update_workload_containers(
        self,
        _context: ::tarpc::context::Context,
        environment_id: Uuid,
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

    async fn establish_data_plane_tunnel(
        self,
        _context: ::tarpc::context::Context,
        controller_endpoint: String,
        auth_token: String,
    ) -> Result<TunnelEstablishmentResponse, String> {
        tracing::info!("Establishing data plane tunnel to {}", controller_endpoint);

        match self
            .manager
            .establish_tunnel(controller_endpoint, auth_token)
            .await
        {
            Ok(response) => {
                tracing::info!("Successfully established tunnel: {:?}", response);
                Ok(response)
            }
            Err(e) => {
                tracing::error!("Failed to establish tunnel: {}", e);
                Err(format!("Failed to establish tunnel: {}", e))
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
}
