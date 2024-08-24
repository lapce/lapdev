use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(OauthConnection::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OauthConnection::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(OauthConnection::UserId).uuid().not_null())
                    .col(
                        ColumnDef::new(OauthConnection::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(OauthConnection::DeletedAt).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(OauthConnection::Provider)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OauthConnection::ProviderId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OauthConnection::ProviderLogin)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OauthConnection::AccessToken)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(OauthConnection::AvatarUrl).string())
                    .col(ColumnDef::new(OauthConnection::Email).string())
                    .col(ColumnDef::new(OauthConnection::Name).string())
                    .col(ColumnDef::new(OauthConnection::ReadRepo).boolean())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("oauth_connection_provider_provider_id_deleted_at_idx")
                    .table(OauthConnection::Table)
                    .unique()
                    .nulls_not_distinct()
                    .col(OauthConnection::Provider)
                    .col(OauthConnection::ProviderId)
                    .col(OauthConnection::DeletedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("oauth_connection_user_id_provider_deleted_at_idx")
                    .table(OauthConnection::Table)
                    .unique()
                    .nulls_not_distinct()
                    .col(OauthConnection::UserId)
                    .col(OauthConnection::Provider)
                    .col(OauthConnection::DeletedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum OauthConnection {
    Table,
    Id,
    UserId,
    CreatedAt,
    DeletedAt,
    Provider,
    ProviderId,
    ProviderLogin,
    AccessToken,
    AvatarUrl,
    Email,
    Name,
    ReadRepo,
}
