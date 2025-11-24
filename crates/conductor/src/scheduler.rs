use std::collections::{HashMap, HashSet};

use anyhow::Result;
use itertools::Itertools;
use lapdev_common::{CpuCore, PrebuildStatus, WorkspaceHostStatus};
use lapdev_db::links;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseTransaction, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect,
};
use uuid::Uuid;

pub const LAPDEV_CPU_OVERCOMMIT: &str = "lapdev-cpu-overcommit";

pub struct AvailableCore {
    pub total: usize,
    pub overcommit: usize,
    pub available: HashMap<usize, usize>,
}

impl AvailableCore {
    pub fn dedicated(&self) -> HashSet<usize> {
        HashSet::from_iter(
            self.available
                .iter()
                .filter(|(_, n)| **n == 0)
                .map(|(k, _)| k)
                .copied(),
        )
    }

    pub fn sorted(&self) -> Vec<(usize, usize)> {
        self.available
            .iter()
            .map(|(k, n)| (*k, *n))
            .sorted_by_key(|(_, n)| -(*n as i32))
            .collect()
    }
}

#[derive(Default)]
struct ExistingResource {
    cores: Vec<(HashSet<usize>, bool)>,
    memory: usize,
    disk: usize,
}

#[allow(clippy::too_many_arguments)]
pub async fn pick_workspce_host(
    txn: &DatabaseTransaction,
    prebuild_workspace_host: Option<Uuid>,
    prebuild_replica_hosts: Vec<Uuid>,
    shared: bool,
    cpu: usize,
    memory: usize,
    disk: usize,
    region: String,
    cpu_overcommit: usize,
) -> Result<(lapdev_db_entities::workspace_host::Model, CpuCore)> {
    let workspace_host = decide_workspace_host(
        txn,
        prebuild_workspace_host,
        prebuild_replica_hosts,
        shared,
        cpu,
        memory,
        disk,
        region,
    )
    .await?;

    let existing = get_existing_resource(txn, &workspace_host).await?;

    let cores = if !shared {
        let available =
            available_cores(workspace_host.cpu as usize, cpu_overcommit, &existing.cores);
        let available = available.dedicated();
        let cores: Vec<usize> = available.into_iter().take(cpu).collect();
        if cores.len() != cpu {
            return Err(anyhow::anyhow!("can't allocate the number of cpus"));
        }
        CpuCore::Dedicated(cores)
    } else {
        CpuCore::Shared(cpu)
    };

    Ok((workspace_host, cores))
}

#[allow(clippy::too_many_arguments)]
async fn decide_workspace_host(
    txn: &DatabaseTransaction,
    prebuild_workspace_host: Option<Uuid>,
    prebuild_replica_hosts: Vec<Uuid>,
    shared: bool,
    cpu: usize,
    memory: usize,
    disk: usize,
    region: String,
) -> Result<lapdev_db_entities::workspace_host::Model> {
    // workspace_spreaded controls if Lapdev prefers to pick workspace host
    // that's busy or idle. When it's true, Lapdev prefers idle server.
    let workspace_spreaded = true;

    let cpu_column = if !shared {
        lapdev_db_entities::workspace_host::Column::AvailableDedicatedCpu
    } else {
        lapdev_db_entities::workspace_host::Column::AvailableSharedCpu
    };

    let order = if workspace_spreaded {
        sea_orm::Order::Desc
    } else {
        sea_orm::Order::Asc
    };

    let query = lapdev_db_entities::workspace_host::Entity::find()
        .filter(lapdev_db_entities::workspace_host::Column::Region.eq(region))
        .filter(lapdev_db_entities::workspace_host::Column::DeletedAt.is_null())
        .filter(
            lapdev_db_entities::workspace_host::Column::Status
                .eq(WorkspaceHostStatus::Active.to_string()),
        )
        .filter(lapdev_db_entities::workspace_host::Column::AvailableMemory.gte(memory as i32))
        .filter(lapdev_db_entities::workspace_host::Column::AvailableDisk.gte(disk as i32))
        .filter(cpu_column.gte(cpu as i32));

    if let Some(id) = prebuild_workspace_host {
        if let Some(host) = query
            .clone()
            .filter(lapdev_db_entities::workspace_host::Column::Id.eq(id))
            .lock_exclusive()
            .one(txn)
            .await?
        {
            return Ok(host);
        }
    }

    if !prebuild_replica_hosts.is_empty() {
        if let Some(host) = query
            .clone()
            .order_by(cpu_column, order.clone())
            .order_by(
                lapdev_db_entities::workspace_host::Column::AvailableMemory,
                order.clone(),
            )
            .order_by(
                lapdev_db_entities::workspace_host::Column::AvailableDisk,
                order.clone(),
            )
            .filter(lapdev_db_entities::workspace_host::Column::Id.is_in(prebuild_replica_hosts))
            .limit(1)
            .lock_exclusive()
            .one(txn)
            .await?
        {
            return Ok(host);
        }
    }

    let workspace_host = query
        .clone()
        .order_by(cpu_column, order.clone())
        .order_by(
            lapdev_db_entities::workspace_host::Column::AvailableMemory,
            order.clone(),
        )
        .order_by(
            lapdev_db_entities::workspace_host::Column::AvailableDisk,
            order.clone(),
        )
        .limit(1)
        .lock_exclusive()
        .one(txn)
        .await?
        .ok_or_else(|| anyhow::anyhow!("can't find available workspace host"))?;

    Ok(workspace_host)
}

pub async fn recalcuate_workspce_host(
    txn: &DatabaseTransaction,
    host: &lapdev_db_entities::workspace_host::Model,
    cpu_overcommit: usize,
) -> Result<lapdev_db_entities::workspace_host::Model> {
    let existing = get_existing_resource(txn, host).await?;
    let available_core = available_cores(host.cpu as usize, cpu_overcommit, &existing.cores);
    let available_memory = host.memory - existing.memory as i32;
    let available_disk = host.disk - existing.disk as i32;

    let model = lapdev_db_entities::workspace_host::ActiveModel {
        id: ActiveValue::Set(host.id),
        available_shared_cpu: ActiveValue::Set(available_core.available.len() as i32),
        available_dedicated_cpu: ActiveValue::Set(available_core.dedicated().len() as i32),
        available_memory: ActiveValue::Set(available_memory),
        available_disk: ActiveValue::Set(available_disk),
        ..Default::default()
    }
    .update(txn)
    .await?;

    Ok(model)
}

async fn get_existing_resource(
    txn: &DatabaseTransaction,
    host: &lapdev_db_entities::workspace_host::Model,
) -> Result<ExistingResource> {
    let mut existing = ExistingResource::default();

    let models = lapdev_db_entities::workspace::Entity::find()
        .find_also_related(lapdev_db_entities::machine_type::Entity)
        .filter(lapdev_db_entities::workspace::Column::HostId.eq(host.id))
        .filter(lapdev_db_entities::workspace::Column::DeletedAt.is_null())
        .filter(lapdev_db_entities::workspace::Column::ComposeParent.is_null())
        .all(txn)
        .await?;

    for (workspace, machine_type) in models {
        if let Some(machine_type) = machine_type {
            let cores: CpuCore = serde_json::from_str(&workspace.cores)?;
            if let CpuCore::Dedicated(cores) = cores {
                let cores: HashSet<usize> = HashSet::from_iter(cores.into_iter());
                existing.cores.push((cores, false));
            }
            existing.memory += machine_type.memory.max(0) as usize;
            existing.disk += machine_type.disk.max(0) as usize;
        }
    }

    let result = lapdev_db_entities::prebuild::Entity::find()
        .find_also_linked(links::PrebuildToMachineType)
        .filter(lapdev_db_entities::prebuild::Column::HostId.eq(host.id))
        .filter(
            lapdev_db_entities::prebuild::Column::Status.eq(PrebuildStatus::Building.to_string()),
        )
        .filter(lapdev_db_entities::prebuild::Column::DeletedAt.is_null())
        .filter(lapdev_db_entities::prebuild::Column::ByWorkspace.eq(false))
        .all(txn)
        .await?;
    for (prebuild, machine_type) in result {
        if let Some(machine_type) = machine_type {
            let cores: CpuCore = serde_json::from_str(&prebuild.cores)?;
            if let CpuCore::Dedicated(cores) = cores {
                let cores: HashSet<usize> = HashSet::from_iter(cores.into_iter());
                existing.cores.push((cores, false));
            }
            existing.memory += machine_type.memory.max(0) as usize;
            existing.disk += machine_type.disk.max(0) as usize;
        }
    }

    Ok(existing)
}

fn available_cores(
    total: usize,
    overcommit: usize,
    existing: &[(HashSet<usize>, bool)],
) -> AvailableCore {
    let mut all = HashSet::new();
    for i in 0..total {
        all.insert(i);
    }
    for (cores, shared) in existing {
        if !*shared {
            for i in cores.iter() {
                all.remove(i);
            }
        }
    }

    let mut num = HashMap::new();
    for i in all {
        num.insert(i, 0);
    }

    for (cores, shared) in existing {
        if *shared {
            for i in cores.iter() {
                if let Some(num) = num.get_mut(i) {
                    *num += 1;
                }
            }
        }
    }

    let available = num.into_iter().filter(|(_, n)| *n < overcommit).collect();
    AvailableCore {
        total,
        overcommit,
        available,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use chrono::Utc;
    use lapdev_db::tests::prepare_db;
    use sea_orm::{ActiveModelTrait, ActiveValue, TransactionTrait};
    use uuid::Uuid;

    use crate::scheduler::{self, available_cores};

    #[test]
    fn test_available_cores() {
        let total = 16;
        let overcommit = 4;
        let w1 = (HashSet::from_iter(vec![0, 1, 2, 3]), false);
        let w2 = (HashSet::from_iter(vec![4, 5]), false);
        let w3 = (HashSet::from_iter(vec![6, 7]), true);
        let cores = available_cores(total, overcommit, &[w1.clone(), w2.clone(), w3.clone()]);
        assert_eq!(cores.dedicated().len(), 8);
        assert_eq!(cores.available.len(), 10);

        let w4 = (HashSet::from_iter(vec![6, 7]), true);
        let cores = available_cores(
            total,
            overcommit,
            &[w1.clone(), w2.clone(), w3.clone(), w4.clone()],
        );
        assert_eq!(cores.dedicated().len(), 8);
        assert_eq!(cores.available.len(), 10);

        let w5 = (HashSet::from_iter(vec![7, 8]), true);
        let cores = available_cores(
            total,
            overcommit,
            &[w1.clone(), w2.clone(), w3.clone(), w4.clone(), w5.clone()],
        );
        assert_eq!(cores.dedicated().len(), 7);
        assert_eq!(cores.available.len(), 10);

        let w6 = (HashSet::from_iter(vec![7, 8]), true);
        let cores = available_cores(
            total,
            overcommit,
            &[
                w1.clone(),
                w2.clone(),
                w3.clone(),
                w4.clone(),
                w5.clone(),
                w6.clone(),
            ],
        );
        assert_eq!(cores.dedicated().len(), 7);
        assert_eq!(cores.available.len(), 9);

        let cores = available_cores(2, 4, &[(HashSet::from_iter(vec![0, 1, 2, 3]), false)]);
        assert_eq!(cores.dedicated().len(), 0);
        assert_eq!(cores.available.len(), 0);

        let cores = available_cores(4, 4, &[(HashSet::from_iter(vec![0, 1, 2, 3]), false)]);
        assert_eq!(cores.dedicated().len(), 0);
        assert_eq!(cores.available.len(), 0);
    }

    // #[tokio::test]
    // async fn test_recalcuate_workspce_host() {
    //     let db = prepare_db().await.unwrap();
    //     let txn = db.conn.begin().await.unwrap();

    //     let machine_type = lapdev_db_entities::machine_type::ActiveModel {
    //         id: ActiveValue::Set(Uuid::new_v4()),
    //         name: ActiveValue::Set("test".to_string()),
    //         shared: ActiveValue::Set(true),
    //         cpu: ActiveValue::Set(2),
    //         memory: ActiveValue::Set(8),
    //         disk: ActiveValue::Set(10),
    //         cost_per_second: ActiveValue::Set(1),
    //         ..Default::default()
    //     }
    //     .insert(&txn)
    //     .await
    //     .unwrap();

    //     let workspace_host = lapdev_db_entities::workspace_host::ActiveModel {
    //         id: ActiveValue::Set(Uuid::new_v4()),
    //         cpu: ActiveValue::Set(12),
    //         memory: ActiveValue::Set(64),
    //         disk: ActiveValue::Set(1000),
    //         status: ActiveValue::Set(WorkspaceHostStatus::Active.to_string()),
    //         host: ActiveValue::Set("127.0.0.1".to_string()),
    //         port: ActiveValue::Set(6123),
    //         inter_port: ActiveValue::Set(6122),
    //         available_dedicated_cpu: ActiveValue::Set(0),
    //         available_shared_cpu: ActiveValue::Set(0),
    //         available_memory: ActiveValue::Set(0),
    //         available_disk: ActiveValue::Set(0),
    //         region: ActiveValue::Set("".to_string()),
    //         zone: ActiveValue::Set("".to_string()),
    //         ..Default::default()
    //     }
    //     .insert(&txn)
    //     .await
    //     .unwrap();

    //     lapdev_db_entities::workspace::ActiveModel {
    //         id: ActiveValue::Set(Uuid::new_v4()),
    //         status: ActiveValue::Set(WorkspaceStatus::Running.to_string()),
    //         name: ActiveValue::Set(utils::rand_string(10)),
    //         created_at: ActiveValue::Set(Utc::now().into()),
    //         repo_url: ActiveValue::Set("".to_string()),
    //         repo_name: ActiveValue::Set("".to_string()),
    //         branch: ActiveValue::Set("".to_string()),
    //         commit: ActiveValue::Set("".to_string()),
    //         organization_id: ActiveValue::Set(Uuid::new_v4()),
    //         user_id: ActiveValue::Set(Uuid::new_v4()),
    //         project_id: ActiveValue::Set(None),
    //         host_id: ActiveValue::Set(workspace_host.id),
    //         osuser: ActiveValue::Set("".to_string()),
    //         ssh_private_key: ActiveValue::Set("".to_string()),
    //         ssh_public_key: ActiveValue::Set("".to_string()),
    //         cores: ActiveValue::Set(serde_json::to_string(&[1, 2]).unwrap()),
    //         ssh_port: ActiveValue::Set(None),
    //         ide_port: ActiveValue::Set(None),
    //         prebuild_id: ActiveValue::Set(None),
    //         service: ActiveValue::Set(None),
    //         usage_id: ActiveValue::Set(None),
    //         machine_type_id: ActiveValue::Set(machine_type.id),
    //         auto_start: ActiveValue::Set(true),
    //         is_compose: ActiveValue::Set(false),
    //         compose_parent: ActiveValue::Set(None),
    //         auto_stop: ActiveValue::Set(None),
    //         build_output: ActiveValue::Set(None),
    //         updated_at: ActiveValue::Set(None),
    //         deleted_at: ActiveValue::Set(None),
    //         env: ActiveValue::Set(None),
    //         last_inactivity: ActiveValue::Set(None),
    //         pinned: ActiveValue::Set(false),
    //     }
    //     .insert(&txn)
    //     .await
    //     .unwrap();

    //     let project = lapdev_db_entities::project::ActiveModel {
    //         id: ActiveValue::Set(Uuid::new_v4()),
    //         name: ActiveValue::Set(utils::rand_string(10)),
    //         created_at: ActiveValue::Set(Utc::now().into()),
    //         created_by: ActiveValue::Set(Uuid::new_v4()),
    //         repo_url: ActiveValue::Set("".to_string()),
    //         repo_name: ActiveValue::Set("".to_string()),
    //         oauth_id: ActiveValue::Set(Uuid::new_v4()),
    //         host_id: ActiveValue::Set(Uuid::new_v4()),
    //         osuser: ActiveValue::Set("".to_string()),
    //         organization_id: ActiveValue::Set(Uuid::new_v4()),
    //         machine_type_id: ActiveValue::Set(machine_type.id),
    //         ..Default::default()
    //     }
    //     .insert(&txn)
    //     .await
    //     .unwrap();

    //     lapdev_db_entities::prebuild::ActiveModel {
    //         id: ActiveValue::Set(Uuid::new_v4()),
    //         status: ActiveValue::Set(PrebuildStatus::Building.to_string()),
    //         created_at: ActiveValue::Set(Utc::now().into()),
    //         branch: ActiveValue::Set("".to_string()),
    //         commit: ActiveValue::Set("".to_string()),
    //         project_id: ActiveValue::Set(project.id),
    //         host_id: ActiveValue::Set(workspace_host.id),
    //         osuser: ActiveValue::Set("".to_string()),
    //         cores: ActiveValue::Set(serde_json::to_string(&[2, 3]).unwrap()),
    //         by_workspace: ActiveValue::Set(false),
    //         ..Default::default()
    //     }
    //     .insert(&txn)
    //     .await
    //     .unwrap();

    //     let workspace_host = scheduler::recalcuate_workspce_host(&txn, &workspace_host, 4)
    //         .await
    //         .unwrap();

    //     txn.commit().await.unwrap();

    //     assert_eq!(workspace_host.available_dedicated_cpu, 9);
    //     assert_eq!(workspace_host.available_shared_cpu, 9);
    //     assert_eq!(workspace_host.available_memory, 48);
    //     assert_eq!(workspace_host.available_disk, 980);
    // }
}
