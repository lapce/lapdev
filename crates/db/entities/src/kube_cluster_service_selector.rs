use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "kube_cluster_service_selector")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub created_at: DateTimeWithTimeZone,
    pub deleted_at: Option<DateTimeWithTimeZone>,
    pub cluster_id: Uuid,
    pub namespace: String,
    pub service_id: Uuid,
    pub label_key: String,
    pub label_value: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::kube_cluster_service::Entity",
        from = "Column::ServiceId",
        to = "super::kube_cluster_service::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    KubeClusterService,
    #[sea_orm(
        belongs_to = "super::kube_cluster::Entity",
        from = "Column::ClusterId",
        to = "super::kube_cluster::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    KubeCluster,
}

impl Related<super::kube_cluster_service::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::KubeClusterService.def()
    }
}

impl Related<super::kube_cluster::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::KubeCluster.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
