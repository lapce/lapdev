use std::collections::HashMap;

use anyhow::{anyhow, Result};
use chrono::{DateTime, Datelike, FixedOffset, TimeZone, Utc};
use lapdev_common::{UsageRecord, UsageResourceKind, UsageResult};
use lapdev_db::api::DbApi;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, Condition, DatabaseTransaction, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect,
};
use uuid::Uuid;

#[derive(Clone)]
pub struct Usage {
    db: DbApi,
}

impl Usage {
    pub fn new(db: DbApi) -> Self {
        Self { db }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn new_usage(
        &self,
        txn: &DatabaseTransaction,
        start: DateTime<FixedOffset>,
        org_id: Uuid,
        user_id: Option<Uuid>,
        resource_kind: UsageResourceKind,
        resource_id: Uuid,
        resource_name: String,
        machine_type_id: Uuid,
        cost_per_second: i32,
    ) -> Result<lapdev_db_entities::usage::Model> {
        let usage = lapdev_db_entities::usage::ActiveModel {
            start: ActiveValue::Set(start),
            organization_id: ActiveValue::Set(org_id),
            user_id: ActiveValue::Set(user_id),
            resource_kind: ActiveValue::Set(resource_kind.to_string()),
            resource_id: ActiveValue::Set(resource_id),
            resource_name: ActiveValue::Set(resource_name),
            cost_per_second: ActiveValue::Set(cost_per_second),
            machine_type_id: ActiveValue::Set(machine_type_id),
            total_cost: ActiveValue::Set(0),
            ..Default::default()
        }
        .insert(txn)
        .await?;
        Ok(usage)
    }

    pub async fn end_usage(
        &self,
        txn: &DatabaseTransaction,
        id: i32,
        end: DateTime<FixedOffset>,
    ) -> Result<lapdev_db_entities::usage::Model> {
        let usage = lapdev_db_entities::usage::Entity::find_by_id(id)
            .one(txn)
            .await?
            .ok_or_else(|| anyhow!("can't find usage"))?;
        let start = usage.start;
        let duration = end - start;
        let duration = duration.num_seconds().max(0) as i32;
        let total_cost = usage.cost_per_second * duration;

        let usage = lapdev_db_entities::usage::ActiveModel {
            id: ActiveValue::Set(id),
            end: ActiveValue::Set(Some(end)),
            duration: ActiveValue::Set(Some(duration)),
            total_cost: ActiveValue::Set(total_cost),
            ..Default::default()
        }
        .update(txn)
        .await?;
        Ok(usage)
    }

    pub async fn get_usage_records(
        &self,
        organization: Uuid,
        user: Option<Uuid>,
        start: DateTime<FixedOffset>,
        end: DateTime<FixedOffset>,
        page_size: u64,
        page: u64,
    ) -> Result<UsageResult> {
        let mut query = lapdev_db_entities::usage::Entity::find()
            .find_also_related(lapdev_db_entities::user::Entity)
            .filter(lapdev_db_entities::usage::Column::OrganizationId.eq(organization));
        if let Some(user) = user {
            query = query.filter(lapdev_db_entities::usage::Column::UserId.eq(user));
        }
        query = query
            .filter(
                Condition::any()
                    .add(
                        Condition::all()
                            .add(lapdev_db_entities::usage::Column::Start.gte(start))
                            .add(lapdev_db_entities::usage::Column::End.lte(end)),
                    )
                    .add(
                        Condition::all()
                            .add(lapdev_db_entities::usage::Column::Start.lt(end))
                            .add(lapdev_db_entities::usage::Column::End.is_null()),
                    )
                    .add(
                        Condition::all()
                            .add(lapdev_db_entities::usage::Column::Start.lt(end))
                            .add(lapdev_db_entities::usage::Column::End.gt(end)),
                    )
                    .add(
                        Condition::all()
                            .add(lapdev_db_entities::usage::Column::Start.lt(start))
                            .add(lapdev_db_entities::usage::Column::End.gt(start)),
                    ),
            )
            .order_by_desc(lapdev_db_entities::usage::Column::Start);

        let result = query.paginate(&self.db.conn, page_size);
        let items_and_pages = result.num_items_and_pages().await?;
        let records = result.fetch_page(page).await?;

        let machine_types = {
            let machine_types = lapdev_db_entities::machine_type::Entity::find()
                .all(&self.db.conn)
                .await
                .unwrap_or_default();
            let machine_types: HashMap<Uuid, lapdev_db_entities::machine_type::Model> =
                machine_types.into_iter().map(|m| (m.id, m)).collect();
            machine_types
        };

        let now = Utc::now().into();

        let records = records
            .into_iter()
            .map(|(record, user)| {
                let machine_type = machine_types
                    .get(&record.machine_type_id)
                    .map(|m| m.name.clone())
                    .unwrap_or_default();

                let usage_start = record.start;
                let usage_start = if usage_start < start {
                    start
                } else {
                    usage_start
                };

                let usage_end = record.end.unwrap_or(now);
                let usage_end = if usage_end > end { end } else { usage_end };
                let duration = usage_end - usage_start;
                let duration = duration.num_seconds().max(0) as usize;
                let cost = duration * record.cost_per_second.max(0) as usize;

                UsageRecord {
                    id: record.id,
                    start: record.start,
                    resource_kind: record.resource_kind.clone(),
                    resource_name: record.resource_name.clone(),
                    machine_type,
                    duration,
                    cost,
                    user: user.clone().and_then(|user| user.name).unwrap_or_default(),
                    avatar: user.and_then(|user| user.avatar_url).unwrap_or_default(),
                }
            })
            .collect();

        let total_cost = self
            .get_cost(organization, user, None, start, end, now)
            .await?;
        Ok(UsageResult {
            total_items: items_and_pages.number_of_items,
            num_pages: items_and_pages.number_of_pages,
            page,
            page_size,
            total_cost,
            records,
        })
    }

    pub async fn get_cost(
        &self,
        organization: Uuid,
        user: Option<Uuid>,
        cost_per_second: Option<i32>,
        start: DateTime<FixedOffset>,
        end: DateTime<FixedOffset>,
        now: DateTime<FixedOffset>,
    ) -> Result<usize> {
        let fixed_cost = {
            let mut query = lapdev_db_entities::usage::Entity::find()
                .filter(lapdev_db_entities::usage::Column::OrganizationId.eq(organization))
                .filter(lapdev_db_entities::usage::Column::End.lte(end))
                .filter(lapdev_db_entities::usage::Column::Start.gte(start));
            if let Some(user) = user {
                query = query.filter(lapdev_db_entities::usage::Column::UserId.eq(user));
            }
            if let Some(cost_per_second) = cost_per_second {
                query = query
                    .filter(lapdev_db_entities::usage::Column::CostPerSecond.eq(cost_per_second));
            }
            let fixed_cost: Vec<Option<i64>> = query
                .select_only()
                .column_as(lapdev_db_entities::usage::Column::TotalCost.sum(), "cost")
                .into_tuple()
                .all(&self.db.conn)
                .await?;
            fixed_cost.first().copied().flatten().unwrap_or(0).max(0) as usize
        };

        let mut query = lapdev_db_entities::usage::Entity::find()
            .filter(lapdev_db_entities::usage::Column::OrganizationId.eq(organization))
            .filter(
                Condition::any()
                    .add(
                        Condition::all()
                            .add(lapdev_db_entities::usage::Column::Start.lt(end))
                            .add(lapdev_db_entities::usage::Column::End.is_null()),
                    )
                    .add(
                        Condition::all()
                            .add(lapdev_db_entities::usage::Column::Start.lt(end))
                            .add(lapdev_db_entities::usage::Column::End.gt(end)),
                    )
                    .add(
                        Condition::all()
                            .add(lapdev_db_entities::usage::Column::Start.lt(start))
                            .add(lapdev_db_entities::usage::Column::End.gt(start)),
                    ),
            );
        if let Some(user) = user {
            query = query.filter(lapdev_db_entities::usage::Column::UserId.eq(user));
        }
        if let Some(cost_per_second) = cost_per_second {
            query =
                query.filter(lapdev_db_entities::usage::Column::CostPerSecond.eq(cost_per_second));
        }

        let usages = query.all(&self.db.conn).await?;

        let mut total_cost = 0;
        for usage in usages {
            let usage_start = usage.start;
            let usage_start = if usage_start < start {
                start
            } else {
                usage_start
            };

            let usage_end = usage.end.unwrap_or(now);
            let usage_end = if usage_end > end { end } else { usage_end };

            let duration = usage_end - usage_start;
            let duration = duration.num_seconds().max(0) as usize;
            let cost = duration * usage.cost_per_second.max(0) as usize;
            total_cost += cost;
        }

        Ok(total_cost + fixed_cost)
    }

    pub async fn get_daily_cost(
        &self,
        organization: Uuid,
        user: Option<Uuid>,
        cost_per_second: Option<i32>,
        date: DateTime<FixedOffset>,
        now: Option<DateTime<FixedOffset>>,
    ) -> Result<usize> {
        let day_start = date
            .timezone()
            .with_ymd_and_hms(date.year(), date.month(), date.day(), 0, 0, 0)
            .single()
            .ok_or_else(|| anyhow!("can't parse date"))?;
        let day_end = day_start + std::time::Duration::from_secs(86400);
        self.get_cost(
            organization,
            user,
            cost_per_second,
            day_start,
            day_end,
            now.unwrap_or_else(|| Utc::now().into()),
        )
        .await
    }

    pub async fn get_monthly_cost(
        &self,
        organization: Uuid,
        user: Option<Uuid>,
        cost_per_second: Option<i32>,
        date: DateTime<FixedOffset>,
        now: Option<DateTime<FixedOffset>>,
    ) -> Result<usize> {
        let year = date.year();
        let month = date.month();
        let month_start = date
            .timezone()
            .with_ymd_and_hms(year, month, 1, 0, 0, 0)
            .single()
            .ok_or_else(|| anyhow!("can't parse date"))?;

        let month_end = {
            let (year, month) = if month == 12 {
                (year + 1, 1)
            } else {
                (year, month + 1)
            };

            date.timezone()
                .with_ymd_and_hms(year, month, 1, 0, 0, 0)
                .single()
                .ok_or_else(|| anyhow!("can't parse date"))?
        };
        self.get_cost(
            organization,
            user,
            cost_per_second,
            month_start,
            month_end,
            now.unwrap_or_else(|| Utc::now().into()),
        )
        .await
    }
}

#[cfg(test)]
pub mod tests {
    use anyhow::Result;
    use chrono::{DateTime, TimeZone, Utc};
    use lapdev_common::{AuthProvider, ProviderUser, UsageResourceKind};
    use lapdev_db::api::DbApi;
    use sea_orm::{ActiveModelTrait, ActiveValue, TransactionTrait};
    use uuid::Uuid;

    use crate::tests::prepare_enterprise;

    async fn insert_machine_type(db: &DbApi) -> Result<lapdev_db_entities::machine_type::Model> {
        Ok(lapdev_db_entities::machine_type::ActiveModel {
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

    async fn insert_usage_record(
        db: &DbApi,
        organization: Uuid,
        user: Option<Uuid>,
        start: DateTime<Utc>,
        end: Option<DateTime<Utc>>,
    ) -> Result<()> {
        let (duration, total_cost) = if let Some(end) = end {
            let duration = end - start;
            let duration = duration.num_seconds().max(0) as i32;
            let total_cost = duration;
            (Some(duration), total_cost)
        } else {
            (None, 0)
        };

        let machine_type = insert_machine_type(db).await.unwrap();
        lapdev_db_entities::usage::ActiveModel {
            start: ActiveValue::Set(start.into()),
            end: ActiveValue::Set(end.map(|end| end.into())),
            organization_id: ActiveValue::Set(organization),
            user_id: ActiveValue::Set(user),
            resource_kind: ActiveValue::Set(UsageResourceKind::Workspace.to_string()),
            resource_id: ActiveValue::Set(Uuid::new_v4()),
            resource_name: ActiveValue::Set("".to_string()),
            cost_per_second: ActiveValue::Set(1),
            duration: ActiveValue::Set(duration),
            total_cost: ActiveValue::Set(total_cost),
            machine_type_id: ActiveValue::Set(machine_type.id),
            ..Default::default()
        }
        .insert(&db.conn)
        .await?;
        Ok(())
    }

    // #[tokio::test]
    // async fn test_get_daily_cost() {
    //     let enterprise = prepare_enterprise().await.unwrap();
    //     let db = enterprise.usage.db.clone();
    //     let organization = uuid::Uuid::new_v4();

    //     let txn = db.conn.begin().await.unwrap();
    //     let user = db
    //         .create_new_user(
    //             &txn,
    //             &AuthProvider::Github,
    //             ProviderUser {
    //                 avatar_url: None,
    //                 email: None,
    //                 id: 0,
    //                 login: "test".to_string(),
    //                 name: None,
    //             },
    //             "".to_string(),
    //         )
    //         .await
    //         .unwrap();
    //     txn.commit().await.unwrap();

    //     insert_usage_record(
    //         &db,
    //         organization,
    //         Some(user.id),
    //         Utc.with_ymd_and_hms(2024, 1, 29, 15, 0, 0).unwrap(),
    //         Some(Utc.with_ymd_and_hms(2024, 1, 29, 16, 0, 0).unwrap()),
    //     )
    //     .await
    //     .unwrap();

    //     let cost = enterprise
    //         .usage
    //         .get_daily_cost(
    //             organization,
    //             Some(user.id),
    //             None,
    //             Utc.with_ymd_and_hms(2024, 1, 29, 1, 0, 0).unwrap().into(),
    //             None,
    //         )
    //         .await
    //         .unwrap();
    //     assert_eq!(cost, 3600);

    //     insert_usage_record(
    //         &db,
    //         organization,
    //         Some(user.id),
    //         Utc.with_ymd_and_hms(2024, 1, 29, 15, 0, 0).unwrap(),
    //         Some(Utc.with_ymd_and_hms(2024, 1, 29, 16, 0, 0).unwrap()),
    //     )
    //     .await
    //     .unwrap();

    //     let cost = enterprise
    //         .usage
    //         .get_daily_cost(
    //             organization,
    //             Some(user.id),
    //             None,
    //             Utc.with_ymd_and_hms(2024, 1, 29, 1, 0, 0).unwrap().into(),
    //             None,
    //         )
    //         .await
    //         .unwrap();
    //     assert_eq!(cost, 7200);

    //     insert_usage_record(
    //         &db,
    //         organization,
    //         Some(user.id),
    //         Utc.with_ymd_and_hms(2024, 1, 29, 15, 0, 0).unwrap(),
    //         None,
    //     )
    //     .await
    //     .unwrap();

    //     let cost = enterprise
    //         .usage
    //         .get_daily_cost(
    //             organization,
    //             Some(user.id),
    //             None,
    //             Utc.with_ymd_and_hms(2024, 1, 29, 1, 0, 0).unwrap().into(),
    //             Some(Utc.with_ymd_and_hms(2024, 1, 29, 15, 1, 0).unwrap().into()),
    //         )
    //         .await
    //         .unwrap();
    //     assert_eq!(cost, 7260);

    //     let cost = enterprise
    //         .usage
    //         .get_daily_cost(
    //             organization,
    //             Some(user.id),
    //             None,
    //             Utc.with_ymd_and_hms(2024, 1, 30, 1, 0, 0).unwrap().into(),
    //             Some(Utc.with_ymd_and_hms(2024, 1, 30, 1, 0, 0).unwrap().into()),
    //         )
    //         .await
    //         .unwrap();
    //     assert_eq!(cost, 3600);

    //     insert_usage_record(
    //         &db,
    //         organization,
    //         Some(user.id),
    //         Utc.with_ymd_and_hms(2024, 1, 29, 23, 0, 0).unwrap(),
    //         Some(Utc.with_ymd_and_hms(2024, 1, 30, 1, 0, 0).unwrap()),
    //     )
    //     .await
    //     .unwrap();

    //     let cost = enterprise
    //         .usage
    //         .get_daily_cost(
    //             organization,
    //             Some(user.id),
    //             None,
    //             Utc.with_ymd_and_hms(2024, 1, 29, 1, 0, 0).unwrap().into(),
    //             Some(Utc.with_ymd_and_hms(2024, 1, 29, 15, 1, 0).unwrap().into()),
    //         )
    //         .await
    //         .unwrap();
    //     assert_eq!(cost, 10860);

    //     insert_usage_record(
    //         &db,
    //         organization,
    //         Some(user.id),
    //         Utc.with_ymd_and_hms(2024, 1, 28, 23, 0, 0).unwrap(),
    //         Some(Utc.with_ymd_and_hms(2024, 1, 30, 1, 0, 0).unwrap()),
    //     )
    //     .await
    //     .unwrap();

    //     let cost = enterprise
    //         .usage
    //         .get_daily_cost(
    //             organization,
    //             Some(user.id),
    //             None,
    //             Utc.with_ymd_and_hms(2024, 1, 29, 1, 0, 0).unwrap().into(),
    //             Some(Utc.with_ymd_and_hms(2024, 1, 29, 15, 1, 0).unwrap().into()),
    //         )
    //         .await
    //         .unwrap();
    //     assert_eq!(cost, 97260);
    // }

    // #[tokio::test]
    // async fn test_get_monthly_cost() {
    //     let enterprise = prepare_enterprise().await.unwrap();
    //     let db = enterprise.usage.db.clone();
    //     let organization = uuid::Uuid::new_v4();

    //     let txn = db.conn.begin().await.unwrap();
    //     let user = db
    //         .create_new_user(
    //             &txn,
    //             &AuthProvider::Github,
    //             ProviderUser {
    //                 avatar_url: None,
    //                 email: None,
    //                 id: 0,
    //                 login: "test".to_string(),
    //                 name: None,
    //             },
    //             "".to_string(),
    //         )
    //         .await
    //         .unwrap();
    //     txn.commit().await.unwrap();

    //     insert_usage_record(
    //         &db,
    //         organization,
    //         Some(user.id),
    //         Utc.with_ymd_and_hms(2024, 1, 29, 15, 0, 0).unwrap(),
    //         Some(Utc.with_ymd_and_hms(2024, 1, 29, 16, 0, 0).unwrap()),
    //     )
    //     .await
    //     .unwrap();

    //     let cost = enterprise
    //         .usage
    //         .get_monthly_cost(
    //             organization,
    //             Some(user.id),
    //             None,
    //             Utc.with_ymd_and_hms(2024, 1, 29, 1, 0, 0).unwrap().into(),
    //             None,
    //         )
    //         .await
    //         .unwrap();
    //     assert_eq!(cost, 3600);

    //     insert_usage_record(
    //         &db,
    //         organization,
    //         Some(user.id),
    //         Utc.with_ymd_and_hms(2023, 12, 31, 23, 0, 0).unwrap(),
    //         Some(Utc.with_ymd_and_hms(2024, 1, 1, 1, 0, 0).unwrap()),
    //     )
    //     .await
    //     .unwrap();

    //     let cost = enterprise
    //         .usage
    //         .get_monthly_cost(
    //             organization,
    //             Some(user.id),
    //             None,
    //             Utc.with_ymd_and_hms(2024, 1, 29, 1, 0, 0).unwrap().into(),
    //             None,
    //         )
    //         .await
    //         .unwrap();
    //     assert_eq!(cost, 7200);

    //     let cost = enterprise
    //         .usage
    //         .get_monthly_cost(
    //             organization,
    //             Some(user.id),
    //             None,
    //             Utc.with_ymd_and_hms(2023, 12, 29, 1, 0, 0).unwrap().into(),
    //             None,
    //         )
    //         .await
    //         .unwrap();
    //     assert_eq!(cost, 3600);
    // }
}
