use sea_orm::{Linked, RelationTrait};

use crate::entities;

pub struct PrebuildToMachineType;

impl Linked for PrebuildToMachineType {
    type FromEntity = entities::prebuild::Entity;
    type ToEntity = entities::machine_type::Entity;

    fn link(&self) -> Vec<sea_orm::LinkDef> {
        vec![
            entities::project::Relation::Prebuild.def().rev(),
            entities::project::Relation::MachineType.def(),
        ]
    }
}
