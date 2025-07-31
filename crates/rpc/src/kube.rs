use lapdev_common::kube::{KubeClusterInfo, KubeWorkloadList, PaginationParams};

#[tarpc::service]
pub trait KubeClusterRpc {
    async fn report_cluster_info(cluster_info: KubeClusterInfo) -> Result<(), String>;
}

#[tarpc::service]
pub trait KubeManagerRpc {
    async fn get_workloads(
        namespace: Option<String>,
        pagination: Option<PaginationParams>,
    ) -> Result<KubeWorkloadList, String>;
    async fn get_workload_details(
        name: String,
        namespace: String,
    ) -> Result<Option<lapdev_common::kube::KubeWorkload>, String>;
}
