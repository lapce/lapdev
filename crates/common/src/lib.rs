pub mod config;
pub mod console;
pub mod devbox;
pub mod devcontainer;
pub mod hrpc;
pub mod kube;
pub mod token;
pub mod utils;

use std::collections::HashMap;

use chrono::{DateTime, FixedOffset};
use devcontainer::PortAttribute;
use serde::{Deserialize, Serialize};
use strum_macros::EnumString;
use uuid::Uuid;

pub const LAPDEV_DEFAULT_OSUSER: &str = "lapdev";
pub const LAPDEV_BASE_HOSTNAME: &str = "lapdev-base-hostname";
pub const LAPDEV_ISOLATE_CONTAINER: &str = "lapdev-isolate-container";

#[derive(Serialize, Deserialize, Debug)]
pub struct NewProject {
    pub repo: String,
    pub machine_type_id: Uuid,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct NewProjectPrebuild {
    pub branch: String,
}

#[derive(
    Serialize, Deserialize, Debug, EnumString, strum_macros::Display, Clone, Eq, PartialEq, Hash,
)]
pub enum PrebuildStatus {
    Building,
    Ready,
    Failed,
}

#[derive(
    Serialize, Deserialize, Debug, EnumString, strum_macros::Display, Clone, Eq, PartialEq, Hash,
)]
pub enum PrebuildReplicaStatus {
    Transferring,
    Ready,
    Failed,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct NewProjectResponse {
    pub id: Uuid,
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GitBranch {
    pub name: String,
    pub commit: String,
    pub summary: String,
    pub time: DateTime<FixedOffset>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProjectPrebuild {
    pub id: Uuid,
    pub project_id: Uuid,
    pub created_at: DateTime<FixedOffset>,
    pub branch: String,
    pub commit: String,
    pub status: PrebuildStatus,
}

#[derive(Serialize, Deserialize, Debug, Clone, Hash, Eq, PartialEq)]
pub struct ProjectInfo {
    pub id: Uuid,
    pub name: String,
    pub repo_url: String,
    pub repo_name: String,
    pub machine_type: Uuid,
    pub created_at: DateTime<FixedOffset>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct MachineType {
    pub id: Uuid,
    pub name: String,
    pub shared: bool,
    pub cpu: usize,
    pub memory: usize,
    pub disk: usize,
    pub cost_per_second: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum CpuCore {
    Shared(usize),
    Dedicated(Vec<usize>),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CreateMachineType {
    pub name: String,
    pub shared: bool,
    pub cpu: usize,
    pub memory: usize,
    pub disk: usize,
    pub cost_per_second: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UpdateMachineType {
    pub name: String,
    pub cost_per_second: usize,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum RepoSource {
    Url(String),
    Project(Uuid),
}

#[derive(Serialize, Deserialize, Debug)]
pub struct NewWorkspace {
    pub source: RepoSource,
    pub branch: Option<String>,
    pub machine_type_id: Uuid,
    pub from_hash: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct NewWorkspaceResponse {
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum RepoContentPosition {
    Initial,
    Append,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct RepoContent {
    pub position: RepoContentPosition,
    pub content: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RepobuildError {
    pub msg: String,
    pub stderr: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RepoBuildResult {
    pub error: Option<RepobuildError>,
    pub output: RepoBuildOutput,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum RepoBuildOutput {
    Compose {
        services: Vec<RepoComposeService>,
        ports_attributes: HashMap<String, PortAttribute>,
    },
    Image {
        image: String,
        info: ContainerImageInfo,
        ports_attributes: HashMap<String, PortAttribute>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RepoComposeService {
    pub name: String,
    pub image: String,
    pub env: Vec<String>,
    pub info: ContainerImageInfo,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum BuildTarget {
    Workspace { id: Uuid, name: String },
    Prebuild { id: Uuid, project: Uuid },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RepoBuildInfo {
    pub target: BuildTarget,
    pub osuser: String,
    pub repo_name: String,
    pub repo_url: String,
    pub head: String,
    pub branch: String,
    pub auth: (String, String),
    pub env: Vec<(String, String)>,
    // the cpu cores that this build will use
    pub cpus: CpuCore,
    // the memory limit that this build will use
    pub memory: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PrebuildImage {
    pub osuser: String,
    pub tag: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PrebuildInfo {
    pub id: Uuid,
    pub osuser: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct NewOrganization {
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UpdateOrganizationAutoStartStop {
    pub auto_start: bool,
    pub auto_stop: Option<i32>,
    pub allow_workspace_change_auto_start: bool,
    pub allow_workspace_change_auto_stop: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UpdateOrganizationName {
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UpdateOrganizationMember {
    pub role: UserRole,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct NewSshKey {
    pub name: String,
    pub key: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SshKey {
    pub id: Uuid,
    pub name: String,
    pub key: String,
    pub created_at: DateTime<FixedOffset>,
}

#[derive(Debug, Deserialize)]
pub struct ProviderUser {
    pub id: i32,
    pub login: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub avatar_url: Option<String>,
}

#[derive(
    Serialize, Deserialize, EnumString, strum_macros::Display, Clone, Debug, PartialEq, Eq, Hash,
)]
pub enum UserRole {
    Owner,
    Admin,
    Member,
}

#[derive(
    Serialize, Deserialize, Debug, EnumString, strum_macros::Display, Clone, Eq, PartialEq, Hash,
)]
pub enum WorkspaceHostStatus {
    New,
    Active,
    Inactive,
    VersionMismatch,
}

#[derive(
    Hash,
    EnumString,
    strum_macros::Display,
    PartialEq,
    Eq,
    Debug,
    Deserialize,
    Serialize,
    Clone,
    Copy,
)]
pub enum AuthProvider {
    Github,
    Gitlab,
}

#[derive(
    Serialize,
    Deserialize,
    Debug,
    EnumString,
    strum_macros::Display,
    Clone,
    Copy,
    Eq,
    PartialEq,
    Hash,
)]
pub enum WorkspaceStatus {
    New,
    PrebuildBuilding,
    PrebuildCopying,
    Building,
    Running,
    Starting,
    Stopping,
    Stopped,
    StopFailed,
    Failed,
    DeleteFailed,
    Deleting,
    Deleted,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct WorkspaceInfo {
    pub name: String,
    pub repo_url: String,
    pub repo_name: String,
    pub branch: String,
    pub commit: String,
    pub status: WorkspaceStatus,
    pub machine_type: Uuid,
    pub services: Vec<WorkspaceService>,
    pub created_at: DateTime<FixedOffset>,
    pub hostname: String,
    pub build_error: Option<RepobuildError>,
    pub pinned: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Hash, Eq, PartialEq)]
pub struct WorkspacePort {
    pub port: u16,
    pub shared: bool,
    pub public: bool,
    pub label: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Hash, Eq, PartialEq)]
pub struct UpdateWorkspacePort {
    pub shared: bool,
    pub public: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Hash, Eq, PartialEq)]
pub struct WorkspaceService {
    pub name: String,
    pub service: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HostWorkspace {
    pub id: Uuid,
    pub name: String,
    pub osuser: String,
    pub ssh_port: Option<i32>,
    pub ide_port: Option<i32>,
    pub last_inactivity: Option<DateTime<FixedOffset>>,
    pub created_at: DateTime<FixedOffset>,
    pub updated_at: Option<DateTime<FixedOffset>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum PrebuildUpdateEvent {
    Status(PrebuildStatus),
    Stdout(String),
    Stderr(String),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum WorkspaceUpdateEvent {
    Status(WorkspaceStatus),
    Stdout(String),
    Stderr(String),
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CreateWorkspaceRequest {
    pub id: Uuid,
    pub workspace_name: String,
    pub volume_name: String,
    pub create_network: bool,
    pub network_name: String,
    pub service: Option<String>,
    pub osuser: String,
    pub image: String,
    pub ssh_public_key: String,
    pub repo_name: String,
    pub env: Vec<String>,
    pub cpus: CpuCore,
    pub memory: usize,
    pub disk: usize,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct StartWorkspaceRequest {
    pub osuser: String,
    pub workspace_name: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ProjectRequest {
    pub id: Uuid,
    pub osuser: String,
    pub repo_url: String,
    pub auth: (String, String),
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DeleteWorkspaceRequest {
    pub osuser: String,
    pub workspace_name: String,
    pub network: Option<String>,
    pub images: Vec<String>,
    pub keep_content: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct StopWorkspaceRequest {
    pub osuser: String,
    pub workspace_name: String,
}

#[derive(EnumString, strum_macros::Display, Clone, Eq, PartialEq)]
pub enum UsageResourceKind {
    Workspace,
    Prebuild,
}

#[derive(EnumString, strum_macros::Display, Clone, Eq, PartialEq)]
pub enum AuditResourceKind {
    Organization,
    User,
    Workspace,
    Project,
    Prebuild,
}

#[derive(EnumString, strum_macros::Display, Clone, Eq, PartialEq)]
pub enum AuditAction {
    OrganizationCreate,
    OrganizationDelete,
    OrganizationUpdate,
    OrganizationUpdateName,
    OrganizationUpdateQuota,
    OrganizationJoin,
    OrganizationDeleteMember,
    OrganizationUpdateMember,
    UserCreate,
    WorkspaceCreate,
    WorkspaceDelete,
    WorkspaceStart,
    WorkspaceStop,
    WorkspaceRebuild,
    WorkspaceUpdate,
    ProjectCreate,
    ProjectDelete,
    ProjectUpdateEnv,
    ProjectUpdateMachineType,
    PrebuildCreate,
    PrebuildDelete,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ContainerVolume {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Mountpoint")]
    pub mountpoint: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct NewContainer {
    #[serde(rename = "Hostname")]
    pub hostname: String,
    #[serde(rename = "User")]
    pub user: String,
    #[serde(rename = "Image")]
    pub image: String,
    #[serde(rename = "Entrypoint")]
    pub entrypoint: Vec<String>,
    #[serde(rename = "Env")]
    pub env: Vec<String>,
    #[serde(rename = "ExposedPorts")]
    pub exposed_ports: HashMap<String, HashMap<String, String>>,
    #[serde(rename = "WorkingDir")]
    pub working_dir: String,
    #[serde(rename = "HostConfig")]
    pub host_config: NewContainerHostConfig,
    #[serde(rename = "NetworkingConfig")]
    pub networking_config: NewContainerNetworkingConfig,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct NewContainerHostConfig {
    #[serde(rename = "PublishAllPorts")]
    pub publish_all_ports: bool,
    #[serde(rename = "Binds")]
    pub binds: Vec<String>,
    #[serde(rename = "CpuPeriod")]
    pub cpu_period: Option<i64>,
    #[serde(rename = "CpuQuota")]
    pub cpu_quota: Option<i64>,
    #[serde(rename = "CpusetCpus")]
    pub cpuset_cpus: Option<String>,
    #[serde(rename = "Memory")]
    pub memory: usize,
    #[serde(rename = "NetworkMode")]
    pub network_mode: String,
    #[serde(rename = "CapAdd")]
    pub cap_add: Vec<String>,
    #[serde(rename = "SecurityOpt")]
    pub security_opt: Option<Vec<String>>,
    #[serde(rename = "StorageOpt")]
    pub storage_opt: Option<HashMap<String, String>>,
    #[serde(rename = "Privileged")]
    pub privileged: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct NewContainerNetworkingConfig {
    #[serde(rename = "EndpointsConfig")]
    pub endpoints_config: HashMap<String, NewContainerEndpointSettings>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct NewContainerEndpointSettings {
    #[serde(rename = "Aliases")]
    pub aliases: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct NewContainerNetwork {
    #[serde(rename = "Name")]
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Container {
    #[serde(rename = "Id")]
    pub id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ContainerPortBinding {
    #[serde(rename = "HostIp")]
    pub host_ip: String,
    #[serde(rename = "HostPort")]
    pub host_port: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ContainerConfig {
    #[serde(rename = "Hostname")]
    pub hostname: String,
    #[serde(rename = "WorkingDir")]
    pub working_dir: String,
    #[serde(rename = "Env")]
    pub env: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct ContainerImageConfig {
    #[serde(rename = "Hostname")]
    pub hostname: String,
    #[serde(rename = "WorkingDir")]
    pub working_dir: String,
    #[serde(rename = "Entrypoint")]
    pub entrypoint: Option<Vec<String>>,
    #[serde(rename = "Cmd")]
    pub cmd: Option<Vec<String>>,
    #[serde(rename = "Env")]
    pub env: Option<Vec<String>>,
    #[serde(rename = "ExposedPorts")]
    pub exposed_ports: Option<HashMap<String, HashMap<String, String>>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ContainerHostConfig {
    #[serde(rename = "PortBindings")]
    pub port_bindings: HashMap<String, Vec<ContainerPortBinding>>,
    #[serde(rename = "Binds")]
    pub binds: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ContainerInfo {
    #[serde(rename = "Config")]
    pub config: ContainerConfig,
    #[serde(rename = "HostConfig")]
    pub host_config: ContainerHostConfig,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct ContainerImageInfo {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Config")]
    pub config: ContainerImageConfig,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Hash, EnumString, strum_macros::Display)]
pub enum QuotaLevel {
    User,
    Organization,
}

#[derive(
    Serialize,
    Deserialize,
    Debug,
    EnumString,
    strum_macros::Display,
    Clone,
    Eq,
    PartialEq,
    Hash,
    Copy,
)]
pub enum QuotaKind {
    Workspace,
    RunningWorkspace,
    Project,
    DailyCost,
    MonthlyCost,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct OrgQuota {
    pub workspace: OrgQuotaValue,
    pub running_workspace: OrgQuotaValue,
    pub project: OrgQuotaValue,
    pub daily_cost: OrgQuotaValue,
    pub monthly_cost: OrgQuotaValue,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct OrgQuotaValue {
    pub existing: usize,
    pub org_quota: usize,
    pub default_user_quota: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UpdateOrgQuota {
    pub kind: QuotaKind,
    pub default_user_quota: usize,
    pub org_quota: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QuotaResult {
    pub kind: QuotaKind,
    pub level: QuotaLevel,
    pub existing: usize,
    pub quota: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QuotaValue {
    pub kind: QuotaKind,
    pub user: usize,
    pub organization: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnterpriseLicense {
    pub users: usize,
    pub expires_at: DateTime<FixedOffset>,
    pub hostname: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct NewLicenseKey {
    pub key: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct NewLicense {
    pub secret: String,
    pub expires_at: DateTime<FixedOffset>,
    pub users: usize,
    pub hostname: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct NewWorkspaceHost {
    pub host: String,
    pub region: String,
    pub zone: String,
    pub cpu: usize,
    pub memory: usize,
    pub disk: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UpdateWorkspaceHost {
    pub region: String,
    pub zone: String,
    pub cpu: usize,
    pub memory: usize,
    pub disk: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct WorkspaceHost {
    pub id: Uuid,
    pub host: String,
    pub port: i32,
    pub status: WorkspaceHostStatus,
    pub cpu: i32,
    pub memory: i32,
    pub disk: i32,
    pub available_dedicated_cpu: i32,
    pub available_shared_cpu: i32,
    pub available_memory: i32,
    pub available_disk: i32,
    pub region: String,
    pub zone: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OauthSettings {
    pub github_client_id: String,
    pub github_client_secret: String,
    pub gitlab_client_id: String,
    pub gitlab_client_secret: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UsageRequest {
    pub page: u64,
    pub page_size: u64,
    pub start: DateTime<FixedOffset>,
    pub end: DateTime<FixedOffset>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UsageResult {
    pub total_items: u64,
    pub num_pages: u64,
    pub page: u64,
    pub page_size: u64,
    pub total_cost: usize,
    pub records: Vec<UsageRecord>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UsageRecord {
    pub id: i32,
    pub start: DateTime<FixedOffset>,
    pub resource_kind: String,
    pub resource_name: String,
    pub machine_type: String,
    pub duration: usize,
    pub cost: usize,
    pub user: String,
    pub avatar: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AuditLogRequest {
    pub page: u64,
    pub page_size: u64,
    pub start: DateTime<FixedOffset>,
    pub end: DateTime<FixedOffset>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AuditLogResult {
    pub total_items: u64,
    pub num_pages: u64,
    pub page: u64,
    pub page_size: u64,
    pub records: Vec<AuditLogRecord>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AuditLogRecord {
    pub id: i32,
    pub time: DateTime<FixedOffset>,
    pub user: String,
    pub avatar: String,
    pub resource_kind: String,
    pub resource_name: String,
    pub action: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ClusterInfo {
    pub auth_providers: Vec<AuthProvider>,
    pub machine_types: Vec<MachineType>,
    pub hostnames: HashMap<String, String>,
    pub has_enterprise: bool,
    pub ssh_proxy_port: u16,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ClusterUserResult {
    pub total_items: u64,
    pub num_pages: u64,
    pub page: u64,
    pub page_size: u64,
    pub users: Vec<ClusterUser>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ClusterUser {
    pub id: Uuid,
    pub created_at: DateTime<FixedOffset>,
    pub auth_provider: AuthProvider,
    pub avatar_url: Option<String>,
    pub name: Option<String>,
    pub email: Option<String>,
    pub cluster_admin: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UpdateClusterUser {
    pub cluster_admin: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct GitProvider {
    pub auth_provider: AuthProvider,
    pub avatar_url: Option<String>,
    pub name: Option<String>,
    pub email: Option<String>,
    pub connected: bool,
    pub read_repo: Option<bool>,
    pub scopes: Vec<String>,
    pub all_scopes: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct UserCreationWebhook {
    pub id: Uuid,
}
