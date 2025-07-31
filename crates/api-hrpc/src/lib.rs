use lapdev_common::kube::{
    CreateKubeClusterResponse, K8sProvider, KubeClusterInfo, KubeWorkload, KubeWorkloadList,
    PaginationParams,
};
use uuid::Uuid;

pub use lapdev_common::hrpc::HrpcError;

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
        pagination: Option<PaginationParams>,
    ) -> Result<KubeWorkloadList, HrpcError>;

    async fn get_workload_details(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
        name: String,
        namespace: String,
    ) -> Result<Option<KubeWorkload>, HrpcError>;
}
