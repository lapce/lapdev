use sea_orm_migration::prelude::*;

use crate::m20250820_000001_create_kube_cluster_service::KubeClusterService;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(KubeClusterServiceSelector::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(KubeClusterServiceSelector::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(KubeClusterServiceSelector::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeClusterServiceSelector::DeletedAt)
                            .timestamp_with_time_zone(),
                    )
                    .col(
                        ColumnDef::new(KubeClusterServiceSelector::ClusterId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeClusterServiceSelector::Namespace)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeClusterServiceSelector::ServiceId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeClusterServiceSelector::LabelKey)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeClusterServiceSelector::LabelValue)
                            .string_len(255)
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                KubeClusterServiceSelector::Table,
                                KubeClusterServiceSelector::ServiceId,
                            )
                            .to(KubeClusterService::Table, KubeClusterService::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("kube_cluster_service_selector_service_idx")
                    .table(KubeClusterServiceSelector::Table)
                    .col(KubeClusterServiceSelector::ServiceId)
                    .col(KubeClusterServiceSelector::DeletedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("kube_cluster_service_selector_lookup_idx")
                    .table(KubeClusterServiceSelector::Table)
                    .col(KubeClusterServiceSelector::ClusterId)
                    .col(KubeClusterServiceSelector::Namespace)
                    .col(KubeClusterServiceSelector::LabelKey)
                    .col(KubeClusterServiceSelector::LabelValue)
                    .col(KubeClusterServiceSelector::DeletedAt)
                    .to_owned(),
            )
            .await?;

        // Cover cluster+namespace filtered scans used by get_matching_cluster_services
        manager
            .create_index(
                Index::create()
                    .name("kube_cluster_service_selector_namespace_deleted_idx")
                    .table(KubeClusterServiceSelector::Table)
                    .col(KubeClusterServiceSelector::ClusterId)
                    .col(KubeClusterServiceSelector::Namespace)
                    .col(KubeClusterServiceSelector::DeletedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(KubeClusterServiceSelector::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
pub enum KubeClusterServiceSelector {
    Table,
    Id,
    CreatedAt,
    DeletedAt,
    ClusterId,
    Namespace,
    ServiceId,
    LabelKey,
    LabelValue,
}
