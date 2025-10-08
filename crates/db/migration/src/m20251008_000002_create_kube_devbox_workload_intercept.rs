use sea_orm_migration::prelude::*;

use crate::m20250809_000001_create_kube_environment::KubeEnvironment;
use crate::m20250809_000002_create_kube_environment_workload::KubeEnvironmentWorkload;
use crate::m20251008_000001_create_kube_devbox_session::KubeDevboxSession;

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
                        ColumnDef::new(KubeDevboxWorkloadIntercept::SessionId)
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
                            .json()
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
                                KubeDevboxWorkloadIntercept::SessionId,
                            )
                            .to(KubeDevboxSession::Table, KubeDevboxSession::Id),
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

        // Create index for session lookups (to find all intercepts for a session)
        manager
            .create_index(
                Index::create()
                    .name("kube_devbox_workload_intercept_session_idx")
                    .table(KubeDevboxWorkloadIntercept::Table)
                    .col(KubeDevboxWorkloadIntercept::SessionId)
                    .to_owned(),
            )
            .await?;

        // Create index for environment lookups
        manager
            .create_index(
                Index::create()
                    .name("kube_devbox_workload_intercept_environment_idx")
                    .table(KubeDevboxWorkloadIntercept::Table)
                    .col(KubeDevboxWorkloadIntercept::EnvironmentId)
                    .to_owned(),
            )
            .await?;

        // Create index for workload lookups
        manager
            .create_index(
                Index::create()
                    .name("kube_devbox_workload_intercept_workload_idx")
                    .table(KubeDevboxWorkloadIntercept::Table)
                    .col(KubeDevboxWorkloadIntercept::WorkloadId)
                    .to_owned(),
            )
            .await?;

        // Create index for active intercepts (where stopped_at IS NULL)
        manager
            .create_index(
                Index::create()
                    .name("kube_devbox_workload_intercept_active_idx")
                    .table(KubeDevboxWorkloadIntercept::Table)
                    .col(KubeDevboxWorkloadIntercept::StoppedAt)
                    .to_owned(),
            )
            .await?;

        // Create composite index for finding active intercepts by session
        manager
            .create_index(
                Index::create()
                    .name("kube_devbox_workload_intercept_session_active_idx")
                    .table(KubeDevboxWorkloadIntercept::Table)
                    .col(KubeDevboxWorkloadIntercept::SessionId)
                    .col(KubeDevboxWorkloadIntercept::StoppedAt)
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
    SessionId,
    EnvironmentId,
    WorkloadId,
    PortMappings,
    CreatedAt,
    StoppedAt,
}
