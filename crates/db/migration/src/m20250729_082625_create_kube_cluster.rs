use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(KubeCluster::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(KubeCluster::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(KubeCluster::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(KubeCluster::DeletedAt).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(KubeCluster::OrganizationId)
                            .uuid()
                            .not_null(),
                    )
                    .col(ColumnDef::new(KubeCluster::CreatedBy).uuid().not_null())
                    .col(ColumnDef::new(KubeCluster::Name).string().not_null())
                    .col(ColumnDef::new(KubeCluster::ClusterVersion).string())
                    .col(ColumnDef::new(KubeCluster::Status).string())
                    .col(ColumnDef::new(KubeCluster::Region).string())
                    .col(ColumnDef::new(KubeCluster::LastReportedAt).timestamp_with_time_zone())
                    .col(ColumnDef::new(KubeCluster::CanDeployPersonal).boolean().not_null())
                    .col(ColumnDef::new(KubeCluster::CanDeployShared).boolean().not_null())
                    .to_owned(),
            )
            .await?;

        // Create optimal composite index for get_all_kube_clusters query
        // Covers: organization_id, deleted_at, created_at (for ordering)
        manager
            .create_index(
                Index::create()
                    .name("kube_cluster_org_deleted_created_idx")
                    .table(KubeCluster::Table)
                    .col(KubeCluster::OrganizationId)
                    .col(KubeCluster::DeletedAt)
                    .col(KubeCluster::CreatedAt)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
pub enum KubeCluster {
    Table,
    Id,
    CreatedAt,
    DeletedAt,
    OrganizationId,
    CreatedBy,
    Name,
    ClusterVersion,
    Status,
    Region,
    LastReportedAt,
    CanDeployPersonal,
    CanDeployShared,
}
