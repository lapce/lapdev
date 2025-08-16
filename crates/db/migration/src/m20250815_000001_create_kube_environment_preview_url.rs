use sea_orm_migration::prelude::*;

use crate::m20250809_000001_create_kube_environment::KubeEnvironment;
use crate::m20250809_000003_create_kube_environment_service::KubeEnvironmentService;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(KubeEnvironmentPreviewUrl::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(KubeEnvironmentPreviewUrl::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentPreviewUrl::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentPreviewUrl::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentPreviewUrl::DeletedAt)
                            .timestamp_with_time_zone(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentPreviewUrl::EnvironmentId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentPreviewUrl::ServiceId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentPreviewUrl::Name)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(KubeEnvironmentPreviewUrl::Description).text())
                    .col(
                        ColumnDef::new(KubeEnvironmentPreviewUrl::Port)
                            .integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(KubeEnvironmentPreviewUrl::PortName).string())
                    .col(
                        ColumnDef::new(KubeEnvironmentPreviewUrl::Protocol)
                            .string()
                            .not_null()
                            .default("HTTP"),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentPreviewUrl::AccessLevel)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentPreviewUrl::CreatedBy)
                            .uuid()
                            .not_null(),
                    )
                    // Track last access time for analytics and cleanup
                    .col(
                        ColumnDef::new(KubeEnvironmentPreviewUrl::LastAccessedAt)
                            .timestamp_with_time_zone(),
                    )
                    // Metadata for custom headers, rate limiting, etc.
                    .col(
                        ColumnDef::new(KubeEnvironmentPreviewUrl::Metadata)
                            .json()
                            .not_null()
                            .default("{}"),
                    )
                    // Foreign key to environment (for ownership)
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                KubeEnvironmentPreviewUrl::Table,
                                KubeEnvironmentPreviewUrl::EnvironmentId,
                            )
                            .to(KubeEnvironment::Table, KubeEnvironment::Id),
                    )
                    // Foreign key to service
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                KubeEnvironmentPreviewUrl::Table,
                                KubeEnvironmentPreviewUrl::ServiceId,
                            )
                            .to(KubeEnvironmentService::Table, KubeEnvironmentService::Id),
                    )
                    .to_owned(),
            )
            .await?;

        // Index for efficient lookups by environment and deleted status
        manager
            .create_index(
                Index::create()
                    .name("kube_environment_preview_url_environment_deleted_idx")
                    .table(KubeEnvironmentPreviewUrl::Table)
                    .col(KubeEnvironmentPreviewUrl::EnvironmentId)
                    .col(KubeEnvironmentPreviewUrl::DeletedAt)
                    .to_owned(),
            )
            .await?;

        // Index for efficient lookups by service
        manager
            .create_index(
                Index::create()
                    .name("kube_environment_preview_url_service_idx")
                    .table(KubeEnvironmentPreviewUrl::Table)
                    .col(KubeEnvironmentPreviewUrl::ServiceId)
                    .col(KubeEnvironmentPreviewUrl::DeletedAt)
                    .to_owned(),
            )
            .await?;

        // Unique index to prevent duplicate names (globally unique)
        manager
            .create_index(
                Index::create()
                    .name("kube_environment_preview_url_name_unique_idx")
                    .table(KubeEnvironmentPreviewUrl::Table)
                    .col(KubeEnvironmentPreviewUrl::Name)
                    .col(KubeEnvironmentPreviewUrl::DeletedAt)
                    .unique()
                    .nulls_not_distinct()
                    .to_owned(),
            )
            .await?;

        // Unique index for port within service (prevent duplicate ports per service)
        manager
            .create_index(
                Index::create()
                    .name("kube_environment_preview_url_service_port_unique_idx")
                    .table(KubeEnvironmentPreviewUrl::Table)
                    .col(KubeEnvironmentPreviewUrl::ServiceId)
                    .col(KubeEnvironmentPreviewUrl::Port)
                    .col(KubeEnvironmentPreviewUrl::DeletedAt)
                    .unique()
                    .nulls_not_distinct()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
pub enum KubeEnvironmentPreviewUrl {
    Table,
    Id,
    CreatedAt,
    UpdatedAt,
    DeletedAt,
    EnvironmentId,
    ServiceId,
    Name,
    Description,
    Port,
    PortName,
    Protocol,
    AccessLevel,
    CreatedBy,
    LastAccessedAt,
    Metadata,
}
