use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(K8sProvider::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(K8sProvider::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(K8sProvider::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(K8sProvider::DeletedAt).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(K8sProvider::OrganizationId)
                            .uuid()
                            .not_null(),
                    )
                    .col(ColumnDef::new(K8sProvider::CreatedBy).uuid().not_null())
                    .col(ColumnDef::new(K8sProvider::Name).string().not_null())
                    .col(ColumnDef::new(K8sProvider::Provider).string().not_null())
                    .col(ColumnDef::new(K8sProvider::Config).string().not_null())
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum K8sProvider {
    Table,
    Id,
    CreatedAt,
    DeletedAt,
    OrganizationId,
    CreatedBy,
    Name,
    Provider,
    Config,
}
