use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "kube_app_catalog_workload_label")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub created_at: DateTimeWithTimeZone,
    pub deleted_at: Option<DateTimeWithTimeZone>,
    pub app_catalog_id: Uuid,
    pub workload_id: Uuid,
    pub cluster_id: Uuid,
    pub namespace: String,
    pub label_key: String,
    pub label_value: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::kube_app_catalog::Entity",
        from = "Column::AppCatalogId",
        to = "super::kube_app_catalog::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    KubeAppCatalog,
    #[sea_orm(
        belongs_to = "super::kube_app_catalog_workload::Entity",
        from = "Column::WorkloadId",
        to = "super::kube_app_catalog_workload::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    KubeAppCatalogWorkload,
    #[sea_orm(
        belongs_to = "super::kube_cluster::Entity",
        from = "Column::ClusterId",
        to = "super::kube_cluster::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    KubeCluster,
}

impl Related<super::kube_app_catalog::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::KubeAppCatalog.def()
    }
}

impl Related<super::kube_app_catalog_workload::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::KubeAppCatalogWorkload.def()
    }
}

impl Related<super::kube_cluster::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::KubeCluster.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
