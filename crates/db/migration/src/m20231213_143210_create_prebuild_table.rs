use sea_orm_migration::prelude::*;

use crate::{
    m20231105_193627_create_workspace_host_table::WorkspaceHost,
    m20231109_171859_create_project_table::Project,
};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Prebuild::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Prebuild::Id).uuid().not_null().primary_key())
                    .col(
                        ColumnDef::new(Prebuild::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Prebuild::DeletedAt).timestamp_with_time_zone())
                    .col(ColumnDef::new(Prebuild::ProjectId).uuid().not_null())
                    .col(ColumnDef::new(Prebuild::UserId).uuid())
                    .col(ColumnDef::new(Prebuild::Cores).string().not_null())
                    .col(ColumnDef::new(Prebuild::Branch).string().not_null())
                    .col(ColumnDef::new(Prebuild::Commit).string().not_null())
                    .col(ColumnDef::new(Prebuild::HostId).uuid().not_null())
                    .col(ColumnDef::new(Prebuild::Osuser).string().not_null())
                    .col(ColumnDef::new(Prebuild::Status).string().not_null())
                    .col(ColumnDef::new(Prebuild::ByWorkspace).boolean().not_null())
                    .col(ColumnDef::new(Prebuild::BuildOutput).string())
                    .foreign_key(
                        ForeignKey::create()
                            .from_tbl(Prebuild::Table)
                            .from_col(Prebuild::ProjectId)
                            .to_tbl(Project::Table)
                            .to_col(Project::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from_tbl(Prebuild::Table)
                            .from_col(Prebuild::HostId)
                            .to_tbl(WorkspaceHost::Table)
                            .to_col(WorkspaceHost::Id),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("prebuild_project_id_branch_commit_deleted_at_idx")
                    .table(Prebuild::Table)
                    .unique()
                    .nulls_not_distinct()
                    .col(Prebuild::ProjectId)
                    .col(Prebuild::Branch)
                    .col(Prebuild::Commit)
                    .col(Prebuild::DeletedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("prebuild_host_id_deleted_at_status_idx")
                    .table(Prebuild::Table)
                    .col(Prebuild::HostId)
                    .col(Prebuild::DeletedAt)
                    .col(Prebuild::Status)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
pub enum Prebuild {
    Table,
    Id,
    CreatedAt,
    DeletedAt,
    ProjectId,
    UserId,
    Cores,
    Branch,
    Commit,
    HostId,
    Osuser,
    Status,
    ByWorkspace,
    BuildOutput,
}
