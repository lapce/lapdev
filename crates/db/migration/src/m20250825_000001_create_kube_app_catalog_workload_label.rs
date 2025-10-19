use sea_orm_migration::prelude::*;

use crate::{
    m20250729_082625_create_kube_cluster::KubeCluster,
    m20250801_000000_create_kube_app_catalog::KubeAppCatalog,
    m20250801_000002_create_kube_app_catalog_workload::KubeAppCatalogWorkload,
};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(KubeAppCatalogWorkloadLabel::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkloadLabel::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkloadLabel::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkloadLabel::DeletedAt)
                            .timestamp_with_time_zone(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkloadLabel::AppCatalogId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkloadLabel::WorkloadId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkloadLabel::ClusterId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkloadLabel::Namespace)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkloadLabel::LabelKey)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkloadLabel::LabelValue)
                            .string_len(255)
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                KubeAppCatalogWorkloadLabel::Table,
                                KubeAppCatalogWorkloadLabel::AppCatalogId,
                            )
                            .to(KubeAppCatalog::Table, KubeAppCatalog::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                KubeAppCatalogWorkloadLabel::Table,
                                KubeAppCatalogWorkloadLabel::WorkloadId,
                            )
                            .to(KubeAppCatalogWorkload::Table, KubeAppCatalogWorkload::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                KubeAppCatalogWorkloadLabel::Table,
                                KubeAppCatalogWorkloadLabel::ClusterId,
                            )
                            .to(KubeCluster::Table, KubeCluster::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("kube_app_catalog_workload_label_workload_idx")
                    .table(KubeAppCatalogWorkloadLabel::Table)
                    .col(KubeAppCatalogWorkloadLabel::WorkloadId)
                    .col(KubeAppCatalogWorkloadLabel::DeletedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("kube_app_catalog_workload_label_selector_idx")
                    .table(KubeAppCatalogWorkloadLabel::Table)
                    .col(KubeAppCatalogWorkloadLabel::ClusterId)
                    .col(KubeAppCatalogWorkloadLabel::Namespace)
                    .col(KubeAppCatalogWorkloadLabel::LabelKey)
                    .col(KubeAppCatalogWorkloadLabel::LabelValue)
                    .col(KubeAppCatalogWorkloadLabel::DeletedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(KubeAppCatalogWorkloadLabel::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
pub enum KubeAppCatalogWorkloadLabel {
    Table,
    Id,
    CreatedAt,
    DeletedAt,
    AppCatalogId,
    WorkloadId,
    ClusterId,
    Namespace,
    LabelKey,
    LabelValue,
}
