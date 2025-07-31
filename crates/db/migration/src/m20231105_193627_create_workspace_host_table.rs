use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(WorkspaceHost::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(WorkspaceHost::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(WorkspaceHost::DeletedAt).timestamp_with_time_zone())
                    .col(ColumnDef::new(WorkspaceHost::Host).string().not_null())
                    .col(ColumnDef::new(WorkspaceHost::Port).integer().not_null())
                    .col(
                        ColumnDef::new(WorkspaceHost::InterPort)
                            .integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(WorkspaceHost::Status).string().not_null())
                    .col(ColumnDef::new(WorkspaceHost::Cpu).integer().not_null())
                    .col(ColumnDef::new(WorkspaceHost::Memory).integer().not_null())
                    .col(ColumnDef::new(WorkspaceHost::Disk).integer().not_null())
                    .col(
                        ColumnDef::new(WorkspaceHost::AvailableDedicatedCpu)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WorkspaceHost::AvailableSharedCpu)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WorkspaceHost::AvailableMemory)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WorkspaceHost::AvailableDisk)
                            .integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(WorkspaceHost::Region).string().not_null())
                    .col(ColumnDef::new(WorkspaceHost::Zone).string().not_null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("workspace_host_host_deleted_at_idx")
                    .table(WorkspaceHost::Table)
                    .unique()
                    .nulls_not_distinct()
                    .col(WorkspaceHost::Host)
                    .col(WorkspaceHost::DeletedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("workspace_host_deleted_at_status_available_dedicated_cpu_idx")
                    .table(WorkspaceHost::Table)
                    .col(WorkspaceHost::DeletedAt)
                    .col(WorkspaceHost::Status)
                    .col(WorkspaceHost::AvailableDedicatedCpu)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("workspace_host_deleted_at_status_available_shared_cpu_idx")
                    .table(WorkspaceHost::Table)
                    .col(WorkspaceHost::DeletedAt)
                    .col(WorkspaceHost::Status)
                    .col(WorkspaceHost::AvailableSharedCpu)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
pub enum WorkspaceHost {
    Table,
    Id,
    DeletedAt,
    Host,
    Port,
    InterPort,
    Status,
    Cpu,
    Memory,
    Disk,
    AvailableDedicatedCpu,
    AvailableSharedCpu,
    AvailableMemory,
    AvailableDisk,
    Region,
    Zone,
}
