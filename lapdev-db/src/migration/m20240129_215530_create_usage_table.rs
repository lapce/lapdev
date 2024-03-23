use sea_orm_migration::prelude::*;

use super::{
    m20231105_152940_create_machine_type_table::MachineType,
    m20231106_100019_create_user_table::User,
};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Usage::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Usage::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Usage::Start)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Usage::End).timestamp_with_time_zone())
                    .col(ColumnDef::new(Usage::ResourceKind).string().not_null())
                    .col(ColumnDef::new(Usage::ResourceId).uuid().not_null())
                    .col(ColumnDef::new(Usage::ResourceName).string().not_null())
                    .col(ColumnDef::new(Usage::OrganizationId).uuid().not_null())
                    .col(ColumnDef::new(Usage::MachineTypeId).uuid().not_null())
                    .col(ColumnDef::new(Usage::UserId).uuid())
                    .col(ColumnDef::new(Usage::Duration).integer())
                    .col(ColumnDef::new(Usage::CostPerSecond).integer().not_null())
                    .col(ColumnDef::new(Usage::TotalCost).integer().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .from_tbl(Usage::Table)
                            .from_col(Usage::UserId)
                            .to_tbl(User::Table)
                            .to_col(User::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from_tbl(Usage::Table)
                            .from_col(Usage::MachineTypeId)
                            .to_tbl(MachineType::Table)
                            .to_col(MachineType::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("usage_organization_id_end_user_id_idx")
                    .table(Usage::Table)
                    .col(Usage::OrganizationId)
                    .col(Usage::Start)
                    .col(Usage::End)
                    .col(Usage::UserId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum Usage {
    Table,
    Id,
    Start,
    End,
    ResourceId,
    ResourceKind,
    ResourceName,
    MachineTypeId,
    OrganizationId,
    UserId,
    Duration,
    CostPerSecond,
    TotalCost,
}
