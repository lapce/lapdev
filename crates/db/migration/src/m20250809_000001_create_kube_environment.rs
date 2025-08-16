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
                    .col(
                        ColumnDef::new(KubeEnvironment::IsShared)
                            .boolean()
                            .not_null(),
                    )
                    .col(ColumnDef::new(KubeEnvironment::BaseEnvironmentId).uuid())
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
                    .foreign_key(
                        ForeignKey::create()
                            .from(KubeEnvironment::Table, KubeEnvironment::BaseEnvironmentId)
                            .to(KubeEnvironment::Table, KubeEnvironment::Id),
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

        // Create optimal composite index for get_all_kube_environments query
        // Covers: organization_id, is_shared, user_id, deleted_at
        manager
            .create_index(
                Index::create()
                    .name("kube_environment_org_shared_base_deleted_user_idx")
                    .table(KubeEnvironment::Table)
                    .col(KubeEnvironment::OrganizationId)
                    .col(KubeEnvironment::IsShared)
                    .col(KubeEnvironment::BaseEnvironmentId)
                    .col(KubeEnvironment::DeletedAt)
                    .col(KubeEnvironment::UserId)
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

        // Create unique index to ensure same app catalog can only be deployed once per (cluster_id, namespace)
        // for base environments (base_environment_id IS NULL), but allow multiple branch environments
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE UNIQUE INDEX kube_environment_app_cluster_namespace_unique_idx
                 ON kube_environment (app_catalog_id, cluster_id, namespace, deleted_at) NULLS NOT DISTINCT
                 WHERE base_environment_id IS NULL",
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
pub enum KubeEnvironment {
    Table,
    Id,
    CreatedAt,
    DeletedAt,
    OrganizationId,
    UserId,
    AppCatalogId,
    ClusterId,
    Name,
    Namespace,
    Status,
    IsShared,
    BaseEnvironmentId,
}
