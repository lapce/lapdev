use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(User::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(User::Id).uuid().not_null().primary_key())
                    .col(
                        ColumnDef::new(User::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(User::DeletedAt).timestamp_with_time_zone())
                    .col(ColumnDef::new(User::Provider).string().not_null())
                    .col(ColumnDef::new(User::ProviderId).integer().not_null())
                    .col(ColumnDef::new(User::ProviderLogin).string().not_null())
                    .col(ColumnDef::new(User::AccessToken).string().not_null())
                    .col(ColumnDef::new(User::AvatarUrl).string())
                    .col(ColumnDef::new(User::Email).string())
                    .col(ColumnDef::new(User::Name).string())
                    .col(ColumnDef::new(User::Osuser).string().not_null())
                    .col(ColumnDef::new(User::CurrentOrganization).uuid().not_null())
                    .col(ColumnDef::new(User::ClusterAdmin).boolean().not_null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("user_provider_provider_id_idx")
                    .table(User::Table)
                    .unique()
                    .nulls_not_distinct()
                    .col(User::Provider)
                    .col(User::ProviderId)
                    .col(User::DeletedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
pub enum User {
    Table,
    Id,
    CreatedAt,
    DeletedAt,
    Provider,
    ProviderId,
    ProviderLogin,
    AccessToken,
    AvatarUrl,
    Email,
    Name,
    Osuser,
    CurrentOrganization,
    ClusterAdmin,
}
