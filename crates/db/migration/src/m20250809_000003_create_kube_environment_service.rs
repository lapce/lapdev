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
                    .table(KubeEnvironmentService::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(KubeEnvironmentService::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentService::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentService::DeletedAt)
                            .timestamp_with_time_zone(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentService::EnvironmentId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentService::Name)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentService::Namespace)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentService::Yaml)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentService::Ports)
                            .json()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentService::Selector)
                            .json()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                KubeEnvironmentService::Table,
                                KubeEnvironmentService::EnvironmentId,
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
                    .name("kube_environment_service_environment_deleted_idx")
                    .table(KubeEnvironmentService::Table)
                    .col(KubeEnvironmentService::EnvironmentId)
                    .col(KubeEnvironmentService::DeletedAt)
                    .to_owned(),
            )
            .await?;

        // Create unique index to prevent duplicate services per environment
        manager
            .create_index(
                Index::create()
                    .name("kube_environment_service_unique_idx")
                    .table(KubeEnvironmentService::Table)
                    .col(KubeEnvironmentService::EnvironmentId)
                    .col(KubeEnvironmentService::Name)
                    .col(KubeEnvironmentService::Namespace)
                    .col(KubeEnvironmentService::DeletedAt)
                    .unique()
                    .nulls_not_distinct()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(KubeEnvironmentService::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
pub enum KubeEnvironmentService {
    Table,
    Id,
    CreatedAt,
    DeletedAt,
    EnvironmentId,
    Name,
    Namespace,
    Yaml,
    Ports,
    Selector,
}
