use std::{collections::HashMap, time::Duration};

use anyhow::Result;
use chrono::Utc;
use lapdev_common::WorkspaceStatus;
use lapdev_db::{api::DbApi, entities};
use sea_orm::{
    sea_query::{Expr, Query},
    ColumnTrait, Condition, EntityTrait, QueryFilter,
};

pub struct AutoStartStop {
    db: DbApi,
}

impl AutoStartStop {
    pub fn new(db: DbApi) -> Self {
        Self { db }
    }

    pub async fn get_organization_auto_stop(&self) -> Result<Vec<entities::organization::Model>> {
        let organizations: Vec<entities::organization::Model> =
            entities::organization::Entity::update_many()
                .col_expr(
                    entities::organization::Column::LastAutoStopCheck,
                    Expr::value(Some(Utc::now())),
                )
                .filter(
                    Condition::any().add(
                        entities::organization::Column::Id.in_subquery(
                            Query::select()
                                .column(entities::organization::Column::Id)
                                .from(entities::organization::Entity)
                                .and_where(entities::organization::Column::DeletedAt.is_null())
                                .and_where(
                                    entities::organization::Column::HasRunningWorkspace.eq(true),
                                )
                                .cond_where(
                                    Condition::any()
                                        .add(
                                            entities::organization::Column::LastAutoStopCheck
                                                .lt(Utc::now() - Duration::from_secs(60)),
                                        )
                                        .add(
                                            entities::organization::Column::LastAutoStopCheck
                                                .is_null(),
                                        ),
                                )
                                .order_by(
                                    entities::organization::Column::LastAutoStopCheck,
                                    sea_orm::Order::Asc,
                                )
                                .limit(1)
                                .to_owned(),
                        ),
                    ),
                )
                .exec_with_returning(&self.db.conn)
                .await?;
        Ok(organizations)
    }

    pub async fn organization_auto_stop_workspaces(
        &self,
        org: &entities::organization::Model,
    ) -> Result<Vec<entities::workspace::Model>> {
        let workspaces = match (org.auto_stop, org.allow_workspace_change_auto_stop) {
            (None, false) => {
                // auto stop is not set and we don't allow workspace to set their own
                return Ok(vec![]);
            }
            (Some(timeout), false) => {
                entities::workspace::Entity::find()
                    .filter(
                        entities::workspace::Column::Status
                            .eq(WorkspaceStatus::Running.to_string()),
                    )
                    .filter(entities::workspace::Column::DeletedAt.is_null())
                    .filter(
                        entities::workspace::Column::LastInactivity
                            .lt(Utc::now() - Duration::from_secs(timeout as u64)),
                    )
                    .all(&self.db.conn)
                    .await?
            }
            (_, true) => {
                let mut workspaces = entities::workspace::Entity::find()
                    .filter(
                        entities::workspace::Column::Status
                            .eq(WorkspaceStatus::Running.to_string()),
                    )
                    .filter(entities::workspace::Column::DeletedAt.is_null())
                    .filter(entities::workspace::Column::LastInactivity.is_not_null())
                    .filter(entities::workspace::Column::AutoStop.is_not_null())
                    .all(&self.db.conn)
                    .await?;
                workspaces.retain(|w| {
                    let Some(last_inactivity) = w.last_inactivity else {
                        return false;
                    };

                    let Some(auto_stop) = w.auto_stop else {
                        return false;
                    };

                    (Utc::now() - last_inactivity.with_timezone(&Utc)).num_seconds()
                        > auto_stop as i64
                });
                workspaces
            }
        };

        let services = workspaces
            .iter()
            .filter_map(|ws| {
                if ws.is_compose && ws.compose_parent.is_some() {
                    Some((ws.id, ws.id))
                } else {
                    None
                }
            })
            .collect::<HashMap<_, _>>();

        let mut remaining = Vec::new();
        for ws in workspaces {
            if !ws.is_compose {
                remaining.push(ws);
            } else if ws.compose_parent.is_none() {
                let children = entities::workspace::Entity::find()
                    .filter(entities::workspace::Column::DeletedAt.is_null())
                    .filter(entities::workspace::Column::ComposeParent.eq(ws.id))
                    .all(&self.db.conn)
                    .await?;
                if children.iter().all(|ws| services.contains_key(&ws.id)) {
                    // only stop the workspace if all services had inactivity timeout
                    remaining.push(ws);
                }
            }
        }

        Ok(remaining)
    }

    pub async fn can_workspace_auto_start(&self, ws: &entities::workspace::Model) -> Result<bool> {
        let org = self.db.get_organization(ws.organization_id).await?;
        let auto_start = if org.allow_workspace_change_auto_start {
            ws.auto_start
        } else {
            org.auto_start
        };
        Ok(auto_start)
    }
}

#[cfg(test)]
pub mod tests {
    use std::time::Duration;

    use chrono::Utc;
    use lapdev_common::{utils, WorkspaceHostStatus, WorkspaceStatus};
    use lapdev_db::entities;
    use sea_orm::{ActiveModelTrait, ActiveValue};
    use uuid::Uuid;

    use crate::tests::prepare_enterprise;

    #[tokio::test]
    async fn test_organization_auto_stop_workspaces() {
        let enterprise = prepare_enterprise().await.unwrap();

        let db = enterprise.auto_start_stop.db.clone();

        let user_id = Uuid::new_v4();

        let workspace_host = entities::workspace_host::ActiveModel {
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
        .await
        .unwrap();
        let machine_type = entities::machine_type::ActiveModel {
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
        .await
        .unwrap();

        let org = entities::organization::ActiveModel {
            id: ActiveValue::Set(uuid::Uuid::new_v4()),
            name: ActiveValue::Set("Personal".to_string()),
            auto_start: ActiveValue::Set(true),
            auto_stop: ActiveValue::Set(Some(3600)),
            allow_workspace_change_auto_start: ActiveValue::Set(true),
            allow_workspace_change_auto_stop: ActiveValue::Set(false),
            usage_limit: ActiveValue::Set(108000),
            running_workspace_limit: ActiveValue::Set(3),
            has_running_workspace: ActiveValue::Set(false),
            deleted_at: ActiveValue::Set(None),
            last_auto_stop_check: ActiveValue::Set(None),
            max_cpu: ActiveValue::Set(4),
        }
        .insert(&db.conn)
        .await
        .unwrap();

        entities::workspace::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            updated_at: ActiveValue::Set(None),
            deleted_at: ActiveValue::Set(None),
            name: ActiveValue::Set(utils::rand_string(10)),
            created_at: ActiveValue::Set(Utc::now().into()),
            status: ActiveValue::Set(WorkspaceStatus::Running.to_string()),
            repo_url: ActiveValue::Set("".to_string()),
            repo_name: ActiveValue::Set("".to_string()),
            branch: ActiveValue::Set("".to_string()),
            commit: ActiveValue::Set("".to_string()),
            organization_id: ActiveValue::Set(org.id),
            user_id: ActiveValue::Set(user_id),
            project_id: ActiveValue::Set(None),
            host_id: ActiveValue::Set(workspace_host.id),
            osuser: ActiveValue::Set("".to_string()),
            ssh_private_key: ActiveValue::Set("".to_string()),
            ssh_public_key: ActiveValue::Set("".to_string()),
            cores: ActiveValue::Set(serde_json::to_string(&[1, 2]).unwrap()),
            ssh_port: ActiveValue::Set(None),
            ide_port: ActiveValue::Set(None),
            prebuild_id: ActiveValue::Set(None),
            service: ActiveValue::Set(None),
            usage_id: ActiveValue::Set(None),
            machine_type_id: ActiveValue::Set(machine_type.id),
            last_inactivity: ActiveValue::Set(None),
            auto_stop: ActiveValue::Set(None),
            auto_start: ActiveValue::Set(true),
            env: ActiveValue::Set(None),
            build_output: ActiveValue::Set(None),
            is_compose: ActiveValue::Set(false),
            compose_parent: ActiveValue::Set(None),
            pinned: ActiveValue::Set(false),
        }
        .insert(&enterprise.auto_start_stop.db.conn)
        .await
        .unwrap();

        let worksapces = enterprise
            .auto_start_stop
            .organization_auto_stop_workspaces(&org)
            .await
            .unwrap();
        assert!(worksapces.is_empty());

        let ws = entities::workspace::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            updated_at: ActiveValue::Set(None),
            deleted_at: ActiveValue::Set(None),
            name: ActiveValue::Set(utils::rand_string(10)),
            created_at: ActiveValue::Set(Utc::now().into()),
            status: ActiveValue::Set(WorkspaceStatus::Running.to_string()),
            repo_url: ActiveValue::Set("".to_string()),
            repo_name: ActiveValue::Set("".to_string()),
            branch: ActiveValue::Set("".to_string()),
            commit: ActiveValue::Set("".to_string()),
            organization_id: ActiveValue::Set(org.id),
            user_id: ActiveValue::Set(user_id),
            project_id: ActiveValue::Set(None),
            host_id: ActiveValue::Set(workspace_host.id),
            osuser: ActiveValue::Set("".to_string()),
            ssh_private_key: ActiveValue::Set("".to_string()),
            ssh_public_key: ActiveValue::Set("".to_string()),
            cores: ActiveValue::Set(serde_json::to_string(&[1, 2]).unwrap()),
            ssh_port: ActiveValue::Set(None),
            ide_port: ActiveValue::Set(None),
            prebuild_id: ActiveValue::Set(None),
            service: ActiveValue::Set(None),
            usage_id: ActiveValue::Set(None),
            machine_type_id: ActiveValue::Set(machine_type.id),
            last_inactivity: ActiveValue::Set(Some(
                (Utc::now() - Duration::from_secs(3700)).into(),
            )),
            auto_stop: ActiveValue::Set(None),
            auto_start: ActiveValue::Set(true),
            env: ActiveValue::Set(None),
            build_output: ActiveValue::Set(None),
            is_compose: ActiveValue::Set(false),
            compose_parent: ActiveValue::Set(None),
            pinned: ActiveValue::Set(false),
        }
        .insert(&enterprise.auto_start_stop.db.conn)
        .await
        .unwrap();

        let worksapces = enterprise
            .auto_start_stop
            .organization_auto_stop_workspaces(&org)
            .await
            .unwrap();
        assert_eq!(worksapces.len(), 1);

        let org = entities::organization::ActiveModel {
            id: ActiveValue::Set(org.id),
            allow_workspace_change_auto_stop: ActiveValue::Set(true),
            ..Default::default()
        }
        .update(&db.conn)
        .await
        .unwrap();

        let worksapces = enterprise
            .auto_start_stop
            .organization_auto_stop_workspaces(&org)
            .await
            .unwrap();
        assert_eq!(worksapces.len(), 0);

        entities::workspace::ActiveModel {
            id: ActiveValue::Set(ws.id),
            auto_stop: ActiveValue::Set(Some(3600)),
            ..Default::default()
        }
        .update(&db.conn)
        .await
        .unwrap();

        let worksapces = enterprise
            .auto_start_stop
            .organization_auto_stop_workspaces(&org)
            .await
            .unwrap();
        assert_eq!(worksapces.len(), 1);
    }
}
