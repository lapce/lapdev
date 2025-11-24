use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(KubeClusterToken::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(KubeClusterToken::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(KubeClusterToken::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(KubeClusterToken::DeletedAt).timestamp_with_time_zone())
                    .col(ColumnDef::new(KubeClusterToken::LastUsedAt).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(KubeClusterToken::ClusterId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeClusterToken::CreatedBy)
                            .uuid()
                            .not_null(),
                    )
                    .col(ColumnDef::new(KubeClusterToken::Name).string().not_null())
                    .col(ColumnDef::new(KubeClusterToken::Token).binary().not_null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("kube_cluster_token_token_deleted_at_idx")
                    .table(KubeClusterToken::Table)
                    .unique()
                    .nulls_not_distinct()
                    .col(KubeClusterToken::Token)
                    .col(KubeClusterToken::DeletedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum KubeClusterToken {
    Table,
    Id,
    CreatedAt,
    DeletedAt,
    LastUsedAt,
    ClusterId,
    CreatedBy,
    Name,
    Token,
}
