use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Devbox session RPC - Server to CLI
// These are methods that the CLI can call on the server
#[tarpc::service]
pub trait DevboxSessionRpc {
    async fn whoami() -> Result<DevboxSessionInfo, String>;
    async fn get_active_environment() -> Result<Option<DevboxEnvironmentInfo>, String>;
    async fn set_active_environment(environment_id: Uuid) -> Result<(), String>;
    async fn heartbeat() -> Result<(), String>;
    async fn list_workload_intercepts(
        environment_id: Uuid,
    ) -> Result<Vec<WorkloadInterceptInfo>, String>;
    async fn list_services(environment_id: Uuid) -> Result<Vec<ServiceInfo>, String>;
    async fn update_device_name(device_name: String) -> Result<(), String>;
}

// Devbox client RPC - CLI to Server
// These are methods that the server calls on the CLI
#[tarpc::service]
pub trait DevboxClientRpc {
    async fn session_displaced(new_device_name: String);
    async fn environment_changed(environment: Option<DevboxEnvironmentInfo>);
    async fn ping() -> Result<(), String>;
}

// Devbox intercept control RPC - Dashboard/CLI to Server
#[tarpc::service]
pub trait DevboxInterceptRpc {
    async fn start_workload_intercept(
        workload_id: Uuid,
        port_mappings: Vec<PortMappingOverride>,
    ) -> Result<Uuid, String>;

    async fn stop_workload_intercept(intercept_id: Uuid) -> Result<(), String>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevboxSessionInfo {
    pub session_id: Uuid,
    pub user_id: Uuid,
    pub email: String,
    pub device_name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub last_used_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevboxEnvironmentInfo {
    pub environment_id: Uuid,
    pub cluster_name: String,
    pub namespace: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadInterceptInfo {
    pub intercept_id: Uuid,
    pub workload_id: Uuid,
    pub workload_name: String,
    pub namespace: String,
    pub port_mappings: Vec<PortMapping>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub device_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortMapping {
    pub workload_port: u16,
    pub local_port: u16,
    #[serde(default = "default_protocol")]
    pub protocol: String,
}

fn default_protocol() -> String {
    "TCP".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortMappingOverride {
    pub workload_port: u16,
    pub local_port: Option<u16>, // None means use same port as workload
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceInfo {
    pub name: String,
    pub namespace: String,
    pub ports: Vec<ServicePort>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServicePort {
    pub name: Option<String>,
    pub port: u16,
    #[serde(default = "default_protocol")]
    pub protocol: String,
}
