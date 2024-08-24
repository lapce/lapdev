use anyhow::Result;
use chrono::Utc;
use lapdev_common::{
    OrgQuota, OrgQuotaValue, QuotaKind, QuotaLevel, QuotaResult, QuotaValue, WorkspaceStatus,
};
use lapdev_db::{api::DbApi, entities};
use sea_orm::{
    sea_query::OnConflict, ActiveValue, ColumnTrait, Condition, DatabaseTransaction, EntityTrait,
    PaginatorTrait, QueryFilter, QuerySelect,
};
use uuid::Uuid;

use crate::usage::Usage;

pub struct Quota {
    usage: Usage,
    db: DbApi,
}

impl Quota {
    pub fn new(usage: Usage, db: DbApi) -> Self {
        Self { usage, db }
    }

    async fn quota_value(
        &self,
        txn: &DatabaseTransaction,
        kind: &QuotaKind,
        organization: Uuid,
        user: Uuid,
    ) -> Result<QuotaValue> {
        // lock on quota rows so that we don't calculate the existing
        // resources at the same time
        let default_user = Uuid::from_u128(0);
        let quotas = entities::quota::Entity::find()
            .filter(entities::quota::Column::DeletedAt.is_null())
            .filter(entities::quota::Column::Organization.eq(organization))
            .filter(entities::quota::Column::Kind.eq(kind.to_string()))
            .filter(
                Condition::any()
                    .add(entities::quota::Column::User.is_null())
                    .add(entities::quota::Column::User.eq(Some(user)))
                    .add(entities::quota::Column::User.eq(Some(default_user))),
            )
            .lock_exclusive()
            .all(txn)
            .await?;

        let mut value = QuotaValue {
            kind: kind.to_owned(),
            user: 0,
            organization: 0,
        };

        for quota in &quotas {
            if quota.user.is_none() && quota.value > 0 {
                value.organization = quota.value as usize;
            } else if quota.user == Some(user) && quota.value > 0 {
                value.user = quota.value as usize;
            }
        }

        if value.user == 0 {
            // we haven't got a user specific quota, so we do another pass to find
            // default user quota
            for quota in &quotas {
                if quota.user == Some(default_user) && quota.value > 0 {
                    value.user = quota.value as usize;
                }
            }
        }

        Ok(value)
    }

    async fn do_check(
        &self,
        quota: QuotaValue,
        organization: Uuid,
        user: Uuid,
    ) -> Result<Option<QuotaResult>> {
        if quota.user > 0 {
            let n = self
                .get_user_existing(&quota.kind, organization, user)
                .await?;
            if n >= quota.user {
                return Ok(Some(QuotaResult {
                    kind: quota.kind,
                    level: QuotaLevel::User,
                    existing: n,
                    quota: quota.user,
                }));
            }
        }

        if quota.organization > 0 {
            let n = self
                .get_organization_existing(&quota.kind, organization)
                .await?;
            if n >= quota.organization {
                return Ok(Some(QuotaResult {
                    kind: quota.kind,
                    level: QuotaLevel::Organization,
                    existing: n,
                    quota: quota.organization,
                }));
            }
        }

        Ok(None)
    }

    pub(crate) async fn check(
        &self,
        txn: &DatabaseTransaction,
        kind: QuotaKind,
        organization: Uuid,
        user: Uuid,
    ) -> Result<Option<QuotaResult>> {
        let quota = self.quota_value(txn, &kind, organization, user).await?;
        self.do_check(quota, organization, user).await
    }

    async fn update(
        &self,
        txn: &DatabaseTransaction,
        organization: Uuid,
        user: Option<Uuid>,
        kind: &QuotaKind,
        value: usize,
    ) -> Result<entities::quota::Model> {
        let model = entities::quota::ActiveModel {
            id: ActiveValue::set(Uuid::new_v4()),
            created_at: ActiveValue::Set(Utc::now().into()),
            deleted_at: ActiveValue::Set(None),
            kind: ActiveValue::Set(kind.to_string()),
            value: ActiveValue::Set(value as i32),
            organization: ActiveValue::Set(organization),
            user: ActiveValue::Set(user),
        };
        let model = entities::quota::Entity::insert(model)
            .on_conflict(
                OnConflict::columns([
                    entities::quota::Column::DeletedAt,
                    entities::quota::Column::Organization,
                    entities::quota::Column::User,
                    entities::quota::Column::Kind,
                ])
                .update_column(entities::quota::Column::Value)
                .to_owned(),
            )
            .exec_with_returning(txn)
            .await?;
        Ok(model)
    }

    pub async fn update_for_organization(
        &self,
        txn: &DatabaseTransaction,
        organization: Uuid,
        kind: QuotaKind,
        value: usize,
    ) -> Result<entities::quota::Model> {
        self.update(txn, organization, None, &kind, value).await
    }

    pub async fn update_for_default_user(
        &self,
        txn: &DatabaseTransaction,
        organization: Uuid,
        kind: QuotaKind,
        value: usize,
    ) -> Result<entities::quota::Model> {
        self.update(txn, organization, Some(Uuid::from_u128(0)), &kind, value)
            .await
    }

    pub async fn update_for_user(
        &self,
        txn: &DatabaseTransaction,
        organization: Uuid,
        user: Uuid,
        kind: QuotaKind,
        value: usize,
    ) -> Result<entities::quota::Model> {
        self.update(txn, organization, Some(user), &kind, value)
            .await
    }

    async fn get_user_existing(
        &self,
        kind: &QuotaKind,
        organization: Uuid,
        user: Uuid,
    ) -> Result<usize> {
        let existing = match kind {
            QuotaKind::Workspace => {
                entities::workspace::Entity::find()
                    .filter(entities::workspace::Column::DeletedAt.is_null())
                    .filter(entities::workspace::Column::OrganizationId.eq(organization))
                    .filter(entities::workspace::Column::UserId.eq(user))
                    .count(&self.db.conn)
                    .await? as usize
            }
            QuotaKind::RunningWorkspace => {
                entities::workspace::Entity::find()
                    .filter(entities::workspace::Column::DeletedAt.is_null())
                    .filter(entities::workspace::Column::OrganizationId.eq(organization))
                    .filter(entities::workspace::Column::UserId.eq(user))
                    .filter(
                        entities::workspace::Column::Status
                            .ne(WorkspaceStatus::Stopped.to_string()),
                    )
                    .filter(entities::workspace::Column::ComposeParent.is_null())
                    .count(&self.db.conn)
                    .await? as usize
            }
            QuotaKind::Project => 0,
            QuotaKind::DailyCost => {
                self.usage
                    .get_daily_cost(organization, Some(user), Utc::now().into(), None)
                    .await?
            }
            QuotaKind::MonthlyCost => {
                self.usage
                    .get_monthly_cost(organization, Some(user), Utc::now().into(), None)
                    .await?
            }
        };
        Ok(existing)
    }

    pub async fn get_organization_existing(
        &self,
        kind: &QuotaKind,
        organization: Uuid,
    ) -> Result<usize> {
        let existing = match kind {
            QuotaKind::Workspace => {
                entities::workspace::Entity::find()
                    .filter(entities::workspace::Column::DeletedAt.is_null())
                    .filter(entities::workspace::Column::OrganizationId.eq(organization))
                    .count(&self.db.conn)
                    .await? as usize
            }
            QuotaKind::RunningWorkspace => {
                entities::workspace::Entity::find()
                    .filter(entities::workspace::Column::DeletedAt.is_null())
                    .filter(entities::workspace::Column::OrganizationId.eq(organization))
                    .filter(
                        entities::workspace::Column::Status
                            .ne(WorkspaceStatus::Stopped.to_string()),
                    )
                    .filter(entities::workspace::Column::ComposeParent.is_null())
                    .count(&self.db.conn)
                    .await? as usize
            }
            QuotaKind::Project => {
                entities::project::Entity::find()
                    .filter(entities::project::Column::DeletedAt.is_null())
                    .filter(entities::project::Column::OrganizationId.eq(organization))
                    .count(&self.db.conn)
                    .await? as usize
            }
            QuotaKind::DailyCost => {
                self.usage
                    .get_daily_cost(organization, None, Utc::now().into(), None)
                    .await?
                    / 60
                    / 60
            }
            QuotaKind::MonthlyCost => {
                self.usage
                    .get_monthly_cost(organization, None, Utc::now().into(), None)
                    .await?
                    / 60
                    / 60
            }
        };
        Ok(existing)
    }

    pub async fn get_org_quota(&self, organization: Uuid) -> Result<OrgQuota> {
        let org_quota = OrgQuota {
            workspace: self
                .get_org_quota_value(organization, QuotaKind::Workspace)
                .await?,
            running_workspace: self
                .get_org_quota_value(organization, QuotaKind::RunningWorkspace)
                .await?,
            project: self
                .get_org_quota_value(organization, QuotaKind::Project)
                .await?,
            daily_cost: self
                .get_org_quota_value(organization, QuotaKind::DailyCost)
                .await?,
            monthly_cost: self
                .get_org_quota_value(organization, QuotaKind::MonthlyCost)
                .await?,
        };

        Ok(org_quota)
    }

    async fn get_org_quota_value(
        &self,
        organization: Uuid,
        quota_kind: QuotaKind,
    ) -> Result<OrgQuotaValue> {
        let existing = self
            .get_organization_existing(&quota_kind, organization)
            .await?;

        let org_quota = entities::quota::Entity::find()
            .filter(entities::quota::Column::DeletedAt.is_null())
            .filter(entities::quota::Column::Organization.eq(organization))
            .filter(entities::quota::Column::Kind.eq(quota_kind.to_string()))
            .filter(entities::quota::Column::User.is_null())
            .one(&self.db.conn)
            .await?
            .map(|q| q.value as usize)
            .unwrap_or(0);
        let default_user = Uuid::from_u128(0);
        let default_user_quota = entities::quota::Entity::find()
            .filter(entities::quota::Column::DeletedAt.is_null())
            .filter(entities::quota::Column::Organization.eq(organization))
            .filter(entities::quota::Column::Kind.eq(quota_kind.to_string()))
            .filter(entities::quota::Column::User.eq(Some(default_user)))
            .one(&self.db.conn)
            .await?
            .map(|q| q.value as usize)
            .unwrap_or(0);
        Ok(OrgQuotaValue {
            existing,
            org_quota,
            default_user_quota,
        })
    }
}

#[cfg(test)]
pub mod tests {
    use anyhow::Result;
    use chrono::Utc;
    use lapdev_common::{
        utils, AuthProvider, ProviderUser, QuotaKind, QuotaLevel, WorkspaceHostStatus,
        WorkspaceStatus,
    };
    use lapdev_db::{api::DbApi, entities};
    use sea_orm::{ActiveModelTrait, ActiveValue, TransactionTrait};
    use uuid::Uuid;

    use crate::tests::prepare_enterprise;

    async fn insert_workspace_host(db: &DbApi) -> Result<entities::workspace_host::Model> {
        Ok(entities::workspace_host::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            host: ActiveValue::Set("".to_string()),
            port: ActiveValue::Set(6123),
            inter_port: ActiveValue::Set(6122),
            cpu: ActiveValue::Set(12),
            memory: ActiveValue::Set(64),
            disk: ActiveValue::Set(1000),
            status: ActiveValue::Set(WorkspaceHostStatus::Active.to_string()),
            available_shared_cpu: ActiveValue::Set(12),
            available_dedicated_cpu: ActiveValue::Set(12),
            available_memory: ActiveValue::Set(64),
            available_disk: ActiveValue::Set(1000),
            region: ActiveValue::Set("".to_string()),
            zone: ActiveValue::Set("".to_string()),
            ..Default::default()
        }
        .insert(&db.conn)
        .await?)
    }

    async fn insert_machine_type(db: &DbApi) -> Result<entities::machine_type::Model> {
        Ok(entities::machine_type::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            name: ActiveValue::Set("".to_string()),
            shared: ActiveValue::Set(true),
            cpu: ActiveValue::Set(2),
            memory: ActiveValue::Set(8),
            disk: ActiveValue::Set(10),
            cost_per_second: ActiveValue::Set(1),
            ..Default::default()
        }
        .insert(&db.conn)
        .await?)
    }

    async fn insert_workspace(
        db: &DbApi,
        organization: Uuid,
        user: Uuid,
        status: Option<WorkspaceStatus>,
        workspace_host_id: Uuid,
        machine_type_id: Uuid,
    ) -> Result<entities::workspace::Model> {
        let ws = entities::workspace::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            deleted_at: ActiveValue::Set(None),
            name: ActiveValue::Set(utils::rand_string(10)),
            created_at: ActiveValue::Set(Utc::now().into()),
            status: ActiveValue::Set(status.unwrap_or(WorkspaceStatus::New).to_string()),
            repo_url: ActiveValue::Set("".to_string()),
            repo_name: ActiveValue::Set("".to_string()),
            branch: ActiveValue::Set("".to_string()),
            commit: ActiveValue::Set("".to_string()),
            organization_id: ActiveValue::Set(organization),
            user_id: ActiveValue::Set(user),
            project_id: ActiveValue::Set(None),
            host_id: ActiveValue::Set(workspace_host_id),
            osuser: ActiveValue::Set("".to_string()),
            ssh_private_key: ActiveValue::Set("".to_string()),
            ssh_public_key: ActiveValue::Set("".to_string()),
            cores: ActiveValue::Set(serde_json::to_string(&[1, 2])?),
            ssh_port: ActiveValue::Set(None),
            ide_port: ActiveValue::Set(None),
            prebuild_id: ActiveValue::Set(None),
            service: ActiveValue::Set(None),
            usage_id: ActiveValue::Set(None),
            machine_type_id: ActiveValue::Set(machine_type_id),
            last_inactivity: ActiveValue::Set(None),
            auto_stop: ActiveValue::Set(None),
            auto_start: ActiveValue::Set(true),
            env: ActiveValue::Set(None),
            build_output: ActiveValue::Set(None),
            is_compose: ActiveValue::Set(false),
            compose_parent: ActiveValue::Set(None),
        };
        let ws = ws.insert(&db.conn).await?;
        Ok(ws)
    }

    #[tokio::test]
    async fn test_insert_quota() {
        let enterprise = prepare_enterprise().await.unwrap();
        let user = uuid::Uuid::new_v4();
        let organization = uuid::Uuid::new_v4();
        let db = enterprise.quota.db.clone();
        let txn = db.conn.begin().await.unwrap();
        let quota = enterprise
            .quota
            .update_for_user(&txn, organization, user, QuotaKind::Workspace, 10)
            .await
            .unwrap();
        assert_eq!(quota.user, Some(user));
        assert_eq!(quota.organization, organization);
        assert_eq!(quota.kind, QuotaKind::Workspace.to_string());
        assert_eq!(quota.value, 10);
    }

    #[tokio::test]
    async fn test_update_quota() {
        let enterprise = prepare_enterprise().await.unwrap();
        let user = uuid::Uuid::new_v4();
        let organization = uuid::Uuid::new_v4();
        let db = enterprise.quota.db.clone();
        let txn = db.conn.begin().await.unwrap();
        let quota = enterprise
            .quota
            .update_for_user(&txn, organization, user, QuotaKind::Workspace, 10)
            .await
            .unwrap();
        assert_eq!(quota.user, Some(user));
        assert_eq!(quota.organization, organization);
        assert_eq!(quota.kind, QuotaKind::Workspace.to_string());
        assert_eq!(quota.value, 10);

        let quota = enterprise
            .quota
            .update_for_user(&txn, organization, user, QuotaKind::Workspace, 20)
            .await
            .unwrap();
        assert_eq!(quota.user, Some(user));
        assert_eq!(quota.organization, organization);
        assert_eq!(quota.kind, QuotaKind::Workspace.to_string());
        assert_eq!(quota.value, 20);
    }

    #[tokio::test]
    async fn test_workspace_quota() {
        let enterprise = prepare_enterprise().await.unwrap();
        let db = enterprise.quota.db.clone();
        let txn = db.conn.begin().await.unwrap();
        let user = db
            .create_new_user(
                &txn,
                &AuthProvider::Github,
                ProviderUser {
                    avatar_url: None,
                    email: None,
                    id: 0,
                    login: "test".to_string(),
                    name: None,
                },
                "".to_string(),
            )
            .await
            .unwrap();
        enterprise
            .quota
            .update_for_user(
                &txn,
                user.current_organization,
                user.id,
                QuotaKind::Workspace,
                10,
            )
            .await
            .unwrap();
        txn.commit().await.unwrap();

        let workspace_host = insert_workspace_host(&db).await.unwrap();
        let machine_type = insert_machine_type(&db).await.unwrap();
        for _ in 0..10 {
            insert_workspace(
                &db,
                user.current_organization,
                user.id,
                None,
                workspace_host.id,
                machine_type.id,
            )
            .await
            .unwrap();
        }
        let txn = db.conn.begin().await.unwrap();
        let quota = enterprise
            .quota
            .quota_value(
                &txn,
                &QuotaKind::Workspace,
                user.current_organization,
                user.id,
            )
            .await
            .unwrap();
        txn.commit().await.unwrap();
        let result = enterprise
            .quota
            .do_check(quota, user.current_organization, user.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.existing, 10);
        assert_eq!(result.quota, 10);
        assert_eq!(result.level, QuotaLevel::User);
        assert_eq!(result.kind, QuotaKind::Workspace);
    }

    #[tokio::test]
    async fn test_workspace_quota_organization_level() {
        let enterprise = prepare_enterprise().await.unwrap();
        let db = enterprise.quota.db.clone();
        let txn = db.conn.begin().await.unwrap();
        let user = db
            .create_new_user(
                &txn,
                &AuthProvider::Github,
                ProviderUser {
                    avatar_url: None,
                    email: None,
                    id: 0,
                    login: "test".to_string(),
                    name: None,
                },
                "".to_string(),
            )
            .await
            .unwrap();
        enterprise
            .quota
            .update_for_user(
                &txn,
                user.current_organization,
                user.id,
                QuotaKind::Workspace,
                10,
            )
            .await
            .unwrap();
        enterprise
            .quota
            .update_for_organization(&txn, user.current_organization, QuotaKind::Workspace, 5)
            .await
            .unwrap();
        txn.commit().await.unwrap();

        let workspace_host = insert_workspace_host(&db).await.unwrap();
        let machine_type = insert_machine_type(&db).await.unwrap();
        for _ in 0..6 {
            insert_workspace(
                &db,
                user.current_organization,
                user.id,
                None,
                workspace_host.id,
                machine_type.id,
            )
            .await
            .unwrap();
        }
        let txn = db.conn.begin().await.unwrap();
        let quota = enterprise
            .quota
            .quota_value(
                &txn,
                &QuotaKind::Workspace,
                user.current_organization,
                user.id,
            )
            .await
            .unwrap();
        txn.commit().await.unwrap();
        let result = enterprise
            .quota
            .do_check(quota, user.current_organization, user.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.existing, 6);
        assert_eq!(result.quota, 5);
        assert_eq!(result.level, QuotaLevel::Organization);
        assert_eq!(result.kind, QuotaKind::Workspace);
    }

    #[tokio::test]
    async fn test_running_workspace_quota() {
        let enterprise = prepare_enterprise().await.unwrap();
        let db = enterprise.quota.db.clone();
        let txn = db.conn.begin().await.unwrap();
        let user = db
            .create_new_user(
                &txn,
                &AuthProvider::Github,
                ProviderUser {
                    avatar_url: None,
                    email: None,
                    id: 0,
                    login: "test".to_string(),
                    name: None,
                },
                "".to_string(),
            )
            .await
            .unwrap();
        enterprise
            .quota
            .update_for_user(
                &txn,
                user.current_organization,
                user.id,
                QuotaKind::RunningWorkspace,
                5,
            )
            .await
            .unwrap();
        enterprise
            .quota
            .update_for_user(
                &txn,
                user.current_organization,
                user.id,
                QuotaKind::Workspace,
                15,
            )
            .await
            .unwrap();
        txn.commit().await.unwrap();

        let workspace_host = insert_workspace_host(&db).await.unwrap();
        let machine_type = insert_machine_type(&db).await.unwrap();
        for _ in 0..5 {
            insert_workspace(
                &db,
                user.current_organization,
                user.id,
                Some(WorkspaceStatus::Running),
                workspace_host.id,
                machine_type.id,
            )
            .await
            .unwrap();
        }
        for _ in 0..5 {
            insert_workspace(
                &db,
                user.current_organization,
                user.id,
                Some(WorkspaceStatus::Stopped),
                workspace_host.id,
                machine_type.id,
            )
            .await
            .unwrap();
        }
        let txn = db.conn.begin().await.unwrap();
        let quota = enterprise
            .quota
            .quota_value(
                &txn,
                &QuotaKind::RunningWorkspace,
                user.current_organization,
                user.id,
            )
            .await
            .unwrap();
        txn.commit().await.unwrap();
        let result = enterprise
            .quota
            .do_check(quota, user.current_organization, user.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.existing, 5);
        assert_eq!(result.quota, 5);
        assert_eq!(result.level, QuotaLevel::User);
        assert_eq!(result.kind, QuotaKind::RunningWorkspace);
    }
}
