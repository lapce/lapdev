use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Quota::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Quota::Id).uuid().not_null().primary_key())
                    .col(
                        ColumnDef::new(Quota::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Quota::DeletedAt).timestamp_with_time_zone())
                    .col(ColumnDef::new(Quota::Kind).string().not_null())
                    .col(ColumnDef::new(Quota::Value).integer().not_null())
                    .col(ColumnDef::new(Quota::Organization).uuid().not_null())
                    .col(ColumnDef::new(Quota::User).uuid())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("quota_deleted_at_organization_user_kind_idx")
                    .table(Quota::Table)
                    .unique()
                    .nulls_not_distinct()
                    .col(Quota::DeletedAt)
                    .col(Quota::Organization)
                    .col(Quota::User)
                    .col(Quota::Kind)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum Quota {
    Table,
    Id,
    CreatedAt,
    DeletedAt,
    Kind,
    Value,
    Organization,
    User,
}
