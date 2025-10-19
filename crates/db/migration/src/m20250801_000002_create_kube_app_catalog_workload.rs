use sea_orm_migration::prelude::*;

use crate::{
    m20250729_082625_create_kube_cluster::KubeCluster,
    m20250801_000000_create_kube_app_catalog::KubeAppCatalog,
};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(KubeAppCatalogWorkload::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkload::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkload::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkload::DeletedAt)
                            .timestamp_with_time_zone(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkload::AppCatalogId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkload::ClusterId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkload::Name)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkload::Namespace)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkload::Kind)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkload::Containers)
                            .json()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkload::Ports)
                            .json()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalogWorkload::WorkloadYaml)
                            .text()
                            .not_null()
                            .default(""),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                KubeAppCatalogWorkload::Table,
                                KubeAppCatalogWorkload::AppCatalogId,
                            )
                            .to(KubeAppCatalog::Table, KubeAppCatalog::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                KubeAppCatalogWorkload::Table,
                                KubeAppCatalogWorkload::ClusterId,
                            )
                            .to(KubeCluster::Table, KubeCluster::Id),
                    )
                    .to_owned(),
            )
            .await?;

        // Create index for efficient lookups by app catalog
        manager
            .create_index(
                Index::create()
                    .name("kube_app_catalog_workload_app_catalog_deleted_idx")
                    .table(KubeAppCatalogWorkload::Table)
                    .col(KubeAppCatalogWorkload::AppCatalogId)
                    .col(KubeAppCatalogWorkload::DeletedAt)
                    .to_owned(),
            )
            .await?;

        // Cluster scoped index to quickly find workloads by cluster/name/namespace
        manager
            .create_index(
                Index::create()
                    .name("kube_app_catalog_workload_cluster_lookup_idx")
                    .table(KubeAppCatalogWorkload::Table)
                    .col(KubeAppCatalogWorkload::ClusterId)
                    .col(KubeAppCatalogWorkload::Name)
                    .col(KubeAppCatalogWorkload::Namespace)
                    .col(KubeAppCatalogWorkload::DeletedAt)
                    .to_owned(),
            )
            .await?;

        // Create unique index to prevent duplicate workloads per app catalog
        manager
            .create_index(
                Index::create()
                    .name("kube_app_catalog_workload_unique_idx")
                    .table(KubeAppCatalogWorkload::Table)
                    .col(KubeAppCatalogWorkload::AppCatalogId)
                    .col(KubeAppCatalogWorkload::Name)
                    .col(KubeAppCatalogWorkload::Namespace)
                    .col(KubeAppCatalogWorkload::Kind)
                    .col(KubeAppCatalogWorkload::DeletedAt)
                    .unique()
                    .nulls_not_distinct()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
pub enum KubeAppCatalogWorkload {
    Table,
    Id,
    CreatedAt,
    DeletedAt,
    AppCatalogId,
    ClusterId,
    Name,
    Namespace,
    Kind,
    Containers,
    Ports,
    WorkloadYaml,
}
