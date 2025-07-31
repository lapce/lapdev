use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(OrganizationMember::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OrganizationMember::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(OrganizationMember::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(OrganizationMember::DeletedAt).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(OrganizationMember::OrganizationId)
                            .uuid()
                            .not_null(),
                    )
                    .col(ColumnDef::new(OrganizationMember::UserId).uuid().not_null())
                    .col(ColumnDef::new(OrganizationMember::Role).string().not_null())
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("organization_member_user_id_idx")
                    .table(OrganizationMember::Table)
                    .col(OrganizationMember::UserId)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("organization_member_organization_id_idx")
                    .table(OrganizationMember::Table)
                    .col(OrganizationMember::OrganizationId)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("organization_member_organization_id_user_id_deleted_at_idx")
                    .unique()
                    .nulls_not_distinct()
                    .table(OrganizationMember::Table)
                    .col(OrganizationMember::OrganizationId)
                    .col(OrganizationMember::UserId)
                    .col(OrganizationMember::DeletedAt)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum OrganizationMember {
    Table,
    Id,
    CreatedAt,
    DeletedAt,
    OrganizationId,
    UserId,
    Role,
}
