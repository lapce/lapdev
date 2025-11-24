use sea_orm_migration::prelude::*;

use crate::m20250729_082625_create_kube_cluster::KubeCluster;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(KubeClusterService::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(KubeClusterService::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(KubeClusterService::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeClusterService::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeClusterService::DeletedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(KubeClusterService::ClusterId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeClusterService::Namespace)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(KubeClusterService::Name).string().not_null())
                    .col(
                        ColumnDef::new(KubeClusterService::ResourceVersion)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeClusterService::ServiceYaml)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(KubeClusterService::Selector)
                            .json()
                            .not_null()
                            .default("{}"),
                    )
                    .col(
                        ColumnDef::new(KubeClusterService::Ports)
                            .json()
                            .not_null()
                            .default("[]"),
                    )
                    .col(
                        ColumnDef::new(KubeClusterService::ServiceType)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(KubeClusterService::ClusterIp)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(KubeClusterService::LastObservedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(KubeClusterService::Table, KubeClusterService::ClusterId)
                            .to(KubeCluster::Table, KubeCluster::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("kube_cluster_service_cluster_namespace_name_idx")
                    .table(KubeClusterService::Table)
                    .col(KubeClusterService::ClusterId)
                    .col(KubeClusterService::Namespace)
                    .col(KubeClusterService::Name)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("kube_cluster_service_cluster_namespace_deleted_idx")
                    .table(KubeClusterService::Table)
                    .col(KubeClusterService::ClusterId)
                    .col(KubeClusterService::Namespace)
                    .col(KubeClusterService::DeletedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
pub enum KubeClusterService {
    Table,
    Id,
    CreatedAt,
    UpdatedAt,
    DeletedAt,
    ClusterId,
    Namespace,
    Name,
    ResourceVersion,
    ServiceYaml,
    Selector,
    Ports,
    ServiceType,
    ClusterIp,
    LastObservedAt,
}
