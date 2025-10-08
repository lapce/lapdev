use std::{collections::HashMap, str::FromStr, sync::Arc};

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    Json,
};
use axum_extra::{
    extract::Host,
    headers::{self, Cookie},
    TypedHeader,
};
use chrono::Utc;
use hyper::StatusCode;
use lapdev_common::{
    console::{MeUser, NewSessionResponse, Organization},
    GitProvider, NewSshKey, SshKey, UserRole,
};
use lapdev_db::api::DbApi;
use lapdev_rpc::error::ApiError;
use russh::keys::PublicKeyBase64;
use sea_orm::{prelude::Uuid, ActiveModelTrait, ActiveValue};

use crate::{session::create_oauth_connection, state::CoreState};

pub async fn me(
    State(state): State<Arc<CoreState>>,
    TypedHeader(cookies): TypedHeader<headers::Cookie>,
) -> Result<Response, ApiError> {
    let user = state.authenticate(&cookies).await?;
    let (org, member) = user_current_organization(&user, &state.db).await?;
    let all_orgs = all_user_organizations(&user, &state.db)
        .await
        .unwrap_or_default();
    Ok(Json(MeUser {
        avatar_url: user.avatar_url,
        email: user.email,
        name: user.name,
        cluster_admin: user.cluster_admin,
        organization: Organization {
            id: org.id,
            name: org.name,
            role: UserRole::from_str(&member.role)?,
            auto_start: org.auto_start,
            auto_stop: org.auto_stop,
            allow_workspace_change_auto_start: org.allow_workspace_change_auto_start,
            allow_workspace_change_auto_stop: org.allow_workspace_change_auto_stop,
        },
        all_organizations: all_orgs
            .into_iter()
            .filter_map(|(org, member)| {
                Some(Organization {
                    id: org.id,
                    name: org.name,
                    role: UserRole::from_str(&member.role).ok()?,
                    auto_start: org.auto_start,
                    auto_stop: org.auto_stop,
                    allow_workspace_change_auto_start: org.allow_workspace_change_auto_start,
                    allow_workspace_change_auto_stop: org.allow_workspace_change_auto_stop,
                })
            })
            .collect(),
    })
    .into_response())
}

pub async fn set_current_organization(
    State(state): State<Arc<CoreState>>,
    Path(org_id): Path<Uuid>,
    TypedHeader(cookies): TypedHeader<headers::Cookie>,
) -> Result<Response, ApiError> {
    let user = state.authenticate(&cookies).await?;
    state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    lapdev_db_entities::user::ActiveModel {
        id: ActiveValue::Set(user.id),
        current_organization: ActiveValue::Set(org_id),
        ..Default::default()
    }
    .update(&state.db.conn)
    .await?;
    Ok(().into_response())
}

async fn user_current_organization(
    user: &lapdev_db_entities::user::Model,
    db: &DbApi,
) -> anyhow::Result<(
    lapdev_db_entities::organization::Model,
    lapdev_db_entities::organization_member::Model,
)> {
    if let Ok(r) = user_organization(user, user.current_organization, db).await {
        return Ok(r);
    }
    pick_user_organization(user, db).await
}

async fn user_organization(
    user: &lapdev_db_entities::user::Model,
    org_id: Uuid,
    db: &DbApi,
) -> anyhow::Result<(
    lapdev_db_entities::organization::Model,
    lapdev_db_entities::organization_member::Model,
)> {
    let org = db.get_organization(org_id).await?;
    let member = db.get_organization_member(user.id, org.id).await?;
    Ok((org, member))
}

async fn pick_user_organization(
    user: &lapdev_db_entities::user::Model,
    db: &DbApi,
) -> anyhow::Result<(
    lapdev_db_entities::organization::Model,
    lapdev_db_entities::organization_member::Model,
)> {
    let org_members = db.get_user_organizations(user.id).await?;
    for org_member in org_members {
        if let Ok(org) = db.get_organization(org_member.organization_id).await {
            lapdev_db_entities::user::ActiveModel {
                id: ActiveValue::Set(user.id),
                current_organization: ActiveValue::Set(org.id),
                ..Default::default()
            }
            .update(&db.conn)
            .await?;
            return Ok((org, org_member));
        }
    }
    Err(anyhow::anyhow!("don't have any orgnizations"))
}

async fn all_user_organizations(
    user: &lapdev_db_entities::user::Model,
    db: &DbApi,
) -> anyhow::Result<
    Vec<(
        lapdev_db_entities::organization::Model,
        lapdev_db_entities::organization_member::Model,
    )>,
> {
    let mut result = Vec::new();
    let org_members = db.get_user_organizations(user.id).await?;
    for org_member in org_members {
        if let Ok(org) = db.get_organization(org_member.organization_id).await {
            result.push((org, org_member));
        }
    }
    Ok(result)
}

pub async fn create_ssh_key(
    TypedHeader(cookie): TypedHeader<Cookie>,
    State(state): State<Arc<CoreState>>,
    Json(ssh_key): Json<NewSshKey>,
) -> Result<Response, ApiError> {
    let user = state.authenticate(&cookie).await?;
    let name = ssh_key.name.trim();
    if name.is_empty() {
        return Err(ApiError::InvalidRequest(
            "ssh key name can't be empty".to_string(),
        ));
    }

    let mut split = ssh_key.key.split_whitespace();
    let key = match (split.next(), split.next()) {
        (Some(_), Some(key)) => key,
        (Some(key), None) => key,
        _ => {
            return Err(ApiError::InvalidRequest(
                "The SSH public key is invalid".to_string(),
            ))
        }
    };
    let parsed_key = russh::keys::parse_public_key_base64(key)
        .map_err(|_| ApiError::InvalidRequest("The SSH public key is invalid".to_string()))?;
    let parsed_key = parsed_key.public_key_base64();

    let model = lapdev_db_entities::ssh_public_key::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        created_at: ActiveValue::Set(Utc::now().into()),
        name: ActiveValue::Set(name.to_string()),
        key: ActiveValue::Set(ssh_key.key),
        parsed_key: ActiveValue::Set(parsed_key),
        user_id: ActiveValue::Set(user.id),
        ..Default::default()
    }
    .insert(&state.db.conn)
    .await?;

    Ok(Json(SshKey {
        id: model.id,
        name: model.name,
        key: model.key,
        created_at: model.created_at,
    })
    .into_response())
}

pub async fn all_ssh_keys(
    State(state): State<Arc<CoreState>>,
    TypedHeader(cookie): TypedHeader<Cookie>,
) -> Result<Response, ApiError> {
    let user = state.authenticate(&cookie).await?;
    let ssh_keys = state.db.get_all_ssh_keys(user.id).await?;
    Ok(Json(
        ssh_keys
            .into_iter()
            .map(|key| SshKey {
                id: key.id,
                name: key.name,
                key: key.key,
                created_at: key.created_at,
            })
            .collect::<Vec<_>>(),
    )
    .into_response())
}

pub async fn delete_ssh_key(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path(key_id): Path<Uuid>,
    State(state): State<Arc<CoreState>>,
) -> Result<Response, ApiError> {
    let user = state.authenticate(&cookie).await?;
    let key = state.db.get_ssh_key(key_id).await?;
    if key.user_id != user.id {
        return Err(ApiError::Unauthorized);
    }

    lapdev_db_entities::ssh_public_key::ActiveModel {
        id: ActiveValue::Set(key.id),
        deleted_at: ActiveValue::Set(Some(Utc::now().into())),
        ..Default::default()
    }
    .update(&state.db.conn)
    .await?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn get_git_providers(
    TypedHeader(cookie): TypedHeader<Cookie>,
    State(state): State<Arc<CoreState>>,
) -> Result<impl IntoResponse, ApiError> {
    let user = state.authenticate(&cookie).await?;
    let all_oauths = state.db.get_user_all_oauth(user.id).await?;

    let mut git_providers = Vec::new();
    for (auth_provider, (_, config)) in state.auth.clients.read().await.iter() {
        let oauth = all_oauths
            .iter()
            .find(|o| o.provider == auth_provider.to_string());

        let git_provider = GitProvider {
            auth_provider: *auth_provider,
            connected: oauth.is_some(),
            avatar_url: oauth.as_ref().and_then(|o| o.avatar_url.clone()),
            email: oauth.as_ref().and_then(|o| o.email.clone()),
            name: oauth.as_ref().and_then(|o| o.name.clone()),
            read_repo: oauth.as_ref().map(|o| o.read_repo.unwrap_or(false)),
            scopes: config.scopes.iter().map(|s| s.to_string()).collect(),
            all_scopes: config
                .read_repo_scopes
                .iter()
                .map(|s| s.to_string())
                .collect(),
        };
        git_providers.push(git_provider);
    }

    Ok(Json(git_providers))
}

pub async fn connect_git_provider(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Host(hostname): Host,
    Query(query): Query<HashMap<String, String>>,
    State(state): State<Arc<CoreState>>,
) -> Result<impl IntoResponse, ApiError> {
    let user = state.authenticate(&cookie).await?;
    let provider_name = query
        .get("provider")
        .ok_or_else(|| ApiError::InvalidRequest("no provider in query string".to_string()))?;

    let oauth = state.db.get_user_oauth(user.id, provider_name).await?;
    if oauth.is_some() {
        return Err(ApiError::InvalidRequest(
            "provider already connected".to_string(),
        ))?;
    }

    let (headers, url) =
        create_oauth_connection(&state, Some(user.id), false, &hostname, &query)
            .await?;

    Ok((headers, Json(NewSessionResponse { url })).into_response())
}

pub async fn update_scope(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Host(hostname): Host,
    Query(query): Query<HashMap<String, String>>,
    State(state): State<Arc<CoreState>>,
) -> Result<impl IntoResponse, ApiError> {
    let user = state.authenticate(&cookie).await?;
    let all_oauths = state.db.get_user_all_oauth(user.id).await?;
    let provider_name = query
        .get("provider")
        .ok_or_else(|| ApiError::InvalidRequest("no provider in query string".to_string()))?;
    if !all_oauths.iter().any(|o| &o.provider == provider_name) {
        return Err(ApiError::InvalidRequest(
            "provider isn't connected".to_string(),
        ))?;
    }

    let read_repo = query
        .get("read_repo")
        .ok_or_else(|| ApiError::InvalidRequest("no read_repo in query string".to_string()))?;

    let read_repo = match read_repo.as_str() {
        "yes" => true,
        "no" => false,
        _ => {
            return Err(ApiError::InvalidRequest(
                "read_repo should be either yes or no".to_string(),
            ))
        }
    };

    let (headers, url) = create_oauth_connection(
        &state,
        Some(user.id),
        read_repo,
        &hostname,
        &query,
    )
    .await?;

    Ok((headers, Json(NewSessionResponse { url })).into_response())
}

pub async fn disconnect_git_provider(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Query(query): Query<HashMap<String, String>>,
    State(state): State<Arc<CoreState>>,
) -> Result<impl IntoResponse, ApiError> {
    let user = state.authenticate(&cookie).await?;
    let all_oauths = state.db.get_user_all_oauth(user.id).await?;
    if all_oauths.len() < 2 {
        return Err(ApiError::InvalidRequest(
            "You can't disconnect all git providers".to_string(),
        ))?;
    }

    let provider_name = query
        .get("provider")
        .ok_or_else(|| ApiError::InvalidRequest("no provider in query string".to_string()))?;
    let oauth = state
        .db
        .get_user_oauth(user.id, provider_name)
        .await?
        .ok_or_else(|| ApiError::InvalidRequest("provider isn't connected".to_string()))?;

    lapdev_db_entities::oauth_connection::ActiveModel {
        id: ActiveValue::Set(oauth.id),
        deleted_at: ActiveValue::Set(Some(Utc::now().into())),
        ..Default::default()
    }
    .update(&state.db.conn)
    .await?;

    Ok(())
}
