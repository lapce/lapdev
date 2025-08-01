use lapdev_common::kube::{
    CreateKubeClusterResponse, K8sProvider, KubeClusterInfo, KubeNamespace, KubeWorkload,
    KubeWorkloadKind, KubeWorkloadList, PaginationParams,
};
use uuid::Uuid;

pub use lapdev_common::hrpc::HrpcError;
pub use lapdev_common::kube::{KubeAppCatalog, KubeEnvironment};
// For backward compatibility
pub use lapdev_common::kube::KubeAppCatalog as AppCatalog;


#[lapdev_hrpc::service]
pub trait HrpcService {
    async fn all_k8s_providers(&self, org_id: Uuid) -> Result<Vec<K8sProvider>, HrpcError>;

    async fn create_k8s_gcp_provider(
        &self,
        org_id: Uuid,
        name: String,
        key: String,
    ) -> Result<(), HrpcError>;

    async fn all_kube_clusters(&self, org_id: Uuid) -> Result<Vec<KubeClusterInfo>, HrpcError>;

    async fn create_kube_cluster(
        &self,
        org_id: Uuid,
        name: String,
    ) -> Result<CreateKubeClusterResponse, HrpcError>;

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
        resources: String,
    ) -> Result<(), HrpcError>;

    async fn all_app_catalogs(&self, org_id: Uuid) -> Result<Vec<KubeAppCatalog>, HrpcError>;

    async fn all_kube_environments(&self, org_id: Uuid) -> Result<Vec<KubeEnvironment>, HrpcError>;
}
