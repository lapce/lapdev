use sea_orm::{Linked, RelationTrait};

pub struct PrebuildToMachineType;

impl Linked for PrebuildToMachineType {
    type FromEntity = lapdev_db_entities::prebuild::Entity;
    type ToEntity = lapdev_db_entities::machine_type::Entity;

    fn link(&self) -> Vec<sea_orm::LinkDef> {
        vec![
            lapdev_db_entities::project::Relation::Prebuild.def().rev(),
            lapdev_db_entities::project::Relation::MachineType.def(),
        ]
    }
}
