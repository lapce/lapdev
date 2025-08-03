use lapdev_common::kube::{
    KubeClusterInfo, KubeNamespace, KubeWorkloadKind, KubeWorkloadList, PaginationParams,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeWorkloadWithServices {
    pub workload_yaml: String,
    pub services: Vec<String>, // YAML strings of matching services
    pub configmaps: Vec<String>, // YAML strings of referenced ConfigMaps
    pub secrets: Vec<String>, // YAML strings of referenced Secrets
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadIdentifier {
    pub name: String,
    pub namespace: String,
    pub kind: KubeWorkloadKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KubeWorkloadYaml {
    Deployment(KubeWorkloadWithServices),
    StatefulSet(KubeWorkloadWithServices),
    DaemonSet(KubeWorkloadWithServices),
    ReplicaSet(KubeWorkloadWithServices),
    Pod(KubeWorkloadWithServices),
    Job(KubeWorkloadWithServices),
    CronJob(KubeWorkloadWithServices),
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
    async fn get_workloads_yaml(
        workloads: Vec<WorkloadIdentifier>,
    ) -> Result<Vec<KubeWorkloadYaml>, String>;
    async fn deploy_workload_yaml(
        namespace: String,
        workload_yaml: KubeWorkloadYaml,
        labels: std::collections::HashMap<String, String>,
    ) -> Result<(), String>;
}
