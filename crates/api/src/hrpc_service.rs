use lapdev_api_hrpc::HrpcService;
use lapdev_common::{
    hrpc::HrpcError,
    kube::{
        CreateKubeClusterResponse, KubeAppCatalog, KubeCluster, KubeClusterInfo,
        KubeEnvironment, KubeNamespace, KubeWorkload, KubeWorkloadKind,
        KubeWorkloadList, PagePaginationParams, PaginatedResult, PaginationParams,
    },
    UserRole,
};
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
        can_deploy: bool,
    ) -> Result<(), HrpcError> {
        let _ = self.authorize(headers, org_id, Some(UserRole::Admin)).await?;
        
        self.kube_controller
            .set_cluster_deployable(org_id, cluster_id, can_deploy)
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

    async fn get_namespaces(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
    ) -> Result<Vec<KubeNamespace>, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;

        self.kube_controller
            .get_namespaces(org_id, cluster_id)
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
        resources: String,
    ) -> Result<(), HrpcError> {
        let user = self.authorize(headers, org_id, None).await?;

        self.kube_controller
            .create_app_catalog(
                org_id,
                user.id,
                cluster_id,
                name,
                description,
                resources,
            )
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

    async fn all_kube_environments(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        search: Option<String>,
        pagination: Option<PagePaginationParams>,
    ) -> Result<PaginatedResult<KubeEnvironment>, HrpcError> {
        let user = self.authorize(headers, org_id, None).await?;

        self.kube_controller
            .get_all_kube_environments(org_id, user.id, search, pagination)
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

    async fn create_kube_environment(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        app_catalog_id: Uuid,
        cluster_id: Uuid,
        name: String,
        namespace: String,
    ) -> Result<(), HrpcError> {
        let user = self.authorize(headers, org_id, None).await?;

        self.kube_controller
            .create_kube_environment(
                org_id,
                user.id,
                app_catalog_id,
                cluster_id,
                name,
                namespace,
            )
            .await
            .map_err(HrpcError::from)
    }
}