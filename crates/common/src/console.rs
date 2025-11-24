use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::UserRole;

#[derive(Serialize, Deserialize)]
pub struct NewSessionResponse {
    pub url: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Organization {
    pub id: Uuid,
    pub name: String,
    pub role: UserRole,
    pub auto_start: bool,
    pub auto_stop: Option<i32>,
    pub allow_workspace_change_auto_start: bool,
    pub allow_workspace_change_auto_stop: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MeUser {
    pub avatar_url: Option<String>,
    pub email: Option<String>,
    pub name: Option<String>,
    pub cluster_admin: bool,
    pub organization: Organization,
    pub all_organizations: Vec<Organization>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct OrganizationMember {
    pub user_id: Uuid,
    pub avatar_url: Option<String>,
    pub name: Option<String>,
    pub role: UserRole,
    pub joined: DateTime<FixedOffset>,
}
