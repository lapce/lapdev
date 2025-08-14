use lapdev_api_hrpc::HrpcService;
use lapdev_common::{
    hrpc::HrpcError,
    kube::{
        CreateKubeClusterResponse, KubeAppCatalog, KubeAppCatalogWorkload,
        KubeAppCatalogWorkloadCreate, KubeCluster, KubeClusterInfo, KubeEnvironment, KubeEnvironmentWorkload, KubeNamespace,
        KubeNamespaceInfo, KubeWorkload, KubeWorkloadKind, KubeWorkloadList, PagePaginationParams,
        PaginatedResult, PaginationParams,
    },
    UserRole,
};
use lapdev_rpc::error::ApiError;
use uuid::Uuid;

use crate::state::CoreState;

impl HrpcService for CoreState {
    async fn all_kube_clusters(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
    ) -> Result<Vec<KubeCluster>, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;
        self.kube_controller
            .get_all_kube_clusters(org_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn create_kube_cluster(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        name: String,
    ) -> Result<CreateKubeClusterResponse, HrpcError> {
        let user = self
            .authorize(headers, org_id, Some(UserRole::Admin))
            .await?;

        self.kube_controller
            .create_kube_cluster(org_id, user.id, name)
            .await
            .map_err(HrpcError::from)
    }

    async fn delete_kube_cluster(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
    ) -> Result<(), HrpcError> {
        let _ = self
            .authorize(headers, org_id, Some(UserRole::Admin))
            .await?;

        self.kube_controller
            .delete_kube_cluster(org_id, cluster_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn set_cluster_deployable(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
        can_deploy_personal: bool,
        can_deploy_shared: bool,
    ) -> Result<(), HrpcError> {
        let _ = self
            .authorize(headers, org_id, Some(UserRole::Admin))
            .await?;

        self.kube_controller
            .set_cluster_deployable(org_id, cluster_id, can_deploy_personal, can_deploy_shared)
            .await
            .map_err(HrpcError::from)
    }

    async fn set_cluster_personal_deployable(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
        can_deploy: bool,
    ) -> Result<(), HrpcError> {
        let _ = self
            .authorize(headers, org_id, Some(UserRole::Admin))
            .await?;

        self.kube_controller
            .set_cluster_personal_deployable(org_id, cluster_id, can_deploy)
            .await
            .map_err(HrpcError::from)
    }

    async fn set_cluster_shared_deployable(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
        can_deploy: bool,
    ) -> Result<(), HrpcError> {
        let _ = self
            .authorize(headers, org_id, Some(UserRole::Admin))
            .await?;

        self.kube_controller
            .set_cluster_shared_deployable(org_id, cluster_id, can_deploy)
            .await
            .map_err(HrpcError::from)
    }

    async fn get_workloads(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
        namespace: Option<String>,
        workload_kind_filter: Option<KubeWorkloadKind>,
        include_system_workloads: bool,
        pagination: Option<PaginationParams>,
    ) -> Result<KubeWorkloadList, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;

        self.kube_controller
            .get_workloads(
                org_id,
                cluster_id,
                namespace,
                workload_kind_filter,
                include_system_workloads,
                pagination,
            )
            .await
            .map_err(HrpcError::from)
    }

    async fn get_workload_details(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
        name: String,
        namespace: String,
    ) -> Result<Option<KubeWorkload>, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;

        self.kube_controller
            .get_workload_details(org_id, cluster_id, name, namespace)
            .await
            .map_err(HrpcError::from)
    }

    async fn get_cluster_namespaces(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
    ) -> Result<Vec<KubeNamespaceInfo>, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;

        self.kube_controller
            .get_cluster_namespaces(org_id, cluster_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn get_cluster_info(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
    ) -> Result<KubeClusterInfo, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;

        self.kube_controller
            .get_cluster_info(org_id, cluster_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn create_app_catalog(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
        name: String,
        description: Option<String>,
        workloads: Vec<KubeAppCatalogWorkloadCreate>,
    ) -> Result<Uuid, HrpcError> {
        let user = self
            .authorize(headers, org_id, Some(UserRole::Admin))
            .await?;

        self.kube_controller
            .create_app_catalog(org_id, user.id, cluster_id, name, description, workloads)
            .await
            .map_err(HrpcError::from)
    }

    async fn all_app_catalogs(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        search: Option<String>,
        pagination: Option<PagePaginationParams>,
    ) -> Result<PaginatedResult<KubeAppCatalog>, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;

        self.kube_controller
            .get_all_app_catalogs(org_id, search, pagination)
            .await
            .map_err(HrpcError::from)
    }

    async fn get_app_catalog(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        catalog_id: Uuid,
    ) -> Result<KubeAppCatalog, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;

        self.kube_controller
            .get_app_catalog(org_id, catalog_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn get_app_catalog_workloads(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        catalog_id: Uuid,
    ) -> Result<Vec<KubeAppCatalogWorkload>, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;

        self.kube_controller
            .get_app_catalog_workloads(org_id, catalog_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn all_kube_environments(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        search: Option<String>,
        is_shared: bool,
        pagination: Option<PagePaginationParams>,
    ) -> Result<PaginatedResult<KubeEnvironment>, HrpcError> {
        let user = self.authorize(headers, org_id, None).await?;

        self.kube_controller
            .get_all_kube_environments(org_id, user.id, search, is_shared, pagination)
            .await
            .map_err(HrpcError::from)
    }

    async fn get_kube_environment(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        environment_id: Uuid,
    ) -> Result<KubeEnvironment, HrpcError> {
        let user = self.authorize(headers, org_id, None).await?;
        self.kube_controller
            .get_kube_environment(org_id, user.id, environment_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn delete_kube_environment(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        environment_id: Uuid,
    ) -> Result<(), HrpcError> {
        let user = self.authorize(headers, org_id, None).await?;
        self.kube_controller
            .delete_kube_environment(org_id, user.id, environment_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn delete_app_catalog(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        catalog_id: Uuid,
    ) -> Result<(), HrpcError> {
        let _ = self
            .authorize(headers, org_id, Some(UserRole::Admin))
            .await?;

        self.kube_controller
            .delete_app_catalog(org_id, catalog_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn delete_app_catalog_workload(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        workload_id: Uuid,
    ) -> Result<(), HrpcError> {
        let _ = self
            .authorize(headers, org_id, Some(UserRole::Admin))
            .await?;

        self.kube_controller
            .delete_app_catalog_workload(org_id, workload_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn update_app_catalog_workload(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        workload_id: Uuid,
        containers: Vec<lapdev_common::kube::KubeContainerInfo>,
    ) -> Result<(), HrpcError> {
        let _ = self
            .authorize(headers, org_id, Some(UserRole::Admin))
            .await?;

        self.kube_controller
            .update_app_catalog_workload(org_id, workload_id, containers)
            .await
            .map_err(HrpcError::from)
    }

    async fn add_workloads_to_app_catalog(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        catalog_id: Uuid,
        workloads: Vec<KubeAppCatalogWorkloadCreate>,
    ) -> Result<(), HrpcError> {
        let user = self
            .authorize(headers, org_id, Some(UserRole::Admin))
            .await?;

        self.kube_controller
            .add_workloads_to_app_catalog(org_id, user.id, catalog_id, workloads)
            .await
            .map_err(HrpcError::from)
    }

    async fn create_kube_environment(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        app_catalog_id: Uuid,
        cluster_id: Uuid,
        name: String,
        namespace: String,
        is_shared: bool,
    ) -> Result<KubeEnvironment, HrpcError> {
        let user = self.authorize(headers, org_id, None).await?;

        self.kube_controller
            .create_kube_environment(
                org_id,
                user.id,
                app_catalog_id,
                cluster_id,
                name,
                namespace,
                is_shared,
            )
            .await
            .map_err(HrpcError::from)
    }

    async fn get_environment_workloads(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        environment_id: Uuid,
    ) -> Result<Vec<KubeEnvironmentWorkload>, HrpcError> {
        let user = self.authorize(headers, org_id, None).await?;
        self.kube_controller
            .get_environment_workloads(org_id, user.id, environment_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn get_environment_services(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        environment_id: Uuid,
    ) -> Result<Vec<lapdev_common::kube::KubeEnvironmentService>, HrpcError> {
        let user = self.authorize(headers, org_id, None).await?;
        self.kube_controller
            .get_environment_services(org_id, user.id, environment_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn get_environment_workload(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        workload_id: Uuid,
    ) -> Result<Option<KubeEnvironmentWorkload>, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;
        self.kube_controller
            .get_environment_workload(org_id, workload_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn delete_environment_workload(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        workload_id: Uuid,
    ) -> Result<(), HrpcError> {
        let _ = self
            .authorize(headers, org_id, Some(UserRole::Admin))
            .await?;
        self.kube_controller
            .delete_environment_workload(org_id, workload_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn create_kube_namespace(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        name: String,
        description: Option<String>,
        is_shared: bool,
    ) -> Result<KubeNamespace, HrpcError> {
        // Only admin can create shared namespaces, regular users can create personal ones
        let required_role = if is_shared {
            Some(UserRole::Admin)
        } else {
            None
        };
        let user = self.authorize(headers, org_id, required_role).await?;

        self.kube_controller
            .create_kube_namespace(org_id, user.id, name, description, is_shared)
            .await
            .map_err(HrpcError::from)
    }

    async fn all_kube_namespaces(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        is_shared: bool,
    ) -> Result<Vec<KubeNamespace>, HrpcError> {
        let user = self.authorize(headers, org_id, None).await?;

        self.kube_controller
            .get_all_kube_namespaces(org_id, user.id, is_shared)
            .await
            .map_err(HrpcError::from)
    }

    async fn delete_kube_namespace(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        namespace_id: Uuid,
    ) -> Result<(), HrpcError> {
        // Get the namespace first to check if it's shared
        let namespace = self
            .kube_controller
            .db
            .get_kube_namespace(namespace_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Namespace not found".to_string()))?;

        if namespace.organization_id != org_id {
            return Err(ApiError::Unauthorized)?;
        }

        // For shared namespaces, only admin can delete
        // For personal namespaces, any user can delete (with ownership check below)
        let required_role = if namespace.is_shared {
            Some(UserRole::Admin)
        } else {
            None
        };
        let user = self.authorize(headers, org_id, required_role).await?;

        // For personal namespaces, only the creator can delete
        if !namespace.is_shared && namespace.user_id != user.id {
            return Err(ApiError::InvalidRequest(
                "You can only delete namespaces you created".to_string(),
            ))?;
        }

        self.kube_controller
            .delete_kube_namespace(org_id, namespace_id)
            .await
            .map_err(HrpcError::from)
    }
}
