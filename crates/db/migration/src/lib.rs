use sea_orm_migration::prelude::*;

mod m20231105_152940_create_machine_type_table;
mod m20231105_193627_create_workspace_host_table;
mod m20231106_100019_create_user_table;
mod m20231106_100804_create_workspace_table;
mod m20231109_171859_create_project_table;
mod m20231113_170211_create_ssh_public_key_table;
mod m20231114_110943_create_config_table;
mod m20231130_151650_create_organization_table;
mod m20231130_151937_create_organization_member_table;
mod m20231213_143210_create_prebuild_table;
mod m20240125_135149_create_quota_table;
mod m20240129_215530_create_usage_table;
mod m20240205_113409_create_audit_log_table;
mod m20240228_141013_create_user_invitation_table;
mod m20240311_220708_create_prebuild_replica_table;
mod m20240312_175753_create_table_update_trigger;
mod m20240316_194115_create_workspace_port_table;
mod m20240823_165223_create_oauth_table;
mod m20250729_082625_create_kube_cluster;
mod m20250729_091307_create_kube_cluster_token;
mod m20250801_000000_create_kube_app_catalog;
mod m20250801_000002_create_kube_app_catalog_workload;
mod m20250808_000001_create_kube_namespace;
mod m20250809_000001_create_kube_environment;
mod m20250809_000002_create_kube_environment_workload;
mod m20250809_000003_create_kube_environment_service;
mod m20250815_000001_create_kube_environment_preview_url;
mod m20251008_000001_create_kube_devbox_session;
mod m20251008_000002_create_kube_devbox_workload_intercept;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20231105_152940_create_machine_type_table::Migration),
            Box::new(m20231105_193627_create_workspace_host_table::Migration),
            Box::new(m20231106_100019_create_user_table::Migration),
            Box::new(m20231106_100804_create_workspace_table::Migration),
            Box::new(m20231109_171859_create_project_table::Migration),
            Box::new(m20231113_170211_create_ssh_public_key_table::Migration),
            Box::new(m20231114_110943_create_config_table::Migration),
            Box::new(m20231130_151650_create_organization_table::Migration),
            Box::new(m20231130_151937_create_organization_member_table::Migration),
            Box::new(m20231213_143210_create_prebuild_table::Migration),
            Box::new(m20240125_135149_create_quota_table::Migration),
            Box::new(m20240129_215530_create_usage_table::Migration),
            Box::new(m20240205_113409_create_audit_log_table::Migration),
            Box::new(m20240228_141013_create_user_invitation_table::Migration),
            Box::new(m20240311_220708_create_prebuild_replica_table::Migration),
            Box::new(m20240312_175753_create_table_update_trigger::Migration),
            Box::new(m20240316_194115_create_workspace_port_table::Migration),
            Box::new(m20240823_165223_create_oauth_table::Migration),
            Box::new(m20250729_082625_create_kube_cluster::Migration),
            Box::new(m20250729_091307_create_kube_cluster_token::Migration),
            Box::new(m20250801_000000_create_kube_app_catalog::Migration),
            Box::new(m20250801_000002_create_kube_app_catalog_workload::Migration),
            Box::new(m20250808_000001_create_kube_namespace::Migration),
            Box::new(m20250809_000001_create_kube_environment::Migration),
            Box::new(m20250809_000002_create_kube_environment_workload::Migration),
            Box::new(m20250809_000003_create_kube_environment_service::Migration),
            Box::new(m20250815_000001_create_kube_environment_preview_url::Migration),
            Box::new(m20251008_000001_create_kube_devbox_session::Migration),
            Box::new(m20251008_000002_create_kube_devbox_workload_intercept::Migration),
        ]
    }
}
