use anyhow::Result;
use lapdev_common::{BuildTarget, PrebuildUpdateEvent, RunningWorkspace, WorkspaceUpdateEvent};
use lapdev_db::entities;
use lapdev_rpc::{error::ApiError, ConductorService};
use sea_orm::{ActiveModelTrait, ActiveValue};
use tarpc::context;
use uuid::Uuid;

use crate::Conductor;

#[derive(Clone)]
pub struct ConductorRpc {
    pub ws_host_id: Uuid,
    pub conductor: Conductor,
}

impl ConductorService for ConductorRpc {
    async fn update_build_repo_stdout(
        self,
        _context: context::Context,
        target: BuildTarget,
        line: String,
    ) {
        match target {
            BuildTarget::Workspace { id, .. } => {
                self.conductor
                    .add_workspace_update_event(None, id, WorkspaceUpdateEvent::Stdout(line))
                    .await;
            }
            BuildTarget::Prebuild { id, .. } => {
                self.conductor
                    .add_prebuild_update_event(id, PrebuildUpdateEvent::Stdout(line))
                    .await;
            }
        }
    }

    async fn update_workspace_last_inactivity(
        self,
        _context: tarpc::context::Context,
        workspace_id: Uuid,
        last_inactivity: Option<chrono::prelude::DateTime<chrono::prelude::FixedOffset>>,
    ) {
        let _ = entities::workspace::ActiveModel {
            id: ActiveValue::Set(workspace_id),
            last_inactivity: ActiveValue::Set(last_inactivity),
            ..Default::default()
        }
        .update(&self.conductor.db.conn)
        .await;
    }

    async fn update_build_repo_stderr(
        self,
        _context: context::Context,
        target: BuildTarget,
        line: String,
    ) {
        match target {
            BuildTarget::Workspace { id, .. } => {
                self.conductor
                    .add_workspace_update_event(None, id, WorkspaceUpdateEvent::Stderr(line))
                    .await;
            }
            BuildTarget::Prebuild { id, .. } => {
                self.conductor
                    .add_prebuild_update_event(id, PrebuildUpdateEvent::Stderr(line))
                    .await;
            }
        }
    }

    async fn running_workspaces(
        self,
        _context: tarpc::context::Context,
    ) -> Result<Vec<RunningWorkspace>, ApiError> {
        let workspaces = self
            .conductor
            .db
            .get_running_workspaces_on_host(self.ws_host_id)
            .await?;
        let workspaces = workspaces
            .into_iter()
            .map(|ws| RunningWorkspace {
                id: ws.id,
                ssh_port: ws.ssh_port,
                ide_port: ws.ide_port,
                last_inactivity: ws.last_inactivity,
            })
            .collect();
        Ok(workspaces)
    }
}
