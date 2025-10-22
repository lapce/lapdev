//! `SeaORM` Entity, @generated manually

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "kube_environment_workload_label")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub created_at: DateTimeWithTimeZone,
    pub deleted_at: Option<DateTimeWithTimeZone>,
    pub environment_id: Uuid,
    pub workload_id: Uuid,
    pub label_key: String,
    pub label_value: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::kube_environment::Entity",
        from = "Column::EnvironmentId",
        to = "super::kube_environment::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    KubeEnvironment,
    #[sea_orm(
        belongs_to = "super::kube_environment_workload::Entity",
        from = "Column::WorkloadId",
        to = "super::kube_environment_workload::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    KubeEnvironmentWorkload,
}

impl Related<super::kube_environment::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::KubeEnvironment.def()
    }
}

impl Related<super::kube_environment_workload::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::KubeEnvironmentWorkload.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
