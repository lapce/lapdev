use std::{str::FromStr, time::Duration};

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    Json,
};
use axum_extra::{headers::Cookie, TypedHeader};
use chrono::Utc;
use hyper::StatusCode;
use lapdev_common::{
    console::{Organization, OrganizationMember},
    AuditAction, AuditLogRequest, AuditLogResult, AuditResourceKind, NewOrganization, OrgQuota,
    UpdateOrgQuota, UpdateOrganizationAutoStartStop, UpdateOrganizationMember,
    UpdateOrganizationName, UsageRequest, UsageResult, UserRole,
};
use lapdev_db::entities;
use lapdev_rpc::error::ApiError;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter, TransactionTrait,
};
use uuid::Uuid;

use crate::state::{CoreState, RequestInfo};

pub async fn create_organization(
    TypedHeader(cookie): TypedHeader<Cookie>,
    State(state): State<CoreState>,
    info: RequestInfo,
    Json(org): Json<NewOrganization>,
) -> Result<Response, ApiError> {
    let user = state.authenticate(&cookie).await?;
    let name = org.name.trim();
    if name.is_empty() {
        return Err(ApiError::InvalidRequest(
            "organization name can't be empty".to_string(),
        ));
    }

    let now = Utc::now();
    let txn = state.db.conn.begin().await?;

    let org = state
        .db
        .create_new_organization(&txn, name.to_string())
        .await?;

    entities::organization_member::ActiveModel {
        created_at: ActiveValue::Set(Utc::now().into()),
        user_id: ActiveValue::Set(user.id),
        organization_id: ActiveValue::Set(org.id),
        role: ActiveValue::Set(UserRole::Owner.to_string()),
        ..Default::default()
    }
    .insert(&txn)
    .await?;

    entities::user::ActiveModel {
        id: ActiveValue::Set(user.id),
        current_organization: ActiveValue::Set(org.id),
        ..Default::default()
    }
    .update(&txn)
    .await?;

    state
        .conductor
        .enterprise
        .insert_audit_log(
            &txn,
            now.into(),
            user.id,
            org.id,
            AuditResourceKind::Organization.to_string(),
            org.id,
            org.name.clone(),
            AuditAction::OrganizationCreate.to_string(),
            info.ip,
            info.user_agent,
        )
        .await?;

    txn.commit().await?;

    Ok(Json(Organization {
        id: org.id,
        name: org.name,
        role: UserRole::Owner,
        auto_start: org.auto_start,
        auto_stop: org.auto_stop,
        allow_workspace_change_auto_start: org.allow_workspace_change_auto_start,
        allow_workspace_change_auto_stop: org.allow_workspace_change_auto_stop,
    })
    .into_response())
}

pub async fn delete_organization(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path(org_id): Path<Uuid>,
    State(state): State<CoreState>,
    info: RequestInfo,
) -> Result<Response, ApiError> {
    let user = state.authenticate(&cookie).await?;
    let member = state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    if member.role != UserRole::Owner.to_string() {
        return Err(ApiError::Unauthorized);
    }
    let org = state.db.get_organization(member.organization_id).await?;

    let orgs = state.db.get_user_organizations(user.id).await?;
    if orgs.len() == 1 {
        return Err(ApiError::InvalidRequest(
            "you can't delete the last organization you are in".to_string(),
        ));
    }

    let now = Utc::now();
    let txn = state.db.conn.begin().await?;

    entities::organization_member::ActiveModel {
        id: ActiveValue::Set(member.id),
        deleted_at: ActiveValue::Set(Some(now.into())),
        ..Default::default()
    }
    .update(&txn)
    .await?;

    entities::organization::ActiveModel {
        id: ActiveValue::Set(org.id),
        deleted_at: ActiveValue::Set(Some(now.into())),
        ..Default::default()
    }
    .update(&txn)
    .await?;

    state
        .conductor
        .enterprise
        .insert_audit_log(
            &txn,
            now.into(),
            user.id,
            org.id,
            AuditResourceKind::Organization.to_string(),
            org.id,
            org.name.clone(),
            AuditAction::OrganizationDelete.to_string(),
            info.ip,
            info.user_agent,
        )
        .await?;

    txn.commit().await?;

    Ok(().into_response())
}

pub async fn update_org_name(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path(org_id): Path<Uuid>,
    State(state): State<CoreState>,
    info: RequestInfo,
    Json(update_org): Json<UpdateOrganizationName>,
) -> Result<Response, ApiError> {
    let user = state.authenticate(&cookie).await?;
    let member = state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    if member.role != UserRole::Owner.to_string() && member.role != UserRole::Admin.to_string() {
        return Err(ApiError::Unauthorized);
    }
    let org = state.db.get_organization(member.organization_id).await?;

    let now = Utc::now();
    let txn = state.db.conn.begin().await?;

    entities::organization::ActiveModel {
        id: ActiveValue::Set(org_id),
        name: ActiveValue::Set(update_org.name),
        ..Default::default()
    }
    .update(&txn)
    .await?;

    state
        .conductor
        .enterprise
        .insert_audit_log(
            &txn,
            now.into(),
            user.id,
            org.id,
            AuditResourceKind::Organization.to_string(),
            org.id,
            org.name.clone(),
            AuditAction::OrganizationUpdateName.to_string(),
            info.ip,
            info.user_agent,
        )
        .await?;

    txn.commit().await?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn update_org_auto_start_stop(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path(org_id): Path<Uuid>,
    State(state): State<CoreState>,
    info: RequestInfo,
    Json(update_org): Json<UpdateOrganizationAutoStartStop>,
) -> Result<Response, ApiError> {
    state.require_enterprise().await?;
    let user = state.authenticate(&cookie).await?;
    let member = state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    if member.role != UserRole::Owner.to_string() && member.role != UserRole::Admin.to_string() {
        return Err(ApiError::Unauthorized);
    }
    let org = state.db.get_organization(member.organization_id).await?;

    let now = Utc::now();
    let txn = state.db.conn.begin().await?;

    entities::organization::ActiveModel {
        id: ActiveValue::Set(org_id),
        auto_start: ActiveValue::Set(update_org.auto_start),
        auto_stop: ActiveValue::Set(update_org.auto_stop),
        allow_workspace_change_auto_start: ActiveValue::Set(
            update_org.allow_workspace_change_auto_start,
        ),
        allow_workspace_change_auto_stop: ActiveValue::Set(
            update_org.allow_workspace_change_auto_stop,
        ),
        ..Default::default()
    }
    .update(&txn)
    .await?;

    state
        .conductor
        .enterprise
        .insert_audit_log(
            &txn,
            now.into(),
            user.id,
            org.id,
            AuditResourceKind::Organization.to_string(),
            org.id,
            org.name.clone(),
            AuditAction::OrganizationUpdate.to_string(),
            info.ip,
            info.user_agent,
        )
        .await?;

    txn.commit().await?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn join_organization(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path(invitation_id): Path<Uuid>,
    State(state): State<CoreState>,
    info: RequestInfo,
) -> Result<Response, ApiError> {
    let user = state.authenticate(&cookie).await?;

    let invitation = entities::user_invitation::Entity::find()
        .filter(entities::user_invitation::Column::Id.eq(invitation_id))
        .filter(entities::user_invitation::Column::UsedAt.is_null())
        .one(&state.db.conn)
        .await?
        .ok_or_else(|| ApiError::InvalidRequest("invitation id is invalid".to_string()))?;
    if invitation.expires_at < Utc::now() {
        return Err(ApiError::InvalidRequest(
            "invitation is expired".to_string(),
        ));
    }

    let org_id = invitation.organization_id;
    let member = entities::organization_member::Entity::find()
        .filter(entities::organization_member::Column::UserId.eq(user.id))
        .filter(entities::organization_member::Column::OrganizationId.eq(org_id))
        .filter(entities::organization_member::Column::DeletedAt.is_null())
        .one(&state.db.conn)
        .await?;
    if member.is_some() {
        return Err(ApiError::InvalidRequest(
            "user is already a member of the organization".to_string(),
        ));
    }

    let now = Utc::now();
    let txn = state.db.conn.begin().await?;
    entities::organization_member::ActiveModel {
        created_at: ActiveValue::Set(now.into()),
        user_id: ActiveValue::Set(user.id),
        organization_id: ActiveValue::Set(org_id),
        role: ActiveValue::Set(UserRole::Member.to_string()),
        ..Default::default()
    }
    .insert(&txn)
    .await?;

    entities::user::ActiveModel {
        id: ActiveValue::Set(user.id),
        current_organization: ActiveValue::Set(org_id),
        ..Default::default()
    }
    .update(&txn)
    .await?;

    entities::user_invitation::ActiveModel {
        id: ActiveValue::Set(invitation_id),
        used_at: ActiveValue::Set(Some(now.into())),
        ..Default::default()
    }
    .update(&txn)
    .await?;

    state
        .conductor
        .enterprise
        .insert_audit_log(
            &txn,
            now.into(),
            user.id,
            org_id,
            AuditResourceKind::User.to_string(),
            user.id,
            user.name.clone().unwrap_or_else(|| "".to_string()),
            AuditAction::OrganizationJoin.to_string(),
            info.ip,
            info.user_agent,
        )
        .await?;
    txn.commit().await?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn get_organization_members(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path(org_id): Path<Uuid>,
    State(state): State<CoreState>,
) -> Result<Json<Vec<OrganizationMember>>, ApiError> {
    let user = state.authenticate(&cookie).await?;
    let member = state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    if member.role != UserRole::Owner.to_string() && member.role != UserRole::Admin.to_string() {
        return Err(ApiError::Unauthorized);
    }
    let org = state.db.get_organization(member.organization_id).await?;

    let mut users = Vec::new();
    let members = state.db.get_all_organization_members(org.id).await?;
    for member in members {
        if let Ok(Some(user)) = state.db.get_user(member.user_id).await {
            if let Ok(role) = UserRole::from_str(&member.role) {
                users.push(OrganizationMember {
                    user_id: user.id,
                    avatar_url: user.avatar_url,
                    name: user.name,
                    role,
                    joined: member.created_at,
                });
            }
        }
    }

    Ok(Json(users))
}

pub async fn update_organization_member(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path((org_id, member_user_id)): Path<(Uuid, Uuid)>,
    State(state): State<CoreState>,
    info: RequestInfo,
    Json(update_org_member): Json<UpdateOrganizationMember>,
) -> Result<Json<OrganizationMember>, ApiError> {
    let user = state.authenticate(&cookie).await?;
    let member = state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    if member.role != UserRole::Owner.to_string() && member.role != UserRole::Admin.to_string() {
        return Err(ApiError::Unauthorized);
    }
    state.db.get_organization(member.organization_id).await?;

    let member = state
        .db
        .get_organization_member(member_user_id, org_id)
        .await
        .map_err(|_| ApiError::InvalidRequest(format!("member {member_user_id} doesn't exist")))?;
    if member.role == UserRole::Owner.to_string() {
        return Err(ApiError::InvalidRequest(
            "organization owner can't be updated".to_string(),
        ));
    }
    let member_user = state.db.get_user(member_user_id).await?.ok_or_else(|| {
        ApiError::InvalidRequest(format!("member {member_user_id} doesn't exist"))
    })?;

    let now = Utc::now();
    let txn = state.db.conn.begin().await?;

    let member = entities::organization_member::ActiveModel {
        id: ActiveValue::Set(member.id),
        role: ActiveValue::Set(update_org_member.role.to_string()),
        ..Default::default()
    }
    .update(&txn)
    .await?;

    state
        .conductor
        .enterprise
        .insert_audit_log(
            &txn,
            now.into(),
            user.id,
            org_id,
            AuditResourceKind::User.to_string(),
            user.id,
            user.name.clone().unwrap_or_else(|| "".to_string()),
            AuditAction::OrganizationUpdateMember.to_string(),
            info.ip,
            info.user_agent,
        )
        .await?;
    txn.commit().await?;

    Ok(Json(OrganizationMember {
        user_id: member_user.id,
        avatar_url: member_user.avatar_url,
        name: member_user.name,
        role: UserRole::from_str(&member.role)?,
        joined: member.created_at,
    }))
}

pub async fn delete_organization_member(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path((org_id, member_user_id)): Path<(Uuid, Uuid)>,
    State(state): State<CoreState>,
    info: RequestInfo,
) -> Result<Response, ApiError> {
    let user = state.authenticate(&cookie).await?;
    let member = state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    if member.role != UserRole::Owner.to_string() && member.role != UserRole::Admin.to_string() {
        return Err(ApiError::Unauthorized);
    }
    state.db.get_organization(member.organization_id).await?;

    let member = state
        .db
        .get_organization_member(member_user_id, org_id)
        .await
        .map_err(|_| ApiError::InvalidRequest(format!("member {member_user_id} doesn't exist")))?;
    if member.role == UserRole::Owner.to_string() {
        return Err(ApiError::InvalidRequest(
            "organization owner can't be deleted".to_string(),
        ));
    }

    let now = Utc::now();
    let txn = state.db.conn.begin().await?;

    entities::organization_member::ActiveModel {
        id: ActiveValue::Set(member.id),
        deleted_at: ActiveValue::Set(Some(now.into())),
        ..Default::default()
    }
    .update(&txn)
    .await?;

    state
        .conductor
        .enterprise
        .insert_audit_log(
            &txn,
            now.into(),
            user.id,
            org_id,
            AuditResourceKind::User.to_string(),
            user.id,
            user.name.clone().unwrap_or_else(|| "".to_string()),
            AuditAction::OrganizationDeleteMember.to_string(),
            info.ip,
            info.user_agent,
        )
        .await?;
    txn.commit().await?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn create_user_invitation(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path(org_id): Path<Uuid>,
    State(state): State<CoreState>,
) -> Result<String, ApiError> {
    let user = state.authenticate(&cookie).await?;
    let member = state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    if member.role != UserRole::Owner.to_string() && member.role != UserRole::Admin.to_string() {
        return Err(ApiError::Unauthorized);
    }
    let org = state.db.get_organization(member.organization_id).await?;

    let now = Utc::now();
    let invitation = entities::user_invitation::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        created_at: ActiveValue::Set(now.into()),
        expires_at: ActiveValue::Set((now + Duration::from_secs(1800)).into()),
        organization_id: ActiveValue::Set(org.id),
        used_at: ActiveValue::Set(None),
    }
    .insert(&state.db.conn)
    .await?;

    Ok(invitation.id.to_string())
}

pub async fn get_organization_usage(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path(org_id): Path<Uuid>,
    State(state): State<CoreState>,
    Query(usage_request): Query<UsageRequest>,
) -> Result<Json<UsageResult>, ApiError> {
    state.require_enterprise().await?;
    let user = state.authenticate(&cookie).await?;
    let member = state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    if member.role != UserRole::Owner.to_string() && member.role != UserRole::Admin.to_string() {
        return Err(ApiError::Unauthorized);
    }
    let org = state.db.get_organization(member.organization_id).await?;

    let usage = state
        .conductor
        .enterprise
        .usage
        .get_usage_records(
            org.id,
            None,
            usage_request.start,
            usage_request.end,
            usage_request.page_size,
            usage_request.page,
        )
        .await?;

    Ok(Json(usage))
}

pub async fn get_organization_audit_log(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path(org_id): Path<Uuid>,
    State(state): State<CoreState>,
    Query(audit_log_request): Query<AuditLogRequest>,
) -> Result<Json<AuditLogResult>, ApiError> {
    state.require_enterprise().await?;
    let user = state.authenticate(&cookie).await?;
    let member = state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    if member.role != UserRole::Owner.to_string() && member.role != UserRole::Admin.to_string() {
        return Err(ApiError::Unauthorized);
    }
    let org = state.db.get_organization(member.organization_id).await?;

    let result = state
        .conductor
        .enterprise
        .get_audit_logs(
            org.id,
            audit_log_request.start,
            audit_log_request.end,
            audit_log_request.page_size,
            audit_log_request.page,
        )
        .await?;

    Ok(Json(result))
}

pub async fn get_organization_quota(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path(org_id): Path<Uuid>,
    State(state): State<CoreState>,
) -> Result<Json<OrgQuota>, ApiError> {
    state.require_enterprise().await?;
    let user = state.authenticate(&cookie).await?;
    let member = state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    if member.role != UserRole::Owner.to_string() && member.role != UserRole::Admin.to_string() {
        return Err(ApiError::Unauthorized);
    }
    let org = state.db.get_organization(member.organization_id).await?;

    let result = state
        .conductor
        .enterprise
        .quota
        .get_org_quota(org.id)
        .await?;

    Ok(Json(result))
}

pub async fn update_organization_quota(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path(org_id): Path<Uuid>,
    State(state): State<CoreState>,
    info: RequestInfo,
    Json(update_quota): Json<UpdateOrgQuota>,
) -> Result<StatusCode, ApiError> {
    state.require_enterprise().await?;
    let user = state.authenticate(&cookie).await?;
    let member = state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    if member.role != UserRole::Owner.to_string() && member.role != UserRole::Admin.to_string() {
        return Err(ApiError::Unauthorized);
    }
    let org = state.db.get_organization(member.organization_id).await?;

    let now = Utc::now();
    let txn = state.db.conn.begin().await?;
    state
        .conductor
        .enterprise
        .quota
        .update_for_default_user(
            &txn,
            org.id,
            update_quota.kind,
            update_quota.default_user_quota,
        )
        .await?;
    state
        .conductor
        .enterprise
        .quota
        .update_for_organization(&txn, org.id, update_quota.kind, update_quota.org_quota)
        .await?;
    state
        .conductor
        .enterprise
        .insert_audit_log(
            &txn,
            now.into(),
            user.id,
            org.id,
            AuditResourceKind::Organization.to_string(),
            org.id,
            org.name.clone(),
            AuditAction::OrganizationUpdateQuota.to_string(),
            info.ip,
            info.user_agent,
        )
        .await?;
    txn.commit().await?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn clear_organization(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path(org_id): Path<Uuid>,
    State(state): State<CoreState>,
) -> Result<StatusCode, ApiError> {
    state.authenticate_cluster_admin(&cookie).await?;
    state.db.get_organization(org_id).await?;

    let models = entities::workspace::Entity::find()
        .filter(entities::workspace::Column::OrganizationId.eq(org_id))
        .filter(entities::workspace::Column::DeletedAt.is_null())
        .all(&state.db.conn)
        .await?;

    for ws in models {
        state.conductor.delete_workspace(&ws, None, None).await?;
    }

    Ok(StatusCode::NO_CONTENT)
}
