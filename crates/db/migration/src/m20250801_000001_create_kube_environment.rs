use sea_orm_migration::prelude::*;

use crate::m20250729_082625_create_kube_cluster::KubeCluster;
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
                    .col(ColumnDef::new(KubeEnvironment::UserId).uuid().not_null())
                    .col(
                        ColumnDef::new(KubeEnvironment::AppCatalogId)
                            .uuid()
                            .not_null(),
                    )
                    .col(ColumnDef::new(KubeEnvironment::ClusterId).uuid().not_null())
                    .col(ColumnDef::new(KubeEnvironment::Name).string().not_null())
                    .col(
                        ColumnDef::new(KubeEnvironment::Namespace)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(KubeEnvironment::Status).string())
                    .foreign_key(
                        ForeignKey::create()
                            .from(KubeEnvironment::Table, KubeEnvironment::AppCatalogId)
                            .to(KubeAppCatalog::Table, KubeAppCatalog::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(KubeEnvironment::Table, KubeEnvironment::ClusterId)
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
                    .name("kube_environment_org_user_deleted_created_idx")
                    .table(KubeEnvironment::Table)
                    .col(KubeEnvironment::OrganizationId)
                    .col(KubeEnvironment::UserId)
                    .col(KubeEnvironment::DeletedAt)
                    .col(KubeEnvironment::CreatedAt)
                    .to_owned(),
            )
            .await?;

        // Create GIN index for case-insensitive wildcard name searches
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX kube_environment_gin_search_idx ON kube_environment 
                 USING gin (organization_id, user_id, LOWER(name) gin_trgm_ops)
                 WHERE deleted_at IS NULL;",
            )
            .await?;

        // Create index for cluster dependency checks (used when deleting clusters)
        manager
            .create_index(
                Index::create()
                    .name("kube_environment_cluster_deleted_idx")
                    .table(KubeEnvironment::Table)
                    .col(KubeEnvironment::ClusterId)
                    .col(KubeEnvironment::DeletedAt)
                    .to_owned(),
            )
            .await?;

        // Create index for app catalog dependency checks (used when deleting app catalogs)
        manager
            .create_index(
                Index::create()
                    .name("kube_environment_app_catalog_deleted_idx")
                    .table(KubeEnvironment::Table)
                    .col(KubeEnvironment::AppCatalogId)
                    .col(KubeEnvironment::DeletedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
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
    UserId,
    AppCatalogId,
    ClusterId,
    Name,
    Namespace,
    Status,
}
