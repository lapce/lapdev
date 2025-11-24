use sea_orm_migration::prelude::*;

use super::{
    m20231105_193627_create_workspace_host_table::WorkspaceHost,
    m20231213_143210_create_prebuild_table::Prebuild,
};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(PrebuildReplica::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PrebuildReplica::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(PrebuildReplica::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(PrebuildReplica::DeletedAt).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(PrebuildReplica::PrebuildId)
                            .uuid()
                            .not_null(),
                    )
                    .col(ColumnDef::new(PrebuildReplica::HostId).uuid().not_null())
                    .col(ColumnDef::new(PrebuildReplica::UserId).uuid())
                    .col(ColumnDef::new(PrebuildReplica::Status).string().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .from_tbl(PrebuildReplica::Table)
                            .from_col(PrebuildReplica::PrebuildId)
                            .to_tbl(Prebuild::Table)
                            .to_col(Prebuild::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from_tbl(PrebuildReplica::Table)
                            .from_col(PrebuildReplica::HostId)
                            .to_tbl(WorkspaceHost::Table)
                            .to_col(WorkspaceHost::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("prebuild_replica_prebuild_id_deleted_at_host_id_idx")
                    .table(PrebuildReplica::Table)
                    .unique()
                    .nulls_not_distinct()
                    .col(PrebuildReplica::PrebuildId)
                    .col(PrebuildReplica::DeletedAt)
                    .col(PrebuildReplica::HostId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum PrebuildReplica {
    Table,
    Id,
    CreatedAt,
    DeletedAt,
    PrebuildId,
    UserId,
    HostId,
    Status,
}
