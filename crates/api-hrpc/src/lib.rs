use lapdev_common::{
    devbox::{
        DevboxEnvironmentSelection, DevboxPortMappingOverride, DevboxSessionListResponse,
        DevboxSessionWhoAmI, DevboxStartWorkloadInterceptResponse,
        DevboxWorkloadInterceptListResponse,
    },
    kube::{
        CreateKubeClusterResponse, CreateKubeEnvironmentPreviewUrlRequest, KubeAppCatalogWorkload,
        KubeAppCatalogWorkloadCreate, KubeCluster, KubeContainerInfo, KubeEnvironmentPreviewUrl,
        KubeEnvironmentService, KubeEnvironmentWorkload, KubeEnvironmentWorkloadDetail,
        KubeNamespace, KubeNamespaceInfo, KubeWorkload, KubeWorkloadKind, KubeWorkloadList,
        PagePaginationParams, PaginatedResult, PaginationParams, UpdateKubeEnvironmentPreviewUrlRequest,
    },
};
use uuid::Uuid;

pub use lapdev_common::hrpc::HrpcError;
pub use lapdev_common::kube::{
    KubeAppCatalog, KubeEnvironment, KubeEnvironmentDashboardSummary, KubeEnvironmentStatusCount,
};
// For backward compatibility
pub use lapdev_common::kube::KubeAppCatalog as AppCatalog;

#[lapdev_hrpc::service]
pub trait HrpcService {
    async fn devbox_session_list_sessions(&self) -> Result<DevboxSessionListResponse, HrpcError>;

    async fn devbox_session_revoke_session(&self, session_id: Uuid) -> Result<(), HrpcError>;

    async fn devbox_session_get_active_environment(
        &self,
    ) -> Result<Option<DevboxEnvironmentSelection>, HrpcError>;

    async fn devbox_session_set_active_environment(
        &self,
        environment_id: Uuid,
    ) -> Result<(), HrpcError>;

    async fn devbox_session_whoami(&self) -> Result<DevboxSessionWhoAmI, HrpcError>;

    async fn devbox_intercept_list(
        &self,
        environment_id: Uuid,
    ) -> Result<DevboxWorkloadInterceptListResponse, HrpcError>;

    async fn devbox_intercept_start(
        &self,
        workload_id: Uuid,
        port_mappings: Vec<DevboxPortMappingOverride>,
    ) -> Result<DevboxStartWorkloadInterceptResponse, HrpcError>;

    async fn devbox_intercept_stop(&self, intercept_id: Uuid) -> Result<(), HrpcError>;

    async fn all_kube_clusters(&self, org_id: Uuid) -> Result<Vec<KubeCluster>, HrpcError>;

    async fn create_kube_cluster(
        &self,
        org_id: Uuid,
        name: String,
    ) -> Result<CreateKubeClusterResponse, HrpcError>;

    async fn delete_kube_cluster(&self, org_id: Uuid, cluster_id: Uuid) -> Result<(), HrpcError>;

    async fn set_cluster_deployable(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
        can_deploy_personal: bool,
        can_deploy_shared: bool,
    ) -> Result<(), HrpcError>;

    async fn set_cluster_personal_deployable(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
        can_deploy: bool,
    ) -> Result<(), HrpcError>;

    async fn set_cluster_shared_deployable(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
        can_deploy: bool,
    ) -> Result<(), HrpcError>;

    async fn get_workloads(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
        namespace: Option<String>,
        workload_kind_filter: Option<KubeWorkloadKind>,
        include_system_workloads: bool,
        pagination: Option<PaginationParams>,
    ) -> Result<KubeWorkloadList, HrpcError>;

    async fn get_workload_details(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
        name: String,
        namespace: String,
    ) -> Result<Option<KubeWorkload>, HrpcError>;

    async fn get_cluster_namespaces(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
    ) -> Result<Vec<KubeNamespaceInfo>, HrpcError>;

    async fn get_cluster_info(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
    ) -> Result<KubeCluster, HrpcError>;

    async fn create_app_catalog(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
        name: String,
        description: Option<String>,
        workloads: Vec<KubeAppCatalogWorkloadCreate>,
    ) -> Result<Uuid, HrpcError>;

    async fn all_app_catalogs(
        &self,
        org_id: Uuid,
        search: Option<String>,
        pagination: Option<PagePaginationParams>,
    ) -> Result<PaginatedResult<KubeAppCatalog>, HrpcError>;

    async fn get_app_catalog(
        &self,
        org_id: Uuid,
        catalog_id: Uuid,
    ) -> Result<KubeAppCatalog, HrpcError>;

    async fn get_app_catalog_workloads(
        &self,
        org_id: Uuid,
        catalog_id: Uuid,
    ) -> Result<Vec<KubeAppCatalogWorkload>, HrpcError>;

    async fn delete_app_catalog(&self, org_id: Uuid, catalog_id: Uuid) -> Result<(), HrpcError>;

    async fn delete_app_catalog_workload(
        &self,
        org_id: Uuid,
        workload_id: Uuid,
    ) -> Result<(), HrpcError>;

    async fn update_app_catalog_workload(
        &self,
        org_id: Uuid,
        workload_id: Uuid,
        containers: Vec<KubeContainerInfo>,
    ) -> Result<(), HrpcError>;

    async fn add_workloads_to_app_catalog(
        &self,
        org_id: Uuid,
        catalog_id: Uuid,
        workloads: Vec<KubeAppCatalogWorkloadCreate>,
    ) -> Result<(), HrpcError>;

    async fn create_kube_environment(
        &self,
        org_id: Uuid,
        app_catalog_id: Uuid,
        cluster_id: Uuid,
        name: String,
        is_shared: bool,
    ) -> Result<KubeEnvironment, HrpcError>;

    async fn create_branch_environment(
        &self,
        org_id: Uuid,
        base_environment_id: Uuid,
        name: String,
    ) -> Result<KubeEnvironment, HrpcError>;

    async fn all_kube_environments(
        &self,
        org_id: Uuid,
        search: Option<String>,
        is_shared: bool,
        is_branch: bool,
        pagination: Option<PagePaginationParams>,
    ) -> Result<PaginatedResult<KubeEnvironment>, HrpcError>;

    async fn get_environment_dashboard_summary(
        &self,
        org_id: Uuid,
        recent_limit: Option<usize>,
    ) -> Result<KubeEnvironmentDashboardSummary, HrpcError>;

    async fn get_kube_environment(
        &self,
        org_id: Uuid,
        environment_id: Uuid,
    ) -> Result<KubeEnvironment, HrpcError>;

    async fn delete_kube_environment(
        &self,
        org_id: Uuid,
        environment_id: Uuid,
    ) -> Result<(), HrpcError>;

    async fn pause_kube_environment(
        &self,
        org_id: Uuid,
        environment_id: Uuid,
    ) -> Result<(), HrpcError>;

    async fn resume_kube_environment(
        &self,
        org_id: Uuid,
        environment_id: Uuid,
    ) -> Result<(), HrpcError>;

    async fn sync_environment_from_catalog(
        &self,
        org_id: Uuid,
        environment_id: Uuid,
    ) -> Result<(), HrpcError>;

    // Kube Environment Workload operations
    async fn get_environment_workloads(
        &self,
        org_id: Uuid,
        environment_id: Uuid,
    ) -> Result<Vec<KubeEnvironmentWorkload>, HrpcError>;

    async fn get_environment_services(
        &self,
        org_id: Uuid,
        environment_id: Uuid,
    ) -> Result<Vec<KubeEnvironmentService>, HrpcError>;

    async fn get_environment_workload(
        &self,
        org_id: Uuid,
        workload_id: Uuid,
    ) -> Result<Option<KubeEnvironmentWorkload>, HrpcError>;

    async fn get_environment_workload_detail(
        &self,
        org_id: Uuid,
        environment_id: Uuid,
        workload_id: Uuid,
    ) -> Result<KubeEnvironmentWorkloadDetail, HrpcError>;

    async fn update_environment_workload(
        &self,
        org_id: Uuid,
        environment_id: Uuid,
        workload_id: Uuid,
        containers: Vec<KubeContainerInfo>,
    ) -> Result<Uuid, HrpcError>;

    // Kube Namespace operations
    async fn create_kube_namespace(
        &self,
        org_id: Uuid,
        name: String,
        description: Option<String>,
        is_shared: bool,
    ) -> Result<KubeNamespace, HrpcError>;

    async fn all_kube_namespaces(
        &self,
        org_id: Uuid,
        is_shared: bool,
    ) -> Result<Vec<KubeNamespace>, HrpcError>;

    async fn delete_kube_namespace(
        &self,
        org_id: Uuid,
        namespace_id: Uuid,
    ) -> Result<(), HrpcError>;

    // Kube Environment Preview URL operations
    async fn create_environment_preview_url(
        &self,
        org_id: Uuid,
        environment_id: Uuid,
        request: CreateKubeEnvironmentPreviewUrlRequest,
    ) -> Result<KubeEnvironmentPreviewUrl, HrpcError>;

    async fn get_environment_preview_urls(
        &self,
        org_id: Uuid,
        environment_id: Uuid,
    ) -> Result<Vec<KubeEnvironmentPreviewUrl>, HrpcError>;

    async fn update_environment_preview_url(
        &self,
        org_id: Uuid,
        preview_url_id: Uuid,
        request: UpdateKubeEnvironmentPreviewUrlRequest,
    ) -> Result<KubeEnvironmentPreviewUrl, HrpcError>;

    async fn delete_environment_preview_url(
        &self,
        org_id: Uuid,
        preview_url_id: Uuid,
    ) -> Result<(), HrpcError>;
}
