use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const KUBE_CLUSTER_TOKEN_HEADER: &str = "X-Cluster-Token";
pub const KUBE_CLUSTER_TOKEN_ENV_VAR: &str = "LAPDEV_KUBE_CLUSTER_TOKEN";
pub const KUBE_CLUSTER_URL_ENV_VAR: &str = "LAPDEV_KUBE_CLUSTER_URL";
pub const DEFAULT_KUBE_CLUSTER_URL: &str = "wss://ws.lap.dev/api/v1/kube/cluster/ws";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeCluster {
    pub id: Uuid,
    pub name: String,
    pub can_deploy: bool,
    pub info: KubeClusterInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeClusterInfo {
    pub cluster_name: Option<String>,
    pub cluster_version: String,
    pub node_count: u32,
    pub available_cpu: String,
    pub available_memory: String,
    pub provider: Option<String>,
    pub region: Option<String>,
    pub status: KubeClusterStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, strum_macros::EnumString, strum_macros::Display)]
pub enum KubeClusterStatus {
    Ready,
    #[strum(to_string = "Not Ready")]
    NotReady,
    Provisioning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeWorkload {
    pub name: String,
    pub namespace: String,
    pub kind: KubeWorkloadKind,
    pub replicas: Option<i32>,
    pub ready_replicas: Option<i32>,
    pub status: KubeWorkloadStatus,
    pub created_at: Option<String>,
    pub labels: std::collections::HashMap<String, String>,
}

#[derive(
    PartialEq, Debug, Clone, Serialize, Deserialize, strum_macros::Display, strum_macros::EnumString,
)]
pub enum KubeWorkloadKind {
    Deployment,
    StatefulSet,
    DaemonSet,
    ReplicaSet,
    Pod,
    Job,
    CronJob,
}

#[derive(
    Debug, Clone, Serialize, Deserialize, PartialEq, strum_macros::EnumString, strum_macros::Display,
)]
pub enum KubeWorkloadStatus {
    Running,
    Pending,
    Failed,
    Succeeded,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeWorkloadList {
    pub workloads: Vec<KubeWorkload>,
    pub next_cursor: Option<PaginationCursor>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaginationCursor {
    pub workload_kind: KubeWorkloadKind,
    pub continue_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginationParams {
    pub cursor: Option<PaginationCursor>,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PagePaginationParams {
    pub page: usize,
    pub page_size: usize,
}

impl Default for PagePaginationParams {
    fn default() -> Self {
        Self {
            page: 1,
            page_size: 20,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedResult<T> {
    pub data: Vec<T>,
    pub pagination_info: PaginatedInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedInfo {
    pub total_count: usize,
    pub page: usize,
    pub page_size: usize,
    pub total_pages: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateKubeClusterResponse {
    pub cluster_id: Uuid,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeNamespace {
    pub name: String,
    pub status: String,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeAppCatalog {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime<FixedOffset>,
    pub created_by: Uuid,
    pub cluster_id: Uuid,
    pub cluster_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeAppCatalogWorkload {
    pub id: Uuid,
    pub name: String,
    pub namespace: String,
    pub kind: KubeWorkloadKind,
    pub containers: Vec<KubeContainerInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeAppCatalogWorkloadCreate {
    pub name: String,
    pub namespace: String,
    pub kind: KubeWorkloadKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeEnvVar {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KubeContainerImage {
    /// Follow the image from the original workload (tracks updates)
    FollowOriginal,
    /// Use a custom specified image
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeContainerInfo {
    pub name: String,
    pub original_image: String,
    pub image: KubeContainerImage,
    pub cpu_request: Option<String>,
    pub cpu_limit: Option<String>,
    pub memory_request: Option<String>,
    pub memory_limit: Option<String>,
    pub env_vars: Vec<KubeEnvVar>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeWorkloadDetails {
    pub name: String,
    pub namespace: String,
    pub kind: KubeWorkloadKind,
    pub containers: Vec<KubeContainerInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeEnvironment {
    pub id: Uuid,
    pub name: String,
    pub namespace: String,
    pub app_catalog_id: Uuid,
    pub app_catalog_name: String,
    pub status: Option<String>,
    pub created_at: String,
    pub created_by: Uuid,
}
