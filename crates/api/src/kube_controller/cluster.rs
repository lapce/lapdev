use std::str::FromStr;
use uuid::Uuid;

use lapdev_common::{
    kube::{
        CreateKubeClusterResponse, KubeCluster, KubeClusterInfo, KubeClusterStatus,
        KubeNamespaceInfo, KubeWorkload, KubeWorkloadKind, KubeWorkloadList, PaginationParams,
    },
    token::PlainToken,
};
use lapdev_rpc::error::ApiError;
use secrecy::ExposeSecret;

use super::KubeController;

impl KubeController {
    pub async fn get_all_kube_clusters(&self, org_id: Uuid) -> Result<Vec<KubeCluster>, ApiError> {
        let clusters = self
            .db
            .get_all_kube_clusters(org_id)
            .await
            .map_err(ApiError::from)?
            .into_iter()
            .map(|c| KubeCluster {
                id: c.id,
                name: c.name.clone(),
                can_deploy_personal: c.can_deploy_personal,
                can_deploy_shared: c.can_deploy_shared,
                info: KubeClusterInfo {
                    cluster_version: c.cluster_version.unwrap_or("Unknown".to_string()),
                    node_count: 0, // TODO: Get actual node count from kube-manager
                    available_cpu: "N/A".to_string(), // TODO: Get actual CPU from kube-manager
                    available_memory: "N/A".to_string(), // TODO: Get actual memory from kube-manager
                    provider: c.provider.clone(),
                    region: c.region,
                    status: KubeClusterStatus::from_str(&c.status)
                        .unwrap_or(KubeClusterStatus::NotReady),
                    manager_namespace: c.manager_namespace.clone(),
                },
            })
            .collect();
        Ok(clusters)
    }

    pub async fn create_kube_cluster(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        name: String,
    ) -> Result<CreateKubeClusterResponse, ApiError> {
        // Generate cluster ID and token
        let cluster_id = Uuid::new_v4();
        let token = PlainToken::generate();
        let hashed_token = token.hashed();
        let token_name = format!("{name}-default");

        // Create the cluster
        self.db
            .create_kube_cluster(
                cluster_id,
                org_id,
                user_id,
                name,
                KubeClusterStatus::Provisioning.to_string(),
            )
            .await
            .map_err(ApiError::from)?;

        // Create the cluster token
        self.db
            .create_kube_cluster_token(
                cluster_id,
                user_id,
                token_name,
                hashed_token.expose_secret().to_vec(),
            )
            .await
            .map_err(ApiError::from)?;

        Ok(CreateKubeClusterResponse {
            cluster_id,
            token: token.expose_secret().to_string(),
        })
    }

    pub async fn delete_kube_cluster(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
    ) -> Result<(), ApiError> {
        // Verify cluster belongs to the organization
        let cluster = self
            .db
            .get_kube_cluster(cluster_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Cluster not found".to_string()))?;

        if cluster.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // Check for dependencies - kube_app_catalog
        let has_app_catalogs = self
            .db
            .check_kube_cluster_has_app_catalogs(cluster_id)
            .await
            .map_err(ApiError::from)?;

        if has_app_catalogs {
            return Err(ApiError::InvalidRequest(
                "Cannot delete cluster: it has active app catalogs. Please delete them first."
                    .to_string(),
            ));
        }

        // Check for dependencies - kube_environment
        let has_environments = self
            .db
            .check_kube_cluster_has_environments(cluster_id)
            .await
            .map_err(ApiError::from)?;

        if has_environments {
            return Err(ApiError::InvalidRequest(
                "Cannot delete cluster: it has active environments. Please delete them first."
                    .to_string(),
            ));
        }

        // Soft delete the cluster
        self.db
            .delete_kube_cluster(cluster_id)
            .await
            .map_err(ApiError::from)
    }

    pub async fn set_cluster_deployable(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
        can_deploy_personal: bool,
        can_deploy_shared: bool,
    ) -> Result<(), ApiError> {
        // Verify cluster belongs to the organization
        let cluster = self
            .db
            .get_kube_cluster(cluster_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Cluster not found".to_string()))?;

        if cluster.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // Update the deployment capability fields
        self.db
            .set_cluster_deployable(cluster_id, can_deploy_personal, can_deploy_shared)
            .await
            .map_err(ApiError::from)?;

        Ok(())
    }

    pub async fn set_cluster_personal_deployable(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
        can_deploy: bool,
    ) -> Result<(), ApiError> {
        // Verify cluster belongs to the organization
        let cluster = self
            .db
            .get_kube_cluster(cluster_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Cluster not found".to_string()))?;

        if cluster.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // Update only the personal deployment capability
        self.db
            .set_cluster_deployable(cluster_id, can_deploy, cluster.can_deploy_shared)
            .await
            .map_err(ApiError::from)?;

        Ok(())
    }

    pub async fn set_cluster_shared_deployable(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
        can_deploy: bool,
    ) -> Result<(), ApiError> {
        // Verify cluster belongs to the organization
        let cluster = self
            .db
            .get_kube_cluster(cluster_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Cluster not found".to_string()))?;

        if cluster.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // Update only the shared deployment capability
        self.db
            .set_cluster_deployable(cluster_id, cluster.can_deploy_personal, can_deploy)
            .await
            .map_err(ApiError::from)?;

        Ok(())
    }

    pub async fn get_workloads(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
        namespace: Option<String>,
        workload_kind_filter: Option<KubeWorkloadKind>,
        include_system_workloads: bool,
        pagination: Option<PaginationParams>,
    ) -> Result<KubeWorkloadList, ApiError> {
        // Verify cluster belongs to the organization
        let cluster = self
            .db
            .get_kube_cluster(cluster_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Cluster not found".to_string()))?;

        if cluster.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // Get a connected KubeClusterServer for this cluster
        let server = self
            .get_random_kube_cluster_server(cluster_id)
            .await
            .ok_or_else(|| {
                ApiError::InvalidRequest("No connected KubeManager for this cluster".to_string())
            })?;

        let pagination = pagination.unwrap_or(PaginationParams {
            cursor: None,
            limit: 20,
        });

        // Call KubeManager to get workloads
        match server
            .rpc_client
            .get_workloads(
                tarpc::context::current(),
                namespace,
                workload_kind_filter,
                include_system_workloads,
                Some(pagination),
            )
            .await
        {
            Ok(Ok(workload_list)) => Ok(workload_list),
            Ok(Err(e)) => Err(ApiError::InvalidRequest(format!(
                "KubeManager error: {}",
                e
            ))),
            Err(e) => Err(ApiError::InvalidRequest(format!("Connection error: {}", e))),
        }
    }

    pub async fn get_workload_details(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
        name: String,
        namespace: String,
    ) -> Result<Option<KubeWorkload>, ApiError> {
        // Verify cluster belongs to the organization
        let cluster = self
            .db
            .get_kube_cluster(cluster_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Cluster not found".to_string()))?;

        if cluster.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // Get a connected KubeClusterServer for this cluster
        let server = self
            .get_random_kube_cluster_server(cluster_id)
            .await
            .ok_or_else(|| {
                ApiError::InvalidRequest("No connected KubeManager for this cluster".to_string())
            })?;

        // Call KubeManager to get workload details
        match server
            .rpc_client
            .get_workload_details(tarpc::context::current(), name, namespace)
            .await
        {
            Ok(Ok(workload_details)) => Ok(workload_details),
            Ok(Err(e)) => Err(ApiError::InvalidRequest(format!(
                "KubeManager error: {}",
                e
            ))),
            Err(e) => Err(ApiError::InvalidRequest(format!("Connection error: {}", e))),
        }
    }

    pub async fn get_cluster_namespaces(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
    ) -> Result<Vec<KubeNamespaceInfo>, ApiError> {
        // Verify cluster belongs to the organization
        let cluster = self
            .db
            .get_kube_cluster(cluster_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Cluster not found".to_string()))?;

        if cluster.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // Get a connected KubeClusterServer for this cluster
        let server = self
            .get_random_kube_cluster_server(cluster_id)
            .await
            .ok_or_else(|| {
                ApiError::InvalidRequest("No connected KubeManager for this cluster".to_string())
            })?;

        // Call KubeManager to get namespaces
        match server
            .rpc_client
            .get_namespaces(tarpc::context::current())
            .await
        {
            Ok(Ok(namespaces)) => Ok(namespaces),
            Ok(Err(e)) => Err(ApiError::InvalidRequest(format!(
                "KubeManager error: {}",
                e
            ))),
            Err(e) => Err(ApiError::InvalidRequest(format!("Connection error: {}", e))),
        }
    }

    pub async fn get_cluster_info(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
    ) -> Result<KubeCluster, ApiError> {
        // Get cluster from database
        let cluster = self
            .db
            .get_kube_cluster(cluster_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Cluster not found".to_string()))?;

        if cluster.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // Convert database cluster to KubeClusterInfo
        let cluster_info = KubeClusterInfo {
            cluster_version: cluster
                .cluster_version
                .clone()
                .unwrap_or_else(|| "Unknown".to_string()),
            node_count: 0, // TODO: Get actual node count from kube-manager
            available_cpu: "N/A".to_string(), // TODO: Get actual CPU from kube-manager
            available_memory: "N/A".to_string(), // TODO: Get actual memory from kube-manager
            provider: cluster.provider.clone(),
            region: cluster.region.clone(),
            status: KubeClusterStatus::from_str(&cluster.status)
                .unwrap_or(KubeClusterStatus::NotReady),
            manager_namespace: cluster.manager_namespace.clone(),
        };

        Ok(KubeCluster {
            id: cluster.id,
            name: cluster.name,
            can_deploy_personal: cluster.can_deploy_personal,
            can_deploy_shared: cluster.can_deploy_shared,
            info: cluster_info,
        })
    }
}
