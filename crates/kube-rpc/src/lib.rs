use lapdev_common::kube::{
    KubeAppCatalogWorkload, KubeClusterInfo, KubeNamespaceInfo, KubeServiceWithYaml,
    KubeWorkloadDetails, KubeWorkloadKind, KubeWorkloadList, PaginationParams,
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

    // Preview URL tunnel methods
    async fn establish_data_plane_tunnel(
        controller_endpoint: String,
        auth_token: String,
    ) -> Result<TunnelEstablishmentResponse, String>;

    async fn get_tunnel_status() -> Result<TunnelStatus, String>;

    async fn close_tunnel_connection(tunnel_id: String) -> Result<(), String>;
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
