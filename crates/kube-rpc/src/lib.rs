use k8s_openapi::api::{
    apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet},
    batch::v1::{CronJob, Job},
    core::v1::Pod,
};
use lapdev_common::kube::{
    KubeClusterInfo, KubeNamespace, KubeWorkloadKind, KubeWorkloadList, PaginationParams,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KubeWorkloadObject {
    Deployment(Deployment),
    StatefulSet(StatefulSet),
    DaemonSet(DaemonSet),
    ReplicaSet(ReplicaSet),
    Pod(Pod),
    Job(Job),
    CronJob(CronJob),
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
    async fn get_workload_object(
        name: String,
        namespace: String,
        kind: KubeWorkloadKind,
    ) -> Result<KubeWorkloadObject, String>;
    async fn deploy_workload_object(
        namespace: String,
        workload_object: KubeWorkloadObject,
        labels: std::collections::HashMap<String, String>,
    ) -> Result<(), String>;
}
