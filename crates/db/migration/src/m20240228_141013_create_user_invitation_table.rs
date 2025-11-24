use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(UserInvitation::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(UserInvitation::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(UserInvitation::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UserInvitation::ExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(UserInvitation::UsedAt).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(UserInvitation::OrganizationId)
                            .uuid()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum UserInvitation {
    Table,
    Id,
    CreatedAt,
    ExpiresAt,
    OrganizationId,
    UsedAt,
}
