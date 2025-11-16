use sea_orm_migration::prelude::*;

use crate::m20231106_100019_create_user_table::User;
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
                    .table(KubeDevboxWorkloadIntercept::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(KubeDevboxWorkloadIntercept::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(KubeDevboxWorkloadIntercept::UserId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeDevboxWorkloadIntercept::EnvironmentId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeDevboxWorkloadIntercept::WorkloadId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeDevboxWorkloadIntercept::PortMappings)
                            .json_binary()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeDevboxWorkloadIntercept::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(KubeDevboxWorkloadIntercept::StoppedAt)
                            .timestamp_with_time_zone(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                KubeDevboxWorkloadIntercept::Table,
                                KubeDevboxWorkloadIntercept::UserId,
                            )
                            .to(User::Table, User::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                KubeDevboxWorkloadIntercept::Table,
                                KubeDevboxWorkloadIntercept::EnvironmentId,
                            )
                            .to(KubeEnvironment::Table, KubeEnvironment::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                KubeDevboxWorkloadIntercept::Table,
                                KubeDevboxWorkloadIntercept::WorkloadId,
                            )
                            .to(KubeEnvironmentWorkload::Table, KubeEnvironmentWorkload::Id),
                    )
                    .to_owned(),
            )
            .await?;

        // Create partial unique index to prevent duplicate active intercepts per workload
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE UNIQUE INDEX kube_devbox_workload_intercept_user_workload_unique_idx
                 ON kube_devbox_workload_intercept (user_id, workload_id)
                 WHERE stopped_at IS NULL",
            )
            .await?;

        // Composite index to cover active + historical intercept lookups
        manager
            .create_index(
                Index::create()
                    .name("kube_devbox_workload_intercept_user_workload_stopped_idx")
                    .table(KubeDevboxWorkloadIntercept::Table)
                    .col(KubeDevboxWorkloadIntercept::UserId)
                    .col(KubeDevboxWorkloadIntercept::WorkloadId)
                    .col(KubeDevboxWorkloadIntercept::StoppedAt)
                    .to_owned(),
            )
            .await?;

        // Composite index for environment-scoped active intercept lookups
        manager
            .create_index(
                Index::create()
                    .name("kube_devbox_workload_intercept_environment_stopped_idx")
                    .table(KubeDevboxWorkloadIntercept::Table)
                    .col(KubeDevboxWorkloadIntercept::EnvironmentId)
                    .col(KubeDevboxWorkloadIntercept::StoppedAt)
                    .to_owned(),
            )
            .await?;

        // Composite index for environment-scoped ordering by created_at
        manager
            .create_index(
                Index::create()
                    .name("kube_devbox_workload_intercept_environment_created_idx")
                    .table(KubeDevboxWorkloadIntercept::Table)
                    .col(KubeDevboxWorkloadIntercept::EnvironmentId)
                    .col((
                        KubeDevboxWorkloadIntercept::CreatedAt,
                        IndexOrder::Desc,
                    ))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(KubeDevboxWorkloadIntercept::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
pub enum KubeDevboxWorkloadIntercept {
    Table,
    Id,
    UserId,
    EnvironmentId,
    WorkloadId,
    PortMappings,
    CreatedAt,
    StoppedAt,
}
