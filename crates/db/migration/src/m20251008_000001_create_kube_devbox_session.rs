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

        // Create unique index to ensure only one session row per user
        manager
            .create_index(
                Index::create()
                    .name("kube_devbox_session_user_unique_idx")
                    .table(KubeDevboxSession::Table)
                    .col(KubeDevboxSession::UserId)
                    .unique()
                    .to_owned(),
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
