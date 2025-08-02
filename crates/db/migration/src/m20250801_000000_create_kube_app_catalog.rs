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
                            .to(KubeCluster::Table, KubeCluster::Id),
                    )
                    .to_owned(),
            )
            .await?;

        // Enable extensions for trigram text search and btree types in GIN
        manager
            .get_connection()
            .execute_unprepared("CREATE EXTENSION IF NOT EXISTS pg_trgm;")
            .await?;

        manager
            .get_connection()
            .execute_unprepared("CREATE EXTENSION IF NOT EXISTS btree_gin;")
            .await?;

        // Create B-tree index for exact lookups without name search
        manager
            .create_index(
                Index::create()
                    .name("kube_app_catalog_org_deleted_created_idx")
                    .table(KubeAppCatalog::Table)
                    .col(KubeAppCatalog::OrganizationId)
                    .col(KubeAppCatalog::DeletedAt)
                    .col(KubeAppCatalog::CreatedAt)
                    .to_owned(),
            )
            .await?;

        // Create GIN index for case-insensitive wildcard name searches
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX kube_app_catalog_gin_search_idx ON kube_app_catalog 
                 USING gin (organization_id, LOWER(name) gin_trgm_ops)
                 WHERE deleted_at IS NULL;",
            )
            .await?;

        // Create index for cluster dependency checks (used when deleting clusters)
        manager
            .create_index(
                Index::create()
                    .name("kube_app_catalog_cluster_deleted_idx")
                    .table(KubeAppCatalog::Table)
                    .col(KubeAppCatalog::ClusterId)
                    .col(KubeAppCatalog::DeletedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
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
