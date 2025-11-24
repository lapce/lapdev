use sea_orm_migration::prelude::*;

use super::m20231106_100019_create_user_table::User;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AuditLog::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AuditLog::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(AuditLog::Time)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(AuditLog::UserId).uuid().not_null())
                    .col(ColumnDef::new(AuditLog::OrganizationId).uuid().not_null())
                    .col(ColumnDef::new(AuditLog::ResourceKind).string().not_null())
                    .col(ColumnDef::new(AuditLog::ResourceId).uuid().not_null())
                    .col(ColumnDef::new(AuditLog::ResourceName).string().not_null())
                    .col(ColumnDef::new(AuditLog::Action).string().not_null())
                    .col(ColumnDef::new(AuditLog::Ip).string())
                    .col(ColumnDef::new(AuditLog::UserAgent).string())
                    .foreign_key(
                        ForeignKey::create()
                            .from_tbl(AuditLog::Table)
                            .from_col(AuditLog::UserId)
                            .to_tbl(User::Table)
                            .to_col(User::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("audit_log_organization_id_time_idx")
                    .table(AuditLog::Table)
                    .col(AuditLog::OrganizationId)
                    .col(AuditLog::Time)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum AuditLog {
    Table,
    Id,
    Time,
    UserId,
    OrganizationId,
    ResourceKind,
    ResourceId,
    ResourceName,
    Action,
    Ip,
    UserAgent,
}
