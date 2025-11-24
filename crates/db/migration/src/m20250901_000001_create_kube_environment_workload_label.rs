use sea_orm_migration::prelude::*;

use crate::m20250809_000001_create_kube_environment::KubeEnvironment;
use crate::m20250809_000002_create_kube_environment_workload::KubeEnvironmentWorkload;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(KubeEnvironmentWorkloadLabel::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(KubeEnvironmentWorkloadLabel::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentWorkloadLabel::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentWorkloadLabel::DeletedAt)
                            .timestamp_with_time_zone(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentWorkloadLabel::EnvironmentId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentWorkloadLabel::WorkloadId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentWorkloadLabel::LabelKey)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeEnvironmentWorkloadLabel::LabelValue)
                            .string()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                KubeEnvironmentWorkloadLabel::Table,
                                KubeEnvironmentWorkloadLabel::EnvironmentId,
                            )
                            .to(KubeEnvironment::Table, KubeEnvironment::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                KubeEnvironmentWorkloadLabel::Table,
                                KubeEnvironmentWorkloadLabel::WorkloadId,
                            )
                            .to(KubeEnvironmentWorkload::Table, KubeEnvironmentWorkload::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("kube_env_workload_label_workload_deleted_idx")
                    .table(KubeEnvironmentWorkloadLabel::Table)
                    .col(KubeEnvironmentWorkloadLabel::WorkloadId)
                    .col(KubeEnvironmentWorkloadLabel::DeletedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("kube_env_workload_label_unique_idx")
                    .table(KubeEnvironmentWorkloadLabel::Table)
                    .col(KubeEnvironmentWorkloadLabel::WorkloadId)
                    .col(KubeEnvironmentWorkloadLabel::LabelKey)
                    .col(KubeEnvironmentWorkloadLabel::DeletedAt)
                    .unique()
                    .nulls_not_distinct()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
pub enum KubeEnvironmentWorkloadLabel {
    Table,
    Id,
    CreatedAt,
    DeletedAt,
    EnvironmentId,
    WorkloadId,
    LabelKey,
    LabelValue,
}
