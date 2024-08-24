use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::Utc;
use lapdev_common::{
    AuthProvider, ProviderUser, UserRole, WorkspaceStatus, LAPDEV_BASE_HOSTNAME,
    LAPDEV_ISOLATE_CONTAINER,
};
use pasetors::{
    keys::{Generate, SymmetricKey},
    version4::V4,
};
use sea_orm::{
    sea_query::{Expr, Func, OnConflict},
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, DatabaseTransaction,
    EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};
use sea_orm_migration::MigratorTrait;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{entities, migration::Migrator};

use super::entities::workspace;

pub const LAPDEV_CLUSTER_NOT_INITIATED: &str = "lapdev-cluster-not-initiated";
const LAPDEV_API_AUTH_TOKEN_KEY: &str = "lapdev-api-auth-token-key";
const LAPDEV_DEFAULT_USAGE_LIMIT: &str = "lapdev-default-org-usage-limit";
const LAPDEV_DEFAULT_RUNNING_WORKSPACE_LIMIT: &str = "lapdev-default-org-running-workspace-limit";

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
        if entities::config::Entity::find()
            .filter(entities::config::Column::Name.eq(LAPDEV_CLUSTER_NOT_INITIATED))
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
            let _ = entities::config::ActiveModel {
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

    pub async fn update_config(&self, name: &str, value: &str) -> Result<entities::config::Model> {
        let model = entities::config::Entity::insert(entities::config::ActiveModel {
            name: ActiveValue::set(name.to_string()),
            value: ActiveValue::set(value.to_string()),
        })
        .on_conflict(
            OnConflict::column(entities::config::Column::Name)
                .update_column(entities::config::Column::Value)
                .to_owned(),
        )
        .exec_with_returning(&self.conn)
        .await?;
        Ok(model)
    }

    pub async fn get_config(&self, name: &str) -> Result<String> {
        let model = entities::config::Entity::find()
            .filter(entities::config::Column::Name.eq(name))
            .one(&self.conn)
            .await?
            .ok_or_else(|| anyhow!("no config found"))?;
        Ok(model.value)
    }

    async fn get_config_in_txn(&self, txn: &DatabaseTransaction, name: &str) -> Result<String> {
        let model = entities::config::Entity::find()
            .filter(entities::config::Column::Name.eq(name))
            .one(txn)
            .await?
            .ok_or_else(|| anyhow!("no config found"))?;
        Ok(model.value)
    }

    pub async fn get_base_hostname(&self) -> Result<String> {
        self.get_config(LAPDEV_BASE_HOSTNAME).await
    }

    pub async fn is_container_isolated(&self) -> Result<bool> {
        Ok(entities::config::Entity::find()
            .filter(entities::config::Column::Name.eq(LAPDEV_ISOLATE_CONTAINER))
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
            entities::workspace::Model,
            Option<entities::workspace_host::Model>,
        )>,
    > {
        let model = workspace::Entity::find()
            .find_also_related(entities::workspace_host::Entity)
            .filter(entities::workspace::Column::UserId.eq(user_id))
            .filter(entities::workspace::Column::OrganizationId.eq(org_id))
            .filter(entities::workspace::Column::DeletedAt.is_null())
            .order_by_asc(entities::workspace::Column::CreatedAt)
            .all(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_workspace(&self, id: Uuid) -> Result<entities::workspace::Model> {
        let model = workspace::Entity::find_by_id(id)
            .filter(entities::workspace::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?
            .ok_or_else(|| anyhow!("no workspace found"))?;
        Ok(model)
    }

    pub async fn get_workspace_by_name(&self, name: &str) -> Result<entities::workspace::Model> {
        let model = workspace::Entity::find()
            .filter(entities::workspace::Column::Name.eq(name))
            .filter(entities::workspace::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?
            .ok_or_else(|| anyhow!("no workspace found"))?;
        Ok(model)
    }

    pub async fn get_running_workspaces_on_host(
        &self,
        ws_host_id: Uuid,
    ) -> Result<Vec<entities::workspace::Model>> {
        let model = workspace::Entity::find()
            .filter(entities::workspace::Column::HostId.eq(ws_host_id))
            .filter(entities::workspace::Column::Status.eq(WorkspaceStatus::Running.to_string()))
            .filter(entities::workspace::Column::DeletedAt.is_null())
            .all(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn validate_ssh_public_key(
        &self,
        user_id: Uuid,
        public_key: &str,
    ) -> Result<entities::ssh_public_key::Model> {
        let model = entities::ssh_public_key::Entity::find()
            .filter(entities::ssh_public_key::Column::UserId.eq(user_id))
            .filter(entities::ssh_public_key::Column::ParsedKey.eq(public_key))
            .filter(entities::ssh_public_key::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?
            .ok_or_else(|| anyhow!("no workspace found"))?;
        Ok(model)
    }

    pub async fn get_organization(&self, id: Uuid) -> Result<entities::organization::Model> {
        let model = entities::organization::Entity::find_by_id(id)
            .filter(entities::organization::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?
            .ok_or_else(|| anyhow!("no organization found"))?;
        Ok(model)
    }

    pub async fn get_organization_member(
        &self,
        user_id: Uuid,
        org_id: Uuid,
    ) -> Result<entities::organization_member::Model> {
        let model = entities::organization_member::Entity::find()
            .filter(entities::organization_member::Column::UserId.eq(user_id))
            .filter(entities::organization_member::Column::OrganizationId.eq(org_id))
            .filter(entities::organization_member::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?
            .ok_or_else(|| anyhow!("no organization member found"))?;
        Ok(model)
    }

    pub async fn get_all_organization_members(
        &self,
        org_id: Uuid,
    ) -> Result<Vec<entities::organization_member::Model>> {
        let models = entities::organization_member::Entity::find()
            .filter(entities::organization_member::Column::OrganizationId.eq(org_id))
            .filter(entities::organization_member::Column::DeletedAt.is_null())
            .all(&self.conn)
            .await?;
        Ok(models)
    }

    pub async fn create_new_organization(
        &self,
        txn: &DatabaseTransaction,
        name: String,
    ) -> Result<entities::organization::Model> {
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

        let org = entities::organization::ActiveModel {
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
    ) -> Result<entities::user::Model> {
        let cluster_admin = if !self.is_cluster_initiated(txn).await {
            entities::config::ActiveModel {
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

        let user = entities::user::ActiveModel {
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
        }
        .insert(txn)
        .await?;

        entities::oauth_connection::ActiveModel {
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
            read_repo: ActiveValue::Set(false),
        }
        .insert(txn)
        .await?;

        entities::organization_member::ActiveModel {
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

    pub async fn get_user(&self, user_id: Uuid) -> Result<Option<entities::user::Model>> {
        let model = entities::user::Entity::find()
            .filter(entities::user::Column::Id.eq(user_id))
            .filter(entities::user::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_oauth(&self, id: Uuid) -> Result<Option<entities::oauth_connection::Model>> {
        let model = entities::oauth_connection::Entity::find()
            .filter(entities::oauth_connection::Column::Id.eq(id))
            .filter(entities::oauth_connection::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_user_all_oauth(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<entities::oauth_connection::Model>> {
        let model = entities::oauth_connection::Entity::find()
            .filter(entities::oauth_connection::Column::UserId.eq(user_id))
            .filter(entities::oauth_connection::Column::DeletedAt.is_null())
            .all(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_user_oauth(
        &self,
        user_id: Uuid,
        provider_name: &str,
    ) -> Result<Option<entities::oauth_connection::Model>> {
        let model = entities::oauth_connection::Entity::find()
            .filter(entities::oauth_connection::Column::UserId.eq(user_id))
            .filter(entities::oauth_connection::Column::Provider.eq(provider_name))
            .filter(entities::oauth_connection::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_user_organizations(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<entities::organization_member::Model>> {
        let model = entities::organization_member::Entity::find()
            .filter(entities::organization_member::Column::UserId.eq(user_id))
            .filter(entities::organization_member::Column::DeletedAt.is_null())
            .order_by_asc(entities::organization_member::Column::CreatedAt)
            .all(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_all_ssh_keys(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<entities::ssh_public_key::Model>> {
        let model = entities::ssh_public_key::Entity::find()
            .filter(entities::ssh_public_key::Column::UserId.eq(user_id))
            .filter(entities::ssh_public_key::Column::DeletedAt.is_null())
            .order_by_asc(entities::ssh_public_key::Column::CreatedAt)
            .all(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_ssh_key(&self, id: Uuid) -> Result<entities::ssh_public_key::Model> {
        let model = entities::ssh_public_key::Entity::find_by_id(id)
            .filter(entities::ssh_public_key::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?
            .ok_or_else(|| anyhow!("no ssh key found"))?;
        Ok(model)
    }

    pub async fn get_project(&self, id: Uuid) -> Result<entities::project::Model> {
        let model = entities::project::Entity::find_by_id(id)
            .filter(entities::project::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?
            .ok_or_else(|| anyhow!("no project found"))?;
        Ok(model)
    }

    pub async fn get_project_by_repo(
        &self,
        org_id: Uuid,
        repo: &str,
    ) -> Result<Option<entities::project::Model>> {
        let model = entities::project::Entity::find()
            .filter(entities::project::Column::OrganizationId.eq(org_id))
            .filter(entities::project::Column::DeletedAt.is_null())
            .filter(
                Expr::expr(Func::lower(entities::project::Column::RepoUrl.into_expr()))
                    .eq(repo.to_lowercase()),
            )
            .one(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_all_projects(&self, org_id: Uuid) -> Result<Vec<entities::project::Model>> {
        let model = entities::project::Entity::find()
            .filter(entities::project::Column::OrganizationId.eq(org_id))
            .filter(entities::project::Column::DeletedAt.is_null())
            .order_by_asc(entities::project::Column::CreatedAt)
            .all(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_prebuild(&self, id: Uuid) -> Result<Option<entities::prebuild::Model>> {
        let model = entities::prebuild::Entity::find_by_id(id)
            .filter(entities::prebuild::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_prebuild_by_branch_and_commit(
        &self,
        project_id: Uuid,
        branch: &str,
        commit: &str,
    ) -> Result<Option<entities::prebuild::Model>> {
        let model = entities::prebuild::Entity::find()
            .filter(entities::prebuild::Column::ProjectId.eq(project_id))
            .filter(entities::prebuild::Column::Branch.eq(branch))
            .filter(entities::prebuild::Column::Commit.eq(commit))
            .filter(entities::prebuild::Column::DeletedAt.is_null())
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
    ) -> Result<Option<entities::prebuild::Model>> {
        let model = entities::prebuild::Entity::find()
            .filter(entities::prebuild::Column::ProjectId.eq(project_id))
            .filter(entities::prebuild::Column::Branch.eq(branch))
            .filter(entities::prebuild::Column::Commit.eq(commit))
            .filter(entities::prebuild::Column::DeletedAt.is_null())
            .lock_exclusive()
            .one(txn)
            .await?;
        Ok(model)
    }

    pub async fn get_prebuilds(&self, project_id: Uuid) -> Result<Vec<entities::prebuild::Model>> {
        let model = entities::prebuild::Entity::find()
            .filter(entities::prebuild::Column::ProjectId.eq(project_id))
            .filter(entities::prebuild::Column::DeletedAt.is_null())
            .order_by_desc(entities::prebuild::Column::CreatedAt)
            .all(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_prebuild_replica(
        &self,
        prebuild_id: Uuid,
        host_id: Uuid,
    ) -> Result<Option<entities::prebuild_replica::Model>> {
        let replica = entities::prebuild_replica::Entity::find()
            .filter(entities::prebuild_replica::Column::DeletedAt.is_null())
            .filter(entities::prebuild_replica::Column::PrebuildId.eq(prebuild_id))
            .filter(entities::prebuild_replica::Column::HostId.eq(host_id))
            .one(&self.conn)
            .await?;
        Ok(replica)
    }

    pub async fn get_all_workspace_hosts(&self) -> Result<Vec<entities::workspace_host::Model>> {
        let model = entities::workspace_host::Entity::find()
            .filter(entities::workspace_host::Column::DeletedAt.is_null())
            .all(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_workspace_host(
        &self,
        id: Uuid,
    ) -> Result<Option<entities::workspace_host::Model>> {
        let model = entities::workspace_host::Entity::find()
            .filter(entities::workspace_host::Column::Id.eq(id))
            .one(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_workspace_port(
        &self,
        ws_id: Uuid,
        port: u16,
    ) -> Result<Option<entities::workspace_port::Model>> {
        let model = entities::workspace_port::Entity::find()
            .filter(entities::workspace_port::Column::WorkspaceId.eq(ws_id))
            .filter(entities::workspace_port::Column::Port.eq(port))
            .one(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_workspace_host_by_host(
        &self,
        host: &str,
    ) -> Result<Option<entities::workspace_host::Model>> {
        let model = entities::workspace_host::Entity::find()
            .filter(entities::workspace_host::Column::Host.eq(host))
            .filter(entities::workspace_host::Column::DeletedAt.is_null())
            .one(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_workspace_host_with_lock(
        &self,
        txn: &DatabaseTransaction,
        id: Uuid,
    ) -> Result<Option<entities::workspace_host::Model>> {
        let model = entities::workspace_host::Entity::find()
            .filter(entities::workspace_host::Column::Id.eq(id))
            .filter(entities::workspace_host::Column::DeletedAt.is_null())
            .lock_exclusive()
            .one(txn)
            .await?;
        Ok(model)
    }

    pub async fn get_machine_type(
        &self,
        id: Uuid,
    ) -> Result<Option<entities::machine_type::Model>> {
        let model = entities::machine_type::Entity::find_by_id(id)
            .one(&self.conn)
            .await?;
        Ok(model)
    }

    pub async fn get_all_machine_types(&self) -> Result<Vec<entities::machine_type::Model>> {
        let models = entities::machine_type::Entity::find()
            .filter(entities::machine_type::Column::DeletedAt.is_null())
            .order_by_desc(entities::machine_type::Column::Shared)
            .order_by_asc(entities::machine_type::Column::Cpu)
            .all(&self.conn)
            .await?;
        Ok(models)
    }
}
