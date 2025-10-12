use sea_orm_migration::prelude::*;

use crate::m20231106_100019_create_user_table::User;
use crate::m20250809_000001_create_kube_environment::KubeEnvironment;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(KubeDevboxSession::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(KubeDevboxSession::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(KubeDevboxSession::SessionId)
                            .uuid()
                            .not_null(),
                    )
                    .col(ColumnDef::new(KubeDevboxSession::UserId).uuid().not_null())
                    .col(
                        ColumnDef::new(KubeDevboxSession::SessionTokenHash)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeDevboxSession::TokenPrefix)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeDevboxSession::DeviceName)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(KubeDevboxSession::ActiveEnvironmentId).uuid())
                    .col(
                        ColumnDef::new(KubeDevboxSession::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(KubeDevboxSession::ExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeDevboxSession::LastUsedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(ColumnDef::new(KubeDevboxSession::RevokedAt).timestamp_with_time_zone())
                    .foreign_key(
                        ForeignKey::create()
                            .from(KubeDevboxSession::Table, KubeDevboxSession::UserId)
                            .to(User::Table, User::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                KubeDevboxSession::Table,
                                KubeDevboxSession::ActiveEnvironmentId,
                            )
                            .to(KubeEnvironment::Table, KubeEnvironment::Id),
                    )
                    .to_owned(),
            )
            .await?;

        // Create unique partial index to ensure one active session per user
        // (only enforced when revoked_at IS NULL)
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE UNIQUE INDEX kube_devbox_session_user_active_unique_idx
                 ON kube_devbox_session (user_id)
                 WHERE revoked_at IS NULL",
            )
            .await?;

        // Create index for session token hash lookups (used during authentication)
        manager
            .create_index(
                Index::create()
                    .name("kube_devbox_session_token_hash_idx")
                    .table(KubeDevboxSession::Table)
                    .col(KubeDevboxSession::SessionTokenHash)
                    .to_owned(),
            )
            .await?;

        // Create index for session identifiers
        manager
            .create_index(
                Index::create()
                    .name("kube_devbox_session_session_id_idx")
                    .table(KubeDevboxSession::Table)
                    .col(KubeDevboxSession::SessionId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        // Create index for active environment lookups
        manager
            .create_index(
                Index::create()
                    .name("kube_devbox_session_active_environment_idx")
                    .table(KubeDevboxSession::Table)
                    .col(KubeDevboxSession::ActiveEnvironmentId)
                    .to_owned(),
            )
            .await?;

        // Create index for expires_at to support cleanup jobs
        manager
            .create_index(
                Index::create()
                    .name("kube_devbox_session_expires_at_idx")
                    .table(KubeDevboxSession::Table)
                    .col(KubeDevboxSession::ExpiresAt)
                    .to_owned(),
            )
            .await?;

        // Create composite index for active sessions by user
        manager
            .create_index(
                Index::create()
                    .name("kube_devbox_session_user_revoked_idx")
                    .table(KubeDevboxSession::Table)
                    .col(KubeDevboxSession::UserId)
                    .col(KubeDevboxSession::RevokedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
pub enum KubeDevboxSession {
    Table,
    Id,
    SessionId,
    UserId,
    SessionTokenHash,
    TokenPrefix,
    DeviceName,
    ActiveEnvironmentId,
    CreatedAt,
    ExpiresAt,
    LastUsedAt,
    RevokedAt,
}
