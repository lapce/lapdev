use sea_orm_migration::prelude::*;

pub mod m20231105_152940_create_machine_type_table;
pub mod m20231105_193627_create_workspace_host_table;
pub mod m20231106_100019_create_user_table;
pub mod m20231106_100804_create_workspace_table;
pub mod m20231109_171859_create_project_table;
pub mod m20231113_170211_create_ssh_public_key_table;
pub mod m20231114_110943_create_config_table;
pub mod m20231130_151650_create_organization_table;
pub mod m20231130_151937_create_organization_member_table;
pub mod m20231213_143210_create_prebuild_table;
pub mod m20240125_135149_create_quota_table;
pub mod m20240129_215530_create_usage_table;
pub mod m20240205_113409_create_audit_log_table;
pub mod m20240228_141013_create_user_invitation_table;
pub mod m20240311_220708_create_prebuild_replica_table;
pub mod m20240312_175753_create_table_update_trigger;
pub mod m20240316_194115_create_workspace_port_table;

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
        ]
    }
}
