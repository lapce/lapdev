use sea_orm_migration::prelude::*;

use super::m20231106_100804_create_workspace_table::Workspace;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(WorkspacePort::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(WorkspacePort::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(WorkspacePort::WorkspaceId).uuid().not_null())
                    .col(ColumnDef::new(WorkspacePort::Port).integer().not_null())
                    .col(ColumnDef::new(WorkspacePort::HostPort).integer().not_null())
                    .col(ColumnDef::new(WorkspacePort::Shared).boolean().not_null())
                    .col(ColumnDef::new(WorkspacePort::Public).boolean().not_null())
                    .col(ColumnDef::new(WorkspacePort::Label).string())
                    .foreign_key(
                        ForeignKey::create()
                            .from_tbl(WorkspacePort::Table)
                            .from_col(WorkspacePort::WorkspaceId)
                            .to_tbl(Workspace::Table)
                            .to_col(Workspace::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("workspace_port_workspace_id_port_idx")
                    .table(WorkspacePort::Table)
                    .col(WorkspacePort::WorkspaceId)
                    .col(WorkspacePort::Port)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum WorkspacePort {
    Table,
    Id,
    WorkspaceId,
    Port,
    HostPort,
    Shared,
    Public,
    Label,
}
