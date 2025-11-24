use sea_orm_migration::prelude::*;

use crate::m20250729_082625_create_kube_cluster::KubeCluster;
use crate::m20250801_000000_create_kube_app_catalog::KubeAppCatalog;
use crate::m20250801_000002_create_kube_app_catalog_workload::KubeAppCatalogWorkload;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(KubeAppCatalogWorkloadDependency::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkloadDependency::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkloadDependency::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkloadDependency::DeletedAt)
                            .timestamp_with_time_zone(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkloadDependency::AppCatalogId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkloadDependency::WorkloadId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkloadDependency::ClusterId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkloadDependency::Namespace)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkloadDependency::ResourceName)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkloadDependency::ResourceType)
                            .string_len(32)
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                KubeAppCatalogWorkloadDependency::Table,
                                KubeAppCatalogWorkloadDependency::AppCatalogId,
                            )
                            .to(KubeAppCatalog::Table, KubeAppCatalog::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                KubeAppCatalogWorkloadDependency::Table,
                                KubeAppCatalogWorkloadDependency::WorkloadId,
                            )
                            .to(KubeAppCatalogWorkload::Table, KubeAppCatalogWorkload::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                KubeAppCatalogWorkloadDependency::Table,
                                KubeAppCatalogWorkloadDependency::ClusterId,
                            )
                            .to(KubeCluster::Table, KubeCluster::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("kube_app_catalog_workload_dep_workload_idx")
                    .table(KubeAppCatalogWorkloadDependency::Table)
                    .col(KubeAppCatalogWorkloadDependency::WorkloadId)
                    .col(KubeAppCatalogWorkloadDependency::DeletedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("kube_app_catalog_workload_dep_lookup_idx")
                    .table(KubeAppCatalogWorkloadDependency::Table)
                    .col(KubeAppCatalogWorkloadDependency::ClusterId)
                    .col(KubeAppCatalogWorkloadDependency::Namespace)
                    .col(KubeAppCatalogWorkloadDependency::ResourceType)
                    .col(KubeAppCatalogWorkloadDependency::ResourceName)
                    .col(KubeAppCatalogWorkloadDependency::DeletedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(KubeAppCatalogWorkloadDependency::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
pub enum KubeAppCatalogWorkloadDependency {
    Table,
    Id,
    CreatedAt,
    DeletedAt,
    AppCatalogId,
    WorkloadId,
    ClusterId,
    Namespace,
    ResourceName,
    ResourceType,
}
