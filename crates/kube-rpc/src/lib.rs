use lapdev_common::kube::{
    KubeClusterInfo, KubeNamespace, KubeWorkloadKind, KubeWorkloadList, PaginationParams,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeWorkloadWithServices {
    pub workload_yaml: String,
    pub services: Vec<String>,   // Names of matching services
    pub configmaps: Vec<String>, // Names of referenced ConfigMaps
    pub secrets: Vec<String>,    // Names of referenced Secrets
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KubeWorkloadYamlOnly {
    Deployment(String),
    StatefulSet(String),
    DaemonSet(String),
    ReplicaSet(String),
    Pod(String),
    Job(String),
    CronJob(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeWorkloadsWithResources {
    pub workloads: Vec<KubeWorkloadYamlOnly>,
    pub services: HashMap<String, String>, // name -> YAML content
    pub configmaps: HashMap<String, String>, // name -> YAML content
    pub secrets: HashMap<String, String>,  // name -> YAML content
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

    async fn get_workloads_yaml(
        workloads: Vec<WorkloadIdentifier>,
    ) -> Result<KubeWorkloadsWithResources, String>;

    async fn deploy_workload_yaml(
        namespace: String,
        workloads_with_resources: KubeWorkloadsWithResources,
        labels: std::collections::HashMap<String, String>,
    ) -> Result<(), String>;
}
