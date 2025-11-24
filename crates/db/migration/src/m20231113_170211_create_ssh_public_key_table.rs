use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(SshPublicKey::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(SshPublicKey::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(SshPublicKey::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(SshPublicKey::DeletedAt).timestamp_with_time_zone())
                    .col(ColumnDef::new(SshPublicKey::UserId).uuid().not_null())
                    .col(ColumnDef::new(SshPublicKey::Name).string().not_null())
                    .col(ColumnDef::new(SshPublicKey::Key).string().not_null())
                    .col(ColumnDef::new(SshPublicKey::ParsedKey).string().not_null())
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum SshPublicKey {
    Table,
    Id,
    CreatedAt,
    DeletedAt,
    UserId,
    Name,
    Key,
    ParsedKey,
}
