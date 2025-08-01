use sea_orm_migration::prelude::*;

use crate::m20250729_082625_create_kube_cluster::KubeCluster;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(KubeAppCatalog::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(KubeAppCatalog::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(KubeAppCatalog::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(KubeAppCatalog::DeletedAt).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(KubeAppCatalog::OrganizationId)
                            .uuid()
                            .not_null(),
                    )
                    .col(ColumnDef::new(KubeAppCatalog::CreatedBy).uuid().not_null())
                    .col(ColumnDef::new(KubeAppCatalog::Name).string().not_null())
                    .col(ColumnDef::new(KubeAppCatalog::Description).text())
                    .col(ColumnDef::new(KubeAppCatalog::Resources).text().not_null())
                    .col(ColumnDef::new(KubeAppCatalog::ClusterId).uuid().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .from(KubeAppCatalog::Table, KubeAppCatalog::ClusterId)
                            .to(KubeCluster::Table, KubeCluster::Id)
                    )
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
pub enum KubeAppCatalog {
    Table,
    Id,
    CreatedAt,
    DeletedAt,
    OrganizationId,
    CreatedBy,
    Name,
    Description,
    Resources,
    ClusterId,
}