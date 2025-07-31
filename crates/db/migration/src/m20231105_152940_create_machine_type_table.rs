use sea_orm::{ActiveModelTrait, ActiveValue};
use sea_orm_migration::prelude::*;
use uuid::Uuid;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(MachineType::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(MachineType::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(MachineType::DeletedAt).timestamp_with_time_zone())
                    .col(ColumnDef::new(MachineType::Name).string().not_null())
                    .col(ColumnDef::new(MachineType::Shared).boolean().not_null())
                    .col(ColumnDef::new(MachineType::Cpu).integer().not_null())
                    .col(ColumnDef::new(MachineType::Memory).integer().not_null())
                    .col(ColumnDef::new(MachineType::Disk).integer().not_null())
                    .col(
                        ColumnDef::new(MachineType::CostPerSecond)
                            .integer()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        let conn = manager.get_connection();
        lapdev_db_entities::machine_type::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            name: ActiveValue::Set("Standard".to_string()),
            shared: ActiveValue::Set(false),
            cpu: ActiveValue::Set(2),
            memory: ActiveValue::Set(4),
            disk: ActiveValue::Set(100),
            cost_per_second: ActiveValue::Set(0),
            ..Default::default()
        }
        .insert(conn)
        .await?;
        lapdev_db_entities::machine_type::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            name: ActiveValue::Set("Large".to_string()),
            shared: ActiveValue::Set(false),
            cpu: ActiveValue::Set(4),
            memory: ActiveValue::Set(8),
            disk: ActiveValue::Set(200),
            cost_per_second: ActiveValue::Set(0),
            ..Default::default()
        }
        .insert(conn)
        .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
pub enum MachineType {
    Table,
    Id,
    DeletedAt,
    Name,
    Shared,
    Cpu,
    Memory,
    Disk,
    CostPerSecond,
}
