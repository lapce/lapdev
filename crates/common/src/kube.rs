use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const KUBE_CLUSTER_TOKEN_HEADER: &str = "X-Cluster-Token";
pub const KUBE_CLUSTER_TOKEN_ENV_VAR: &str = "LAPDEV_KUBE_CLUSTER_TOKEN";
pub const KUBE_CLUSTER_URL_ENV_VAR: &str = "LAPDEV_KUBE_CLUSTER_URL";
pub const KUBE_CLUSTER_TUNNEL_URL_ENV_VAR: &str = "LAPDEV_KUBE_CLUSTER_TUNNEL_URL";
pub const DEFAULT_KUBE_CLUSTER_URL: &str = "wss://ws.lap.dev/api/v1/kube/cluster/rpc";
pub const DEFAULT_KUBE_CLUSTER_TUNNEL_URL: &str = "wss://ws.lap.dev/api/v1/kube/cluster/tunnel";

pub const SIDECAR_PROXY_MANAGER_ADDR_ENV_VAR: &str = "LAPDEV_SIDECAR_PROXY_MANAGER_ADDR";
pub const SIDECAR_PROXY_MANAGER_PORT_ENV_VAR: &str = "LAPDEV_SIDECAR_PROXY_MANAGER_PORT";
pub const DEFAULT_SIDECAR_PROXY_MANAGER_PORT: u16 = 5001;
pub const SIDECAR_PROXY_PORT_ENV_VAR: &str = "LAPDEV_SIDECAR_PROXY_PORT";
pub const DEFAULT_SIDECAR_PROXY_PORT: u16 = 15001;
pub const SIDECAR_PROXY_WORKLOAD_ENV_VAR: &str = "LAPDEV_SIDECAR_PROXY_WORKLOAD";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeCluster {
    pub id: Uuid,
    pub name: String,
    pub can_deploy_personal: bool,
    pub can_deploy_shared: bool,
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

#[derive(
    Debug, Clone, Serialize, Deserialize, strum_macros::EnumString, strum_macros::Display, PartialEq,
)]
pub enum KubeEnvironmentSyncStatus {
    Idle,
    Syncing,
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

#[derive(
    Debug, Clone, Serialize, Deserialize, PartialEq, strum_macros::EnumString, strum_macros::Display,
)]
pub enum PreviewUrlAccessLevel {
    Personal, // Only accessible by the owner with authentication
    Shared,   // Accessible by organization members with authentication
    Public,   // Accessible by anyone without authentication
}

impl PreviewUrlAccessLevel {
    pub fn get_detailed_message(&self) -> &'static str {
        match self {
            PreviewUrlAccessLevel::Personal => "Personal - Only you can access",
            PreviewUrlAccessLevel::Shared => "Shared - Organization members can access",
            PreviewUrlAccessLevel::Public => "Public - Anyone can access without authentication",
        }
    }
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
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub is_shared: bool,
    pub created_at: DateTime<FixedOffset>,
    pub created_by: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeNamespaceInfo {
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
    pub ports: Vec<KubeServicePort>,
    pub workload_yaml: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeAppCatalogWorkloadCreate {
    pub name: String,
    pub namespace: String,
    pub kind: KubeWorkloadKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeEnvironmentWorkload {
    pub id: Uuid,
    pub created_at: DateTime<FixedOffset>,
    pub environment_id: Uuid,
    pub name: String,
    pub namespace: String,
    pub kind: String,
    pub containers: Vec<KubeContainerInfo>,
    pub ports: Vec<KubeServicePort>,
}

impl KubeEnvironmentWorkload {
    /// Returns true if any containers in this workload have been customized
    pub fn has_customized_containers(&self) -> bool {
        self.containers.iter().any(|c| c.is_customized())
    }
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
pub struct KubeContainerPort {
    pub name: Option<String>,
    pub container_port: i32,
    pub protocol: Option<String>, // TCP, UDP, SCTP
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
    pub original_env_vars: Vec<KubeEnvVar>,
    pub ports: Vec<KubeContainerPort>,
}

impl KubeContainerInfo {
    /// Returns true if this container has been customized from its original configuration
    pub fn is_customized(&self) -> bool {
        // Check if image is customized
        matches!(self.image, KubeContainerImage::Custom(_))
            // Check if environment variables are customized (non-empty indicates customization)
            || !self.env_vars.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeWorkloadDetails {
    pub name: String,
    pub namespace: String,
    pub kind: KubeWorkloadKind,
    pub containers: Vec<KubeContainerInfo>,
    pub ports: Vec<KubeServicePort>,
    pub workload_yaml: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeEnvironment {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub namespace: String,
    pub app_catalog_id: Uuid,
    pub app_catalog_name: String,
    pub cluster_id: Uuid,
    pub cluster_name: String,
    pub status: String,
    pub created_at: String,
    pub is_shared: bool,
    pub base_environment_id: Option<Uuid>,
    pub base_environment_name: Option<String>,
    pub catalog_sync_version: i64,
    pub last_catalog_synced_at: Option<String>,
    pub catalog_update_available: bool,
    pub sync_status: KubeEnvironmentSyncStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeServicePort {
    pub name: Option<String>,
    pub port: i32,
    pub target_port: Option<i32>,
    pub protocol: Option<String>,
    pub node_port: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeServiceDetails {
    pub name: String,
    pub ports: Vec<KubeServicePort>,
    pub selector: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeServiceWithYaml {
    pub yaml: String,
    pub details: KubeServiceDetails,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeEnvironmentService {
    pub id: Uuid,
    pub created_at: DateTime<FixedOffset>,
    pub environment_id: Uuid,
    pub name: String,
    pub namespace: String,
    pub yaml: String,
    pub ports: Vec<KubeServicePort>,
    pub selector: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeEnvironmentPreviewUrl {
    pub id: Uuid,
    pub created_at: DateTime<FixedOffset>,
    pub environment_id: Uuid,
    pub service_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub port: i32,
    pub port_name: Option<String>,
    pub protocol: String,
    pub access_level: PreviewUrlAccessLevel,
    pub created_by: Uuid,
    pub last_accessed_at: Option<DateTime<FixedOffset>>,
    pub url: String, // Full preview URL
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateKubeEnvironmentPreviewUrlRequest {
    pub description: Option<String>,
    pub service_id: Uuid,
    pub port: i32,
    pub port_name: Option<String>,
    pub protocol: Option<String>,                    // Default to "HTTP"
    pub access_level: Option<PreviewUrlAccessLevel>, // Default to Personal
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateKubeEnvironmentPreviewUrlRequest {
    pub description: Option<String>,
    pub access_level: Option<PreviewUrlAccessLevel>,
}
