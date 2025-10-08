//! `SeaORM` Entity for kube_devbox_workload_intercept

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "kube_devbox_workload_intercept")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub session_id: Uuid,
    pub environment_id: Uuid,
    pub workload_id: Uuid,
    pub port_mappings: Json,
    pub created_at: DateTimeWithTimeZone,
    pub stopped_at: Option<DateTimeWithTimeZone>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::kube_devbox_session::Entity",
        from = "Column::SessionId",
        to = "super::kube_devbox_session::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    Session,
    #[sea_orm(
        belongs_to = "super::kube_environment::Entity",
        from = "Column::EnvironmentId",
        to = "super::kube_environment::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    Environment,
    #[sea_orm(
        belongs_to = "super::kube_environment_workload::Entity",
        from = "Column::WorkloadId",
        to = "super::kube_environment_workload::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    Workload,
}

impl Related<super::kube_devbox_session::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Session.def()
    }
}

impl Related<super::kube_environment::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Environment.def()
    }
}

impl Related<super::kube_environment_workload::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Workload.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
