use std::{cmp::Ordering, collections::HashMap, str::FromStr, sync::Arc};

use anyhow::Result;
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use axum_extra::{headers::Cookie, TypedHeader};
use chrono::Utc;
use hyper::StatusCode;
use itertools::Itertools;
use lapdev_common::{
    AuditAction, AuditResourceKind, NewWorkspace, RepoBuildResult, UpdateWorkspacePort,
    WorkspaceInfo, WorkspacePort, WorkspaceService, WorkspaceStatus, WorkspaceUpdateEvent,
};
use lapdev_db::api::LAPDEV_PIN_UNPIN_ERROR;
use lapdev_rpc::error::ApiError;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter, TransactionTrait,
};
use tracing::error;
use uuid::Uuid;

use crate::state::{CoreState, RequestInfo};

pub async fn create_workspace(
    info: RequestInfo,
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path(org_id): Path<Uuid>,
    State(state): State<Arc<CoreState>>,
    Json(workspace): Json<NewWorkspace>,
) -> Result<Response, ApiError> {
    let user = state.authenticate(&cookie).await?;
    state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    let org = state.db.get_organization(org_id).await?;
    let result = state
        .conductor
        .create_workspace(user, org, workspace, info.ip, info.user_agent)
        .await
        .map(Json);
    if let Err(ApiError::InternalError(err)) = &result {
        error!("create workspace internal error: {err}");
    }
    Ok(result.into_response())
}

pub async fn all_workspaces(
    State(state): State<Arc<CoreState>>,
    Path(org_id): Path<Uuid>,
    TypedHeader(cookie): TypedHeader<Cookie>,
) -> Result<Json<Vec<WorkspaceInfo>>, ApiError> {
    let user = state.authenticate(&cookie).await?;
    state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    let workspaces = state.db.get_all_workspaces(user.id, org_id).await?;

    let mut services: HashMap<Uuid, Vec<WorkspaceService>> = HashMap::new();
    for (ws, _) in &workspaces {
        if ws.is_compose {
            if let Some(parent) = ws.compose_parent {
                let services = services.entry(parent).or_default();
                services.push(WorkspaceService {
                    name: ws.name.clone(),
                    service: ws.service.clone().unwrap_or_default(),
                });
            }
        }
    }

    let hostnames = state.conductor.hostnames.read().await;

    let workspaces: Vec<WorkspaceInfo> = workspaces
        .into_iter()
        .filter_map(|(w, host)| {
            let services = if w.is_compose {
                if w.compose_parent.is_some() {
                    return None;
                }
                services.get(&w.id).cloned().unwrap_or_default()
            } else {
                Vec::new()
            };
            let region = host.map(|host| host.region).unwrap_or_default();
            let hostname = hostnames.get(region.trim()).cloned().unwrap_or_default();
            let build_result = w
                .build_output
                .and_then(|o| serde_json::from_str::<RepoBuildResult>(&o).ok());

            Some(WorkspaceInfo {
                name: w.name,
                repo_url: w.repo_url,
                repo_name: w.repo_name,
                branch: w.branch,
                commit: w.commit,
                status: WorkspaceStatus::from_str(&w.status).unwrap_or(WorkspaceStatus::New),
                machine_type: w.machine_type_id,
                services,
                created_at: w.created_at,
                hostname,
                build_error: build_result.and_then(|r| r.error),
                pinned: w.pinned,
            })
        })
        .sorted_by(|a, b| match (a.pinned, b.pinned) {
            (true, true) | (false, false) => b.created_at.cmp(&a.created_at),
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
        })
        .collect();
    Ok(Json(workspaces))
}

pub async fn delete_workspace(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path((org_id, workspace_name)): Path<(Uuid, String)>,
    State(state): State<Arc<CoreState>>,
    info: RequestInfo,
) -> Result<Response, ApiError> {
    let user = state.authenticate(&cookie).await?;
    state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    let ws = state
        .db
        .get_workspace_by_name(&workspace_name)
        .await
        .map_err(|_| ApiError::InvalidRequest("workspace name doesn't exist".to_string()))?;
    if ws.user_id != user.id {
        return Err(ApiError::Unauthorized);
    }

    state
        .conductor
        .delete_workspace(&ws, info.ip, info.user_agent)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn get_workspace(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path((org_id, workspace_name)): Path<(Uuid, String)>,
    State(state): State<Arc<CoreState>>,
) -> Result<Json<WorkspaceInfo>, ApiError> {
    let user = state.authenticate(&cookie).await?;
    state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    let ws = state
        .db
        .get_workspace_by_name(&workspace_name)
        .await
        .map_err(|_| ApiError::InvalidRequest("workspace name doesn't exist".to_string()))?;
    if ws.user_id != user.id {
        return Err(ApiError::Unauthorized);
    }

    let services = if ws.is_compose && ws.compose_parent.is_none() {
        lapdev_db_entities::workspace::Entity::find()
            .filter(lapdev_db_entities::workspace::Column::DeletedAt.is_null())
            .filter(lapdev_db_entities::workspace::Column::ComposeParent.eq(ws.id))
            .all(&state.db.conn)
            .await?
            .into_iter()
            .map(|w| WorkspaceService {
                name: w.name,
                service: w.service.unwrap_or_default(),
            })
            .collect()
    } else {
        Vec::new()
    };

    let host = state.db.get_workspace_host(ws.host_id).await?;
    let region = host.map(|host| host.region).unwrap_or_default();
    let hostname = state
        .conductor
        .hostnames
        .read()
        .await
        .get(region.trim())
        .cloned()
        .unwrap_or_default();
    let build_result = ws
        .build_output
        .and_then(|o| serde_json::from_str::<RepoBuildResult>(&o).ok());

    let info = WorkspaceInfo {
        name: ws.name,
        repo_url: ws.repo_url,
        repo_name: ws.repo_name,
        branch: ws.branch,
        commit: ws.commit,
        status: WorkspaceStatus::from_str(&ws.status).unwrap_or(WorkspaceStatus::New),
        machine_type: ws.machine_type_id,
        services,
        created_at: ws.created_at,
        hostname,
        build_error: build_result.and_then(|r| r.error),
        pinned: ws.pinned,
    };
    Ok(Json(info))
}

pub async fn start_workspace(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path((org_id, workspace_name)): Path<(Uuid, String)>,
    State(state): State<Arc<CoreState>>,
    info: RequestInfo,
) -> Result<Response, ApiError> {
    let user = state.authenticate(&cookie).await?;
    state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    let ws = state
        .db
        .get_workspace_by_name(&workspace_name)
        .await
        .map_err(|_| ApiError::InvalidRequest("workspace name doesn't exist".to_string()))?;
    if ws.user_id != user.id {
        return Err(ApiError::Unauthorized);
    }

    if ws.status == WorkspaceStatus::Running.to_string() {
        return Err(ApiError::InvalidRequest(
            "workspace is already running".to_string(),
        ));
    }

    state
        .conductor
        .start_workspace(ws, false, info.ip, info.user_agent)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn stop_workspace(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path((org_id, workspace_name)): Path<(Uuid, String)>,
    State(state): State<Arc<CoreState>>,
    info: RequestInfo,
) -> Result<Response, ApiError> {
    let user = state.authenticate(&cookie).await?;
    state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    let ws = state
        .db
        .get_workspace_by_name(&workspace_name)
        .await
        .map_err(|_| ApiError::InvalidRequest("workspace name doesn't exist".to_string()))?;
    if ws.user_id != user.id {
        return Err(ApiError::Unauthorized);
    }

    if ws.status == WorkspaceStatus::Stopped.to_string() {
        return Err(ApiError::InvalidRequest(
            "workspace is already stopped".to_string(),
        ));
    }

    state
        .conductor
        .stop_workspace(ws, info.ip, info.user_agent)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn pin_workspace(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path((org_id, workspace_name)): Path<(Uuid, String)>,
    State(state): State<Arc<CoreState>>,
    info: RequestInfo,
) -> Result<Response, ApiError> {
    let user = state.authenticate(&cookie).await?;
    state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    let ws = state
        .db
        .get_workspace_by_name(&workspace_name)
        .await
        .map_err(|_| ApiError::InvalidRequest("workspace name doesn't exist".to_string()))?;
    if ws.user_id != user.id {
        return Err(ApiError::Unauthorized);
    }

    let org = state.db.get_organization(ws.organization_id).await?;
    if org.running_workspace_limit > 0 {
        return Err(ApiError::InvalidRequest(
            state
                .db
                .get_config(LAPDEV_PIN_UNPIN_ERROR)
                .await
                .unwrap_or_else(|_| "You can't pin/unpin workspaces".to_string()),
        ));
    }

    if ws.compose_parent.is_some() {
        return Err(ApiError::InvalidRequest(
            "you can only pin the main workspace".to_string(),
        ));
    }

    if ws.pinned {
        return Err(ApiError::InvalidRequest(
            "workspace is already pinned".to_string(),
        ));
    }

    let now = Utc::now();
    let txn = state.db.conn.begin().await?;
    state
        .conductor
        .enterprise
        .insert_audit_log(
            &txn,
            now.into(),
            ws.user_id,
            ws.organization_id,
            AuditResourceKind::Workspace.to_string(),
            ws.id,
            format!("{} pin", ws.name),
            AuditAction::WorkspaceUpdate.to_string(),
            info.ip.clone(),
            info.user_agent.clone(),
        )
        .await?;
    let ws = lapdev_db_entities::workspace::ActiveModel {
        id: ActiveValue::Set(ws.id),
        pinned: ActiveValue::Set(true),
        ..Default::default()
    }
    .update(&txn)
    .await?;
    txn.commit().await?;

    // send a status update to trigger frontend update
    state
        .conductor
        .add_workspace_update_event(
            Some(ws.user_id),
            ws.id,
            WorkspaceUpdateEvent::Status(WorkspaceStatus::from_str(&ws.status)?),
        )
        .await;

    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn unpin_workspace(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path((org_id, workspace_name)): Path<(Uuid, String)>,
    State(state): State<Arc<CoreState>>,
    info: RequestInfo,
) -> Result<Response, ApiError> {
    let user = state.authenticate(&cookie).await?;
    state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    let ws = state
        .db
        .get_workspace_by_name(&workspace_name)
        .await
        .map_err(|_| ApiError::InvalidRequest("workspace name doesn't exist".to_string()))?;
    if ws.user_id != user.id {
        return Err(ApiError::Unauthorized);
    }

    let org = state.db.get_organization(ws.organization_id).await?;
    if org.running_workspace_limit > 0 {
        return Err(ApiError::InvalidRequest(
            state
                .db
                .get_config(LAPDEV_PIN_UNPIN_ERROR)
                .await
                .unwrap_or_else(|_| "You can't pin/unpin workspaces".to_string()),
        ));
    }

    if ws.compose_parent.is_some() {
        return Err(ApiError::InvalidRequest(
            "you can only unpin the main workspace".to_string(),
        ));
    }

    if !ws.pinned {
        return Err(ApiError::InvalidRequest(
            "workspace is not pinned".to_string(),
        ));
    }

    let now = Utc::now();
    let txn = state.db.conn.begin().await?;
    state
        .conductor
        .enterprise
        .insert_audit_log(
            &txn,
            now.into(),
            ws.user_id,
            ws.organization_id,
            AuditResourceKind::Workspace.to_string(),
            ws.id,
            format!("{} unpin", ws.name),
            AuditAction::WorkspaceUpdate.to_string(),
            info.ip.clone(),
            info.user_agent.clone(),
        )
        .await?;
    let ws = lapdev_db_entities::workspace::ActiveModel {
        id: ActiveValue::Set(ws.id),
        pinned: ActiveValue::Set(false),
        ..Default::default()
    }
    .update(&txn)
    .await?;
    txn.commit().await?;

    // send a status update to trigger frontend update
    state
        .conductor
        .add_workspace_update_event(
            Some(ws.user_id),
            ws.id,
            WorkspaceUpdateEvent::Status(WorkspaceStatus::from_str(&ws.status)?),
        )
        .await;

    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn rebuild_workspace(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path((org_id, workspace_name)): Path<(Uuid, String)>,
    State(state): State<Arc<CoreState>>,
    info: RequestInfo,
) -> Result<Response, ApiError> {
    let user = state.authenticate(&cookie).await?;
    state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    let ws = state
        .db
        .get_workspace_by_name(&workspace_name)
        .await
        .map_err(|_| ApiError::InvalidRequest("workspace name doesn't exist".to_string()))?;
    if ws.user_id != user.id {
        return Err(ApiError::Unauthorized);
    }

    state
        .conductor
        .rebuild_workspace(ws, info.ip, info.user_agent)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn workspace_ports(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path((org_id, workspace_name)): Path<(Uuid, String)>,
    State(state): State<Arc<CoreState>>,
) -> Result<Response, ApiError> {
    let user = state.authenticate(&cookie).await?;
    state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    let ws = state
        .db
        .get_workspace_by_name(&workspace_name)
        .await
        .map_err(|_| ApiError::InvalidRequest("workspace name doesn't exist".to_string()))?;
    if ws.user_id != user.id {
        return Err(ApiError::Unauthorized);
    }
    let ports = state.db.get_workspace_ports(ws.id).await?;
    Ok(Json(
        ports
            .into_iter()
            .map(|p| WorkspacePort {
                port: p.port as u16,
                shared: p.shared,
                public: p.public,
                label: p.label,
            })
            .collect::<Vec<_>>(),
    )
    .into_response())
}

pub async fn update_workspace_port(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path((org_id, workspace_name, port)): Path<(Uuid, String, u16)>,
    State(state): State<Arc<CoreState>>,
    info: RequestInfo,
    Json(update_workspace_port): Json<UpdateWorkspacePort>,
) -> Result<Response, ApiError> {
    let user = state.authenticate(&cookie).await?;
    state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    let ws = state
        .db
        .get_workspace_by_name(&workspace_name)
        .await
        .map_err(|_| ApiError::InvalidRequest("workspace name doesn't exist".to_string()))?;
    if ws.user_id != user.id {
        return Err(ApiError::Unauthorized);
    }
    let port = state
        .db
        .get_workspace_port(ws.id, port)
        .await?
        .ok_or_else(|| ApiError::InvalidRequest(format!("port {port} not found")))?;

    let now = Utc::now();
    let txn = state.db.conn.begin().await?;
    state
        .conductor
        .enterprise
        .insert_audit_log(
            &txn,
            now.into(),
            ws.user_id,
            ws.organization_id,
            AuditResourceKind::Workspace.to_string(),
            ws.id,
            format!("{} port {}", ws.name, port.port),
            AuditAction::WorkspaceUpdate.to_string(),
            info.ip.clone(),
            info.user_agent.clone(),
        )
        .await?;
    lapdev_db_entities::workspace_port::ActiveModel {
        id: ActiveValue::Set(port.id),
        shared: ActiveValue::Set(update_workspace_port.shared),
        public: ActiveValue::Set(update_workspace_port.public),
        ..Default::default()
    }
    .update(&txn)
    .await?;
    txn.commit().await?;

    Ok(StatusCode::NO_CONTENT.into_response())
}
