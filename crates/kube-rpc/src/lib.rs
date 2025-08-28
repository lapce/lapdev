use lapdev_common::kube::{
    KubeAppCatalogWorkload, KubeClusterInfo, KubeNamespace, KubeNamespaceInfo, KubeServiceWithYaml,
    KubeWorkloadDetails, KubeWorkloadKind, KubeWorkloadList, PaginationParams,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

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
    pub services: HashMap<String, KubeServiceWithYaml>, // name -> service with YAML and details
    pub configmaps: HashMap<String, String>,            // name -> YAML content
    pub secrets: HashMap<String, String>,               // name -> YAML content
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

    async fn get_namespaces() -> Result<Vec<KubeNamespaceInfo>, String>;

    async fn get_workloads_yaml(
        workloads: Vec<KubeAppCatalogWorkload>,
    ) -> Result<KubeWorkloadsWithResources, String>;

    async fn get_workload_yaml(
        workload: KubeAppCatalogWorkload,
    ) -> Result<KubeWorkloadYaml, String>;

    async fn deploy_workload_yaml(
        environment_id: uuid::Uuid,
        namespace: String,
        workloads_with_resources: KubeWorkloadsWithResources,
        labels: std::collections::HashMap<String, String>,
    ) -> Result<(), String>;

    async fn get_workloads_details(
        workloads: Vec<WorkloadIdentifier>,
    ) -> Result<Vec<KubeWorkloadDetails>, String>;

    async fn update_workload_containers(
        environment_id: Uuid,
        workload_id: Uuid,
        name: String,
        namespace: String,
        kind: KubeWorkloadKind,
        containers: Vec<lapdev_common::kube::KubeContainerInfo>,
        labels: std::collections::HashMap<String, String>,
    ) -> Result<(), String>;

    async fn create_branch_workload(
        base_workload_id: uuid::Uuid,
        base_workload_name: String,
        branch_environment_id: uuid::Uuid,
        namespace: String,
        kind: KubeWorkloadKind,
        containers: Vec<lapdev_common::kube::KubeContainerInfo>,
        labels: std::collections::HashMap<String, String>,
    ) -> Result<(), String>;
}

#[tarpc::service]
pub trait SidecarProxyManagerRpc {
    async fn register_sidecar_proxy(workload_id: Uuid) -> Result<(), String>;

    async fn heartbeat() -> Result<(), String>;

    async fn report_routing_metrics(
        request_count: u64,
        byte_count: u64,
        active_connections: u32,
    ) -> Result<(), String>;
}

#[tarpc::service]
pub trait SidecarProxyRpc {
    async fn heartbeat() -> Result<(), String>;
}
