use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::{DateTime, FixedOffset, Utc};
use lapdev_common::{
    config::LAPDEV_CLUSTER_NOT_INITIATED, 
    kube::{CreateKubeClusterResponse, KubeAppCatalog, KubeEnvironment, KubeClusterStatus, PagePaginationParams, PaginatedInfo, PaginatedResult},
    token::PlainToken,
    AuthProvider, ProviderUser, UserRole, WorkspaceStatus,
    LAPDEV_BASE_HOSTNAME, LAPDEV_ISOLATE_CONTAINER,
};
use lapdev_db_migration::Migrator;
use pasetors::{
    keys::{Generate, SymmetricKey},
    version4::V4,
};
use sea_orm::{
    sea_query::{Expr, Func, OnConflict},
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, DatabaseTransaction,
    EntityTrait, QueryFilter, QueryOrder, QuerySelect, PaginatorTrait, TransactionTrait,
};
use secrecy::ExposeSecret;
use sea_orm_migration::MigratorTrait;
use sqlx::PgPool;
use uuid::Uuid;

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

async fn connect_db(conn_url: &str) -> Result<sqlx::PgPool> {
    let pool: sqlx::PgPool = sqlx::pool::PoolOptions::new()
        .max_connections(100)
        .connect(conn_url)
        .await?;
    Ok(pool)
}

impl DbApi {
    pub async fn new(conn_url: &str, no_migration: bool) -> Result<Self> {
        let pool = connect_db(conn_url).await?;
        let conn = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(pool.clone());
        let db = DbApi {
            conn,
            pool: Some(pool),
        };
        if !no_migration {
            db.migrate().await?;
        }
        Ok(db)
    }

    async fn migrate(&self) -> Result<()> {
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
        region: Option<String>,
    ) -> Result<lapdev_db_entities::kube_cluster::Model> {
        use lapdev_db_entities::kube_cluster;
        use sea_orm::ActiveValue;

        let now = chrono::Utc::now().into();
        let model = kube_cluster::ActiveModel {
            id: ActiveValue::Set(id),
            cluster_version: ActiveValue::Set(cluster_version),
            status: ActiveValue::Set(status),
            region: ActiveValue::Set(region),
            last_reported_at: ActiveValue::Set(Some(now)),
            ..Default::default()
        };
        let updated = model.update(&self.conn).await?;
        Ok(updated)
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
    pub async fn create_kube_cluster_with_token(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        name: String,
    ) -> Result<CreateKubeClusterResponse> {
        let cluster_id = Uuid::new_v4();
        let token = PlainToken::generate();
        let hashed_token = token.hashed();
        let token_name = format!("{name}-default");

        // Use a transaction to ensure both cluster and token are created atomically
        let txn = self.conn.begin().await?;

        // Create the cluster
        lapdev_db_entities::kube_cluster::ActiveModel {
            id: ActiveValue::Set(cluster_id),
            created_at: ActiveValue::Set(Utc::now().into()),
            name: ActiveValue::Set(name),
            cluster_version: ActiveValue::Set(None),
            status: ActiveValue::Set(Some(KubeClusterStatus::Provisioning.to_string())),
            region: ActiveValue::Set(None),
            created_by: ActiveValue::Set(user_id),
            organization_id: ActiveValue::Set(org_id),
            deleted_at: ActiveValue::Set(None),
            last_reported_at: ActiveValue::Set(None),
            can_deploy: ActiveValue::Set(true),
        }
        .insert(&txn)
        .await?;

        // Create the cluster token
        lapdev_db_entities::kube_cluster_token::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            created_at: ActiveValue::Set(Utc::now().into()),
            deleted_at: ActiveValue::Set(None),
            last_used_at: ActiveValue::Set(None),
            cluster_id: ActiveValue::Set(cluster_id),
            created_by: ActiveValue::Set(user_id),
            name: ActiveValue::Set(token_name),
            token: ActiveValue::Set(hashed_token.expose_secret().to_vec()),
        }
        .insert(&txn)
        .await?;

        txn.commit().await?;

        Ok(CreateKubeClusterResponse {
            cluster_id,
            token: token.expose_secret().to_string(),
        })
    }

    pub async fn delete_kube_cluster(&self, org_id: Uuid, cluster_id: Uuid) -> Result<()> {
        // Verify cluster belongs to the organization
        let cluster = self
            .get_kube_cluster(cluster_id)
            .await?
            .ok_or_else(|| anyhow!("Cluster not found"))?;

        if cluster.organization_id != org_id {
            return Err(anyhow!("Unauthorized"));
        }

        // Check for dependencies - kube_app_catalog
        let has_app_catalogs = lapdev_db_entities::kube_app_catalog::Entity::find()
            .filter(lapdev_db_entities::kube_app_catalog::Column::ClusterId.eq(cluster_id))
            .filter(lapdev_db_entities::kube_app_catalog::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?
            .is_some();

        if has_app_catalogs {
            return Err(anyhow!(
                "Cannot delete cluster: it has active app catalogs. Please delete them first."
            ));
        }

        // Check for dependencies - kube_environment
        let has_environments = lapdev_db_entities::kube_environment::Entity::find()
            .filter(lapdev_db_entities::kube_environment::Column::ClusterId.eq(cluster_id))
            .filter(lapdev_db_entities::kube_environment::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?
            .is_some();

        if has_environments {
            return Err(anyhow!(
                "Cannot delete cluster: it has active environments. Please delete them first."
            ));
        }

        // Soft delete by setting deleted_at timestamp
        lapdev_db_entities::kube_cluster::ActiveModel {
            id: ActiveValue::Set(cluster_id),
            deleted_at: ActiveValue::Set(Some(Utc::now().into())),
            ..Default::default()
        }
        .update(&self.conn)
        .await?;

        Ok(())
    }

    pub async fn set_cluster_deployable(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
        can_deploy: bool,
    ) -> Result<()> {
        // Find the cluster
        let cluster = lapdev_db_entities::kube_cluster::Entity::find_by_id(cluster_id)
            .one(&self.conn)
            .await?;

        let cluster = cluster.ok_or_else(|| anyhow!("Cluster not found"))?;

        // Verify the cluster belongs to the organization
        if cluster.organization_id != org_id {
            return Err(anyhow!("Unauthorized"));
        }

        // Update the can_deploy field
        let active_model = lapdev_db_entities::kube_cluster::ActiveModel {
            id: ActiveValue::Set(cluster.id),
            can_deploy: ActiveValue::Set(can_deploy),
            ..Default::default()
        };
        
        active_model.update(&self.conn).await?;

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
        resources: String,
    ) -> Result<()> {
        lapdev_db_entities::kube_app_catalog::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            created_at: ActiveValue::Set(Utc::now().into()),
            name: ActiveValue::Set(name),
            description: ActiveValue::Set(description),
            resources: ActiveValue::Set(resources),
            cluster_id: ActiveValue::Set(cluster_id),
            created_by: ActiveValue::Set(user_id),
            organization_id: ActiveValue::Set(org_id),
            deleted_at: ActiveValue::Set(None),
        }
        .insert(&self.conn)
        .await?;

        Ok(())
    }

    pub async fn get_all_app_catalogs_paginated(
        &self,
        org_id: Uuid,
        search: Option<String>,
        pagination: Option<PagePaginationParams>,
    ) -> Result<PaginatedResult<KubeAppCatalog>> {
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
        let total_count = base_query
            .clone()
            .count(&self.conn)
            .await? as usize;

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

        let app_catalogs = catalogs_with_clusters
            .into_iter()
            .filter_map(|(catalog, cluster)| {
                let cluster = cluster?;
                Some(KubeAppCatalog {
                    id: catalog.id,
                    name: catalog.name,
                    description: catalog.description,
                    created_at: catalog.created_at,
                    created_by: catalog.created_by,
                    cluster_id: catalog.cluster_id,
                    cluster_name: cluster.name,
                })
            })
            .collect();

        let total_pages = (total_count + pagination.page_size - 1) / pagination.page_size;

        Ok(PaginatedResult {
            data: app_catalogs,
            pagination_info: PaginatedInfo {
                total_count,
                page: pagination.page,
                page_size: pagination.page_size,
                total_pages,
            },
        })
    }

    pub async fn delete_app_catalog(&self, org_id: Uuid, catalog_id: Uuid) -> Result<()> {
        // Verify catalog belongs to the organization
        let catalog = lapdev_db_entities::kube_app_catalog::Entity::find_by_id(catalog_id)
            .filter(lapdev_db_entities::kube_app_catalog::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?
            .ok_or_else(|| anyhow!("App catalog not found"))?;

        if catalog.organization_id != org_id {
            return Err(anyhow!("Unauthorized"));
        }

        // Check for dependencies - kube_environment (any user's environments using this catalog)
        let has_environments = lapdev_db_entities::kube_environment::Entity::find()
            .filter(lapdev_db_entities::kube_environment::Column::AppCatalogId.eq(catalog_id))
            .filter(lapdev_db_entities::kube_environment::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?
            .is_some();

        if has_environments {
            return Err(anyhow!(
                "Cannot delete app catalog: it has active environments. Please delete them first."
            ));
        }

        // Soft delete the app catalog
        let mut active_catalog: lapdev_db_entities::kube_app_catalog::ActiveModel = catalog.into();
        active_catalog.deleted_at = ActiveValue::Set(Some(Utc::now().into()));

        active_catalog
            .update(&self.conn)
            .await?;

        Ok(())
    }

    // Environment operations
    pub async fn get_all_kube_environments_paginated(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        search: Option<String>,
        pagination: Option<PagePaginationParams>,
    ) -> Result<PaginatedResult<KubeEnvironment>> {
        let pagination = pagination.unwrap_or_default();

        // Build base query - filter by user ID so users only see their own environments
        let mut base_query = lapdev_db_entities::kube_environment::Entity::find()
            .filter(lapdev_db_entities::kube_environment::Column::OrganizationId.eq(org_id))
            .filter(lapdev_db_entities::kube_environment::Column::UserId.eq(user_id))
            .filter(lapdev_db_entities::kube_environment::Column::DeletedAt.is_null());

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
        let total_count = base_query
            .clone()
            .count(&self.conn)
            .await? as usize;

        // Apply pagination
        let offset = (pagination.page.saturating_sub(1)) * pagination.page_size;
        let paginated_query = base_query
            .limit(pagination.page_size as u64)
            .offset(offset as u64)
            .order_by_desc(lapdev_db_entities::kube_environment::Column::OrganizationId)
            .order_by_desc(lapdev_db_entities::kube_environment::Column::UserId)
            .order_by_desc(lapdev_db_entities::kube_environment::Column::DeletedAt)
            .order_by_desc(lapdev_db_entities::kube_environment::Column::CreatedAt);

        let environments_with_catalogs = paginated_query
            .find_also_related(lapdev_db_entities::kube_app_catalog::Entity)
            .all(&self.conn)
            .await?;

        let kube_environments = environments_with_catalogs
            .into_iter()
            .filter_map(|(env, catalog)| {
                let catalog = catalog?;
                Some(KubeEnvironment {
                    id: env.id,
                    name: env.name,
                    namespace: env.namespace,
                    app_catalog_id: env.app_catalog_id,
                    app_catalog_name: catalog.name,
                    status: env.status,
                    created_at: env.created_at.to_string(),
                    created_by: env.created_by,
                })
            })
            .collect();

        let total_pages = (total_count + pagination.page_size - 1) / pagination.page_size;

        Ok(PaginatedResult {
            data: kube_environments,
            pagination_info: PaginatedInfo {
                total_count,
                page: pagination.page,
                page_size: pagination.page_size,
                total_pages,
            },
        })
    }

    pub async fn create_kube_environment(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        app_catalog_id: Uuid,
        cluster_id: Uuid,
        name: String,
        namespace: String,
    ) -> Result<lapdev_db_entities::kube_app_catalog::Model> {
        // Verify app catalog belongs to the organization
        let app_catalog = lapdev_db_entities::kube_app_catalog::Entity::find_by_id(app_catalog_id)
            .filter(lapdev_db_entities::kube_app_catalog::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?
            .ok_or_else(|| anyhow!("App catalog not found"))?;

        if app_catalog.organization_id != org_id {
            return Err(anyhow!("Unauthorized"));
        }

        // Verify cluster belongs to the organization
        let cluster = self
            .get_kube_cluster(cluster_id)
            .await?
            .ok_or_else(|| anyhow!("Cluster not found"))?;

        if cluster.organization_id != org_id {
            return Err(anyhow!("Unauthorized"));
        }

        // Check if the cluster allows deployments
        if !cluster.can_deploy {
            return Err(anyhow!("Deployments are not allowed on this cluster"));
        }

        // Create the kube environment
        lapdev_db_entities::kube_environment::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            created_at: ActiveValue::Set(Utc::now().into()),
            deleted_at: ActiveValue::Set(None),
            organization_id: ActiveValue::Set(org_id),
            created_by: ActiveValue::Set(user_id),
            user_id: ActiveValue::Set(user_id), // Environment belongs to the user who created it
            app_catalog_id: ActiveValue::Set(app_catalog_id),
            cluster_id: ActiveValue::Set(cluster_id),
            name: ActiveValue::Set(name),
            namespace: ActiveValue::Set(namespace),
            status: ActiveValue::Set(Some("Pending".to_string())),
        }
        .insert(&self.conn)
        .await?;

        Ok(app_catalog)
    }
}
