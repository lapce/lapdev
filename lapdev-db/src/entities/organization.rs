//! `SeaORM` Entity. Generated by sea-orm-codegen 0.12.4

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "organization")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub deleted_at: Option<DateTimeWithTimeZone>,
    pub name: String,
    pub auto_start: bool,
    pub allow_workspace_change_auto_start: bool,
    pub auto_stop: Option<i32>,
    pub allow_workspace_change_auto_stop: bool,
    pub last_auto_stop_check: Option<DateTimeWithTimeZone>,
    pub usage_limit: i64,
    pub running_workspace_limit: i32,
    pub has_running_workspace: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
