use lapdev_common::config::LAPDEV_CLUSTER_NOT_INITIATED;
use sea_orm::{ActiveModelTrait, ActiveValue};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Config::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Config::Name)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Config::Value).string().not_null())
                    .to_owned(),
            )
            .await?;

        let conn = manager.get_connection();
        lapdev_db_entities::config::ActiveModel {
            name: ActiveValue::Set(LAPDEV_CLUSTER_NOT_INITIATED.to_string()),
            value: ActiveValue::Set("yes".to_string()),
        }
        .insert(conn)
        .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum Config {
    Table,
    Name,
    Value,
}
