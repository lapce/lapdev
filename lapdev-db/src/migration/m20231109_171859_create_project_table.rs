use sea_orm_migration::prelude::*;

use crate::migration::m20231105_152940_create_machine_type_table::MachineType;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Project::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Project::Id).uuid().not_null().primary_key())
                    .col(
                        ColumnDef::new(Project::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Project::DeletedAt).timestamp_with_time_zone())
                    .col(ColumnDef::new(Project::Name).string().not_null())
                    .col(ColumnDef::new(Project::OrganizationId).uuid().not_null())
                    .col(ColumnDef::new(Project::CreatedBy).uuid().not_null())
                    .col(ColumnDef::new(Project::OauthId).uuid().not_null())
                    .col(ColumnDef::new(Project::RepoUrl).string().not_null())
                    .col(ColumnDef::new(Project::RepoName).string().not_null())
                    .col(ColumnDef::new(Project::MachineTypeId).uuid().not_null())
                    .col(ColumnDef::new(Project::Env).string())
                    .col(ColumnDef::new(Project::HostId).uuid().not_null())
                    .col(ColumnDef::new(Project::Osuser).string().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .from_tbl(Project::Table)
                            .from_col(Project::MachineTypeId)
                            .to_tbl(MachineType::Table)
                            .to_col(MachineType::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("project_organization_id_deleted_at_idx")
                    .table(Project::Table)
                    .col(Project::OrganizationId)
                    .col(Project::DeletedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
pub enum Project {
    Table,
    Id,
    CreatedAt,
    DeletedAt,
    Name,
    OrganizationId,
    CreatedBy,
    OauthId,
    RepoUrl,
    RepoName,
    MachineTypeId,
    Env,
    Osuser,
    HostId,
}
