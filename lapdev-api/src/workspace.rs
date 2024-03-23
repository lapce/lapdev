use std::{collections::HashMap, str::FromStr};

use anyhow::Result;
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use axum_extra::{headers::Cookie, TypedHeader};
use hyper::StatusCode;
use lapdev_common::{NewWorkspace, WorkspaceInfo, WorkspaceService, WorkspaceStatus};
use lapdev_db::entities;
use lapdev_rpc::error::ApiError;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use tracing::error;
use uuid::Uuid;

use crate::state::{CoreState, RequestInfo};

pub async fn create_workspace(
    info: RequestInfo,
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path(org_id): Path<Uuid>,
    State(state): State<CoreState>,
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
    State(state): State<CoreState>,
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
            })
        })
        .collect();
    Ok(Json(workspaces))
}

pub async fn delete_workspace(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path((org_id, workspace_name)): Path<(Uuid, String)>,
    State(state): State<CoreState>,
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
        .delete_workspace(ws, info.ip, info.user_agent)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn get_workspace(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path((org_id, workspace_name)): Path<(Uuid, String)>,
    State(state): State<CoreState>,
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
        entities::workspace::Entity::find()
            .filter(entities::workspace::Column::DeletedAt.is_null())
            .filter(entities::workspace::Column::ComposeParent.eq(ws.id))
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
    };
    Ok(Json(info))
}

pub async fn start_workspace(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path((org_id, workspace_name)): Path<(Uuid, String)>,
    State(state): State<CoreState>,
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
    State(state): State<CoreState>,
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
