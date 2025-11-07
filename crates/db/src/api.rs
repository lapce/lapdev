use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::{DateTime, FixedOffset, Utc};
use lapdev_common::{
    config::LAPDEV_CLUSTER_NOT_INITIATED,
    kube::{
        AppCatalogStatusEvent, ClusterStatusEvent, EnvironmentWorkloadStatusEvent,
        KubeAppCatalogWorkload, KubeClusterStatus, KubeContainerInfo, KubeEnvironment,
        KubeEnvironmentDashboardSummary, KubeEnvironmentStatus, KubeEnvironmentStatusCount,
        KubeEnvironmentSyncStatus, KubeEnvironmentWorkload, KubeServicePort, KubeWorkloadDetails,
        KubeWorkloadKind, PagePaginationParams,
    },
    AuthProvider, ProviderUser, UserRole, WorkspaceStatus, LAPDEV_BASE_HOSTNAME,
    LAPDEV_ISOLATE_CONTAINER,
};
use lapdev_db_entities::{
    kube_app_catalog_workload, kube_app_catalog_workload_dependency,
    kube_app_catalog_workload_label, kube_cluster_service, kube_cluster_service_selector,
    kube_environment_workload_label,
};
use lapdev_db_migration::Migrator;
use pasetors::{
    keys::{Generate, SymmetricKey},
    version4::V4,
};
use sea_orm::{
    prelude::{DateTimeWithTimeZone, Json},
    sea_query::{Alias, Condition, Expr, Func, OnConflict},
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseConnection,
    DatabaseTransaction, EntityTrait, FromQueryResult, JoinType, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect, RelationTrait, TransactionTrait,
};
use sea_orm_migration::MigratorTrait;
use serde::Deserialize;
use serde_json;
use serde_yaml::Value;
use sqlx::PgPool;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::convert::TryFrom;
use std::str::FromStr;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct NewEnvironmentWorkload {
    pub id: Uuid,
    pub details: KubeWorkloadDetails,
}

// Custom result structure for multi-table join
#[derive(FromQueryResult)]
struct KubeEnvironmentWithRelated {
    // Environment fields
    pub env_id: Uuid,
    pub env_name: String,
    pub env_namespace: String,
    pub env_app_catalog_id: Uuid,
    pub env_cluster_id: Uuid,
    pub env_status: String,
    pub env_created_at: DateTimeWithTimeZone,
    pub env_is_shared: bool,
    pub env_organization_id: Uuid,
    pub env_user_id: Uuid,
    pub env_deleted_at: Option<DateTimeWithTimeZone>,
    pub env_base_environment_id: Option<Uuid>,
    pub env_auth_token: String,
    pub env_catalog_sync_version: i64,
    pub env_last_catalog_synced_at: Option<DateTimeWithTimeZone>,
    pub env_paused_at: Option<DateTimeWithTimeZone>,
    pub env_resumed_at: Option<DateTimeWithTimeZone>,
    pub env_sync_status: String,

    // App catalog fields
    pub catalog_name: Option<String>,
    pub catalog_description: Option<String>,
    pub catalog_sync_version: Option<i64>,
    pub catalog_last_synced_at: Option<DateTimeWithTimeZone>,
    pub catalog_last_sync_actor_id: Option<Uuid>,

    // Cluster fields
    pub cluster_name: Option<String>,

    // Base environment fields
    pub base_environment_name: Option<String>,
}

#[derive(FromQueryResult)]
struct StatusCountRow {
    pub status: String,
    pub count: i64,
}

pub const LAPDEV_PIN_UNPIN_ERROR: &str = "lapdev-pin-unpin-error";
pub const LAPDEV_MAX_CPU_ERROR: &str = "lapdev-max-cpu-error";
const LAPDEV_API_AUTH_TOKEN_KEY: &str = "lapdev-api-auth-token-key";
const LAPDEV_DEFAULT_USAGE_LIMIT: &str = "lapdev-default-org-usage-limit";
const LAPDEV_DEFAULT_RUNNING_WORKSPACE_LIMIT: &str = "lapdev-default-org-running-workspace-limit";
const LAPDEV_USER_CREATION_WEBHOOK: &str = "lapdev-user-creation-webhook";

#[derive(Clone)]
pub struct DbApi {
    pub conn: DatabaseConnection,
    pub pool: Option<PgPool>,
}

#[derive(Clone, Debug)]
pub struct CachedClusterService {
    pub name: String,
    pub selector: BTreeMap<String, String>,
    pub ports: Vec<KubeServicePort>,
    pub service_yaml: String,
}

#[derive(Debug, Deserialize)]
struct StoredServicePort {
    pub name: Option<String>,
    pub port: i32,
    #[serde(default)]
    pub target_port: Option<serde_json::Value>,
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub node_port: Option<i32>,
    #[serde(default)]
    pub original_target_port: Option<i32>,
}

impl StoredServicePort {
    fn into_service_port(self) -> Option<KubeServicePort> {
        let target_port = self
            .target_port
            .and_then(|value| value.as_i64())
            .and_then(|v| i32::try_from(v).ok());
        let original_target_port = self.original_target_port.or(target_port);

        Some(KubeServicePort {
            name: self.name,
            port: self.port,
            target_port,
            protocol: self.protocol,
            node_port: self.node_port,
            original_target_port,
        })
    }
}

async fn connect_db(conn_url: &str) -> Result<sqlx::PgPool> {
    let redacted_url = redact_connection_url(conn_url);
    info!(%redacted_url, "Connecting to PostgreSQL database");
    let pool: sqlx::PgPool = sqlx::pool::PoolOptions::new()
        .max_connections(100)
        .connect(conn_url)
        .await?;
    Ok(pool)
}

fn redact_connection_url(conn_url: &str) -> String {
    let (scheme, remainder) = conn_url.split_once("://").unwrap_or(("postgres", conn_url));

    let target_host = remainder
        .split_once('@')
        .map(|(_, after)| after)
        .unwrap_or(remainder);

    let mut redacted = String::from(scheme);
    redacted.push_str("://");

    if target_host.is_empty() {
        redacted.push_str("<unknown>");
        return redacted;
    }

    let mut segments = target_host.splitn(2, '/');
    let host_port = segments.next().unwrap_or("");

    if host_port.is_empty() {
        redacted.push_str("<unknown>");
        return redacted;
    }

    redacted.push_str(host_port);

    if let Some(path_and_query) = segments.next() {
        if let Some(database) = path_and_query.split('?').next() {
            if !database.is_empty() {
                redacted.push('/');
                redacted.push_str(database);
            }
        }
    }

    redacted
}

impl DbApi {
    pub async fn new(conn_url: &str) -> Result<Self> {
        let pool = connect_db(conn_url).await?;
        let conn = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(pool.clone());
        let db = DbApi {
            conn,
            pool: Some(pool),
        };
        Ok(db)
    }

    pub async fn migrate(&self) -> Result<()> {
        Migrator::up(&self.conn, None).await?;
        Ok(())
    }

    pub async fn is_cluster_initiated(&self, txn: &DatabaseTransaction) -> bool {
        // before the cluster is initiated, the auth providers can be configured
        // without any authentication,
        // also the user created before cluster is intiated is a cluster admin,
        // so the value is default to true even when we've got an error
        if lapdev_db_entities::config::Entity::find()
            .filter(lapdev_db_entities::config::Column::Name.eq(LAPDEV_CLUSTER_NOT_INITIATED))
            .one(txn)
            .await
            .as_ref()
            .map(|v| v.as_ref().map(|v| v.value.as_str()))
            == Ok(Some("yes"))
        {
            return false;
        }
        true
    }

    async fn generate_api_auth_token_key(&self) -> SymmetricKey<V4> {
        let key = SymmetricKey::generate().unwrap();
        {
            let key = STANDARD.encode(key.as_bytes());
            let _ = lapdev_db_entities::config::ActiveModel {
                name: ActiveValue::Set(LAPDEV_API_AUTH_TOKEN_KEY.to_string()),
                value: ActiveValue::Set(key),
            }
            .insert(&self.conn)
            .await;
        }

        key
    }

    pub async fn load_api_auth_token_key(&self) -> SymmetricKey<V4> {
        if let Ok(key) = self.get_api_auth_token_key().await {
            return key;
        }

        self.generate_api_auth_token_key().await
    }

    async fn get_api_auth_token_key(&self) -> Result<SymmetricKey<V4>> {
        let key = self.get_config(LAPDEV_API_AUTH_TOKEN_KEY).await?;
        let key = STANDARD.decode(key)?;
        let key = SymmetricKey::from(&key)?;
        Ok(key)
    }

    pub async fn update_config(
        &self,
        name: &str,
        value: &str,
    ) -> Result<lapdev_db_entities::config::Model> {
        let model =
            lapdev_db_entities::config::Entity::insert(lapdev_db_entities::config::ActiveModel {
                name: ActiveValue::set(name.to_string()),
                value: ActiveValue::set(value.to_string()),
            })
            .on_conflict(
                OnConflict::column(lapdev_db_entities::config::Column::Name)
                    .update_column(lapdev_db_entities::config::Column::Value)
                    .to_owned(),
            )
            .exec_with_returning(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_config(&self, name: &str) -> Result<String> {
        let model = lapdev_db_entities::config::Entity::find()
            .filter(lapdev_db_entities::config::Column::Name.eq(name))
            .one(&self.conn)
            .await?
            .ok_or_else(|| anyhow!("no config found"))?;
        Ok(model.value)
    }

    async fn get_config_in_txn(&self, txn: &DatabaseTransaction, name: &str) -> Result<String> {
        let model = lapdev_db_entities::config::Entity::find()
            .filter(lapdev_db_entities::config::Column::Name.eq(name))
            .one(txn)
            .await?
            .ok_or_else(|| anyhow!("no config found"))?;
        Ok(model.value)
    }

    pub async fn get_base_hostname(&self) -> Result<String> {
        self.get_config(LAPDEV_BASE_HOSTNAME).await
    }

    pub async fn get_user_creation_webhook(&self) -> Result<String> {
        self.get_config(LAPDEV_USER_CREATION_WEBHOOK).await
    }

    pub async fn is_container_isolated(&self) -> Result<bool> {
        Ok(lapdev_db_entities::config::Entity::find()
            .filter(lapdev_db_entities::config::Column::Name.eq(LAPDEV_ISOLATE_CONTAINER))
            .one(&self.conn)
            .await?
            .map(|v| v.value == "yes")
            .unwrap_or(false))
    }

    pub async fn get_all_workspaces(
        &self,
        user_id: Uuid,
        org_id: Uuid,
    ) -> Result<
        Vec<(
            lapdev_db_entities::workspace::Model,
            Option<lapdev_db_entities::workspace_host::Model>,
        )>,
    > {
        let model = lapdev_db_entities::workspace::Entity::find()
            .find_also_related(lapdev_db_entities::workspace_host::Entity)
            .filter(lapdev_db_entities::workspace::Column::UserId.eq(user_id))
            .filter(lapdev_db_entities::workspace::Column::OrganizationId.eq(org_id))
            .filter(lapdev_db_entities::workspace::Column::DeletedAt.is_null())
            .order_by_asc(lapdev_db_entities::workspace::Column::CreatedAt)
            .all(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_workspace(&self, id: Uuid) -> Result<lapdev_db_entities::workspace::Model> {
        let model = lapdev_db_entities::workspace::Entity::find_by_id(id)
            .filter(lapdev_db_entities::workspace::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?
            .ok_or_else(|| anyhow!("no workspace found"))?;
        Ok(model)
    }

    pub async fn get_workspace_by_name(
        &self,
        name: &str,
    ) -> Result<lapdev_db_entities::workspace::Model> {
        let model = lapdev_db_entities::workspace::Entity::find()
            .filter(lapdev_db_entities::workspace::Column::Name.eq(name))
            .filter(lapdev_db_entities::workspace::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?
            .ok_or_else(|| anyhow!("no workspace found"))?;
        Ok(model)
    }

    pub async fn get_workspace_by_url(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        url: &str,
    ) -> Result<Option<lapdev_db_entities::workspace::Model>> {
        let model = lapdev_db_entities::workspace::Entity::find()
            .filter(lapdev_db_entities::workspace::Column::OrganizationId.eq(org_id))
            .filter(lapdev_db_entities::workspace::Column::UserId.eq(user_id))
            .filter(lapdev_db_entities::workspace::Column::DeletedAt.is_null())
            .filter(lapdev_db_entities::workspace::Column::RepoUrl.eq(url))
            .one(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_running_workspaces_on_host(
        &self,
        ws_host_id: Uuid,
    ) -> Result<Vec<lapdev_db_entities::workspace::Model>> {
        let model = lapdev_db_entities::workspace::Entity::find()
            .filter(lapdev_db_entities::workspace::Column::HostId.eq(ws_host_id))
            .filter(lapdev_db_entities::workspace::Column::DeletedAt.is_null())
            .filter(
                lapdev_db_entities::workspace::Column::Status
                    .eq(WorkspaceStatus::Running.to_string()),
            )
            .all(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_inactive_workspaces_on_host(
        &self,
        ws_host_id: Uuid,
        last_updated_at: DateTime<FixedOffset>,
    ) -> Result<Vec<lapdev_db_entities::workspace::Model>> {
        let model = lapdev_db_entities::workspace::Entity::find()
            .filter(lapdev_db_entities::workspace::Column::HostId.eq(ws_host_id))
            .filter(lapdev_db_entities::workspace::Column::DeletedAt.is_null())
            .filter(
                lapdev_db_entities::workspace::Column::Status
                    .eq(WorkspaceStatus::Stopped.to_string()),
            )
            .filter(lapdev_db_entities::workspace::Column::Pinned.eq(false))
            .filter(lapdev_db_entities::workspace::Column::UpdatedAt.lt(last_updated_at))
            .all(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_org_running_workspace(
        &self,
        org_id: Uuid,
    ) -> Result<Option<lapdev_db_entities::workspace::Model>> {
        let model = lapdev_db_entities::workspace::Entity::find()
            .filter(lapdev_db_entities::workspace::Column::OrganizationId.eq(org_id))
            .filter(lapdev_db_entities::workspace::Column::DeletedAt.is_null())
            .filter(
                lapdev_db_entities::workspace::Column::Status
                    .eq(WorkspaceStatus::Running.to_string()),
            )
            .one(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_org_running_workspaces(
        &self,
        org_id: Uuid,
    ) -> Result<Vec<lapdev_db_entities::workspace::Model>> {
        let model = lapdev_db_entities::workspace::Entity::find()
            .filter(lapdev_db_entities::workspace::Column::OrganizationId.eq(org_id))
            .filter(
                lapdev_db_entities::workspace::Column::Status
                    .eq(WorkspaceStatus::Running.to_string()),
            )
            .filter(lapdev_db_entities::workspace::Column::DeletedAt.is_null())
            .all(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn validate_ssh_public_key(
        &self,
        user_id: Uuid,
        public_key: &str,
    ) -> Result<lapdev_db_entities::ssh_public_key::Model> {
        let model = lapdev_db_entities::ssh_public_key::Entity::find()
            .filter(lapdev_db_entities::ssh_public_key::Column::UserId.eq(user_id))
            .filter(lapdev_db_entities::ssh_public_key::Column::ParsedKey.eq(public_key))
            .filter(lapdev_db_entities::ssh_public_key::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?
            .ok_or_else(|| anyhow!("no workspace found"))?;
        Ok(model)
    }

    pub async fn get_organization(
        &self,
        id: Uuid,
    ) -> Result<lapdev_db_entities::organization::Model> {
        let model = lapdev_db_entities::organization::Entity::find_by_id(id)
            .filter(lapdev_db_entities::organization::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?
            .ok_or_else(|| anyhow!("no organization found"))?;
        Ok(model)
    }

    pub async fn get_organization_member(
        &self,
        user_id: Uuid,
        org_id: Uuid,
    ) -> Result<lapdev_db_entities::organization_member::Model> {
        let model = lapdev_db_entities::organization_member::Entity::find()
            .filter(lapdev_db_entities::organization_member::Column::UserId.eq(user_id))
            .filter(lapdev_db_entities::organization_member::Column::OrganizationId.eq(org_id))
            .filter(lapdev_db_entities::organization_member::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?
            .ok_or_else(|| anyhow!("no organization member found"))?;
        Ok(model)
    }

    pub async fn get_all_organization_members(
        &self,
        org_id: Uuid,
    ) -> Result<Vec<lapdev_db_entities::organization_member::Model>> {
        let models = lapdev_db_entities::organization_member::Entity::find()
            .filter(lapdev_db_entities::organization_member::Column::OrganizationId.eq(org_id))
            .filter(lapdev_db_entities::organization_member::Column::DeletedAt.is_null())
            .all(&self.conn)
            .await?;
        Ok(models)
    }

    pub async fn create_new_organization(
        &self,
        txn: &DatabaseTransaction,
        name: String,
    ) -> Result<lapdev_db_entities::organization::Model> {
        let default_usage_limit = self
            .get_config_in_txn(txn, LAPDEV_DEFAULT_USAGE_LIMIT)
            .await
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        let default_running_workspace_limit = self
            .get_config_in_txn(txn, LAPDEV_DEFAULT_RUNNING_WORKSPACE_LIMIT)
            .await
            .ok()
            .and_then(|v| v.parse::<i32>().ok())
            .unwrap_or(0);

        let org = lapdev_db_entities::organization::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            deleted_at: ActiveValue::Set(None),
            name: ActiveValue::Set(name.to_string()),
            auto_start: ActiveValue::Set(true),
            allow_workspace_change_auto_start: ActiveValue::Set(true),
            auto_stop: ActiveValue::Set(Some(3600)),
            allow_workspace_change_auto_stop: ActiveValue::Set(true),
            last_auto_stop_check: ActiveValue::Set(None),
            usage_limit: ActiveValue::Set(default_usage_limit),
            running_workspace_limit: ActiveValue::Set(default_running_workspace_limit),
            has_running_workspace: ActiveValue::Set(false),
            max_cpu: ActiveValue::Set(4),
        }
        .insert(txn)
        .await?;

        Ok(org)
    }

    pub async fn create_new_user(
        &self,
        txn: &DatabaseTransaction,
        provider: &AuthProvider,
        provider_user: ProviderUser,
        token: String,
    ) -> Result<lapdev_db_entities::user::Model> {
        let cluster_admin = if !self.is_cluster_initiated(txn).await {
            lapdev_db_entities::config::ActiveModel {
                name: ActiveValue::Set(LAPDEV_CLUSTER_NOT_INITIATED.to_string()),
                value: ActiveValue::Set("no".to_string()),
            }
            .update(txn)
            .await?;
            true
        } else {
            false
        };

        let now = Utc::now();
        let org = self
            .create_new_organization(txn, "Personal".to_string())
            .await?;

        let user = lapdev_db_entities::user::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            created_at: ActiveValue::Set(now.into()),
            deleted_at: ActiveValue::Set(None),
            provider: ActiveValue::Set(provider.to_string()),
            osuser: ActiveValue::Set(format!("{provider}_{}", provider_user.login)),
            avatar_url: ActiveValue::Set(provider_user.avatar_url.clone()),
            email: ActiveValue::Set(provider_user.email.clone()),
            name: ActiveValue::Set(provider_user.name.clone()),
            current_organization: ActiveValue::Set(org.id),
            cluster_admin: ActiveValue::Set(cluster_admin),
            disabled: ActiveValue::Set(false),
        }
        .insert(txn)
        .await?;

        lapdev_db_entities::oauth_connection::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            user_id: ActiveValue::Set(user.id),
            created_at: ActiveValue::Set(now.into()),
            deleted_at: ActiveValue::Set(None),
            provider: ActiveValue::Set(provider.to_string()),
            provider_id: ActiveValue::Set(provider_user.id),
            provider_login: ActiveValue::Set(provider_user.login),
            access_token: ActiveValue::Set(token),
            avatar_url: ActiveValue::Set(provider_user.avatar_url),
            email: ActiveValue::Set(provider_user.email),
            name: ActiveValue::Set(provider_user.name),
            read_repo: ActiveValue::Set(Some(false)),
        }
        .insert(txn)
        .await?;

        lapdev_db_entities::organization_member::ActiveModel {
            created_at: ActiveValue::Set(now.into()),
            user_id: ActiveValue::Set(user.id),
            organization_id: ActiveValue::Set(org.id),
            role: ActiveValue::Set(UserRole::Owner.to_string()),
            ..Default::default()
        }
        .insert(txn)
        .await?;

        Ok(user)
    }

    pub async fn get_user(&self, user_id: Uuid) -> Result<Option<lapdev_db_entities::user::Model>> {
        let model = lapdev_db_entities::user::Entity::find()
            .filter(lapdev_db_entities::user::Column::Id.eq(user_id))
            .filter(lapdev_db_entities::user::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_oauth(
        &self,
        id: Uuid,
    ) -> Result<Option<lapdev_db_entities::oauth_connection::Model>> {
        let model = lapdev_db_entities::oauth_connection::Entity::find()
            .filter(lapdev_db_entities::oauth_connection::Column::Id.eq(id))
            .filter(lapdev_db_entities::oauth_connection::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_kube_token(
        &self,
        token: &[u8],
    ) -> Result<Option<lapdev_db_entities::kube_cluster_token::Model>> {
        let model = lapdev_db_entities::kube_cluster_token::Entity::find()
            .filter(lapdev_db_entities::kube_cluster_token::Column::Token.eq(token))
            .filter(lapdev_db_entities::kube_cluster_token::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn update_kube_token_last_used(
        &self,
        id: Uuid,
    ) -> Result<lapdev_db_entities::kube_cluster_token::Model> {
        let model = lapdev_db_entities::kube_cluster_token::ActiveModel {
            id: ActiveValue::Set(id),
            last_used_at: ActiveValue::Set(Some(Utc::now().into())),
            ..Default::default()
        }
        .update(&self.conn)
        .await?;
        Ok(model)
    }

    pub async fn get_kube_cluster(
        &self,
        id: Uuid,
    ) -> Result<Option<lapdev_db_entities::kube_cluster::Model>> {
        let model = lapdev_db_entities::kube_cluster::Entity::find()
            .filter(lapdev_db_entities::kube_cluster::Column::Id.eq(id))
            .filter(lapdev_db_entities::kube_cluster::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn update_kube_cluster_info(
        &self,
        id: Uuid,
        cluster_version: Option<String>,
        status: Option<String>,
        provider: Option<String>,
        region: Option<String>,
        manager_namespace: Option<String>,
    ) -> Result<lapdev_db_entities::kube_cluster::Model> {
        use lapdev_db_entities::kube_cluster;
        use sea_orm::ActiveValue;

        let now = chrono::Utc::now().into();
        let model = kube_cluster::ActiveModel {
            id: ActiveValue::Set(id),
            cluster_version: ActiveValue::Set(cluster_version),
            status: status.map(ActiveValue::Set).unwrap_or(ActiveValue::NotSet),
            provider: ActiveValue::Set(provider),
            region: ActiveValue::Set(region),
            manager_namespace: ActiveValue::Set(manager_namespace),
            last_reported_at: ActiveValue::Set(Some(now)),
            ..Default::default()
        };
        let updated = model.update(&self.conn).await?;
        self.publish_cluster_status_event(&updated).await;
        Ok(updated)
    }

    async fn publish_cluster_status_event(
        &self,
        cluster: &lapdev_db_entities::kube_cluster::Model,
    ) {
        let Some(pool) = self.pool.as_ref() else {
            return;
        };

        let status =
            KubeClusterStatus::from_str(&cluster.status).unwrap_or(KubeClusterStatus::NotReady);
        let updated_at = cluster
            .last_reported_at
            .map(|ts| ts.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);

        let event = ClusterStatusEvent {
            organization_id: cluster.organization_id,
            cluster_id: cluster.id,
            status,
            cluster_version: cluster.cluster_version.clone(),
            region: cluster.region.clone(),
            updated_at,
        };

        match serde_json::to_string(&event) {
            Ok(payload) => {
                if let Err(err) = sqlx::query("SELECT pg_notify('cluster_status', $1)")
                    .bind(payload)
                    .execute(pool)
                    .await
                {
                    warn!(
                        error = %err,
                        "failed to publish cluster status event via NOTIFY"
                    );
                }
            }
            Err(err) => {
                warn!(
                    error = %err,
                    "failed to serialize cluster status event"
                );
            }
        }
    }

    async fn publish_app_catalog_status_event(
        &self,
        catalog: &lapdev_db_entities::kube_app_catalog::Model,
    ) {
        let Some(pool) = self.pool.as_ref() else {
            return;
        };

        let event = AppCatalogStatusEvent {
            organization_id: catalog.organization_id,
            catalog_id: catalog.id,
            cluster_id: catalog.cluster_id,
            sync_version: catalog.sync_version,
            last_synced_at: catalog.last_synced_at.map(|ts| ts.to_string()),
            last_sync_actor_id: catalog.last_sync_actor_id,
            updated_at: Utc::now(),
        };

        match serde_json::to_string(&event) {
            Ok(payload) => {
                if let Err(err) = sqlx::query("SELECT pg_notify('app_catalog_status', $1)")
                    .bind(payload)
                    .execute(pool)
                    .await
                {
                    warn!(
                        error = %err,
                        "failed to publish app catalog status event via NOTIFY"
                    );
                }
            }
            Err(err) => {
                warn!(
                    error = %err,
                    "failed to serialize app catalog status event"
                );
            }
        }
    }

    pub async fn publish_environment_workload_status_event(
        &self,
        event: &EnvironmentWorkloadStatusEvent,
    ) {
        let Some(pool) = self.pool.as_ref() else {
            return;
        };

        match serde_json::to_string(event) {
            Ok(payload) => {
                if let Err(err) = sqlx::query("SELECT pg_notify('environment_workload_status', $1)")
                    .bind(payload)
                    .execute(pool)
                    .await
                {
                    warn!(
                        error = %err,
                        "failed to publish environment workload status event via NOTIFY"
                    );
                }
            }
            Err(err) => {
                warn!(
                    error = %err,
                    "failed to serialize environment workload status event"
                );
            }
        }
    }

    pub async fn get_user_all_oauth(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<lapdev_db_entities::oauth_connection::Model>> {
        let model = lapdev_db_entities::oauth_connection::Entity::find()
            .filter(lapdev_db_entities::oauth_connection::Column::UserId.eq(user_id))
            .filter(lapdev_db_entities::oauth_connection::Column::DeletedAt.is_null())
            .all(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_user_oauth(
        &self,
        user_id: Uuid,
        provider_name: &str,
    ) -> Result<Option<lapdev_db_entities::oauth_connection::Model>> {
        let model = lapdev_db_entities::oauth_connection::Entity::find()
            .filter(lapdev_db_entities::oauth_connection::Column::UserId.eq(user_id))
            .filter(lapdev_db_entities::oauth_connection::Column::Provider.eq(provider_name))
            .filter(lapdev_db_entities::oauth_connection::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_user_organizations(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<lapdev_db_entities::organization_member::Model>> {
        let model = lapdev_db_entities::organization_member::Entity::find()
            .filter(lapdev_db_entities::organization_member::Column::UserId.eq(user_id))
            .filter(lapdev_db_entities::organization_member::Column::DeletedAt.is_null())
            .order_by_asc(lapdev_db_entities::organization_member::Column::CreatedAt)
            .all(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_all_ssh_keys(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<lapdev_db_entities::ssh_public_key::Model>> {
        let model = lapdev_db_entities::ssh_public_key::Entity::find()
            .filter(lapdev_db_entities::ssh_public_key::Column::UserId.eq(user_id))
            .filter(lapdev_db_entities::ssh_public_key::Column::DeletedAt.is_null())
            .order_by_asc(lapdev_db_entities::ssh_public_key::Column::CreatedAt)
            .all(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_ssh_key(&self, id: Uuid) -> Result<lapdev_db_entities::ssh_public_key::Model> {
        let model = lapdev_db_entities::ssh_public_key::Entity::find_by_id(id)
            .filter(lapdev_db_entities::ssh_public_key::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?
            .ok_or_else(|| anyhow!("no ssh key found"))?;
        Ok(model)
    }

    pub async fn get_project(&self, id: Uuid) -> Result<lapdev_db_entities::project::Model> {
        let model = lapdev_db_entities::project::Entity::find_by_id(id)
            .filter(lapdev_db_entities::project::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?
            .ok_or_else(|| anyhow!("no project found"))?;
        Ok(model)
    }

    pub async fn get_project_by_repo(
        &self,
        org_id: Uuid,
        repo: &str,
    ) -> Result<Option<lapdev_db_entities::project::Model>> {
        let model = lapdev_db_entities::project::Entity::find()
            .filter(lapdev_db_entities::project::Column::OrganizationId.eq(org_id))
            .filter(lapdev_db_entities::project::Column::DeletedAt.is_null())
            .filter(
                Expr::expr(Func::lower(
                    lapdev_db_entities::project::Column::RepoUrl.into_expr(),
                ))
                .eq(repo.to_lowercase()),
            )
            .one(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_all_projects(
        &self,
        org_id: Uuid,
    ) -> Result<Vec<lapdev_db_entities::project::Model>> {
        let model = lapdev_db_entities::project::Entity::find()
            .filter(lapdev_db_entities::project::Column::OrganizationId.eq(org_id))
            .filter(lapdev_db_entities::project::Column::DeletedAt.is_null())
            .order_by_asc(lapdev_db_entities::project::Column::CreatedAt)
            .all(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_all_kube_clusters(
        &self,
        org_id: Uuid,
    ) -> Result<Vec<lapdev_db_entities::kube_cluster::Model>> {
        let model = lapdev_db_entities::kube_cluster::Entity::find()
            .filter(lapdev_db_entities::kube_cluster::Column::OrganizationId.eq(org_id))
            .filter(lapdev_db_entities::kube_cluster::Column::DeletedAt.is_null())
            .order_by_asc(lapdev_db_entities::kube_cluster::Column::CreatedAt)
            .all(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_prebuild(
        &self,
        id: Uuid,
    ) -> Result<Option<lapdev_db_entities::prebuild::Model>> {
        let model = lapdev_db_entities::prebuild::Entity::find_by_id(id)
            .filter(lapdev_db_entities::prebuild::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_prebuild_by_branch_and_commit(
        &self,
        project_id: Uuid,
        branch: &str,
        commit: &str,
    ) -> Result<Option<lapdev_db_entities::prebuild::Model>> {
        let model = lapdev_db_entities::prebuild::Entity::find()
            .filter(lapdev_db_entities::prebuild::Column::ProjectId.eq(project_id))
            .filter(lapdev_db_entities::prebuild::Column::Branch.eq(branch))
            .filter(lapdev_db_entities::prebuild::Column::Commit.eq(commit))
            .filter(lapdev_db_entities::prebuild::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_prebuild_by_branch_and_commit_with_lock(
        &self,
        txn: &DatabaseTransaction,
        project_id: Uuid,
        branch: &str,
        commit: &str,
    ) -> Result<Option<lapdev_db_entities::prebuild::Model>> {
        let model = lapdev_db_entities::prebuild::Entity::find()
            .filter(lapdev_db_entities::prebuild::Column::ProjectId.eq(project_id))
            .filter(lapdev_db_entities::prebuild::Column::Branch.eq(branch))
            .filter(lapdev_db_entities::prebuild::Column::Commit.eq(commit))
            .filter(lapdev_db_entities::prebuild::Column::DeletedAt.is_null())
            .lock_exclusive()
            .one(txn)
            .await?;
        Ok(model)
    }

    pub async fn get_prebuilds(
        &self,
        project_id: Uuid,
    ) -> Result<Vec<lapdev_db_entities::prebuild::Model>> {
        let model = lapdev_db_entities::prebuild::Entity::find()
            .filter(lapdev_db_entities::prebuild::Column::ProjectId.eq(project_id))
            .filter(lapdev_db_entities::prebuild::Column::DeletedAt.is_null())
            .order_by_desc(lapdev_db_entities::prebuild::Column::CreatedAt)
            .all(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_prebuild_replica(
        &self,
        prebuild_id: Uuid,
        host_id: Uuid,
    ) -> Result<Option<lapdev_db_entities::prebuild_replica::Model>> {
        let replica = lapdev_db_entities::prebuild_replica::Entity::find()
            .filter(lapdev_db_entities::prebuild_replica::Column::DeletedAt.is_null())
            .filter(lapdev_db_entities::prebuild_replica::Column::PrebuildId.eq(prebuild_id))
            .filter(lapdev_db_entities::prebuild_replica::Column::HostId.eq(host_id))
            .one(&self.conn)
            .await?;
        Ok(replica)
    }

    pub async fn get_all_workspace_hosts(
        &self,
    ) -> Result<Vec<lapdev_db_entities::workspace_host::Model>> {
        let model = lapdev_db_entities::workspace_host::Entity::find()
            .filter(lapdev_db_entities::workspace_host::Column::DeletedAt.is_null())
            .all(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_workspace_host(
        &self,
        id: Uuid,
    ) -> Result<Option<lapdev_db_entities::workspace_host::Model>> {
        let model = lapdev_db_entities::workspace_host::Entity::find()
            .filter(lapdev_db_entities::workspace_host::Column::Id.eq(id))
            .one(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_workspace_port(
        &self,
        ws_id: Uuid,
        port: u16,
    ) -> Result<Option<lapdev_db_entities::workspace_port::Model>> {
        let model = lapdev_db_entities::workspace_port::Entity::find()
            .filter(lapdev_db_entities::workspace_port::Column::WorkspaceId.eq(ws_id))
            .filter(lapdev_db_entities::workspace_port::Column::Port.eq(port))
            .filter(lapdev_db_entities::workspace_port::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_workspace_ports(
        &self,
        ws_id: Uuid,
    ) -> Result<Vec<lapdev_db_entities::workspace_port::Model>> {
        let model = lapdev_db_entities::workspace_port::Entity::find()
            .filter(lapdev_db_entities::workspace_port::Column::WorkspaceId.eq(ws_id))
            .filter(lapdev_db_entities::workspace_port::Column::DeletedAt.is_null())
            .all(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_workspace_host_by_host(
        &self,
        host: &str,
    ) -> Result<Option<lapdev_db_entities::workspace_host::Model>> {
        let model = lapdev_db_entities::workspace_host::Entity::find()
            .filter(lapdev_db_entities::workspace_host::Column::Host.eq(host))
            .filter(lapdev_db_entities::workspace_host::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_workspace_host_with_lock(
        &self,
        txn: &DatabaseTransaction,
        id: Uuid,
    ) -> Result<Option<lapdev_db_entities::workspace_host::Model>> {
        let model = lapdev_db_entities::workspace_host::Entity::find()
            .filter(lapdev_db_entities::workspace_host::Column::Id.eq(id))
            .filter(lapdev_db_entities::workspace_host::Column::DeletedAt.is_null())
            .lock_exclusive()
            .one(txn)
            .await?;
        Ok(model)
    }

    pub async fn get_machine_type(
        &self,
        id: Uuid,
    ) -> Result<Option<lapdev_db_entities::machine_type::Model>> {
        let model = lapdev_db_entities::machine_type::Entity::find_by_id(id)
            .one(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_all_machine_types(
        &self,
    ) -> Result<Vec<lapdev_db_entities::machine_type::Model>> {
        let models = lapdev_db_entities::machine_type::Entity::find()
            .filter(lapdev_db_entities::machine_type::Column::DeletedAt.is_null())
            .order_by_desc(lapdev_db_entities::machine_type::Column::Shared)
            .order_by_asc(lapdev_db_entities::machine_type::Column::Cpu)
            .all(&self.conn)
            .await?;
        Ok(models)
    }

    // Kubernetes cluster operations
    pub async fn create_kube_cluster(
        &self,
        cluster_id: Uuid,
        org_id: Uuid,
        user_id: Uuid,
        name: String,
        status: String,
    ) -> Result<lapdev_db_entities::kube_cluster::Model> {
        let cluster = lapdev_db_entities::kube_cluster::ActiveModel {
            id: ActiveValue::Set(cluster_id),
            created_at: ActiveValue::Set(Utc::now().into()),
            name: ActiveValue::Set(name),
            cluster_version: ActiveValue::Set(None),
            status: ActiveValue::Set(status),
            provider: ActiveValue::Set(None),
            region: ActiveValue::Set(None),
            manager_namespace: ActiveValue::Set(None),
            created_by: ActiveValue::Set(user_id),
            organization_id: ActiveValue::Set(org_id),
            deleted_at: ActiveValue::Set(None),
            last_reported_at: ActiveValue::Set(None),
            can_deploy_personal: ActiveValue::Set(true),
            can_deploy_shared: ActiveValue::Set(true),
        }
        .insert(&self.conn)
        .await?;
        Ok(cluster)
    }

    pub async fn create_kube_cluster_token(
        &self,
        cluster_id: Uuid,
        user_id: Uuid,
        name: String,
        token_hash: Vec<u8>,
    ) -> Result<lapdev_db_entities::kube_cluster_token::Model> {
        let token = lapdev_db_entities::kube_cluster_token::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            created_at: ActiveValue::Set(Utc::now().into()),
            deleted_at: ActiveValue::Set(None),
            last_used_at: ActiveValue::Set(None),
            cluster_id: ActiveValue::Set(cluster_id),
            created_by: ActiveValue::Set(user_id),
            name: ActiveValue::Set(name),
            token: ActiveValue::Set(token_hash),
        }
        .insert(&self.conn)
        .await?;
        Ok(token)
    }

    pub async fn delete_kube_cluster(&self, cluster_id: Uuid) -> Result<()> {
        lapdev_db_entities::kube_cluster::ActiveModel {
            id: ActiveValue::Set(cluster_id),
            deleted_at: ActiveValue::Set(Some(Utc::now().into())),
            ..Default::default()
        }
        .update(&self.conn)
        .await?;
        Ok(())
    }

    pub async fn check_kube_cluster_has_app_catalogs(&self, cluster_id: Uuid) -> Result<bool> {
        let has_catalogs = lapdev_db_entities::kube_app_catalog::Entity::find()
            .filter(lapdev_db_entities::kube_app_catalog::Column::ClusterId.eq(cluster_id))
            .filter(lapdev_db_entities::kube_app_catalog::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?
            .is_some();
        Ok(has_catalogs)
    }

    pub async fn check_kube_cluster_has_environments(&self, cluster_id: Uuid) -> Result<bool> {
        let has_environments = lapdev_db_entities::kube_environment::Entity::find()
            .filter(lapdev_db_entities::kube_environment::Column::ClusterId.eq(cluster_id))
            .filter(lapdev_db_entities::kube_environment::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?
            .is_some();
        Ok(has_environments)
    }

    pub async fn set_cluster_deployable(
        &self,
        cluster_id: Uuid,
        can_deploy_personal: bool,
        can_deploy_shared: bool,
    ) -> Result<lapdev_db_entities::kube_cluster::Model> {
        let active_model = lapdev_db_entities::kube_cluster::ActiveModel {
            id: ActiveValue::Set(cluster_id),
            can_deploy_personal: ActiveValue::Set(can_deploy_personal),
            can_deploy_shared: ActiveValue::Set(can_deploy_shared),
            ..Default::default()
        };
        let updated = active_model.update(&self.conn).await?;
        Ok(updated)
    }

    pub async fn get_active_cluster_services(
        &self,
        cluster_id: Uuid,
        namespace: &str,
    ) -> Result<Vec<CachedClusterService>> {
        let services = kube_cluster_service::Entity::find()
            .filter(kube_cluster_service::Column::ClusterId.eq(cluster_id))
            .filter(kube_cluster_service::Column::Namespace.eq(namespace))
            .filter(kube_cluster_service::Column::DeletedAt.is_null())
            .all(&self.conn)
            .await?;

        let mut results = Vec::with_capacity(services.len());
        for svc in services {
            let selector: BTreeMap<String, String> =
                serde_json::from_value(svc.selector.clone()).unwrap_or_default();
            let ports_raw: Vec<StoredServicePort> =
                serde_json::from_value(svc.ports.clone()).unwrap_or_default();
            let ports = ports_raw
                .into_iter()
                .filter_map(StoredServicePort::into_service_port)
                .collect();

            results.push(CachedClusterService {
                name: svc.name,
                selector,
                ports,
                service_yaml: svc.service_yaml,
            });
        }

        Ok(results)
    }

    pub async fn upsert_cluster_service(
        &self,
        cluster_id: Uuid,
        namespace: &str,
        name: &str,
        resource_version: &str,
        service_yaml: String,
        selector: serde_json::Value,
        ports: serde_json::Value,
        service_type: Option<String>,
        cluster_ip: Option<String>,
        observed_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Uuid> {
        let observed = observed_at.into();
        let selector_map: BTreeMap<String, String> =
            serde_json::from_value(selector.clone()).unwrap_or_default();
        let selector_json = Json::from(selector);
        let ports_json = Json::from(ports);

        let existing = kube_cluster_service::Entity::find()
            .filter(kube_cluster_service::Column::ClusterId.eq(cluster_id))
            .filter(kube_cluster_service::Column::Namespace.eq(namespace))
            .filter(kube_cluster_service::Column::Name.eq(name))
            .one(&self.conn)
            .await?;

        if let Some(model) = existing {
            let same_resource_version = model.resource_version == resource_version;
            let mut active: kube_cluster_service::ActiveModel = model.into();
            if !same_resource_version {
                active.resource_version = ActiveValue::Set(resource_version.to_string());
            }
            active.service_yaml = ActiveValue::Set(service_yaml);
            active.selector = ActiveValue::Set(selector_json);
            active.ports = ActiveValue::Set(ports_json);
            active.service_type = ActiveValue::Set(service_type);
            active.cluster_ip = ActiveValue::Set(cluster_ip);
            active.updated_at = ActiveValue::Set(observed);
            active.last_observed_at = ActiveValue::Set(observed);
            active.deleted_at = ActiveValue::Set(None);

            let updated = active.update(&self.conn).await?;
            self.replace_service_selectors(
                updated.id,
                cluster_id,
                namespace,
                &selector_map,
                observed,
            )
            .await?;
            Ok(updated.id)
        } else {
            let new_id = Uuid::new_v4();
            let active = kube_cluster_service::ActiveModel {
                id: ActiveValue::Set(new_id),
                created_at: ActiveValue::Set(observed),
                updated_at: ActiveValue::Set(observed),
                deleted_at: ActiveValue::Set(None),
                cluster_id: ActiveValue::Set(cluster_id),
                namespace: ActiveValue::Set(namespace.to_string()),
                name: ActiveValue::Set(name.to_string()),
                resource_version: ActiveValue::Set(resource_version.to_string()),
                service_yaml: ActiveValue::Set(service_yaml),
                selector: ActiveValue::Set(selector_json),
                ports: ActiveValue::Set(ports_json),
                service_type: ActiveValue::Set(service_type),
                cluster_ip: ActiveValue::Set(cluster_ip),
                last_observed_at: ActiveValue::Set(observed),
            };

            active.insert(&self.conn).await?;
            self.replace_service_selectors(new_id, cluster_id, namespace, &selector_map, observed)
                .await?;
            Ok(new_id)
        }
    }

    pub async fn get_service_selector_map(
        &self,
        cluster_id: Uuid,
        namespace: &str,
        name: &str,
    ) -> Result<Option<BTreeMap<String, String>>> {
        let Some(model) = kube_cluster_service::Entity::find()
            .filter(kube_cluster_service::Column::ClusterId.eq(cluster_id))
            .filter(kube_cluster_service::Column::Namespace.eq(namespace))
            .filter(kube_cluster_service::Column::Name.eq(name))
            .one(&self.conn)
            .await?
        else {
            return Ok(None);
        };

        let selector: BTreeMap<String, String> =
            serde_json::from_value(model.selector.clone()).unwrap_or_default();

        Ok(Some(selector))
    }

    pub async fn mark_cluster_service_deleted(
        &self,
        cluster_id: Uuid,
        namespace: &str,
        name: &str,
        observed_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<()> {
        let model = match kube_cluster_service::Entity::find()
            .filter(kube_cluster_service::Column::ClusterId.eq(cluster_id))
            .filter(kube_cluster_service::Column::Namespace.eq(namespace))
            .filter(kube_cluster_service::Column::Name.eq(name))
            .one(&self.conn)
            .await?
        {
            Some(model) => model,
            None => return Ok(()),
        };

        let observed = observed_at.into();
        let service_id = model.id;
        let mut active: kube_cluster_service::ActiveModel = model.into();
        active.deleted_at = ActiveValue::Set(Some(observed));
        active.updated_at = ActiveValue::Set(chrono::Utc::now().into());
        active.last_observed_at = ActiveValue::Set(observed);
        active.update(&self.conn).await?;
        self.mark_service_selectors_deleted(service_id, observed)
            .await?;
        Ok(())
    }

    // App catalog operations
    pub async fn create_app_catalog(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        cluster_id: Uuid,
        name: String,
        description: Option<String>,
        workloads: Vec<lapdev_common::kube::KubeAppCatalogWorkloadCreate>,
    ) -> Result<Uuid> {
        let txn = self.conn.begin().await?;

        let catalog_id = Uuid::new_v4();
        let now = Utc::now().into();

        // Create the app catalog
        lapdev_db_entities::kube_app_catalog::ActiveModel {
            id: ActiveValue::Set(catalog_id),
            created_at: ActiveValue::Set(now),
            name: ActiveValue::Set(name),
            description: ActiveValue::Set(description),
            resources: ActiveValue::Set("".to_string()), // Keep empty for backward compatibility
            cluster_id: ActiveValue::Set(cluster_id),
            created_by: ActiveValue::Set(user_id),
            organization_id: ActiveValue::Set(org_id),
            deleted_at: ActiveValue::Set(None),
            sync_version: ActiveValue::Set(0),
            last_synced_at: ActiveValue::Set(None),
            last_sync_actor_id: ActiveValue::Set(None),
        }
        .insert(&txn)
        .await?;

        // Insert individual workloads
        self.insert_workloads_to_catalog(&txn, catalog_id, cluster_id, workloads, now)
            .await?;

        txn.commit().await?;
        if let Ok(Some(catalog)) = self.get_app_catalog(catalog_id).await {
            self.publish_app_catalog_status_event(&catalog).await;
        }
        Ok(catalog_id)
    }

    pub async fn create_app_catalog_with_enriched_workloads(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        cluster_id: Uuid,
        name: String,
        description: Option<String>,
        enriched_workloads: Vec<KubeWorkloadDetails>,
    ) -> Result<Uuid> {
        let txn = self.conn.begin().await?;

        let catalog_id = Uuid::new_v4();
        let now = Utc::now().into();

        // Create the app catalog
        lapdev_db_entities::kube_app_catalog::ActiveModel {
            id: ActiveValue::Set(catalog_id),
            created_at: ActiveValue::Set(now),
            name: ActiveValue::Set(name),
            description: ActiveValue::Set(description),
            resources: ActiveValue::Set("".to_string()), // Keep empty for backward compatibility
            cluster_id: ActiveValue::Set(cluster_id),
            created_by: ActiveValue::Set(user_id),
            organization_id: ActiveValue::Set(org_id),
            deleted_at: ActiveValue::Set(None),
            sync_version: ActiveValue::Set(0),
            last_synced_at: ActiveValue::Set(None),
            last_sync_actor_id: ActiveValue::Set(Some(user_id)),
        }
        .insert(&txn)
        .await?;

        // Insert enriched workloads
        let _ = self
            .insert_enriched_workloads_to_catalog(
                &txn,
                catalog_id,
                cluster_id,
                enriched_workloads,
                now,
            )
            .await?;

        txn.commit().await?;
        if let Ok(Some(catalog)) = self.get_app_catalog(catalog_id).await {
            self.publish_app_catalog_status_event(&catalog).await;
        }
        Ok(catalog_id)
    }

    pub async fn get_all_app_catalogs_paginated(
        &self,
        org_id: Uuid,
        search: Option<String>,
        pagination: Option<PagePaginationParams>,
    ) -> Result<(
        Vec<(
            lapdev_db_entities::kube_app_catalog::Model,
            Option<lapdev_db_entities::kube_cluster::Model>,
        )>,
        usize,
    )> {
        let pagination = pagination.unwrap_or_default();

        // Build base query
        let mut base_query = lapdev_db_entities::kube_app_catalog::Entity::find()
            .filter(lapdev_db_entities::kube_app_catalog::Column::OrganizationId.eq(org_id))
            .filter(lapdev_db_entities::kube_app_catalog::Column::DeletedAt.is_null());

        if let Some(search_term) = search.as_ref().filter(|s| !s.trim().is_empty()) {
            use sea_orm::sea_query::{extension::postgres::PgExpr, Expr, SimpleExpr};
            let search_pattern = format!("%{}%", search_term.trim().to_lowercase());
            base_query = base_query.filter(
                SimpleExpr::FunctionCall(sea_orm::sea_query::Func::lower(Expr::col((
                    lapdev_db_entities::kube_app_catalog::Entity,
                    lapdev_db_entities::kube_app_catalog::Column::Name,
                ))))
                .ilike(&search_pattern),
            );
        }

        // Get total count
        let total_count = base_query.clone().count(&self.conn).await? as usize;

        // Apply pagination
        let offset = (pagination.page.saturating_sub(1)) * pagination.page_size;
        let paginated_query = base_query
            .limit(pagination.page_size as u64)
            .offset(offset as u64)
            .order_by_desc(lapdev_db_entities::kube_app_catalog::Column::OrganizationId)
            .order_by_desc(lapdev_db_entities::kube_app_catalog::Column::DeletedAt)
            .order_by_desc(lapdev_db_entities::kube_app_catalog::Column::CreatedAt);

        let catalogs_with_clusters = paginated_query
            .find_also_related(lapdev_db_entities::kube_cluster::Entity)
            .all(&self.conn)
            .await?;

        Ok((catalogs_with_clusters, total_count))
    }

    pub async fn delete_app_catalog(&self, catalog_id: Uuid) -> Result<()> {
        lapdev_db_entities::kube_app_catalog::ActiveModel {
            id: ActiveValue::Set(catalog_id),
            deleted_at: ActiveValue::Set(Some(Utc::now().into())),
            ..Default::default()
        }
        .update(&self.conn)
        .await?;
        Ok(())
    }

    pub async fn get_app_catalog(
        &self,
        catalog_id: Uuid,
    ) -> Result<Option<lapdev_db_entities::kube_app_catalog::Model>> {
        let catalog = lapdev_db_entities::kube_app_catalog::Entity::find_by_id(catalog_id)
            .filter(lapdev_db_entities::kube_app_catalog::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?;
        Ok(catalog)
    }

    pub async fn get_app_catalog_workloads(
        &self,
        catalog_id: Uuid,
    ) -> Result<Vec<KubeAppCatalogWorkload>> {
        let workloads = lapdev_db_entities::kube_app_catalog_workload::Entity::find()
            .filter(
                lapdev_db_entities::kube_app_catalog_workload::Column::AppCatalogId.eq(catalog_id),
            )
            .filter(lapdev_db_entities::kube_app_catalog_workload::Column::DeletedAt.is_null())
            .all(&self.conn)
            .await?;

        Ok(workloads
            .into_iter()
            .filter_map(|w| {
                if w.workload_yaml.is_empty() {
                    return None;
                }

                let containers = serde_json::from_value(w.containers.clone()).ok()?;
                let ports: Vec<lapdev_common::kube::KubeServicePort> =
                    serde_json::from_value(w.ports.clone()).unwrap_or_default();

                Some(KubeAppCatalogWorkload {
                    id: w.id,
                    name: w.name,
                    namespace: w.namespace,
                    kind: w
                        .kind
                        .parse()
                        .unwrap_or(lapdev_common::kube::KubeWorkloadKind::Deployment),
                    containers,
                    ports,
                    workload_yaml: w.workload_yaml,
                    catalog_sync_version: w.catalog_sync_version,
                })
            })
            .collect())
    }

    pub async fn get_app_catalog_workload(
        &self,
        workload_id: Uuid,
    ) -> Result<Option<lapdev_db_entities::kube_app_catalog_workload::Model>> {
        let workload =
            lapdev_db_entities::kube_app_catalog_workload::Entity::find_by_id(workload_id)
                .filter(lapdev_db_entities::kube_app_catalog_workload::Column::DeletedAt.is_null())
                .one(&self.conn)
                .await?;
        Ok(workload)
    }

    pub async fn get_cluster_catalog_namespaces(&self, cluster_id: Uuid) -> Result<Vec<String>> {
        let namespaces = kube_app_catalog_workload::Entity::find()
            .filter(kube_app_catalog_workload::Column::ClusterId.eq(cluster_id))
            .filter(kube_app_catalog_workload::Column::DeletedAt.is_null())
            .select_only()
            .column(kube_app_catalog_workload::Column::Namespace)
            .distinct()
            .into_tuple::<String>()
            .all(&self.conn)
            .await?;

        Ok(namespaces)
    }

    pub async fn get_cluster_environment_namespaces(
        &self,
        cluster_id: Uuid,
    ) -> Result<Vec<String>> {
        let namespaces = lapdev_db_entities::kube_environment::Entity::find()
            .filter(lapdev_db_entities::kube_environment::Column::ClusterId.eq(cluster_id))
            .filter(lapdev_db_entities::kube_environment::Column::DeletedAt.is_null())
            .select_only()
            .column(lapdev_db_entities::kube_environment::Column::Namespace)
            .distinct()
            .into_tuple::<String>()
            .all(&self.conn)
            .await?;

        Ok(namespaces)
    }

    pub async fn get_cluster_watch_namespaces(&self, cluster_id: Uuid) -> Result<Vec<String>> {
        let mut namespaces: BTreeSet<String> = BTreeSet::new();
        namespaces.extend(self.get_cluster_catalog_namespaces(cluster_id).await?);
        namespaces.extend(self.get_cluster_environment_namespaces(cluster_id).await?);
        Ok(namespaces.into_iter().collect())
    }

    pub async fn delete_app_catalog_workload(&self, workload_id: Uuid) -> Result<()> {
        lapdev_db_entities::kube_app_catalog_workload::ActiveModel {
            id: ActiveValue::Set(workload_id),
            deleted_at: ActiveValue::Set(Some(Utc::now().into())),
            ..Default::default()
        }
        .update(&self.conn)
        .await?;
        Ok(())
    }

    pub async fn bump_app_catalog_sync_version(
        &self,
        catalog_id: Uuid,
        synced_at: DateTimeWithTimeZone,
    ) -> Result<i64> {
        let updated = lapdev_db_entities::kube_app_catalog::Entity::update_many()
            .filter(lapdev_db_entities::kube_app_catalog::Column::Id.eq(catalog_id))
            .filter(lapdev_db_entities::kube_app_catalog::Column::DeletedAt.is_null())
            .col_expr(
                lapdev_db_entities::kube_app_catalog::Column::SyncVersion,
                Expr::col(lapdev_db_entities::kube_app_catalog::Column::SyncVersion).add(1),
            )
            .col_expr(
                lapdev_db_entities::kube_app_catalog::Column::LastSyncedAt,
                Expr::value(synced_at),
            )
            .exec_with_returning(&self.conn)
            .await?;

        if let Some(model) = updated.into_iter().next() {
            self.publish_app_catalog_status_event(&model).await;
            Ok(model.sync_version)
        } else {
            Err(anyhow!(
                "App catalog {} not found or already deleted",
                catalog_id
            ))
        }
    }

    pub async fn update_app_catalog_workload(
        &self,
        workload_id: Uuid,
        containers: Vec<lapdev_common::kube::KubeContainerInfo>,
    ) -> Result<()> {
        let containers_json = serde_json::to_value(containers)?;
        lapdev_db_entities::kube_app_catalog_workload::ActiveModel {
            id: ActiveValue::Set(workload_id),
            containers: ActiveValue::Set(containers_json),
            ..Default::default()
        }
        .update(&self.conn)
        .await?;
        Ok(())
    }

    pub async fn check_app_catalog_has_environments(&self, catalog_id: Uuid) -> Result<bool> {
        let has_environments = lapdev_db_entities::kube_environment::Entity::find()
            .filter(lapdev_db_entities::kube_environment::Column::AppCatalogId.eq(catalog_id))
            .filter(lapdev_db_entities::kube_environment::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?
            .is_some();
        Ok(has_environments)
    }

    // Environment operations
    pub async fn check_environment_has_branches(&self, environment_id: Uuid) -> Result<bool> {
        let has_branches = lapdev_db_entities::kube_environment::Entity::find()
            .filter(
                lapdev_db_entities::kube_environment::Column::BaseEnvironmentId.eq(environment_id),
            )
            .filter(lapdev_db_entities::kube_environment::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?
            .is_some();
        Ok(has_branches)
    }
    pub async fn get_all_kube_environments_paginated(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        search: Option<String>,
        is_shared: bool,
        is_branch: bool,
        pagination: Option<PagePaginationParams>,
    ) -> Result<(
        Vec<(
            lapdev_db_entities::kube_environment::Model,
            Option<lapdev_db_entities::kube_app_catalog::Model>,
            Option<lapdev_db_entities::kube_cluster::Model>,
            Option<String>, // base_environment_name
        )>,
        usize,
    )> {
        let pagination = pagination.unwrap_or_default();

        // Build base query - filter by shared status and ownership
        let mut base_query = lapdev_db_entities::kube_environment::Entity::find()
            .filter(lapdev_db_entities::kube_environment::Column::OrganizationId.eq(org_id))
            .filter(lapdev_db_entities::kube_environment::Column::IsShared.eq(is_shared))
            .filter(lapdev_db_entities::kube_environment::Column::DeletedAt.is_null());

        // Filter by branch environment
        match is_branch {
            true => {
                // Return only branch environments
                base_query = base_query.filter(
                    lapdev_db_entities::kube_environment::Column::BaseEnvironmentId.is_not_null(),
                );
            }
            false => {
                // Return only regular environments
                base_query = base_query.filter(
                    lapdev_db_entities::kube_environment::Column::BaseEnvironmentId.is_null(),
                );
            }
        }

        // For personal environments, only show user's own environments
        if !is_shared {
            base_query =
                base_query.filter(lapdev_db_entities::kube_environment::Column::UserId.eq(user_id));
        }

        if let Some(search_term) = search.as_ref().filter(|s| !s.trim().is_empty()) {
            use sea_orm::sea_query::{extension::postgres::PgExpr, Expr, SimpleExpr};
            let search_pattern = format!("%{}%", search_term.trim().to_lowercase());
            base_query = base_query.filter(
                SimpleExpr::FunctionCall(sea_orm::sea_query::Func::lower(Expr::col((
                    lapdev_db_entities::kube_environment::Entity,
                    lapdev_db_entities::kube_environment::Column::Name,
                ))))
                .ilike(&search_pattern),
            );
        }

        // Get total count
        let total_count = base_query.clone().count(&self.conn).await? as usize;

        // Apply pagination
        let offset = (pagination.page.saturating_sub(1)) * pagination.page_size;
        let paginated_query = base_query
            .limit(pagination.page_size as u64)
            .offset(offset as u64)
            .order_by_desc(lapdev_db_entities::kube_environment::Column::OrganizationId)
            .order_by_desc(lapdev_db_entities::kube_environment::Column::UserId)
            .order_by_desc(lapdev_db_entities::kube_environment::Column::DeletedAt)
            .order_by_desc(lapdev_db_entities::kube_environment::Column::CreatedAt);

        let environments_with_related: Vec<KubeEnvironmentWithRelated> = paginated_query
            .select_only()
            // Select environment columns with aliases
            .column_as(lapdev_db_entities::kube_environment::Column::Id, "env_id")
            .column_as(
                lapdev_db_entities::kube_environment::Column::Name,
                "env_name",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::Namespace,
                "env_namespace",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::AppCatalogId,
                "env_app_catalog_id",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::ClusterId,
                "env_cluster_id",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::Status,
                "env_status",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::CreatedAt,
                "env_created_at",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::IsShared,
                "env_is_shared",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::CatalogSyncVersion,
                "env_catalog_sync_version",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::LastCatalogSyncedAt,
                "env_last_catalog_synced_at",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::PausedAt,
                "env_paused_at",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::ResumedAt,
                "env_resumed_at",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::OrganizationId,
                "env_organization_id",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::UserId,
                "env_user_id",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::DeletedAt,
                "env_deleted_at",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::BaseEnvironmentId,
                "env_base_environment_id",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::AuthToken,
                "env_auth_token",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::SyncStatus,
                "env_sync_status",
            )
            // Join and select app catalog columns
            .join(
                JoinType::LeftJoin,
                lapdev_db_entities::kube_environment::Relation::KubeAppCatalog.def(),
            )
            .column_as(
                lapdev_db_entities::kube_app_catalog::Column::Name,
                "catalog_name",
            )
            .column_as(
                lapdev_db_entities::kube_app_catalog::Column::Description,
                "catalog_description",
            )
            .column_as(
                lapdev_db_entities::kube_app_catalog::Column::SyncVersion,
                "catalog_sync_version",
            )
            .column_as(
                lapdev_db_entities::kube_app_catalog::Column::LastSyncedAt,
                "catalog_last_synced_at",
            )
            .column_as(
                lapdev_db_entities::kube_app_catalog::Column::LastSyncActorId,
                "catalog_last_sync_actor_id",
            )
            // Join and select cluster columns
            .join(
                JoinType::LeftJoin,
                lapdev_db_entities::kube_environment::Relation::KubeCluster.def(),
            )
            .column_as(
                lapdev_db_entities::kube_cluster::Column::Name,
                "cluster_name",
            )
            // Join with base environment (self-referencing join)
            .join_as(
                JoinType::LeftJoin,
                lapdev_db_entities::kube_environment::Relation::SelfRef.def(),
                Alias::new("base_env"),
            )
            .expr_as(
                Expr::col((
                    Alias::new("base_env"),
                    lapdev_db_entities::kube_environment::Column::Name,
                )),
                "base_environment_name",
            )
            .into_model::<KubeEnvironmentWithRelated>()
            .all(&self.conn)
            .await?;

        // Transform the result to match the expected tuple structure
        let environments_with_catalogs_and_clusters = environments_with_related
            .into_iter()
            .map(|related| {
                let env = lapdev_db_entities::kube_environment::Model {
                    id: related.env_id,
                    created_at: related.env_created_at,
                    deleted_at: related.env_deleted_at,
                    organization_id: related.env_organization_id,
                    user_id: related.env_user_id,
                    app_catalog_id: related.env_app_catalog_id,
                    cluster_id: related.env_cluster_id,
                    name: related.env_name,
                    namespace: related.env_namespace,
                    status: related.env_status,
                    is_shared: related.env_is_shared,
                    catalog_sync_version: related.env_catalog_sync_version,
                    last_catalog_synced_at: related.env_last_catalog_synced_at,
                    paused_at: related.env_paused_at,
                    resumed_at: related.env_resumed_at,
                    sync_status: related.env_sync_status.clone(),
                    base_environment_id: related.env_base_environment_id,
                    auth_token: related.env_auth_token,
                };

                let catalog =
                    related
                        .catalog_name
                        .map(|name| lapdev_db_entities::kube_app_catalog::Model {
                            id: related.env_app_catalog_id,
                            name,
                            description: related.catalog_description,
                            resources: String::new(),
                            cluster_id: related.env_cluster_id,
                            created_at: related.env_created_at,
                            created_by: related.env_user_id,
                            organization_id: related.env_organization_id,
                            deleted_at: None,
                            sync_version: related.catalog_sync_version.unwrap_or(0),
                            last_synced_at: related.catalog_last_synced_at,
                            last_sync_actor_id: related.catalog_last_sync_actor_id,
                        });

                let cluster =
                    related
                        .cluster_name
                        .map(|name| lapdev_db_entities::kube_cluster::Model {
                            id: related.env_cluster_id,
                            name,
                            cluster_version: None,
                            status: "Not Ready".to_string(),
                            provider: None,
                            region: None,
                            manager_namespace: None,
                            created_at: related.env_created_at,
                            created_by: related.env_user_id,
                            organization_id: related.env_organization_id,
                            deleted_at: None,
                            last_reported_at: None,
                            can_deploy_personal: true,
                            can_deploy_shared: true,
                        });

                (env, catalog, cluster, related.base_environment_name)
            })
            .collect();

        Ok((environments_with_catalogs_and_clusters, total_count))
    }

    pub async fn get_environment_dashboard_summary(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        recent_limit: usize,
    ) -> Result<KubeEnvironmentDashboardSummary> {
        let limited_recent = recent_limit.max(1).min(25) as u64;

        // Counts by environment type
        let personal_count = lapdev_db_entities::kube_environment::Entity::find()
            .filter(lapdev_db_entities::kube_environment::Column::OrganizationId.eq(org_id))
            .filter(lapdev_db_entities::kube_environment::Column::DeletedAt.is_null())
            .filter(lapdev_db_entities::kube_environment::Column::IsShared.eq(false))
            .filter(lapdev_db_entities::kube_environment::Column::BaseEnvironmentId.is_null())
            .filter(lapdev_db_entities::kube_environment::Column::UserId.eq(user_id))
            .count(&self.conn)
            .await? as usize;

        let shared_count = lapdev_db_entities::kube_environment::Entity::find()
            .filter(lapdev_db_entities::kube_environment::Column::OrganizationId.eq(org_id))
            .filter(lapdev_db_entities::kube_environment::Column::DeletedAt.is_null())
            .filter(lapdev_db_entities::kube_environment::Column::IsShared.eq(true))
            .filter(lapdev_db_entities::kube_environment::Column::BaseEnvironmentId.is_null())
            .count(&self.conn)
            .await? as usize;

        let branch_access = Condition::all()
            .add(lapdev_db_entities::kube_environment::Column::OrganizationId.eq(org_id))
            .add(lapdev_db_entities::kube_environment::Column::DeletedAt.is_null())
            .add(lapdev_db_entities::kube_environment::Column::BaseEnvironmentId.is_not_null())
            .add(
                Condition::any()
                    .add(lapdev_db_entities::kube_environment::Column::IsShared.eq(true))
                    .add(lapdev_db_entities::kube_environment::Column::UserId.eq(user_id)),
            );

        let branch_count = lapdev_db_entities::kube_environment::Entity::find()
            .filter(branch_access.clone())
            .count(&self.conn)
            .await? as usize;

        // Accessible environments condition (used for status breakdown & recent list)
        let accessible_condition = Condition::all()
            .add(lapdev_db_entities::kube_environment::Column::OrganizationId.eq(org_id))
            .add(lapdev_db_entities::kube_environment::Column::DeletedAt.is_null())
            .add(
                Condition::any()
                    .add(
                        Condition::all()
                            .add(lapdev_db_entities::kube_environment::Column::IsShared.eq(false))
                            .add(
                                lapdev_db_entities::kube_environment::Column::BaseEnvironmentId
                                    .is_null(),
                            )
                            .add(lapdev_db_entities::kube_environment::Column::UserId.eq(user_id)),
                    )
                    .add(lapdev_db_entities::kube_environment::Column::IsShared.eq(true))
                    .add(
                        Condition::all()
                            .add(
                                lapdev_db_entities::kube_environment::Column::BaseEnvironmentId
                                    .is_not_null(),
                            )
                            .add(lapdev_db_entities::kube_environment::Column::UserId.eq(user_id)),
                    ),
            );

        let status_counts_raw: Vec<StatusCountRow> =
            lapdev_db_entities::kube_environment::Entity::find()
                .select_only()
                .column(lapdev_db_entities::kube_environment::Column::Status)
                .column_as(
                    Expr::col((
                        lapdev_db_entities::kube_environment::Entity,
                        lapdev_db_entities::kube_environment::Column::Id,
                    ))
                    .count(),
                    "count",
                )
                .filter(accessible_condition.clone())
                .group_by(lapdev_db_entities::kube_environment::Column::Status)
                .into_model::<StatusCountRow>()
                .all(&self.conn)
                .await?;

        let mut status_breakdown: Vec<KubeEnvironmentStatusCount> = status_counts_raw
            .into_iter()
            .filter_map(|row| {
                KubeEnvironmentStatus::from_str(&row.status)
                    .ok()
                    .map(|status| KubeEnvironmentStatusCount {
                        status,
                        count: row.count as usize,
                    })
            })
            .collect();
        status_breakdown.sort_by(|a, b| b.count.cmp(&a.count));

        let recent_query = lapdev_db_entities::kube_environment::Entity::find()
            .filter(accessible_condition)
            .order_by_desc(lapdev_db_entities::kube_environment::Column::ResumedAt)
            .order_by_desc(lapdev_db_entities::kube_environment::Column::LastCatalogSyncedAt)
            .order_by_desc(lapdev_db_entities::kube_environment::Column::PausedAt)
            .order_by_desc(lapdev_db_entities::kube_environment::Column::CreatedAt)
            .limit(limited_recent);

        let recent_with_related: Vec<KubeEnvironmentWithRelated> = recent_query
            .select_only()
            .column_as(lapdev_db_entities::kube_environment::Column::Id, "env_id")
            .column_as(
                lapdev_db_entities::kube_environment::Column::Name,
                "env_name",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::Namespace,
                "env_namespace",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::AppCatalogId,
                "env_app_catalog_id",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::ClusterId,
                "env_cluster_id",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::Status,
                "env_status",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::CreatedAt,
                "env_created_at",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::IsShared,
                "env_is_shared",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::OrganizationId,
                "env_organization_id",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::UserId,
                "env_user_id",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::DeletedAt,
                "env_deleted_at",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::BaseEnvironmentId,
                "env_base_environment_id",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::AuthToken,
                "env_auth_token",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::CatalogSyncVersion,
                "env_catalog_sync_version",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::LastCatalogSyncedAt,
                "env_last_catalog_synced_at",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::PausedAt,
                "env_paused_at",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::ResumedAt,
                "env_resumed_at",
            )
            .column_as(
                lapdev_db_entities::kube_environment::Column::SyncStatus,
                "env_sync_status",
            )
            .join(
                JoinType::LeftJoin,
                lapdev_db_entities::kube_environment::Relation::KubeAppCatalog.def(),
            )
            .column_as(
                lapdev_db_entities::kube_app_catalog::Column::Name,
                "catalog_name",
            )
            .column_as(
                lapdev_db_entities::kube_app_catalog::Column::Description,
                "catalog_description",
            )
            .column_as(
                lapdev_db_entities::kube_app_catalog::Column::SyncVersion,
                "catalog_sync_version",
            )
            .column_as(
                lapdev_db_entities::kube_app_catalog::Column::LastSyncedAt,
                "catalog_last_synced_at",
            )
            .column_as(
                lapdev_db_entities::kube_app_catalog::Column::LastSyncActorId,
                "catalog_last_sync_actor_id",
            )
            .join(
                JoinType::LeftJoin,
                lapdev_db_entities::kube_environment::Relation::KubeCluster.def(),
            )
            .column_as(
                lapdev_db_entities::kube_cluster::Column::Name,
                "cluster_name",
            )
            .join_as(
                JoinType::LeftJoin,
                lapdev_db_entities::kube_environment::Relation::SelfRef.def(),
                Alias::new("base_env"),
            )
            .expr_as(
                Expr::col((
                    Alias::new("base_env"),
                    lapdev_db_entities::kube_environment::Column::Name,
                )),
                "base_environment_name",
            )
            .into_model::<KubeEnvironmentWithRelated>()
            .all(&self.conn)
            .await?;

        let recent_environments = recent_with_related
            .into_iter()
            .filter_map(|related| {
                let catalog_name = related.catalog_name?;
                let cluster_name = related.cluster_name?;
                let status = KubeEnvironmentStatus::from_str(&related.env_status)
                    .unwrap_or(KubeEnvironmentStatus::Creating);
                let sync_status = KubeEnvironmentSyncStatus::from_str(&related.env_sync_status)
                    .unwrap_or(KubeEnvironmentSyncStatus::Idle);

                let catalog_update_available = related
                    .catalog_sync_version
                    .map(|catalog_version| catalog_version > related.env_catalog_sync_version)
                    .unwrap_or(false);

                Some(KubeEnvironment {
                    id: related.env_id,
                    user_id: related.env_user_id,
                    name: related.env_name,
                    namespace: related.env_namespace,
                    app_catalog_id: related.env_app_catalog_id,
                    app_catalog_name: catalog_name,
                    cluster_id: related.env_cluster_id,
                    cluster_name,
                    status,
                    created_at: related.env_created_at.to_string(),
                    is_shared: related.env_is_shared,
                    base_environment_id: related.env_base_environment_id,
                    base_environment_name: related.base_environment_name,
                    catalog_sync_version: related.env_catalog_sync_version,
                    last_catalog_synced_at: related
                        .env_last_catalog_synced_at
                        .map(|dt| dt.to_string()),
                    paused_at: related.env_paused_at.map(|dt| dt.to_string()),
                    resumed_at: related.env_resumed_at.map(|dt| dt.to_string()),
                    catalog_update_available,
                    catalog_last_sync_actor_id: related.catalog_last_sync_actor_id,
                    sync_status,
                })
            })
            .collect::<Vec<_>>();

        Ok(KubeEnvironmentDashboardSummary {
            personal_count,
            shared_count,
            branch_count,
            total_count: personal_count + shared_count + branch_count,
            status_breakdown,
            recent_environments,
        })
    }

    pub async fn get_kube_environment(
        &self,
        environment_id: Uuid,
    ) -> Result<Option<lapdev_db_entities::kube_environment::Model>, sea_orm::DbErr> {
        lapdev_db_entities::kube_environment::Entity::find()
            .filter(lapdev_db_entities::kube_environment::Column::Id.eq(environment_id))
            .filter(lapdev_db_entities::kube_environment::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await
    }

    pub async fn get_branch_environments(
        &self,
        base_environment_id: Uuid,
    ) -> Result<Vec<lapdev_db_entities::kube_environment::Model>> {
        let environments = lapdev_db_entities::kube_environment::Entity::find()
            .filter(
                lapdev_db_entities::kube_environment::Column::BaseEnvironmentId
                    .eq(base_environment_id),
            )
            .filter(lapdev_db_entities::kube_environment::Column::DeletedAt.is_null())
            .all(&self.conn)
            .await?;
        Ok(environments)
    }

    pub async fn delete_kube_environment(&self, environment_id: Uuid) -> Result<()> {
        lapdev_db_entities::kube_environment::ActiveModel {
            id: ActiveValue::Set(environment_id),
            deleted_at: ActiveValue::Set(Some(Utc::now().into())),
            ..Default::default()
        }
        .update(&self.conn)
        .await?;
        Ok(())
    }

    // Kube Namespace operations
    pub async fn create_kube_namespace(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        name: String,
        description: Option<String>,
        is_shared: bool,
    ) -> Result<lapdev_db_entities::kube_namespace::Model> {
        let namespace = lapdev_db_entities::kube_namespace::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            created_at: ActiveValue::Set(Utc::now().into()),
            deleted_at: ActiveValue::Set(None),
            organization_id: ActiveValue::Set(org_id),
            user_id: ActiveValue::Set(user_id),
            name: ActiveValue::Set(name),
            description: ActiveValue::Set(description),
            is_shared: ActiveValue::Set(is_shared),
        }
        .insert(&self.conn)
        .await?;
        Ok(namespace)
    }

    pub async fn get_all_kube_namespaces(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        is_shared: bool,
    ) -> Result<Vec<lapdev_db_entities::kube_namespace::Model>> {
        let mut query = lapdev_db_entities::kube_namespace::Entity::find()
            .filter(lapdev_db_entities::kube_namespace::Column::OrganizationId.eq(org_id))
            .filter(lapdev_db_entities::kube_namespace::Column::IsShared.eq(is_shared))
            .filter(lapdev_db_entities::kube_namespace::Column::DeletedAt.is_null());

        // For personal namespaces, only show those created by the user
        if !is_shared {
            query = query.filter(lapdev_db_entities::kube_namespace::Column::UserId.eq(user_id));
        }

        let namespaces = query
            .order_by_asc(lapdev_db_entities::kube_namespace::Column::Name)
            .all(&self.conn)
            .await?;
        Ok(namespaces)
    }

    pub async fn get_kube_namespace(
        &self,
        namespace_id: Uuid,
    ) -> Result<Option<lapdev_db_entities::kube_namespace::Model>> {
        let namespace = lapdev_db_entities::kube_namespace::Entity::find_by_id(namespace_id)
            .filter(lapdev_db_entities::kube_namespace::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?;
        Ok(namespace)
    }

    pub async fn delete_kube_namespace(
        &self,
        namespace_id: Uuid,
    ) -> Result<lapdev_db_entities::kube_namespace::Model> {
        let namespace = lapdev_db_entities::kube_namespace::ActiveModel {
            id: ActiveValue::Set(namespace_id),
            deleted_at: ActiveValue::Set(Some(Utc::now().into())),
            ..Default::default()
        };
        let updated = namespace.update(&self.conn).await?;
        Ok(updated)
    }

    async fn insert_workloads_to_catalog(
        &self,
        txn: &sea_orm::DatabaseTransaction,
        catalog_id: Uuid,
        cluster_id: Uuid,
        workloads: Vec<lapdev_common::kube::KubeAppCatalogWorkloadCreate>,
        created_at: sea_orm::prelude::DateTimeWithTimeZone,
    ) -> Result<(), sea_orm::DbErr> {
        for workload in workloads {
            lapdev_db_entities::kube_app_catalog_workload::ActiveModel {
                id: ActiveValue::Set(Uuid::new_v4()),
                created_at: ActiveValue::Set(created_at),
                deleted_at: ActiveValue::Set(None),
                app_catalog_id: ActiveValue::Set(catalog_id),
                cluster_id: ActiveValue::Set(cluster_id),
                name: ActiveValue::Set(workload.name.clone()),
                namespace: ActiveValue::Set(workload.namespace.clone()),
                kind: ActiveValue::Set(workload.kind.to_string()),
                containers: ActiveValue::Set(Json::from(serde_json::json!([]))),
                ports: ActiveValue::Set(Json::from(serde_json::json!([]))),
                workload_yaml: ActiveValue::Set(String::new()),
                catalog_sync_version: ActiveValue::Set(0),
            }
            .insert(txn)
            .await?;
        }
        Ok(())
    }

    pub async fn insert_enriched_workloads_to_catalog(
        &self,
        txn: &sea_orm::DatabaseTransaction,
        catalog_id: Uuid,
        cluster_id: Uuid,
        enriched_workloads: Vec<KubeWorkloadDetails>,
        created_at: sea_orm::prelude::DateTimeWithTimeZone,
    ) -> Result<Vec<Uuid>, sea_orm::DbErr> {
        let mut inserted_ids = Vec::new();
        for workload in enriched_workloads {
            let KubeWorkloadDetails {
                name,
                namespace,
                kind,
                containers,
                ports,
                workload_yaml,
                ..
            } = workload;

            let containers_json = serde_json::to_value(&containers)
                .map(Json::from)
                .unwrap_or_else(|_| Json::from(serde_json::json!([])));

            let ports_json = serde_json::to_value(&ports)
                .map(Json::from)
                .unwrap_or_else(|_| Json::from(serde_json::json!([])));

            let workload_id = Uuid::new_v4();
            let labels = labels_from_workload_yaml(&kind, &workload_yaml);
            let (configmap_refs, secret_refs) =
                dependencies_from_workload_yaml(&kind, &workload_yaml);

            lapdev_db_entities::kube_app_catalog_workload::ActiveModel {
                id: ActiveValue::Set(workload_id),
                created_at: ActiveValue::Set(created_at),
                deleted_at: ActiveValue::Set(None),
                app_catalog_id: ActiveValue::Set(catalog_id),
                cluster_id: ActiveValue::Set(cluster_id),
                name: ActiveValue::Set(name.clone()),
                namespace: ActiveValue::Set(namespace.clone()),
                kind: ActiveValue::Set(kind.to_string()),
                containers: ActiveValue::Set(containers_json),
                ports: ActiveValue::Set(ports_json),
                workload_yaml: ActiveValue::Set(workload_yaml),
                catalog_sync_version: ActiveValue::Set(0),
            }
            .insert(txn)
            .await?;

            self.replace_workload_labels_txn(
                txn,
                workload_id,
                catalog_id,
                cluster_id,
                &namespace,
                &labels,
                created_at,
            )
            .await?;

            self.replace_workload_dependencies_txn(
                txn,
                workload_id,
                catalog_id,
                cluster_id,
                &namespace,
                &configmap_refs,
                &secret_refs,
                created_at,
            )
            .await?;

            inserted_ids.push(workload_id);
        }
        Ok(inserted_ids)
    }

    pub async fn replace_workload_labels_txn(
        &self,
        txn: &DatabaseTransaction,
        workload_id: Uuid,
        app_catalog_id: Uuid,
        cluster_id: Uuid,
        namespace: &str,
        labels: &BTreeMap<String, String>,
        timestamp: DateTimeWithTimeZone,
    ) -> Result<(), sea_orm::DbErr> {
        replace_workload_labels_with_conn(
            txn,
            workload_id,
            app_catalog_id,
            cluster_id,
            namespace,
            labels,
            timestamp,
        )
        .await
    }

    pub async fn replace_workload_labels(
        &self,
        workload_id: Uuid,
        app_catalog_id: Uuid,
        cluster_id: Uuid,
        namespace: &str,
        labels: &BTreeMap<String, String>,
        timestamp: DateTimeWithTimeZone,
    ) -> Result<(), sea_orm::DbErr> {
        replace_workload_labels_with_conn(
            &self.conn,
            workload_id,
            app_catalog_id,
            cluster_id,
            namespace,
            labels,
            timestamp,
        )
        .await
    }

    pub async fn replace_workload_dependencies_txn(
        &self,
        txn: &DatabaseTransaction,
        workload_id: Uuid,
        app_catalog_id: Uuid,
        cluster_id: Uuid,
        namespace: &str,
        configmaps: &BTreeSet<String>,
        secrets: &BTreeSet<String>,
        timestamp: DateTimeWithTimeZone,
    ) -> Result<(), sea_orm::DbErr> {
        replace_workload_dependencies_with_conn(
            txn,
            workload_id,
            app_catalog_id,
            cluster_id,
            namespace,
            configmaps,
            secrets,
            timestamp,
        )
        .await
    }

    pub async fn replace_workload_dependencies(
        &self,
        workload_id: Uuid,
        app_catalog_id: Uuid,
        cluster_id: Uuid,
        namespace: &str,
        configmaps: &BTreeSet<String>,
        secrets: &BTreeSet<String>,
        timestamp: DateTimeWithTimeZone,
    ) -> Result<(), sea_orm::DbErr> {
        replace_workload_dependencies_with_conn(
            &self.conn,
            workload_id,
            app_catalog_id,
            cluster_id,
            namespace,
            configmaps,
            secrets,
            timestamp,
        )
        .await
    }

    pub async fn find_workloads_matching_selector(
        &self,
        cluster_id: Uuid,
        namespace: &str,
        selector: &BTreeMap<String, String>,
    ) -> Result<Vec<Uuid>> {
        if selector.is_empty() {
            return Ok(vec![]);
        }

        let mut condition = Condition::any();
        for (key, value) in selector {
            condition = condition.add(
                Condition::all()
                    .add(
                        lapdev_db_entities::kube_app_catalog_workload_label::Column::LabelKey
                            .eq(key.clone()),
                    )
                    .add(
                        lapdev_db_entities::kube_app_catalog_workload_label::Column::LabelValue
                            .eq(value.clone()),
                    ),
            );
        }

        let required_matches = selector.len() as i32;

        let workloads = lapdev_db_entities::kube_app_catalog_workload_label::Entity::find()
            .select_only()
            .column(lapdev_db_entities::kube_app_catalog_workload_label::Column::WorkloadId)
            .filter(
                lapdev_db_entities::kube_app_catalog_workload_label::Column::ClusterId
                    .eq(cluster_id),
            )
            .filter(
                lapdev_db_entities::kube_app_catalog_workload_label::Column::Namespace
                    .eq(namespace.to_string()),
            )
            .filter(
                lapdev_db_entities::kube_app_catalog_workload_label::Column::DeletedAt.is_null(),
            )
            .filter(condition)
            .group_by(lapdev_db_entities::kube_app_catalog_workload_label::Column::WorkloadId)
            .having(
                Expr::col(lapdev_db_entities::kube_app_catalog_workload_label::Column::WorkloadId)
                    .count()
                    .eq(required_matches),
            )
            .into_tuple::<Uuid>()
            .all(&self.conn)
            .await?;

        Ok(workloads)
    }

    pub async fn get_matching_cluster_services(
        &self,
        cluster_id: Uuid,
        namespace: &str,
        labels: &BTreeMap<String, String>,
    ) -> Result<Vec<CachedClusterService>> {
        if labels.is_empty() {
            return Ok(Vec::new());
        }

        let selector_rows = kube_cluster_service_selector::Entity::find()
            .filter(kube_cluster_service_selector::Column::ClusterId.eq(cluster_id))
            .filter(kube_cluster_service_selector::Column::Namespace.eq(namespace))
            .filter(kube_cluster_service_selector::Column::DeletedAt.is_null())
            .all(&self.conn)
            .await?;

        let mut grouped: HashMap<Uuid, Vec<(String, String)>> = HashMap::new();
        for row in selector_rows {
            grouped
                .entry(row.service_id)
                .or_default()
                .push((row.label_key, row.label_value));
        }

        let matching_ids: Vec<Uuid> = grouped
            .into_iter()
            .filter_map(|(service_id, selectors)| {
                if selectors.iter().all(|(key, value)| {
                    labels
                        .get(key)
                        .map(|existing| existing == value)
                        .unwrap_or(false)
                }) {
                    Some(service_id)
                } else {
                    None
                }
            })
            .collect();

        if matching_ids.is_empty() {
            return Ok(Vec::new());
        }

        let services = kube_cluster_service::Entity::find()
            .filter(kube_cluster_service::Column::Id.is_in(matching_ids))
            .filter(kube_cluster_service::Column::DeletedAt.is_null())
            .all(&self.conn)
            .await?;

        let mut results = Vec::with_capacity(services.len());
        for svc in services {
            let selector: BTreeMap<String, String> =
                serde_json::from_value(svc.selector.clone()).unwrap_or_default();
            let ports_raw: Vec<StoredServicePort> =
                serde_json::from_value(svc.ports.clone()).unwrap_or_default();
            let ports = ports_raw
                .into_iter()
                .filter_map(StoredServicePort::into_service_port)
                .collect();

            results.push(CachedClusterService {
                name: svc.name,
                selector,
                ports,
                service_yaml: svc.service_yaml,
            });
        }

        Ok(results)
    }

    /// Get services that select the given catalog workloads based on their pod labels.
    /// Uses the workload label table and service selector table for efficient matching.
    /// Processes each workload individually and queries service selectors by namespace and label pairs.
    pub async fn get_services_for_catalog_workloads(
        &self,
        cluster_id: Uuid,
        workload_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, Vec<CachedClusterService>>> {
        if workload_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let workload_ids_vec: Vec<Uuid> = workload_ids.to_vec();

        let label_rows = kube_app_catalog_workload_label::Entity::find()
            .filter(
                kube_app_catalog_workload_label::Column::WorkloadId.is_in(workload_ids_vec.clone()),
            )
            .filter(kube_app_catalog_workload_label::Column::DeletedAt.is_null())
            .all(&self.conn)
            .await?;

        if label_rows.is_empty() {
            return Ok(HashMap::new());
        }

        let mut workload_labels: HashMap<Uuid, BTreeMap<String, String>> = HashMap::new();
        let mut workload_namespaces: HashMap<Uuid, String> = HashMap::new();

        for row in label_rows {
            workload_labels
                .entry(row.workload_id)
                .or_default()
                .insert(row.label_key.clone(), row.label_value.clone());
            workload_namespaces
                .entry(row.workload_id)
                .or_insert(row.namespace);
        }

        let mut workloads_by_namespace: HashMap<String, Vec<Uuid>> = HashMap::new();
        for workload_id in workload_ids_vec.iter().copied() {
            if let (Some(labels), Some(namespace)) = (
                workload_labels.get(&workload_id),
                workload_namespaces.get(&workload_id),
            ) {
                if !labels.is_empty() {
                    workloads_by_namespace
                        .entry(namespace.clone())
                        .or_default()
                        .push(workload_id);
                }
            }
        }

        if workloads_by_namespace.is_empty() {
            return Ok(HashMap::new());
        }

        let mut workload_service_ids: HashMap<Uuid, HashSet<Uuid>> = HashMap::new();

        for (namespace, workloads_in_namespace) in workloads_by_namespace {
            let mut label_pairs: BTreeSet<(String, String)> = BTreeSet::new();
            for workload_id in &workloads_in_namespace {
                if let Some(labels) = workload_labels.get(workload_id) {
                    for (key, value) in labels {
                        label_pairs.insert((key.clone(), value.clone()));
                    }
                }
            }

            if label_pairs.is_empty() {
                continue;
            }

            let mut label_condition = Condition::any();
            for (key, value) in &label_pairs {
                label_condition = label_condition.add(
                    Condition::all()
                        .add(kube_cluster_service_selector::Column::LabelKey.eq(key.clone()))
                        .add(kube_cluster_service_selector::Column::LabelValue.eq(value.clone())),
                );
            }

            let selector_rows = kube_cluster_service_selector::Entity::find()
                .filter(kube_cluster_service_selector::Column::ClusterId.eq(cluster_id))
                .filter(kube_cluster_service_selector::Column::Namespace.eq(namespace.clone()))
                .filter(kube_cluster_service_selector::Column::DeletedAt.is_null())
                .filter(label_condition)
                .all(&self.conn)
                .await?;

            if selector_rows.is_empty() {
                continue;
            }

            let candidate_service_ids: HashSet<Uuid> =
                selector_rows.iter().map(|row| row.service_id).collect();

            if candidate_service_ids.is_empty() {
                continue;
            }

            let selector_rows_full = kube_cluster_service_selector::Entity::find()
                .filter(kube_cluster_service_selector::Column::ClusterId.eq(cluster_id))
                .filter(kube_cluster_service_selector::Column::Namespace.eq(namespace.clone()))
                .filter(kube_cluster_service_selector::Column::DeletedAt.is_null())
                .filter(
                    kube_cluster_service_selector::Column::ServiceId
                        .is_in(candidate_service_ids.iter().copied().collect::<Vec<_>>()),
                )
                .all(&self.conn)
                .await?;

            if selector_rows_full.is_empty() {
                continue;
            }

            let mut selectors_by_service: HashMap<Uuid, Vec<(String, String)>> = HashMap::new();
            for row in selector_rows_full {
                selectors_by_service
                    .entry(row.service_id)
                    .or_default()
                    .push((row.label_key, row.label_value));
            }

            for workload_id in workloads_in_namespace {
                if let Some(labels) = workload_labels.get(&workload_id) {
                    for (service_id, selectors) in &selectors_by_service {
                        if selectors.iter().all(|(key, value)| {
                            labels
                                .get(key)
                                .map(|existing| existing == value)
                                .unwrap_or(false)
                        }) {
                            workload_service_ids
                                .entry(workload_id)
                                .or_default()
                                .insert(*service_id);
                        }
                    }
                }
            }
        }

        if workload_service_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let all_service_ids: HashSet<Uuid> = workload_service_ids
            .values()
            .flat_map(|set| set.iter().copied())
            .collect();

        let services = kube_cluster_service::Entity::find()
            .filter(
                kube_cluster_service::Column::Id
                    .is_in(all_service_ids.iter().copied().collect::<Vec<_>>()),
            )
            .filter(kube_cluster_service::Column::DeletedAt.is_null())
            .all(&self.conn)
            .await?;

        let mut service_by_id: HashMap<Uuid, CachedClusterService> = HashMap::new();
        for svc in services {
            let selector: BTreeMap<String, String> =
                serde_json::from_value(svc.selector.clone()).unwrap_or_default();
            let ports_raw: Vec<StoredServicePort> =
                serde_json::from_value(svc.ports.clone()).unwrap_or_default();
            let ports = ports_raw
                .into_iter()
                .filter_map(StoredServicePort::into_service_port)
                .collect();

            service_by_id.insert(
                svc.id,
                CachedClusterService {
                    name: svc.name,
                    selector,
                    ports,
                    service_yaml: svc.service_yaml,
                },
            );
        }

        let mut results: HashMap<Uuid, Vec<CachedClusterService>> = HashMap::new();
        for (workload_id, service_ids) in workload_service_ids {
            let services_for_workload = service_ids
                .into_iter()
                .filter_map(|service_id| service_by_id.get(&service_id).cloned())
                .collect::<Vec<_>>();

            if !services_for_workload.is_empty() {
                results.insert(workload_id, services_for_workload);
            }
        }

        Ok(results)
    }

    pub async fn find_workloads_by_dependency(
        &self,
        cluster_id: Uuid,
        namespace: &str,
        resource_type: &str,
        resource_name: &str,
    ) -> Result<Vec<(Uuid, Uuid)>> {
        let rows = kube_app_catalog_workload_dependency::Entity::find()
            .select_only()
            .column(kube_app_catalog_workload_dependency::Column::WorkloadId)
            .column(kube_app_catalog_workload_dependency::Column::AppCatalogId)
            .filter(kube_app_catalog_workload_dependency::Column::ClusterId.eq(cluster_id))
            .filter(kube_app_catalog_workload_dependency::Column::Namespace.eq(namespace))
            .filter(kube_app_catalog_workload_dependency::Column::ResourceType.eq(resource_type))
            .filter(kube_app_catalog_workload_dependency::Column::ResourceName.eq(resource_name))
            .filter(kube_app_catalog_workload_dependency::Column::DeletedAt.is_null())
            .into_tuple::<(Uuid, Uuid)>()
            .all(&self.conn)
            .await?;

        Ok(rows)
    }

    pub async fn update_catalog_workload_versions(
        &self,
        workload_ids: &[Uuid],
        version: i64,
    ) -> Result<()> {
        if workload_ids.is_empty() {
            return Ok(());
        }

        lapdev_db_entities::kube_app_catalog_workload::Entity::update_many()
            .filter(
                lapdev_db_entities::kube_app_catalog_workload::Column::Id
                    .is_in(workload_ids.iter().cloned().collect::<Vec<_>>()),
            )
            .col_expr(
                lapdev_db_entities::kube_app_catalog_workload::Column::CatalogSyncVersion,
                Expr::value(version),
            )
            .exec(&self.conn)
            .await?;

        Ok(())
    }

    pub async fn replace_service_selectors(
        &self,
        service_id: Uuid,
        cluster_id: Uuid,
        namespace: &str,
        selectors: &BTreeMap<String, String>,
        timestamp: DateTimeWithTimeZone,
    ) -> Result<(), sea_orm::DbErr> {
        replace_service_selectors_with_conn(
            &self.conn, service_id, cluster_id, namespace, selectors, timestamp,
        )
        .await
    }

    pub async fn mark_service_selectors_deleted(
        &self,
        service_id: Uuid,
        timestamp: DateTimeWithTimeZone,
    ) -> Result<(), sea_orm::DbErr> {
        mark_service_selectors_deleted_with_conn(&self.conn, service_id, timestamp).await
    }

    pub async fn get_environment_workloads(
        &self,
        environment_id: Uuid,
    ) -> Result<Vec<KubeEnvironmentWorkload>> {
        let workloads = lapdev_db_entities::kube_environment_workload::Entity::find()
            .filter(
                lapdev_db_entities::kube_environment_workload::Column::EnvironmentId
                    .eq(environment_id),
            )
            .filter(lapdev_db_entities::kube_environment_workload::Column::DeletedAt.is_null())
            .all(&self.conn)
            .await?;

        let mut result = Vec::new();
        for workload in workloads {
            let containers: Vec<KubeContainerInfo> =
                if let Ok(containers) = serde_json::from_value(workload.containers.clone()) {
                    containers
                } else {
                    vec![]
                };

            let ports: Vec<lapdev_common::kube::KubeServicePort> =
                if let Ok(ports) = serde_json::from_value(workload.ports.clone()) {
                    ports
                } else {
                    vec![]
                };

            result.push(KubeEnvironmentWorkload {
                id: workload.id,
                created_at: workload.created_at,
                environment_id: workload.environment_id,
                base_workload_id: workload.base_workload_id,
                name: workload.name,
                namespace: workload.namespace,
                kind: workload.kind,
                containers,
                ports,
                workload_yaml: workload.workload_yaml,
                catalog_sync_version: workload.catalog_sync_version,
                ready_replicas: workload.ready_replicas,
            });
        }
        Ok(result)
    }

    pub async fn get_environment_workload(
        &self,
        workload_id: Uuid,
    ) -> Result<Option<KubeEnvironmentWorkload>> {
        let workload =
            lapdev_db_entities::kube_environment_workload::Entity::find_by_id(workload_id)
                .filter(lapdev_db_entities::kube_environment_workload::Column::DeletedAt.is_null())
                .one(&self.conn)
                .await?;

        if let Some(workload) = workload {
            let containers: Vec<KubeContainerInfo> =
                if let Ok(containers) = serde_json::from_value(workload.containers.clone()) {
                    containers
                } else {
                    vec![]
                };

            let ports: Vec<lapdev_common::kube::KubeServicePort> =
                if let Ok(ports) = serde_json::from_value(workload.ports.clone()) {
                    ports
                } else {
                    vec![]
                };

            Ok(Some(KubeEnvironmentWorkload {
                id: workload.id,
                created_at: workload.created_at,
                environment_id: workload.environment_id,
                base_workload_id: workload.base_workload_id,
                name: workload.name,
                namespace: workload.namespace,
                kind: workload.kind,
                containers,
                ports,
                workload_yaml: workload.workload_yaml,
                catalog_sync_version: workload.catalog_sync_version,
                ready_replicas: workload.ready_replicas,
            }))
        } else {
            Ok(None)
        }
    }

    pub async fn get_environment_workload_labels(
        &self,
        workload_id: Uuid,
    ) -> Result<BTreeMap<String, String>> {
        let rows = kube_environment_workload_label::Entity::find()
            .filter(kube_environment_workload_label::Column::WorkloadId.eq(workload_id))
            .filter(kube_environment_workload_label::Column::DeletedAt.is_null())
            .all(&self.conn)
            .await?;

        let mut map: BTreeMap<String, String> = BTreeMap::new();

        for row in rows {
            map.insert(row.label_key, row.label_value);
        }

        Ok(map)
    }

    pub async fn get_workloads_by_base_workload_id(
        &self,
        base_workload_id: Uuid,
    ) -> Result<Vec<KubeEnvironmentWorkload>> {
        let workloads = lapdev_db_entities::kube_environment_workload::Entity::find()
            .filter(
                lapdev_db_entities::kube_environment_workload::Column::BaseWorkloadId
                    .eq(base_workload_id),
            )
            .filter(lapdev_db_entities::kube_environment_workload::Column::DeletedAt.is_null())
            .all(&self.conn)
            .await?;

        let mut result = Vec::new();

        for workload in workloads {
            let containers: Vec<KubeContainerInfo> =
                if let Ok(containers) = serde_json::from_value(workload.containers.clone()) {
                    containers
                } else {
                    vec![]
                };

            let ports: Vec<lapdev_common::kube::KubeServicePort> =
                if let Ok(ports) = serde_json::from_value(workload.ports.clone()) {
                    ports
                } else {
                    vec![]
                };

            result.push(KubeEnvironmentWorkload {
                id: workload.id,
                created_at: workload.created_at,
                environment_id: workload.environment_id,
                base_workload_id: workload.base_workload_id,
                name: workload.name,
                namespace: workload.namespace,
                kind: workload.kind,
                containers,
                ports,
                workload_yaml: workload.workload_yaml,
                catalog_sync_version: workload.catalog_sync_version,
                ready_replicas: workload.ready_replicas,
            });
        }

        Ok(result)
    }

    pub async fn delete_environment_workload(&self, workload_id: Uuid) -> Result<()> {
        let timestamp = Utc::now().into();

        lapdev_db_entities::kube_environment_workload::ActiveModel {
            id: ActiveValue::Set(workload_id),
            deleted_at: ActiveValue::Set(Some(timestamp)),
            ..Default::default()
        }
        .update(&self.conn)
        .await?;

        kube_environment_workload_label::Entity::update_many()
            .filter(kube_environment_workload_label::Column::WorkloadId.eq(workload_id))
            .filter(kube_environment_workload_label::Column::DeletedAt.is_null())
            .col_expr(
                kube_environment_workload_label::Column::DeletedAt,
                Expr::value(timestamp),
            )
            .exec(&self.conn)
            .await?;

        Ok(())
    }

    pub async fn update_environment_workload(
        &self,
        workload_id: Uuid,
        containers: Vec<lapdev_common::kube::KubeContainerInfo>,
        workload_yaml: String,
    ) -> Result<lapdev_db_entities::kube_environment_workload::Model> {
        let containers_json = serde_json::to_value(containers)?;
        let updated_model = lapdev_db_entities::kube_environment_workload::ActiveModel {
            id: ActiveValue::Set(workload_id),
            containers: ActiveValue::Set(containers_json),
            workload_yaml: ActiveValue::Set(workload_yaml),
            ..Default::default()
        }
        .update(&self.conn)
        .await?;

        if let Ok(kind) = updated_model.kind.parse::<KubeWorkloadKind>() {
            let labels = labels_from_workload_yaml(&kind, &updated_model.workload_yaml);
            let timestamp = Utc::now().into();
            replace_environment_workload_labels_with_conn(
                &self.conn,
                updated_model.id,
                updated_model.environment_id,
                &labels,
                timestamp,
            )
            .await?;
        }

        Ok(updated_model)
    }

    /// Creates a kube environment and its associated workloads within a single database transaction.
    /// This ensures atomicity - either both operations succeed or both are rolled back.
    pub async fn create_kube_environment(
        &self,
        environment_id: Uuid,
        org_id: Uuid,
        user_id: Uuid,
        app_catalog_id: Uuid,
        cluster_id: Uuid,
        name: String,
        namespace: String,
        status: String,
        is_shared: bool,
        catalog_sync_version: i64,
        base_environment_id: Option<Uuid>,
        workloads: Vec<NewEnvironmentWorkload>,
        services: std::collections::HashMap<String, lapdev_common::kube::KubeServiceWithYaml>,
        auth_token: String,
    ) -> Result<lapdev_db_entities::kube_environment::Model, sea_orm::DbErr> {
        let txn = self.conn.begin().await?;

        let created_at = Utc::now().into();

        // Create the environment
        let environment = lapdev_db_entities::kube_environment::ActiveModel {
            id: ActiveValue::Set(environment_id),
            created_at: ActiveValue::Set(created_at),
            deleted_at: ActiveValue::Set(None),
            organization_id: ActiveValue::Set(org_id),
            user_id: ActiveValue::Set(user_id),
            app_catalog_id: ActiveValue::Set(app_catalog_id),
            cluster_id: ActiveValue::Set(cluster_id),
            name: ActiveValue::Set(name),
            namespace: ActiveValue::Set(namespace.clone()),
            status: ActiveValue::Set(status),
            is_shared: ActiveValue::Set(is_shared),
            catalog_sync_version: ActiveValue::Set(catalog_sync_version),
            last_catalog_synced_at: ActiveValue::Set(Some(created_at)),
            paused_at: ActiveValue::Set(None),
            resumed_at: ActiveValue::Set(None),
            sync_status: ActiveValue::Set("idle".to_string()),
            base_environment_id: ActiveValue::Set(base_environment_id),
            auth_token: ActiveValue::Set(auth_token),
        }
        .insert(&txn)
        .await?;

        // Create all associated workloads
        for workload in workloads {
            let NewEnvironmentWorkload {
                id: new_workload_id,
                details,
            } = workload;
            let KubeWorkloadDetails {
                name,
                namespace: workload_namespace,
                kind,
                containers,
                ports,
                workload_yaml,
                base_workload_id,
            } = details;
            let effective_base_workload_id = base_workload_id.unwrap_or(new_workload_id);

            // Serialize all containers
            let containers_json = serde_json::to_value(&containers)
                .map(Json::from)
                .unwrap_or_else(|_| Json::from(serde_json::json!([])));

            // Serialize ports
            let ports_json = serde_json::to_value(&ports)
                .map(Json::from)
                .unwrap_or_else(|_| Json::from(serde_json::json!([])));

            let labels = labels_from_workload_yaml(&kind, &workload_yaml);

            lapdev_db_entities::kube_environment_workload::ActiveModel {
                id: ActiveValue::Set(new_workload_id),
                created_at: ActiveValue::Set(created_at),
                deleted_at: ActiveValue::Set(None),
                environment_id: ActiveValue::Set(environment_id),
                base_workload_id: ActiveValue::Set(Some(effective_base_workload_id)),
                name: ActiveValue::Set(name),
                namespace: ActiveValue::Set(workload_namespace),
                kind: ActiveValue::Set(kind.to_string()),
                containers: ActiveValue::Set(containers_json),
                ports: ActiveValue::Set(ports_json),
                workload_yaml: ActiveValue::Set(workload_yaml.clone()),
                catalog_sync_version: ActiveValue::Set(catalog_sync_version),
                ready_replicas: ActiveValue::Set(None),
            }
            .insert(&txn)
            .await?;

            replace_environment_workload_labels_with_conn(
                &txn,
                new_workload_id,
                environment_id,
                &labels,
                created_at,
            )
            .await?;
        }

        // Create all associated services
        for (service_name, service_with_yaml) in services {
            // Serialize ports
            let ports_json = serde_json::to_value(&service_with_yaml.details.ports)
                .map(Json::from)
                .unwrap_or_else(|_| Json::from(serde_json::json!([])));

            // Serialize selector
            let selector_json = serde_json::to_value(&service_with_yaml.details.selector)
                .map(Json::from)
                .unwrap_or_else(|_| Json::from(serde_json::json!({})));

            lapdev_db_entities::kube_environment_service::ActiveModel {
                id: ActiveValue::Set(Uuid::new_v4()),
                created_at: ActiveValue::Set(created_at),
                deleted_at: ActiveValue::Set(None),
                environment_id: ActiveValue::Set(environment_id),
                name: ActiveValue::Set(service_name),
                namespace: ActiveValue::Set(namespace.clone()),
                yaml: ActiveValue::Set(service_with_yaml.yaml),
                ports: ActiveValue::Set(ports_json),
                selector: ActiveValue::Set(selector_json),
            }
            .insert(&txn)
            .await?;
        }

        // Commit the transaction
        txn.commit().await?;

        Ok(environment)
    }

    pub async fn get_environment_services(
        &self,
        environment_id: Uuid,
    ) -> Result<Vec<lapdev_common::kube::KubeEnvironmentService>> {
        let services = lapdev_db_entities::kube_environment_service::Entity::find()
            .filter(
                lapdev_db_entities::kube_environment_service::Column::EnvironmentId
                    .eq(environment_id),
            )
            .filter(lapdev_db_entities::kube_environment_service::Column::DeletedAt.is_null())
            .all(&self.conn)
            .await?;

        let mut result = Vec::new();
        for service in services {
            let ports: Vec<lapdev_common::kube::KubeServicePort> =
                if let Ok(ports) = serde_json::from_value(service.ports.clone()) {
                    ports
                } else {
                    vec![]
                };

            let selector: std::collections::BTreeMap<String, String> =
                if let Ok(selector) = serde_json::from_value(service.selector.clone()) {
                    selector
                } else {
                    std::collections::BTreeMap::new()
                };

            result.push(lapdev_common::kube::KubeEnvironmentService {
                id: service.id,
                created_at: service.created_at,
                environment_id: service.environment_id,
                name: service.name,
                namespace: service.namespace,
                yaml: service.yaml,
                ports,
                selector,
            });
        }
        Ok(result)
    }

    pub async fn get_kube_environment_service_by_id(
        &self,
        service_id: Uuid,
    ) -> Result<Option<lapdev_common::kube::KubeEnvironmentService>> {
        let service = lapdev_db_entities::kube_environment_service::Entity::find()
            .filter(lapdev_db_entities::kube_environment_service::Column::Id.eq(service_id))
            .filter(lapdev_db_entities::kube_environment_service::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?;

        if let Some(service) = service {
            let ports: Vec<lapdev_common::kube::KubeServicePort> =
                if let Ok(ports) = serde_json::from_value(service.ports.clone()) {
                    ports
                } else {
                    vec![]
                };

            let selector: std::collections::BTreeMap<String, String> =
                serde_json::from_value(service.selector.clone()).unwrap_or_default();

            Ok(Some(lapdev_common::kube::KubeEnvironmentService {
                id: service.id,
                created_at: service.created_at,
                environment_id: service.environment_id,
                name: service.name,
                namespace: service.namespace,
                yaml: service.yaml,
                ports,
                selector,
            }))
        } else {
            Ok(None)
        }
    }

    // Preview URL operations
    pub async fn create_environment_preview_url(
        &self,
        environment_id: Uuid,
        service_id: Uuid,
        user_id: Uuid,
        name: String,
        description: Option<String>,
        port: i32,
        port_name: Option<String>,
        protocol: String,
        access_level: lapdev_common::kube::PreviewUrlAccessLevel,
    ) -> Result<lapdev_db_entities::kube_environment_preview_url::Model, sea_orm::DbErr> {
        let now = Utc::now();
        let preview_url = lapdev_db_entities::kube_environment_preview_url::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            environment_id: ActiveValue::Set(environment_id),
            service_id: ActiveValue::Set(service_id),
            name: ActiveValue::Set(name),
            description: ActiveValue::Set(description),
            port: ActiveValue::Set(port),
            port_name: ActiveValue::Set(port_name),
            protocol: ActiveValue::Set(protocol),
            access_level: ActiveValue::Set(access_level.to_string()),
            created_by: ActiveValue::Set(user_id),
            last_accessed_at: ActiveValue::Set(None),
            metadata: ActiveValue::Set(serde_json::json!({})),
            deleted_at: ActiveValue::Set(None),
            created_at: ActiveValue::Set(now.into()),
            updated_at: ActiveValue::Set(now.into()),
        }
        .insert(&self.conn)
        .await?;
        Ok(preview_url)
    }

    pub async fn get_environment_preview_urls(
        &self,
        environment_id: Uuid,
    ) -> Result<Vec<lapdev_db_entities::kube_environment_preview_url::Model>, sea_orm::DbErr> {
        lapdev_db_entities::kube_environment_preview_url::Entity::find()
            .filter(
                lapdev_db_entities::kube_environment_preview_url::Column::EnvironmentId
                    .eq(environment_id),
            )
            .filter(lapdev_db_entities::kube_environment_preview_url::Column::DeletedAt.is_null())
            .order_by_asc(lapdev_db_entities::kube_environment_preview_url::Column::CreatedAt)
            .all(&self.conn)
            .await
    }

    pub async fn get_environment_preview_url(
        &self,
        preview_url_id: Uuid,
    ) -> Result<Option<lapdev_db_entities::kube_environment_preview_url::Model>, sea_orm::DbErr>
    {
        lapdev_db_entities::kube_environment_preview_url::Entity::find_by_id(preview_url_id)
            .filter(lapdev_db_entities::kube_environment_preview_url::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await
    }

    pub async fn update_environment_preview_url(
        &self,
        preview_url_id: Uuid,
        description: Option<String>,
        access_level: Option<lapdev_common::kube::PreviewUrlAccessLevel>,
    ) -> Result<lapdev_db_entities::kube_environment_preview_url::Model, sea_orm::DbErr> {
        let mut active_model = lapdev_db_entities::kube_environment_preview_url::ActiveModel {
            id: ActiveValue::Set(preview_url_id),
            ..Default::default()
        };

        if let Some(description) = description {
            active_model.description = ActiveValue::Set(Some(description));
        }
        if let Some(access_level) = access_level {
            active_model.access_level = ActiveValue::Set(access_level.to_string());
        }

        active_model.update(&self.conn).await
    }

    pub async fn delete_environment_preview_url(
        &self,
        preview_url_id: Uuid,
    ) -> Result<(), sea_orm::DbErr> {
        lapdev_db_entities::kube_environment_preview_url::ActiveModel {
            id: ActiveValue::Set(preview_url_id),
            deleted_at: ActiveValue::Set(Some(Utc::now().into())),
            ..Default::default()
        }
        .update(&self.conn)
        .await?;
        Ok(())
    }

    pub async fn update_preview_url_last_accessed(
        &self,
        preview_url_id: Uuid,
    ) -> Result<(), sea_orm::DbErr> {
        lapdev_db_entities::kube_environment_preview_url::ActiveModel {
            id: ActiveValue::Set(preview_url_id),
            last_accessed_at: ActiveValue::Set(Some(Utc::now().into())),
            ..Default::default()
        }
        .update(&self.conn)
        .await?;
        Ok(())
    }

    // Methods for preview URL resolution
    pub async fn find_preview_url_by_name(
        &self,
        name: &str,
    ) -> Result<Option<lapdev_db_entities::kube_environment_preview_url::Model>> {
        let preview_url = lapdev_db_entities::kube_environment_preview_url::Entity::find()
            .filter(lapdev_db_entities::kube_environment_preview_url::Column::Name.eq(name))
            .filter(lapdev_db_entities::kube_environment_preview_url::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await
            .map_err(|e| anyhow::anyhow!("Database error: {}", e))?;
        Ok(preview_url)
    }

    pub async fn find_environment_service(
        &self,
        env_id: Uuid,
        service_name: &str,
    ) -> Result<Option<lapdev_db_entities::kube_environment_service::Model>> {
        let service = lapdev_db_entities::kube_environment_service::Entity::find()
            .filter(lapdev_db_entities::kube_environment_service::Column::EnvironmentId.eq(env_id))
            .filter(lapdev_db_entities::kube_environment_service::Column::Name.eq(service_name))
            .filter(lapdev_db_entities::kube_environment_service::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await
            .map_err(|e| anyhow::anyhow!("Database error: {}", e))?;
        Ok(service)
    }

    pub async fn update_preview_url_access(
        &self,
        preview_url_id: Uuid,
    ) -> Result<(), sea_orm::DbErr> {
        lapdev_db_entities::kube_environment_preview_url::ActiveModel {
            id: ActiveValue::Set(preview_url_id),
            last_accessed_at: ActiveValue::Set(Some(Utc::now().into())),
            ..Default::default()
        }
        .update(&self.conn)
        .await?;
        Ok(())
    }

    // Devbox Session Methods

    /// Create or update a devbox session. Revokes any existing active session for the user.
    pub async fn create_or_update_devbox_session(
        &self,
        user_id: Uuid,
        session_id: Uuid,
        session_token: &str,
        device_name: String,
        expires_at: DateTimeWithTimeZone,
    ) -> Result<lapdev_db_entities::kube_devbox_session::Model> {
        use lapdev_common::token::HashedToken;

        // Hash the token
        let token_hash_bytes = HashedToken::hash(session_token);
        let token_hash = hex::encode(&token_hash_bytes);

        // Get first 12 characters (or fewer) as prefix for support tooling
        let token_prefix = session_token.chars().take(12).collect::<String>();

        // Start a transaction to handle the atomic operation
        let txn = self.conn.begin().await?;

        let now: DateTimeWithTimeZone = Utc::now().into();

        // If a session already exists with this session_id, update it in place.
        if let Some(existing) = lapdev_db_entities::kube_devbox_session::Entity::find()
            .filter(lapdev_db_entities::kube_devbox_session::Column::SessionId.eq(session_id))
            .one(&txn)
            .await?
        {
            if existing.user_id != user_id {
                return Err(anyhow!(
                    "session_id is already associated with another user"
                ));
            }

            let mut active: lapdev_db_entities::kube_devbox_session::ActiveModel = existing.into();
            active.session_token_hash = ActiveValue::Set(token_hash.clone());
            active.token_prefix = ActiveValue::Set(token_prefix.clone());
            active.device_name = ActiveValue::Set(device_name.clone());
            active.expires_at = ActiveValue::Set(expires_at);
            active.last_used_at = ActiveValue::Set(now);
            active.revoked_at = ActiveValue::Set(None);
            active.user_id = ActiveValue::Set(user_id);
            let updated = active.update(&txn).await?;
            txn.commit().await?;
            return Ok(updated);
        }

        // Revoke existing active session (if present)
        if let Some(existing) = lapdev_db_entities::kube_devbox_session::Entity::find()
            .filter(lapdev_db_entities::kube_devbox_session::Column::UserId.eq(user_id))
            .filter(lapdev_db_entities::kube_devbox_session::Column::RevokedAt.is_null())
            .one(&txn)
            .await?
        {
            lapdev_db_entities::kube_devbox_session::ActiveModel {
                id: ActiveValue::Set(existing.id),
                revoked_at: ActiveValue::Set(Some(now)),
                ..Default::default()
            }
            .update(&txn)
            .await?;
        }

        // Create new session row
        let session = lapdev_db_entities::kube_devbox_session::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            session_id: ActiveValue::Set(session_id),
            user_id: ActiveValue::Set(user_id),
            session_token_hash: ActiveValue::Set(token_hash),
            token_prefix: ActiveValue::Set(token_prefix),
            device_name: ActiveValue::Set(device_name),
            active_environment_id: ActiveValue::Set(None),
            created_at: ActiveValue::Set(now),
            expires_at: ActiveValue::Set(expires_at),
            last_used_at: ActiveValue::Set(now),
            revoked_at: ActiveValue::Set(None),
        }
        .insert(&txn)
        .await?;

        txn.commit().await?;

        Ok(session)
    }

    /// Get a devbox session by token hash
    pub async fn get_devbox_session_by_token_hash(
        &self,
        session_token: &str,
    ) -> Result<Option<lapdev_db_entities::kube_devbox_session::Model>> {
        use lapdev_common::token::HashedToken;

        let token_hash_bytes = HashedToken::hash(session_token);
        let token_hash = hex::encode(&token_hash_bytes);

        let session = lapdev_db_entities::kube_devbox_session::Entity::find()
            .filter(
                lapdev_db_entities::kube_devbox_session::Column::SessionTokenHash.eq(token_hash),
            )
            .filter(lapdev_db_entities::kube_devbox_session::Column::RevokedAt.is_null())
            .one(&self.conn)
            .await?;

        Ok(session)
    }

    /// Get the active devbox session for a user
    pub async fn get_active_devbox_session(
        &self,
        user_id: Uuid,
    ) -> Result<Option<lapdev_db_entities::kube_devbox_session::Model>> {
        let session = lapdev_db_entities::kube_devbox_session::Entity::find()
            .filter(lapdev_db_entities::kube_devbox_session::Column::UserId.eq(user_id))
            .filter(lapdev_db_entities::kube_devbox_session::Column::RevokedAt.is_null())
            .one(&self.conn)
            .await?;

        Ok(session)
    }

    /// Revoke a devbox session
    pub async fn revoke_devbox_session(&self, session_id: Uuid) -> Result<()> {
        if let Some(existing) = lapdev_db_entities::kube_devbox_session::Entity::find()
            .filter(lapdev_db_entities::kube_devbox_session::Column::SessionId.eq(session_id))
            .one(&self.conn)
            .await?
        {
            lapdev_db_entities::kube_devbox_session::ActiveModel {
                id: ActiveValue::Set(existing.id),
                revoked_at: ActiveValue::Set(Some(Utc::now().into())),
                ..Default::default()
            }
            .update(&self.conn)
            .await?;
        }

        Ok(())
    }

    /// Update the last_used_at timestamp for a devbox session
    pub async fn update_devbox_session_last_used(&self, session_id: Uuid) -> Result<()> {
        if let Some(existing) = lapdev_db_entities::kube_devbox_session::Entity::find()
            .filter(lapdev_db_entities::kube_devbox_session::Column::SessionId.eq(session_id))
            .one(&self.conn)
            .await?
        {
            lapdev_db_entities::kube_devbox_session::ActiveModel {
                id: ActiveValue::Set(existing.id),
                last_used_at: ActiveValue::Set(Utc::now().into()),
                ..Default::default()
            }
            .update(&self.conn)
            .await?;
        }

        Ok(())
    }

    /// Update the device name for a devbox session
    pub async fn update_devbox_session_device_name(
        &self,
        session_id: Uuid,
        device_name: String,
    ) -> Result<()> {
        if let Some(existing) = lapdev_db_entities::kube_devbox_session::Entity::find()
            .filter(lapdev_db_entities::kube_devbox_session::Column::SessionId.eq(session_id))
            .one(&self.conn)
            .await?
        {
            lapdev_db_entities::kube_devbox_session::ActiveModel {
                id: ActiveValue::Set(existing.id),
                device_name: ActiveValue::Set(device_name.into()),
                ..Default::default()
            }
            .update(&self.conn)
            .await?;
        }

        Ok(())
    }

    /// Update the active environment for a devbox session
    pub async fn update_devbox_session_active_environment(
        &self,
        session_id: Uuid,
        environment_id: Option<Uuid>,
    ) -> Result<()> {
        if let Some(existing) = lapdev_db_entities::kube_devbox_session::Entity::find()
            .filter(lapdev_db_entities::kube_devbox_session::Column::SessionId.eq(session_id))
            .one(&self.conn)
            .await?
        {
            lapdev_db_entities::kube_devbox_session::ActiveModel {
                id: ActiveValue::Set(existing.id),
                active_environment_id: ActiveValue::Set(environment_id),
                ..Default::default()
            }
            .update(&self.conn)
            .await?;
        }

        Ok(())
    }

    /// Get a devbox session by ID
    pub async fn get_devbox_session(
        &self,
        session_id: Uuid,
    ) -> Result<Option<lapdev_db_entities::kube_devbox_session::Model>> {
        let session = lapdev_db_entities::kube_devbox_session::Entity::find()
            .filter(lapdev_db_entities::kube_devbox_session::Column::SessionId.eq(session_id))
            .filter(lapdev_db_entities::kube_devbox_session::Column::RevokedAt.is_null())
            .one(&self.conn)
            .await?;

        Ok(session)
    }

    /// Get a devbox session including revoked entries
    pub async fn get_devbox_session_including_revoked(
        &self,
        session_id: Uuid,
    ) -> Result<Option<lapdev_db_entities::kube_devbox_session::Model>> {
        let session = lapdev_db_entities::kube_devbox_session::Entity::find()
            .filter(lapdev_db_entities::kube_devbox_session::Column::SessionId.eq(session_id))
            .one(&self.conn)
            .await?;

        Ok(session)
    }

    /// List devbox sessions for a user (active + historical)
    pub async fn list_devbox_sessions(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<lapdev_db_entities::kube_devbox_session::Model>> {
        let sessions = lapdev_db_entities::kube_devbox_session::Entity::find()
            .filter(lapdev_db_entities::kube_devbox_session::Column::UserId.eq(user_id))
            .order_by_desc(lapdev_db_entities::kube_devbox_session::Column::CreatedAt)
            .all(&self.conn)
            .await?;

        Ok(sessions)
    }

    // Devbox Workload Intercept Methods

    /// Create a new workload intercept
    pub async fn create_workload_intercept(
        &self,
        user_id: Uuid,
        environment_id: Uuid,
        workload_id: Uuid,
        port_mappings: serde_json::Value,
    ) -> Result<lapdev_db_entities::kube_devbox_workload_intercept::Model> {
        use lapdev_db_entities::kube_devbox_workload_intercept;

        let txn = self.conn.begin().await?;

        // Check if an active intercept already exists for this workload
        let existing_active = kube_devbox_workload_intercept::Entity::find()
            .filter(kube_devbox_workload_intercept::Column::UserId.eq(user_id))
            .filter(kube_devbox_workload_intercept::Column::WorkloadId.eq(workload_id))
            .filter(kube_devbox_workload_intercept::Column::StoppedAt.is_null())
            .one(&txn)
            .await?;

        let intercept = if let Some(model) = existing_active {
            let mut active = kube_devbox_workload_intercept::ActiveModel::from(model);
            active.environment_id = ActiveValue::Set(environment_id);
            active.port_mappings = ActiveValue::Set(port_mappings.clone().into());
            active.stopped_at = ActiveValue::Set(None);
            active.update(&txn).await?
        } else {
            let existing_stopped = kube_devbox_workload_intercept::Entity::find()
                .filter(kube_devbox_workload_intercept::Column::UserId.eq(user_id))
                .filter(kube_devbox_workload_intercept::Column::WorkloadId.eq(workload_id))
                .filter(kube_devbox_workload_intercept::Column::StoppedAt.is_not_null())
                .order_by_desc(kube_devbox_workload_intercept::Column::StoppedAt)
                .one(&txn)
                .await?;

            if let Some(model) = existing_stopped {
                let mut active = kube_devbox_workload_intercept::ActiveModel::from(model);
                active.environment_id = ActiveValue::Set(environment_id);
                active.port_mappings = ActiveValue::Set(port_mappings.clone().into());
                active.stopped_at = ActiveValue::Set(None);
                active.update(&txn).await?
            } else {
                kube_devbox_workload_intercept::ActiveModel {
                    id: ActiveValue::Set(Uuid::new_v4()),
                    user_id: ActiveValue::Set(user_id),
                    environment_id: ActiveValue::Set(environment_id),
                    workload_id: ActiveValue::Set(workload_id),
                    port_mappings: ActiveValue::Set(port_mappings.into()),
                    created_at: ActiveValue::Set(Utc::now().into()),
                    stopped_at: ActiveValue::Set(None),
                }
                .insert(&txn)
                .await?
            }
        };

        txn.commit().await?;

        Ok(intercept)
    }

    /// Stop a workload intercept
    pub async fn stop_workload_intercept(
        &self,
        intercept_id: Uuid,
    ) -> Result<lapdev_db_entities::kube_devbox_workload_intercept::Model> {
        use lapdev_db_entities::kube_devbox_workload_intercept;

        let intercept = kube_devbox_workload_intercept::ActiveModel {
            id: ActiveValue::Set(intercept_id),
            stopped_at: ActiveValue::Set(Some(Utc::now().into())),
            ..Default::default()
        }
        .update(&self.conn)
        .await?;

        Ok(intercept)
    }

    /// Get a workload intercept by ID
    pub async fn get_workload_intercept(
        &self,
        intercept_id: Uuid,
    ) -> Result<Option<lapdev_db_entities::kube_devbox_workload_intercept::Model>> {
        use lapdev_db_entities::kube_devbox_workload_intercept;

        let intercept = kube_devbox_workload_intercept::Entity::find_by_id(intercept_id)
            .one(&self.conn)
            .await?;
        Ok(intercept)
    }

    /// Get active workload intercepts for an environment
    pub async fn get_active_intercepts_for_environment(
        &self,
        environment_id: Uuid,
    ) -> Result<Vec<lapdev_db_entities::kube_devbox_workload_intercept::Model>> {
        use lapdev_db_entities::kube_devbox_workload_intercept;

        let intercepts = kube_devbox_workload_intercept::Entity::find()
            .filter(kube_devbox_workload_intercept::Column::EnvironmentId.eq(environment_id))
            .filter(kube_devbox_workload_intercept::Column::StoppedAt.is_null())
            .all(&self.conn)
            .await?;
        Ok(intercepts)
    }

    /// List workload intercepts for an environment (active + historical)
    pub async fn list_workload_intercepts_for_environment(
        &self,
        environment_id: Uuid,
    ) -> Result<Vec<lapdev_db_entities::kube_devbox_workload_intercept::Model>> {
        use lapdev_db_entities::kube_devbox_workload_intercept;

        let intercepts = kube_devbox_workload_intercept::Entity::find()
            .filter(kube_devbox_workload_intercept::Column::EnvironmentId.eq(environment_id))
            .order_by_desc(kube_devbox_workload_intercept::Column::CreatedAt)
            .all(&self.conn)
            .await?;

        Ok(intercepts)
    }
}

fn labels_from_workload_yaml(
    kind: &lapdev_common::kube::KubeWorkloadKind,
    yaml: &str,
) -> BTreeMap<String, String> {
    let value: Value = match serde_yaml::from_str(yaml) {
        Ok(v) => v,
        Err(_) => return BTreeMap::new(),
    };

    let path: &[&str] = match kind {
        lapdev_common::kube::KubeWorkloadKind::Deployment
        | lapdev_common::kube::KubeWorkloadKind::StatefulSet
        | lapdev_common::kube::KubeWorkloadKind::DaemonSet
        | lapdev_common::kube::KubeWorkloadKind::ReplicaSet
        | lapdev_common::kube::KubeWorkloadKind::Job => &["spec", "template", "metadata", "labels"],
        lapdev_common::kube::KubeWorkloadKind::Pod => &["metadata", "labels"],
        lapdev_common::kube::KubeWorkloadKind::CronJob => &[
            "spec",
            "jobTemplate",
            "spec",
            "template",
            "metadata",
            "labels",
        ],
    };

    traverse_yaml(&value, path)
        .map(mapping_to_labels)
        .unwrap_or_default()
}

fn traverse_yaml<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for key in path {
        let mapping = current.as_mapping()?;
        current = mapping.get(&Value::String((*key).to_string()))?;
    }
    Some(current)
}

fn mapping_to_labels(node: &Value) -> BTreeMap<String, String> {
    let mut labels = BTreeMap::new();
    if let Some(mapping) = node.as_mapping() {
        for (key, value) in mapping {
            if let (Value::String(k), Value::String(v)) = (key, value) {
                labels.insert(k.clone(), v.clone());
            }
        }
    }
    labels
}

fn dependencies_from_workload_yaml(
    kind: &lapdev_common::kube::KubeWorkloadKind,
    yaml: &str,
) -> (BTreeSet<String>, BTreeSet<String>) {
    let value: Value = match serde_yaml::from_str(yaml) {
        Ok(v) => v,
        Err(_) => return (BTreeSet::new(), BTreeSet::new()),
    };

    let pod_spec_path: &[&str] = match kind {
        lapdev_common::kube::KubeWorkloadKind::Deployment
        | lapdev_common::kube::KubeWorkloadKind::StatefulSet
        | lapdev_common::kube::KubeWorkloadKind::DaemonSet
        | lapdev_common::kube::KubeWorkloadKind::ReplicaSet
        | lapdev_common::kube::KubeWorkloadKind::Job => &["spec", "template", "spec"],
        lapdev_common::kube::KubeWorkloadKind::Pod => &["spec"],
        lapdev_common::kube::KubeWorkloadKind::CronJob => {
            &["spec", "jobTemplate", "spec", "template", "spec"]
        }
    };

    let pod_spec = match traverse_yaml(&value, pod_spec_path) {
        Some(spec) => spec,
        None => return (BTreeSet::new(), BTreeSet::new()),
    };

    collect_dependencies_from_spec(pod_spec)
}

fn collect_dependencies_from_spec(spec: &Value) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut configmaps = BTreeSet::new();
    let mut secrets = BTreeSet::new();

    let collect_env =
        |container: &Value, configmaps: &mut BTreeSet<String>, secrets: &mut BTreeSet<String>| {
            if let Some(envs) = container.get("env").and_then(|v| v.as_sequence()) {
                for env in envs {
                    if let Some(value_from) = env.get("valueFrom") {
                        if let Some(cfg) = value_from
                            .get("configMapKeyRef")
                            .and_then(|v| v.get("name"))
                            .and_then(|v| v.as_str())
                        {
                            configmaps.insert(cfg.to_string());
                        }
                        if let Some(sec) = value_from
                            .get("secretKeyRef")
                            .and_then(|v| v.get("name"))
                            .and_then(|v| v.as_str())
                        {
                            secrets.insert(sec.to_string());
                        }
                    }
                }
            }

            if let Some(env_from) = container.get("envFrom").and_then(|v| v.as_sequence()) {
                for source in env_from {
                    if let Some(cfg) = source
                        .get("configMapRef")
                        .and_then(|v| v.get("name"))
                        .and_then(|v| v.as_str())
                    {
                        configmaps.insert(cfg.to_string());
                    }
                    if let Some(sec) = source
                        .get("secretRef")
                        .and_then(|v| v.get("name"))
                        .and_then(|v| v.as_str())
                    {
                        secrets.insert(sec.to_string());
                    }
                }
            }
        };

    if let Some(containers) = spec.get("containers").and_then(|v| v.as_sequence()) {
        for container in containers {
            collect_env(container, &mut configmaps, &mut secrets);
        }
    }

    if let Some(init_containers) = spec.get("initContainers").and_then(|v| v.as_sequence()) {
        for container in init_containers {
            collect_env(container, &mut configmaps, &mut secrets);
        }
    }

    if let Some(ephemeral) = spec
        .get("ephemeralContainers")
        .and_then(|v| v.as_sequence())
    {
        for container in ephemeral {
            collect_env(container, &mut configmaps, &mut secrets);
        }
    }

    if let Some(volumes) = spec.get("volumes").and_then(|v| v.as_sequence()) {
        for volume in volumes {
            if let Some(cfg) = volume
                .get("configMap")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
            {
                configmaps.insert(cfg.to_string());
            }
            if let Some(sec) = volume
                .get("secret")
                .and_then(|v| v.get("secretName"))
                .and_then(|v| v.as_str())
            {
                secrets.insert(sec.to_string());
            }
            if let Some(projected_sources) = volume
                .get("projected")
                .and_then(|v| v.get("sources"))
                .and_then(|v| v.as_sequence())
            {
                for source in projected_sources {
                    if let Some(cfg) = source
                        .get("configMap")
                        .and_then(|v| v.get("name"))
                        .and_then(|v| v.as_str())
                    {
                        configmaps.insert(cfg.to_string());
                    }
                    if let Some(sec) = source
                        .get("secret")
                        .and_then(|v| v.get("name"))
                        .and_then(|v| v.as_str())
                    {
                        secrets.insert(sec.to_string());
                    }
                }
            }
        }
    }

    (configmaps, secrets)
}

async fn replace_workload_labels_with_conn<C>(
    conn: &C,
    workload_id: Uuid,
    app_catalog_id: Uuid,
    cluster_id: Uuid,
    namespace: &str,
    labels: &BTreeMap<String, String>,
    timestamp: DateTimeWithTimeZone,
) -> Result<(), sea_orm::DbErr>
where
    C: ConnectionTrait + Send + Sync,
{
    lapdev_db_entities::kube_app_catalog_workload_label::Entity::update_many()
        .filter(
            lapdev_db_entities::kube_app_catalog_workload_label::Column::WorkloadId.eq(workload_id),
        )
        .filter(lapdev_db_entities::kube_app_catalog_workload_label::Column::DeletedAt.is_null())
        .col_expr(
            lapdev_db_entities::kube_app_catalog_workload_label::Column::DeletedAt,
            Expr::value(timestamp),
        )
        .exec(conn)
        .await?;

    for (key, value) in labels {
        lapdev_db_entities::kube_app_catalog_workload_label::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            created_at: ActiveValue::Set(timestamp),
            deleted_at: ActiveValue::Set(None),
            app_catalog_id: ActiveValue::Set(app_catalog_id),
            workload_id: ActiveValue::Set(workload_id),
            cluster_id: ActiveValue::Set(cluster_id),
            namespace: ActiveValue::Set(namespace.to_owned()),
            label_key: ActiveValue::Set(key.clone()),
            label_value: ActiveValue::Set(value.clone()),
        }
        .insert(conn)
        .await?;
    }

    Ok(())
}

async fn replace_environment_workload_labels_with_conn<C>(
    conn: &C,
    workload_id: Uuid,
    environment_id: Uuid,
    labels: &BTreeMap<String, String>,
    timestamp: DateTimeWithTimeZone,
) -> Result<(), sea_orm::DbErr>
where
    C: ConnectionTrait + Send + Sync,
{
    kube_environment_workload_label::Entity::update_many()
        .filter(kube_environment_workload_label::Column::WorkloadId.eq(workload_id))
        .filter(kube_environment_workload_label::Column::DeletedAt.is_null())
        .col_expr(
            kube_environment_workload_label::Column::DeletedAt,
            Expr::value(timestamp),
        )
        .exec(conn)
        .await?;

    for (key, value) in labels {
        kube_environment_workload_label::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            created_at: ActiveValue::Set(timestamp),
            deleted_at: ActiveValue::Set(None),
            environment_id: ActiveValue::Set(environment_id),
            workload_id: ActiveValue::Set(workload_id),
            label_key: ActiveValue::Set(key.clone()),
            label_value: ActiveValue::Set(value.clone()),
        }
        .insert(conn)
        .await?;
    }

    Ok(())
}

async fn replace_workload_dependencies_with_conn<C>(
    conn: &C,
    workload_id: Uuid,
    app_catalog_id: Uuid,
    cluster_id: Uuid,
    namespace: &str,
    configmaps: &BTreeSet<String>,
    secrets: &BTreeSet<String>,
    timestamp: DateTimeWithTimeZone,
) -> Result<(), sea_orm::DbErr>
where
    C: ConnectionTrait + Send + Sync,
{
    kube_app_catalog_workload_dependency::Entity::update_many()
        .filter(kube_app_catalog_workload_dependency::Column::WorkloadId.eq(workload_id))
        .filter(kube_app_catalog_workload_dependency::Column::DeletedAt.is_null())
        .col_expr(
            kube_app_catalog_workload_dependency::Column::DeletedAt,
            Expr::value(timestamp),
        )
        .exec(conn)
        .await?;

    for name in configmaps {
        kube_app_catalog_workload_dependency::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            created_at: ActiveValue::Set(timestamp),
            deleted_at: ActiveValue::Set(None),
            app_catalog_id: ActiveValue::Set(app_catalog_id),
            workload_id: ActiveValue::Set(workload_id),
            cluster_id: ActiveValue::Set(cluster_id),
            namespace: ActiveValue::Set(namespace.to_owned()),
            resource_name: ActiveValue::Set(name.clone()),
            resource_type: ActiveValue::Set("configmap".to_string()),
        }
        .insert(conn)
        .await?;
    }

    for name in secrets {
        kube_app_catalog_workload_dependency::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            created_at: ActiveValue::Set(timestamp),
            deleted_at: ActiveValue::Set(None),
            app_catalog_id: ActiveValue::Set(app_catalog_id),
            workload_id: ActiveValue::Set(workload_id),
            cluster_id: ActiveValue::Set(cluster_id),
            namespace: ActiveValue::Set(namespace.to_owned()),
            resource_name: ActiveValue::Set(name.clone()),
            resource_type: ActiveValue::Set("secret".to_string()),
        }
        .insert(conn)
        .await?;
    }

    Ok(())
}

async fn replace_service_selectors_with_conn<C>(
    conn: &C,
    service_id: Uuid,
    cluster_id: Uuid,
    namespace: &str,
    selectors: &BTreeMap<String, String>,
    timestamp: DateTimeWithTimeZone,
) -> Result<(), sea_orm::DbErr>
where
    C: ConnectionTrait + Send + Sync,
{
    kube_cluster_service_selector::Entity::update_many()
        .filter(kube_cluster_service_selector::Column::ServiceId.eq(service_id))
        .filter(kube_cluster_service_selector::Column::DeletedAt.is_null())
        .col_expr(
            kube_cluster_service_selector::Column::DeletedAt,
            Expr::value(timestamp),
        )
        .exec(conn)
        .await?;

    for (key, value) in selectors {
        kube_cluster_service_selector::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            created_at: ActiveValue::Set(timestamp),
            deleted_at: ActiveValue::Set(None),
            cluster_id: ActiveValue::Set(cluster_id),
            namespace: ActiveValue::Set(namespace.to_owned()),
            service_id: ActiveValue::Set(service_id),
            label_key: ActiveValue::Set(key.clone()),
            label_value: ActiveValue::Set(value.clone()),
        }
        .insert(conn)
        .await?;
    }

    Ok(())
}

async fn mark_service_selectors_deleted_with_conn<C>(
    conn: &C,
    service_id: Uuid,
    timestamp: DateTimeWithTimeZone,
) -> Result<(), sea_orm::DbErr>
where
    C: ConnectionTrait + Send + Sync,
{
    kube_cluster_service_selector::Entity::update_many()
        .filter(kube_cluster_service_selector::Column::ServiceId.eq(service_id))
        .filter(kube_cluster_service_selector::Column::DeletedAt.is_null())
        .col_expr(
            kube_cluster_service_selector::Column::DeletedAt,
            Expr::value(timestamp),
        )
        .exec(conn)
        .await?;
    Ok(())
}
