use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevboxSessionSummary {
    pub id: Uuid,
    pub device_name: String,
    pub token_prefix: String,
    pub active_environment_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub last_used_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevboxSessionListResponse {
    pub sessions: Vec<DevboxSessionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevboxSessionWhoAmI {
    pub user_id: Uuid,
    pub email: Option<String>,
    pub device_name: String,
    pub authenticated_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevboxEnvironmentSelection {
    pub environment_id: Uuid,
    pub cluster_name: String,
    pub namespace: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevboxPortMapping {
    pub workload_port: u16,
    pub local_port: u16,
    pub protocol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevboxPortMappingOverride {
    pub workload_port: u16,
    pub local_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevboxWorkloadInterceptSummary {
    pub intercept_id: Uuid,
    pub session_id: Uuid,
    pub workload_id: Uuid,
    pub workload_name: String,
    pub namespace: String,
    pub port_mappings: Vec<DevboxPortMapping>,
    pub created_at: DateTime<Utc>,
    pub restored_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevboxWorkloadInterceptListResponse {
    pub intercepts: Vec<DevboxWorkloadInterceptSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevboxStartWorkloadInterceptResponse {
    pub intercept_id: Uuid,
}
