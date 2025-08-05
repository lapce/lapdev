use lapdev_common::kube::{
    CreateKubeClusterResponse, KubeCluster, KubeClusterInfo, KubeNamespace, KubeWorkload,
    KubeWorkloadKind, KubeWorkloadList, PaginationParams, PagePaginationParams, PaginatedResult,
    KubeAppCatalogWorkload, KubeAppCatalogWorkloadCreate,
};
use uuid::Uuid;

pub use lapdev_common::hrpc::HrpcError;
pub use lapdev_common::kube::{KubeAppCatalog, KubeEnvironment};
// For backward compatibility
pub use lapdev_common::kube::KubeAppCatalog as AppCatalog;


#[lapdev_hrpc::service]
pub trait HrpcService {
    async fn all_kube_clusters(&self, org_id: Uuid) -> Result<Vec<KubeCluster>, HrpcError>;

    async fn create_kube_cluster(
        &self,
        org_id: Uuid,
        name: String,
    ) -> Result<CreateKubeClusterResponse, HrpcError>;

    async fn delete_kube_cluster(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
    ) -> Result<(), HrpcError>;

    async fn set_cluster_deployable(
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

    async fn get_namespaces(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
    ) -> Result<Vec<KubeNamespace>, HrpcError>;

    async fn get_cluster_info(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
    ) -> Result<KubeClusterInfo, HrpcError>;

    async fn create_app_catalog(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
        name: String,
        description: Option<String>,
        workloads: Vec<KubeAppCatalogWorkloadCreate>,
    ) -> Result<Uuid, HrpcError>;

    async fn all_app_catalogs(&self, org_id: Uuid, search: Option<String>, pagination: Option<PagePaginationParams>) -> Result<PaginatedResult<KubeAppCatalog>, HrpcError>;

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

    async fn delete_app_catalog(
        &self,
        org_id: Uuid,
        catalog_id: Uuid,
    ) -> Result<(), HrpcError>;

    async fn delete_app_catalog_workload(
        &self,
        org_id: Uuid,
        workload_id: Uuid,
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
        namespace: String,
    ) -> Result<(), HrpcError>;

    async fn all_kube_environments(&self, org_id: Uuid, search: Option<String>, pagination: Option<PagePaginationParams>) -> Result<PaginatedResult<KubeEnvironment>, HrpcError>;
}
