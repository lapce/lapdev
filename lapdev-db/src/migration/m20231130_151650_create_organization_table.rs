use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Organization::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Organization::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Organization::DeletedAt).timestamp_with_time_zone())
                    .col(ColumnDef::new(Organization::Name).string().not_null())
                    .col(ColumnDef::new(Organization::AutoStart).boolean().not_null())
                    .col(
                        ColumnDef::new(Organization::AllowWorkspaceChangeAutoStart)
                            .boolean()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Organization::AutoStop).integer())
                    .col(
                        ColumnDef::new(Organization::AllowWorkspaceChangeAutoStop)
                            .boolean()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Organization::LastAutoStopCheck).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(Organization::UsageLimit)
                            .big_integer()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("organization_deleted_at_last_auto_stop_check_idx")
                    .table(Organization::Table)
                    .col(Organization::DeletedAt)
                    .col(Organization::LastAutoStopCheck)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum Organization {
    Table,
    Id,
    DeletedAt,
    Name,
    AutoStart,
    AllowWorkspaceChangeAutoStart,
    AutoStop,
    AllowWorkspaceChangeAutoStop,
    LastAutoStopCheck,
    UsageLimit,
}
