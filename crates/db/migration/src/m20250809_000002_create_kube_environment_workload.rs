use sea_orm_migration::prelude::*;

use crate::m20250809_000001_create_kube_environment::KubeEnvironment;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(KubeEnvironmentWorkload::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(KubeEnvironmentWorkload::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentWorkload::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentWorkload::DeletedAt)
                            .timestamp_with_time_zone(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentWorkload::EnvironmentId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentWorkload::Name)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentWorkload::Namespace)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentWorkload::Kind)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentWorkload::Containers)
                            .json()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentWorkload::Ports)
                            .json()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentWorkload::CatalogSyncVersion)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                KubeEnvironmentWorkload::Table,
                                KubeEnvironmentWorkload::EnvironmentId,
                            )
                            .to(KubeEnvironment::Table, KubeEnvironment::Id),
                    )
                    .to_owned(),
            )
            .await?;

        // Create index for efficient lookups by environment
        manager
            .create_index(
                Index::create()
                    .name("kube_environment_workload_environment_deleted_idx")
                    .table(KubeEnvironmentWorkload::Table)
                    .col(KubeEnvironmentWorkload::EnvironmentId)
                    .col(KubeEnvironmentWorkload::DeletedAt)
                    .to_owned(),
            )
            .await?;

        // Create unique index to prevent duplicate workloads per environment
        manager
            .create_index(
                Index::create()
                    .name("kube_environment_workload_unique_idx")
                    .table(KubeEnvironmentWorkload::Table)
                    .col(KubeEnvironmentWorkload::EnvironmentId)
                    .col(KubeEnvironmentWorkload::Name)
                    .col(KubeEnvironmentWorkload::Namespace)
                    .col(KubeEnvironmentWorkload::Kind)
                    .col(KubeEnvironmentWorkload::DeletedAt)
                    .unique()
                    .nulls_not_distinct()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
pub enum KubeEnvironmentWorkload {
    Table,
    Id,
    CreatedAt,
    DeletedAt,
    EnvironmentId,
    Name,
    Namespace,
    Kind,
    Containers,
    Ports,
    CatalogSyncVersion,
}
