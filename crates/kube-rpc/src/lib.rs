use lapdev_common::kube::{
    KubeClusterInfo, KubeNamespace, KubeWorkloadKind, KubeWorkloadList, PaginationParams,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KubeWorkloadYaml {
    Deployment(String),
    StatefulSet(String),
    DaemonSet(String),
    ReplicaSet(String),
    Pod(String),
    Job(String),
    CronJob(String),
}

#[tarpc::service]
pub trait KubeClusterRpc {
    async fn report_cluster_info(cluster_info: KubeClusterInfo) -> Result<(), String>;
}

#[tarpc::service]
pub trait KubeManagerRpc {
    async fn get_workloads(
        namespace: Option<String>,
        workload_kind_filter: Option<KubeWorkloadKind>,
        include_system_workloads: bool,
        pagination: Option<PaginationParams>,
    ) -> Result<KubeWorkloadList, String>;
    async fn get_workload_details(
        name: String,
        namespace: String,
    ) -> Result<Option<lapdev_common::kube::KubeWorkload>, String>;
    async fn get_namespaces() -> Result<Vec<KubeNamespace>, String>;
    async fn get_workload_yaml(
        name: String,
        namespace: String,
        kind: KubeWorkloadKind,
    ) -> Result<KubeWorkloadYaml, String>;
    async fn deploy_workload_yaml(
        namespace: String,
        workload_yaml: KubeWorkloadYaml,
        labels: std::collections::HashMap<String, String>,
    ) -> Result<(), String>;
}
