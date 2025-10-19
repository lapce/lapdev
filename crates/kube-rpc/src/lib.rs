use chrono::{DateTime, Utc};
use lapdev_common::kube::{
    KubeClusterInfo, KubeNamespaceInfo, KubeServiceWithYaml, KubeWorkloadKind, KubeWorkloadList,
    PaginationParams,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

// HTTP parsing utilities
pub mod http_parser;

// TCP Tunneling Protocol Structures

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelEstablishmentResponse {
    pub success: bool,
    pub tunnel_id: String,
    pub websocket_endpoint: String,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelStatus {
    pub tunnel_id: Option<String>,
    pub is_connected: bool,
    pub connected_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_heartbeat: Option<chrono::DateTime<chrono::Utc>>,
    pub active_connections: u32,
    pub total_connections: u64,
    pub bytes_transferred: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelInfo {
    pub tunnel_id: String,
    pub websocket_endpoint: String,
    pub supported_protocols: Vec<String>,
    pub max_concurrent_connections: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ResourceType {
    Deployment,
    StatefulSet,
    DaemonSet,
    ReplicaSet,
    Job,
    CronJob,
    ConfigMap,
    Secret,
    Service,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ResourceChangeType {
    Created,
    Updated,
    Deleted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceChangeEvent {
    pub namespace: String,
    pub resource_type: ResourceType,
    pub resource_name: String,
    pub change_type: ResourceChangeType,
    pub resource_version: String,
    pub resource_yaml: Option<String>,
    pub timestamp: DateTime<Utc>,
}

// Messages sent FROM KubeManager TO Server (Client -> Server)
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum ClientTunnelMessage {
    // Connection responses (KubeManager responds to server requests)
    ConnectionOpened {
        tunnel_id: String,
        local_addr: String,
    },
    ConnectionFailed {
        tunnel_id: String,
        error: String,
        error_code: TunnelErrorCode,
    },
    ConnectionClosed {
        tunnel_id: String,
        bytes_transferred: u64,
    },

    // Connection lifecycle (KubeManager can also initiate close)
    CloseConnection {
        tunnel_id: String,
        reason: CloseReason,
    },

    // Data transfer (bidirectional)
    Data {
        tunnel_id: String,
        payload: Vec<u8>,
        sequence_num: Option<u32>,
    },

    // Control messages
    Pong {
        timestamp: u64,
    },

    // Tunnel management
    TunnelStats {
        active_connections: u32,
        total_connections: u64,
        bytes_sent: u64,
        bytes_received: u64,
        connection_errors: u64,
    },

    // Authentication
    Authenticate {
        cluster_id: Uuid,
        auth_token: String,
        tunnel_capabilities: Vec<String>,
    },
}

// Messages sent FROM Server TO KubeManager (Server -> Client)
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum ServerTunnelMessage {
    // Connection lifecycle (server requests connections)
    OpenConnection {
        tunnel_id: String,
        target_host: String,
        target_port: u16,
        protocol_hint: Option<String>,
    },
    CloseConnection {
        tunnel_id: String,
        reason: CloseReason,
    },

    // Data transfer (bidirectional)
    Data {
        tunnel_id: String,
        payload: Vec<u8>,
        sequence_num: Option<u32>,
    },

    // Control messages
    Ping {
        timestamp: u64,
    },

    // Authentication response
    AuthenticationResult {
        success: bool,
        session_id: Option<String>,
        error_message: Option<String>,
    },
}

// Legacy enum for backward compatibility during transition
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum TunnelMessage {
    // Connection lifecycle
    OpenConnection {
        tunnel_id: String,
        target_host: String,
        target_port: u16,
        protocol_hint: Option<String>,
    },
    ConnectionOpened {
        tunnel_id: String,
        local_addr: String,
    },
    ConnectionFailed {
        tunnel_id: String,
        error: String,
        error_code: TunnelErrorCode,
    },
    CloseConnection {
        tunnel_id: String,
        reason: CloseReason,
    },
    ConnectionClosed {
        tunnel_id: String,
        bytes_transferred: u64,
    },

    // Data transfer
    Data {
        tunnel_id: String,
        payload: Vec<u8>,
        sequence_num: Option<u32>,
    },

    // Control messages
    Ping {
        timestamp: u64,
    },
    Pong {
        timestamp: u64,
    },

    // Tunnel management
    TunnelStats {
        active_connections: u32,
        total_connections: u64,
        bytes_sent: u64,
        bytes_received: u64,
        connection_errors: u64,
    },

    // Authentication
    Authenticate {
        cluster_id: Uuid,
        auth_token: String,
        tunnel_capabilities: Vec<String>,
    },
    AuthenticationResult {
        success: bool,
        session_id: Option<String>,
        error_message: Option<String>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum TunnelErrorCode {
    ConnectionRefused,
    Timeout,
    NetworkUnreachable,
    PermissionDenied,
    InternalError,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum CloseReason {
    ClientRequest,
    ServerRequest,
    Timeout,
    Error,
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientTunnelFrame {
    pub message: ClientTunnelMessage,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub message_id: Uuid,
}

impl ClientTunnelFrame {
    pub fn serialize(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    pub fn deserialize(data: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(data)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerTunnelFrame {
    pub message: ServerTunnelMessage,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub message_id: Uuid,
}

impl ServerTunnelFrame {
    pub fn serialize(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    pub fn deserialize(data: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(data)
    }
}

// Legacy frame for backward compatibility during transition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelFrame {
    pub message: TunnelMessage,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub message_id: Uuid,
}

impl TunnelFrame {
    pub fn serialize(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    pub fn deserialize(data: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(data)
    }
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamespacedResourceRequest {
    pub namespace: String,
    pub configmaps: Vec<String>,
    pub secrets: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamespacedResourceResponse {
    pub namespace: String,
    pub configmaps: HashMap<String, String>,
    pub secrets: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KubeRawWorkloadYaml {
    pub name: String,
    pub namespace: String,
    pub kind: KubeWorkloadKind,
    pub workload_yaml: String,
}

#[tarpc::service]
pub trait KubeClusterRpc {
    async fn report_cluster_info(cluster_info: KubeClusterInfo) -> Result<(), String>;

    // Tunnel lifecycle management
    async fn tunnel_heartbeat() -> Result<(), String>;

    async fn report_tunnel_metrics(
        active_connections: u32,
        bytes_transferred: u64,
        connection_count: u64,
        connection_errors: u64,
    ) -> Result<(), String>;

    async fn report_resource_change(event: ResourceChangeEvent) -> Result<(), String>;
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

    async fn deploy_workload_yaml(
        environment_id: uuid::Uuid,
        environment_auth_token: String,
        namespace: String,
        workloads_with_resources: KubeWorkloadsWithResources,
        labels: std::collections::HashMap<String, String>,
    ) -> Result<(), String>;

    async fn get_workloads_raw_yaml(
        workloads: Vec<WorkloadIdentifier>,
    ) -> Result<Vec<KubeRawWorkloadYaml>, String>;

    async fn get_namespaced_resources(
        requests: Vec<NamespacedResourceRequest>,
    ) -> Result<Vec<NamespacedResourceResponse>, String>;

    async fn refresh_branch_service_routes(environment_id: Uuid) -> Result<(), String>;

    async fn destroy_environment(environment_id: Uuid, namespace: String) -> Result<(), String>;

    // Preview URL tunnel methods
    async fn get_tunnel_status() -> Result<TunnelStatus, String>;

    async fn close_tunnel_connection(tunnel_id: String) -> Result<(), String>;

    // Devbox-proxy management methods
    async fn add_branch_environment(
        base_environment_id: Uuid,
        branch: BranchEnvironmentInfo,
    ) -> Result<(), String>;

    async fn remove_branch_environment(
        base_environment_id: Uuid,
        branch_environment_id: Uuid,
    ) -> Result<(), String>;
}

#[tarpc::service]
pub trait SidecarProxyManagerRpc {
    async fn register_sidecar_proxy(
        workload_id: Uuid,
        environment_id: Uuid,
        namespace: String,
    ) -> Result<(), String>;

    async fn heartbeat() -> Result<(), String>;

    async fn report_routing_metrics(
        request_count: u64,
        byte_count: u64,
        active_connections: u32,
    ) -> Result<(), String>;

    /// Request a devbox tunnel connection for service interception
    /// Returns tunnel info for establishing direct WebSocket connection to API
    async fn request_devbox_tunnel(
        intercept_id: Uuid,
        source_addr: String,
        target_port: u16,
    ) -> Result<DevboxTunnelInfo, String>;
}

/// Information returned when requesting a devbox tunnel
/// Sidecar uses this to establish direct WebSocket connection to API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevboxTunnelInfo {
    /// Unique tunnel identifier
    pub tunnel_id: String,
    /// WebSocket URL to connect to (e.g., "wss://api.lapdev.io/kube/devbox/tunnel/{tunnel_id}")
    pub websocket_url: String,
    /// Authentication token for the tunnel
    pub auth_token: String,
    /// Session ID for tracking
    pub session_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevboxRouteConfig {
    pub intercept_id: Uuid,
    pub session_id: Uuid,
    pub target_port: u16,
    pub auth_token: String,
    /// Path pattern for this route (e.g., "/*" for all traffic)
    pub path_pattern: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyRouteConfig {
    pub path: String,
    pub service_name: String,
    pub namespace: String,
    pub port: u16,
}

#[tarpc::service]
pub trait SidecarProxyRpc {
    async fn heartbeat() -> Result<(), String>;

    /// Replace the service routes with the provided configuration
    async fn set_service_routes(routes: Vec<ProxyRouteConfig>) -> Result<(), String>;

    /// Add a DevboxTunnel route for service interception
    /// Returns true if route was added successfully
    async fn add_devbox_route(route: DevboxRouteConfig) -> Result<bool, String>;

    /// Remove a DevboxTunnel route by intercept_id
    /// Returns true if route was found and removed
    async fn remove_devbox_route(intercept_id: Uuid) -> Result<bool, String>;

    /// List all active DevboxTunnel routes
    async fn list_devbox_routes() -> Result<Vec<DevboxRouteConfig>, String>;
}

/// Information about a branch environment that shares the devbox-proxy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchEnvironmentInfo {
    pub environment_id: Uuid,
    pub auth_token: String,
    pub namespace: String,
}

#[tarpc::service]
pub trait DevboxProxyRpc {
    /// Heartbeat to keep the RPC connection alive
    async fn heartbeat() -> Result<(), String>;

    /// Add a new branch environment to the devbox-proxy (shared environments only)
    /// Called when a branch environment is created
    async fn add_branch_environment(branch: BranchEnvironmentInfo) -> Result<(), String>;

    /// Remove a branch environment from the devbox-proxy (shared environments only)
    /// Called when a branch environment is deleted
    async fn remove_branch_environment(environment_id: Uuid) -> Result<(), String>;

    /// Get list of all configured branch environments (shared environments only)
    async fn list_branch_environments() -> Result<Vec<BranchEnvironmentInfo>, String>;

    /// Start a tunnel
    /// - Personal environment: pass None to start the base tunnel
    /// - Shared environment: pass Some(branch_environment_id) to start a branch tunnel
    async fn start_tunnel(environment_id: Option<Uuid>) -> Result<(), String>;

    /// Stop a tunnel
    /// - Personal environment: pass None to stop the base tunnel
    /// - Shared environment: pass Some(branch_environment_id) to stop a branch tunnel
    async fn stop_tunnel(environment_id: Option<Uuid>) -> Result<(), String>;

    /// Get tunnel status
    /// - Personal environment: pass None to check base tunnel status
    /// - Shared environment: pass Some(branch_environment_id) to check branch tunnel status
    async fn get_tunnel_status(environment_id: Option<Uuid>) -> Result<bool, String>;
}

#[tarpc::service]
pub trait DevboxProxyManagerRpc {
    /// Register a devbox-proxy with the manager
    /// Called when devbox-proxy starts up
    async fn register_devbox_proxy(environment_id: Uuid) -> Result<(), String>;

    /// Heartbeat from devbox-proxy to manager
    async fn heartbeat(environment_id: Uuid) -> Result<(), String>;
}
