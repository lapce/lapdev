use std::str::FromStr;

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use axum_extra::{headers::Cookie, TypedHeader};
use chrono::Utc;
use hyper::StatusCode;
use lapdev_common::{
    AuditAction, AuditResourceKind, NewProject, NewProjectPrebuild, PrebuildStatus, ProjectInfo,
    ProjectPrebuild, UserRole,
};
use lapdev_db::entities;
use lapdev_rpc::error::ApiError;
use sea_orm::{ActiveModelTrait, ActiveValue, TransactionTrait};
use uuid::Uuid;

use crate::state::{CoreState, RequestInfo};

pub async fn create_project(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path(org_id): Path<Uuid>,
    State(state): State<CoreState>,
    info: RequestInfo,
    Json(project): Json<NewProject>,
) -> Result<Response, ApiError> {
    let user = state.authenticate(&cookie).await?;
    state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    let result = state
        .conductor
        .create_project(user, org_id, project, info.ip, info.user_agent)
        .await
        .map(Json);
    if let Err(ApiError::InternalError(err)) = &result {
        tracing::error!("create workspace internal error: {err}");
    }
    Ok(result.into_response())
}

pub async fn all_projects(
    State(state): State<CoreState>,
    Path(org_id): Path<Uuid>,
    TypedHeader(cookie): TypedHeader<Cookie>,
) -> Result<Response, ApiError> {
    let user = state.authenticate(&cookie).await?;
    state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    let projects = state.db.get_all_projects(org_id).await?;
    let projects: Vec<ProjectInfo> = projects
        .into_iter()
        .map(|p| ProjectInfo {
            id: p.id,
            name: p.name,
            repo_url: p.repo_url,
            repo_name: p.repo_name,
            machine_type: p.machine_type_id,
            created_at: p.created_at,
        })
        .collect();
    Ok(Json(projects).into_response())
}

pub async fn get_project(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path((org_id, project_id)): Path<(Uuid, Uuid)>,
    State(state): State<CoreState>,
) -> Result<Json<ProjectInfo>, ApiError> {
    let (_, project) = state.get_project(&cookie, org_id, project_id).await?;

    let info = ProjectInfo {
        id: project.id,
        name: project.name,
        repo_url: project.repo_url,
        repo_name: project.repo_name,
        machine_type: project.machine_type_id,
        created_at: project.created_at,
    };
    Ok(Json(info))
}

pub async fn delete_project(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path((org_id, project_id)): Path<(Uuid, Uuid)>,
    State(state): State<CoreState>,
    info: RequestInfo,
) -> Result<Response, ApiError> {
    let (user, project) = state.get_project(&cookie, org_id, project_id).await?;
    let member = state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    if user.id != project.created_by
        && member.role != UserRole::Owner.to_string()
        && member.role != UserRole::Admin.to_string()
    {
        return Err(ApiError::InvalidRequest(
            "Only project owner or orgnization admin can delete the project".to_string(),
        ));
    }

    state
        .conductor
        .delete_project(org_id, user.id, &project, info.ip, info.user_agent)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn get_project_branches(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path((org_id, project_id)): Path<(Uuid, Uuid)>,
    State(state): State<CoreState>,
    info: RequestInfo,
) -> Result<Response, ApiError> {
    let (user, project) = state.get_project(&cookie, org_id, project_id).await?;
    let auth = if let Ok(Some(oauth)) = state.db.get_oauth(project.oauth_id).await {
        (oauth.provider_login, oauth.access_token)
    } else {
        let oauth = state
            .conductor
            .find_match_oauth_for_repo(&user, &project.repo_url)
            .await?;
        (oauth.provider_login, oauth.access_token)
    };
    let branches = state
        .conductor
        .project_branches(user.id, &project, auth, info.ip, info.user_agent)
        .await?;
    Ok(Json(branches).into_response())
}

pub async fn get_project_prebuilds(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path((org_id, project_id)): Path<(Uuid, Uuid)>,
    State(state): State<CoreState>,
) -> Result<Response, ApiError> {
    let (_, project) = state.get_project(&cookie, org_id, project_id).await?;
    let prebuilds = state.db.get_prebuilds(project.id).await?;
    let prebuilds: Vec<ProjectPrebuild> = prebuilds
        .into_iter()
        .filter_map(|prebuild| {
            Some(ProjectPrebuild {
                id: prebuild.id,
                project_id: prebuild.project_id,
                created_at: prebuild.created_at,
                branch: prebuild.branch,
                commit: prebuild.commit,
                status: PrebuildStatus::from_str(&prebuild.status).ok()?,
            })
        })
        .collect();
    Ok(Json(prebuilds).into_response())
}

pub async fn create_project_prebuild(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path((org_id, project_id)): Path<(Uuid, Uuid)>,
    State(state): State<CoreState>,
    info: RequestInfo,
    Json(prebuild): Json<NewProjectPrebuild>,
) -> Result<Response, ApiError> {
    let (user, project) = state.get_project(&cookie, org_id, project_id).await?;
    let repo = state
        .conductor
        .get_project_repo_details(
            &user,
            &project,
            Some(&prebuild.branch),
            info.ip.clone(),
            info.user_agent.clone(),
        )
        .await?;
    let prebuild = state
        .conductor
        .create_project_prebuild(&user, &project, None, &repo, info.ip, info.user_agent)
        .await?;

    let status = PrebuildStatus::from_str(&prebuild.status)?;
    let prebuild = Json(ProjectPrebuild {
        id: prebuild.id,
        project_id: prebuild.project_id,
        created_at: prebuild.created_at,
        branch: prebuild.branch,
        commit: prebuild.commit,
        status,
    });

    Ok(prebuild.into_response())
}

pub async fn delete_project_prebuild(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path((org_id, project_id, prebuild_id)): Path<(Uuid, Uuid, Uuid)>,
    State(state): State<CoreState>,
    info: RequestInfo,
) -> Result<Response, ApiError> {
    let (user, project) = state.get_project(&cookie, org_id, project_id).await?;
    let prebuild = state
        .db
        .get_prebuild(prebuild_id)
        .await?
        .ok_or_else(|| ApiError::InvalidRequest("Prebuild doesn't exist".to_string()))?;
    if prebuild.project_id != project.id {
        return Err(ApiError::Unauthorized);
    }
    state
        .conductor
        .delete_prebuild(
            org_id,
            user.id,
            &project,
            &prebuild,
            info.ip,
            info.user_agent,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn update_project_machine_type(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path((org_id, project_id, machine_type_id)): Path<(Uuid, Uuid, Uuid)>,
    State(state): State<CoreState>,
    info: RequestInfo,
) -> Result<Response, ApiError> {
    let (user, project) = state.get_project(&cookie, org_id, project_id).await?;
    let member = state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    if user.id != project.created_by
        && member.role != UserRole::Owner.to_string()
        && member.role != UserRole::Admin.to_string()
    {
        return Err(ApiError::InvalidRequest(
            "Only project owner or orgnization admin can update the project".to_string(),
        ));
    }
    let machine_type = state
        .db
        .get_machine_type(machine_type_id)
        .await?
        .ok_or_else(|| ApiError::InvalidRequest("Machine type doesn't exist".to_string()))?;

    let txn = state.db.conn.begin().await?;
    entities::project::ActiveModel {
        id: ActiveValue::Set(project.id),
        machine_type_id: ActiveValue::Set(machine_type.id),
        ..Default::default()
    }
    .update(&txn)
    .await?;
    state
        .conductor
        .enterprise
        .insert_audit_log(
            &txn,
            Utc::now().into(),
            user.id,
            org_id,
            AuditResourceKind::Project.to_string(),
            project.id,
            project.name.clone(),
            AuditAction::ProjectUpdateMachineType.to_string(),
            info.ip,
            info.user_agent,
        )
        .await?;
    txn.commit().await?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn get_project_env(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path((org_id, project_id)): Path<(Uuid, Uuid)>,
    State(state): State<CoreState>,
) -> Result<Json<Vec<(String, String)>>, ApiError> {
    let (user, project) = state.get_project(&cookie, org_id, project_id).await?;
    state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    let envs: Option<Vec<(String, String)>> = project
        .env
        .as_ref()
        .and_then(|envs| serde_json::from_str(envs).ok());
    Ok(Json(envs.unwrap_or_default()))
}

pub async fn update_project_env(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path((org_id, project_id)): Path<(Uuid, Uuid)>,
    State(state): State<CoreState>,
    info: RequestInfo,
    Json(envs): Json<Vec<(String, String)>>,
) -> Result<Response, ApiError> {
    let (user, project) = state.get_project(&cookie, org_id, project_id).await?;
    let member = state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    if user.id != project.created_by
        && member.role != UserRole::Owner.to_string()
        && member.role != UserRole::Admin.to_string()
    {
        return Err(ApiError::InvalidRequest(
            "Only project owner or orgnization admin can update the project".to_string(),
        ));
    }

    let env = serde_json::to_string(&envs)?;

    let txn = state.db.conn.begin().await?;
    entities::project::ActiveModel {
        id: ActiveValue::Set(project.id),
        env: ActiveValue::Set(Some(env)),
        ..Default::default()
    }
    .update(&txn)
    .await?;
    state
        .conductor
        .enterprise
        .insert_audit_log(
            &txn,
            Utc::now().into(),
            user.id,
            org_id,
            AuditResourceKind::Project.to_string(),
            project.id,
            project.name.clone(),
            AuditAction::ProjectUpdateEnv.to_string(),
            info.ip,
            info.user_agent,
        )
        .await?;
    txn.commit().await?;

    Ok(StatusCode::NO_CONTENT.into_response())
}
