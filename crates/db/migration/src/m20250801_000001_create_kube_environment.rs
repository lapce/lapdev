use sea_orm_migration::prelude::*;

use crate::m20250801_000000_create_kube_app_catalog::KubeAppCatalog;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(KubeEnvironment::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(KubeEnvironment::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironment::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(KubeEnvironment::DeletedAt).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(KubeEnvironment::OrganizationId)
                            .uuid()
                            .not_null(),
                    )
                    .col(ColumnDef::new(KubeEnvironment::CreatedBy).uuid().not_null())
                    .col(
                        ColumnDef::new(KubeEnvironment::AppCatalogId)
                            .uuid()
                            .not_null(),
                    )
                    .col(ColumnDef::new(KubeEnvironment::Name).string().not_null())
                    .col(ColumnDef::new(KubeEnvironment::Namespace).string().not_null())
                    .col(ColumnDef::new(KubeEnvironment::Status).string())
                    .foreign_key(
                        ForeignKey::create()
                            .from(KubeEnvironment::Table, KubeEnvironment::AppCatalogId)
                            .to(KubeAppCatalog::Table, KubeAppCatalog::Id)
                    )
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum KubeEnvironment {
    Table,
    Id,
    CreatedAt,
    DeletedAt,
    OrganizationId,
    CreatedBy,
    AppCatalogId,
    Name,
    Namespace,
    Status,
}