use std::collections::HashMap;

use anyhow::Result;
use chrono::{DateTime, FixedOffset, Utc};
use lapdev_common::{AuditLogRecord, LAPDEV_BASE_HOSTNAME};
use lapdev_common::{AuditLogResult, QuotaKind, QuotaResult};
use lapdev_db::{api::DbApi, entities};
use lapdev_rpc::error::ApiError;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseTransaction, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect,
};
use uuid::Uuid;

use crate::usage::Usage;
use crate::{auto_start_stop::AutoStartStop, license::License, quota::Quota};

const LAPDEV_USAGE_LIMIT_ERROR: &str = "lapdev-usage-limit-error";
const LAPDEV_RUNNING_WORKSPACE_LIMIT_ERROR: &str = "lapdev-running-workspace-limit-error";

pub struct Enterprise {
    pub quota: Quota,
    pub auto_start_stop: AutoStartStop,
    pub license: License,
    pub usage: Usage,
    db: DbApi,
}

impl Enterprise {
    pub async fn new(db: DbApi) -> Result<Self> {
        let license = License::new(db.clone()).await?;
        let usage = Usage::new(db.clone());
        let quota = Quota::new(usage.clone(), db.clone());
        let auto_start_stop = AutoStartStop::new(db.clone());
        Ok(Self {
            quota,
            license,
            auto_start_stop,
            usage,
            db,
        })
    }

    pub async fn has_valid_license(&self) -> bool {
        self.license.has_valid().await
    }

    pub async fn check_organization_limit(
        &self,
        organization: &entities::organization::Model,
        user_id: Uuid,
    ) -> Result<(), ApiError> {
        if !self.license.has_valid().await {
            return Ok(());
        }

        if organization.running_workspace_limit > 0 {
            let count = self
                .quota
                .get_organization_existing(&QuotaKind::RunningWorkspace, organization.id)
                .await?;
            if count as i32 >= organization.running_workspace_limit {
                return Err(ApiError::InvalidRequest(
                    self.db
                        .get_config(LAPDEV_RUNNING_WORKSPACE_LIMIT_ERROR)
                        .await
                        .unwrap_or_else(|_| {
                            "You have reached the running workspace limit".to_string()
                        }),
                ));
            }
        }

        if organization.usage_limit > 0 {
            let usage = self
                .usage
                .get_monthly_cost(organization.id, None, Utc::now().into(), None)
                .await?;
            if usage as i64 >= organization.usage_limit {
                return Err(ApiError::InvalidRequest(
                    self.db
                        .get_config(LAPDEV_USAGE_LIMIT_ERROR)
                        .await
                        .unwrap_or_else(|_| "You have reached the usage limit".to_string()),
                ));
            }

            if organization.running_workspace_limit > 0 {
                let all_orgs = self.db.get_user_organizations(user_id).await?;
                if all_orgs.len() > 1 {
                    let mut personal_usage = 0;
                    for org in all_orgs {
                        let usage = self
                            .usage
                            .get_monthly_cost(
                                org.organization_id,
                                Some(user_id),
                                Utc::now().into(),
                                None,
                            )
                            .await?;
                        personal_usage += usage;
                    }
                    if personal_usage as i64 >= organization.usage_limit {
                        return Err(ApiError::InvalidRequest(
                            "You have reached your personal usage limit".to_string(),
                        ));
                    }
                }
            }
        }

        Ok(())
    }

    pub async fn check_create_workspace_quota(
        &self,
        txn: &DatabaseTransaction,
        organization: Uuid,
        user: Uuid,
    ) -> Result<Option<QuotaResult>> {
        if !self.license.has_valid().await {
            // if no license available, it doesn't support Quota so allow everything
            return Ok(None);
        }

        if let Some(quota) = self
            .quota
            .check(txn, QuotaKind::Workspace, organization, user)
            .await?
        {
            return Ok(Some(quota));
        }

        if let Some(quota) = self
            .quota
            .check(txn, QuotaKind::RunningWorkspace, organization, user)
            .await?
        {
            return Ok(Some(quota));
        }

        if let Some(quota) = self
            .quota
            .check(txn, QuotaKind::DailyCost, organization, user)
            .await?
        {
            return Ok(Some(quota));
        }

        if let Some(quota) = self
            .quota
            .check(txn, QuotaKind::MonthlyCost, organization, user)
            .await?
        {
            return Ok(Some(quota));
        }

        Ok(None)
    }

    pub async fn check_start_workspace_quota(
        &self,
        txn: &DatabaseTransaction,
        organization: Uuid,
        user: Uuid,
    ) -> Result<Option<QuotaResult>> {
        if !self.license.has_valid().await {
            // if no license available, it doesn't support Quota so allow everything
            return Ok(None);
        }

        if let Some(quota) = self
            .quota
            .check(txn, QuotaKind::RunningWorkspace, organization, user)
            .await?
        {
            return Ok(Some(quota));
        }

        if let Some(quota) = self
            .quota
            .check(txn, QuotaKind::DailyCost, organization, user)
            .await?
        {
            return Ok(Some(quota));
        }

        if let Some(quota) = self
            .quota
            .check(txn, QuotaKind::MonthlyCost, organization, user)
            .await?
        {
            return Ok(Some(quota));
        }

        Ok(None)
    }

    pub async fn check_create_project_quota(
        &self,
        txn: &DatabaseTransaction,
        organization: Uuid,
        user: Uuid,
    ) -> Result<Option<QuotaResult>> {
        if !self.license.has_valid().await {
            // if no license available, it doesn't support Quota so allow everything
            return Ok(None);
        }

        if let Some(quota) = self
            .quota
            .check(txn, QuotaKind::Project, organization, user)
            .await?
        {
            return Ok(Some(quota));
        }

        Ok(None)
    }

    pub async fn check_create_prebuild_quota(
        &self,
        txn: &DatabaseTransaction,
        organization: Uuid,
        user: Uuid,
    ) -> Result<Option<QuotaResult>> {
        if !self.license.has_valid().await {
            // if no license available, it doesn't support Quota so allow everything
            return Ok(None);
        }

        if let Some(quota) = self
            .quota
            .check(txn, QuotaKind::DailyCost, organization, user)
            .await?
        {
            return Ok(Some(quota));
        }

        if let Some(quota) = self
            .quota
            .check(txn, QuotaKind::MonthlyCost, organization, user)
            .await?
        {
            return Ok(Some(quota));
        }

        Ok(None)
    }

    pub async fn get_audit_logs(
        &self,
        organization: Uuid,
        start: DateTime<FixedOffset>,
        end: DateTime<FixedOffset>,
        page_size: u64,
        page: u64,
    ) -> Result<AuditLogResult> {
        let result = entities::audit_log::Entity::find()
            .find_also_related(entities::user::Entity)
            .filter(entities::audit_log::Column::OrganizationId.eq(organization))
            .filter(entities::audit_log::Column::Time.gte(start))
            .filter(entities::audit_log::Column::Time.lte(end))
            .order_by_desc(entities::audit_log::Column::Time)
            .paginate(&self.db.conn, page_size);

        let items_and_pages = result.num_items_and_pages().await?;
        let records = result.fetch_page(page).await?;

        let records = records
            .into_iter()
            .map(|(record, user)| AuditLogRecord {
                id: record.id,
                time: record.time,
                user: user.clone().and_then(|user| user.name).unwrap_or_default(),
                avatar: user.and_then(|user| user.avatar_url).unwrap_or_default(),
                resource_kind: record.resource_kind.clone(),
                resource_name: record.resource_name.clone(),
                action: record.action.clone(),
            })
            .collect();

        Ok(AuditLogResult {
            total_items: items_and_pages.number_of_items,
            num_pages: items_and_pages.number_of_pages,
            page,
            page_size,
            records,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn insert_audit_log(
        &self,
        txn: &DatabaseTransaction,
        time: DateTime<FixedOffset>,
        user_id: Uuid,
        org_id: Uuid,
        resource_kind: String,
        resource_id: Uuid,
        resource_name: String,
        action: String,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<entities::audit_log::Model> {
        Ok(entities::audit_log::ActiveModel {
            time: ActiveValue::Set(time),
            user_id: ActiveValue::Set(user_id),
            organization_id: ActiveValue::Set(org_id),
            resource_kind: ActiveValue::Set(resource_kind),
            resource_id: ActiveValue::Set(resource_id),
            resource_name: ActiveValue::Set(resource_name),
            action: ActiveValue::Set(action),
            ip: ActiveValue::Set(ip),
            user_agent: ActiveValue::Set(user_agent),
            ..Default::default()
        }
        .insert(txn)
        .await?)
    }

    pub async fn get_hostnames(&self) -> Result<HashMap<String, String>> {
        let regions: Vec<Option<String>> = entities::workspace_host::Entity::find()
            .filter(entities::workspace_host::Column::DeletedAt.is_null())
            .select_only()
            .column_as(entities::workspace_host::Column::Region, "region")
            .distinct()
            .into_tuple()
            .all(&self.db.conn)
            .await?;
        let regions: Vec<String> = regions
            .into_iter()
            .map(|r| r.map(|r| r.trim().to_string()).unwrap_or_default())
            .collect();
        let mut hostnames: HashMap<String, String> = self
            .db
            .get_config(LAPDEV_BASE_HOSTNAME)
            .await
            .ok()
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default();

        for region in &regions {
            if !hostnames.contains_key(region) {
                hostnames.insert(region.to_owned(), "".to_string());
            }
        }

        if regions.is_empty() || !self.has_valid_license().await {
            hostnames.retain(|key, _| key.is_empty());
        } else {
            hostnames.retain(|key, _| regions.contains(key));
        }

        Ok(hostnames)
    }
}
