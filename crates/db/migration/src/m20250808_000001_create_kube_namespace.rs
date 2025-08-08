use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(KubeNamespace::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(KubeNamespace::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(KubeNamespace::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(KubeNamespace::DeletedAt).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(KubeNamespace::OrganizationId)
                            .uuid()
                            .not_null(),
                    )
                    .col(ColumnDef::new(KubeNamespace::UserId).uuid().not_null())
                    .col(ColumnDef::new(KubeNamespace::Name).string().not_null())
                    .col(ColumnDef::new(KubeNamespace::Description).string())
                    .col(
                        ColumnDef::new(KubeNamespace::IsShared)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await?;

        // Create index for organization lookups
        manager
            .create_index(
                Index::create()
                    .name("kube_namespace_org_deleted_idx")
                    .table(KubeNamespace::Table)
                    .col(KubeNamespace::OrganizationId)
                    .col(KubeNamespace::DeletedAt)
                    .to_owned(),
            )
            .await?;

        // Create unique index to prevent duplicate namespace names within organization
        manager
            .create_index(
                Index::create()
                    .name("kube_namespace_org_deleted_name_unique_idx")
                    .table(KubeNamespace::Table)
                    .col(KubeNamespace::OrganizationId)
                    .col(KubeNamespace::DeletedAt)
                    .col(KubeNamespace::Name)
                    .unique()
                    .nulls_not_distinct()
                    .to_owned(),
            )
            .await?;

        // Create optimal composite index for get_all_kube_namespaces query
        // Covers: organization_id, is_shared, user_id, deleted_at
        manager
            .create_index(
                Index::create()
                    .name("kube_namespace_org_shared_user_deleted_idx")
                    .table(KubeNamespace::Table)
                    .col(KubeNamespace::OrganizationId)
                    .col(KubeNamespace::IsShared)
                    .col(KubeNamespace::UserId)
                    .col(KubeNamespace::DeletedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
pub enum KubeNamespace {
    Table,
    Id,
    CreatedAt,
    DeletedAt,
    OrganizationId,
    UserId,
    Name,
    Description,
    IsShared,
}
