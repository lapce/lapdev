use std::{collections::HashMap, net::IpAddr, str::FromStr, sync::Arc};

use axum::{
    extract::{Query, State},
    http::{header::SET_COOKIE, HeaderMap},
    response::{IntoResponse, Redirect, Response},
    Json,
};
use axum_extra::{extract::Host, headers, TypedHeader};
use chrono::Utc;
use hyper::StatusCode;
use lapdev_common::{
    console::NewSessionResponse, AuditAction, AuditResourceKind, AuthProvider, ProviderUser,
    UserCreationWebhook,
};
use lapdev_rpc::error::ApiError;
use pasetors::claims::Claims;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter,
    TransactionTrait,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::state::{CoreState, OAUTH_STATE_COOKIE, RequestInfo, TOKEN_COOKIE_NAME};

pub const OAUTH_STATE: &str = "oauth_state";
pub const REDIRECT_URL: &str = "redirect_url";
pub const CONNECT_USER: &str = "connect_user";
pub const READ_REPO: &str = "read_repo";

#[derive(Debug, Deserialize)]
pub struct AuthRequest {
    code: String,
    state: String,
    provider: AuthProvider,
    next: Option<String>,
    connect_provider: Option<String>,
}

pub async fn create_oauth_connection(
    state: &CoreState,
    connect_user_id: Option<Uuid>,
    read_repo: bool,
    hostname: &str,
    query: &HashMap<String, String>,
) -> Result<(HeaderMap, String), ApiError> {
    let host = query
        .get("host")
        .ok_or_else(|| ApiError::InvalidRequest("no host in query string".to_string()))?;
    let next = query
        .get("next")
        .ok_or_else(|| ApiError::InvalidRequest("no next url in query string".to_string()))?;
    let provider = query
        .get("provider")
        .ok_or_else(|| ApiError::InvalidRequest("no provider in query string".to_string()))?;
    let redirect_url = format!(
        "{host}/api/private/session/authorize?provider={provider}{}&next={next}",
        if connect_user_id.is_some() {
            "&connect_provider=yes"
        } else {
            ""
        }
    );

    let provider = AuthProvider::from_str(provider)
        .map_err(|_| ApiError::InvalidRequest(format!("provider {provider} is invalid")))?;
    let (url, csrf) = state
        .auth
        .authorize_url(provider, &redirect_url, read_repo)
        .await?;

    let mut claims = Claims::new()?;
    claims.add_additional(OAUTH_STATE, csrf.clone())?;
    claims.add_additional(REDIRECT_URL, redirect_url.clone())?;
    claims.add_additional(READ_REPO, read_repo)?;
    if let Some(id) = connect_user_id {
        claims.add_additional(CONNECT_USER, id.to_string())?;
    }
    let token = pasetors::local::encrypt(&state.auth_token_key, &claims, None, None)?;
    let cookie = format!("{OAUTH_STATE_COOKIE}={token}; Path=/");
    let cookie = if let Some(hostname) = hostname.split(':').next() {
        if hostname.parse::<IpAddr>().is_err() {
            format!("{cookie}; Domain=.{hostname}")
        } else {
            cookie
        }
    } else {
        cookie
    };
    let mut headers = HeaderMap::new();
    headers.insert(SET_COOKIE, cookie.parse()?);
    Ok((headers, url))
}

pub(crate) async fn new_session(
    Host(hostname): Host,
    Query(query): Query<HashMap<String, String>>,
    State(state): State<Arc<CoreState>>,
) -> Result<Response, ApiError> {
    let (headers, url) = create_oauth_connection(&state, None, false, &hostname, &query).await?;
    Ok((headers, Json(NewSessionResponse { url })).into_response())
}

pub(crate) async fn session_authorize(
    Host(hostname): Host,
    Query(query): Query<AuthRequest>,
    State(state): State<Arc<CoreState>>,
    info: RequestInfo,
    TypedHeader(cookie): TypedHeader<headers::Cookie>,
) -> Result<Response, ApiError> {
    let token = state.auth_state_token(&cookie)?;
    let claims = token.payload_claims().ok_or(ApiError::Unauthenticated)?;

    let redirect_url = claims
        .get_claim(REDIRECT_URL)
        .ok_or_else(|| ApiError::InvalidRequest("doens't have redirect_url".to_string()))?;
    let redirect_url: String = serde_json::from_value(redirect_url.to_owned())
        .map_err(|_| ApiError::InvalidRequest("invalid redirect_url parameter".to_string()))?;
    let session_state = claims
        .get_claim(OAUTH_STATE)
        .ok_or_else(|| ApiError::InvalidRequest("doens't have oauth_state".to_string()))?;
    let session_state: String = serde_json::from_value(session_state.to_owned())
        .map_err(|_| ApiError::InvalidRequest("invalid state parameter".to_string()))?;
    if session_state != query.state {
        return Err(ApiError::InvalidRequest(
            "state parameter doesn't match".to_string(),
        ));
    }

    let token = state
        .auth
        .exchange_code(&query.provider, query.code, redirect_url)
        .await?;

    let provider_user = match &query.provider {
        AuthProvider::Github => {
            let ghuser = state.github_client.current_user(&token).await?;
            if Utc::now()
                .signed_duration_since(ghuser.created_at)
                .num_seconds()
                < 86400
            {
                return Err(ApiError::InvalidRequest("user not allowed".to_string()));
            }
            let mut email = None;
            if let Ok(emails) = state.github_client.user_email(&token).await {
                for e in &emails {
                    if e.primary {
                        email = Some(e.email.clone());
                        break;
                    }
                }
                if email.is_none() {
                    email = emails.first().map(|e| e.email.clone());
                }
            }
            ProviderUser {
                id: ghuser.id,
                login: ghuser.login,
                name: ghuser.name,
                email,
                avatar_url: ghuser.avatar_url,
            }
        }
        AuthProvider::Gitlab => {
            let user = state.auth.gitlab_client.current_user(&token).await?;
            if Utc::now()
                .signed_duration_since(user.created_at)
                .num_seconds()
                < 86400
            {
                return Err(ApiError::InvalidRequest("user not allowed".to_string()));
            }
            ProviderUser {
                id: user.id,
                login: user.username,
                email: user.email,
                name: user.name,
                avatar_url: user.avatar_url,
            }
        }
    };

    if query.connect_provider.as_deref() == Some("yes") {
        let now = Utc::now();

        let connect_user = claims
            .get_claim(CONNECT_USER)
            .ok_or_else(|| ApiError::InvalidRequest("doens't have connect user".to_string()))?;
        let connect_user: String = serde_json::from_value(connect_user.to_owned())
            .map_err(|_| ApiError::InvalidRequest("invalid connect user".to_string()))?;
        let user_id = Uuid::from_str(&connect_user)?;

        let read_repo = claims
            .get_claim(READ_REPO)
            .ok_or_else(|| ApiError::InvalidRequest("doens't have read repo".to_string()))?;
        let read_repo: bool = serde_json::from_value(read_repo.to_owned())
            .map_err(|_| ApiError::InvalidRequest("invalid read repo".to_string()))?;

        match lapdev_db_entities::oauth_connection::Entity::find()
            .filter(
                lapdev_db_entities::oauth_connection::Column::Provider
                    .eq(query.provider.to_string()),
            )
            .filter(lapdev_db_entities::oauth_connection::Column::UserId.eq(user_id))
            .filter(lapdev_db_entities::oauth_connection::Column::DeletedAt.is_null())
            .one(&state.db.conn)
            .await?
        {
            Some(c) => {
                lapdev_db_entities::oauth_connection::ActiveModel {
                    id: ActiveValue::Set(c.id),
                    provider_login: ActiveValue::Set(provider_user.login),
                    access_token: ActiveValue::Set(token.secret().to_string()),
                    avatar_url: ActiveValue::Set(provider_user.avatar_url.clone()),
                    email: ActiveValue::Set(provider_user.email.clone()),
                    name: ActiveValue::Set(provider_user.name.clone()),
                    read_repo: ActiveValue::Set(Some(read_repo)),
                    ..Default::default()
                }
                .update(&state.db.conn)
                .await?;
            }
            None => {
                lapdev_db_entities::oauth_connection::ActiveModel {
                    id: ActiveValue::Set(Uuid::new_v4()),
                    user_id: ActiveValue::Set(user_id),
                    created_at: ActiveValue::Set(now.into()),
                    deleted_at: ActiveValue::Set(None),
                    provider: ActiveValue::Set(query.provider.to_string()),
                    provider_id: ActiveValue::Set(provider_user.id),
                    provider_login: ActiveValue::Set(provider_user.login),
                    access_token: ActiveValue::Set(token.secret().to_string()),
                    avatar_url: ActiveValue::Set(provider_user.avatar_url),
                    email: ActiveValue::Set(provider_user.email),
                    name: ActiveValue::Set(provider_user.name),
                    read_repo: ActiveValue::Set(Some(read_repo)),
                }
                .insert(&state.db.conn)
                .await?;
            }
        }

        return Ok(Redirect::temporary(query.next.as_deref().unwrap_or("/")).into_response());
    }

    let user = match lapdev_db_entities::oauth_connection::Entity::find()
        .filter(
            lapdev_db_entities::oauth_connection::Column::Provider.eq(query.provider.to_string()),
        )
        .filter(lapdev_db_entities::oauth_connection::Column::ProviderId.eq(provider_user.id))
        .filter(lapdev_db_entities::oauth_connection::Column::DeletedAt.is_null())
        .one(&state.db.conn)
        .await?
    {
        Some(conn) => {
            let conn = lapdev_db_entities::oauth_connection::ActiveModel {
                id: ActiveValue::Set(conn.id),
                provider_login: ActiveValue::Set(provider_user.login),
                access_token: ActiveValue::Set(token.secret().to_string()),
                avatar_url: ActiveValue::Set(provider_user.avatar_url.clone()),
                email: ActiveValue::Set(provider_user.email.clone()),
                name: ActiveValue::Set(provider_user.name.clone()),
                ..Default::default()
            }
            .update(&state.db.conn)
            .await?;

            lapdev_db_entities::user::ActiveModel {
                id: ActiveValue::Set(conn.user_id),
                avatar_url: ActiveValue::Set(provider_user.avatar_url),
                email: ActiveValue::Set(provider_user.email),
                name: ActiveValue::Set(provider_user.name),
                ..Default::default()
            }
            .update(&state.db.conn)
            .await?
        }
        None => {
            if let Some(enterprise) = state
                .conductor
                .enterprise
                .license
                .license
                .read()
                .await
                .as_ref()
            {
                if enterprise.users > 0 {
                    let count = lapdev_db_entities::user::Entity::find()
                        .filter(lapdev_db_entities::user::Column::DeletedAt.is_null())
                        .count(&state.db.conn)
                        .await?;
                    if count >= enterprise.users as u64 {
                        return Err(ApiError::InvalidRequest(
                            "You've reached the users limit in your enterprise license".to_string(),
                        ));
                    }
                }
            }
            let now = Utc::now();
            let txn = state.db.conn.begin().await?;
            let user = state
                .db
                .create_new_user(
                    &txn,
                    &query.provider,
                    provider_user,
                    token.secret().to_string(),
                )
                .await?;

            state
                .conductor
                .enterprise
                .insert_audit_log(
                    &txn,
                    now.into(),
                    user.id,
                    user.current_organization,
                    AuditResourceKind::User.to_string(),
                    user.id,
                    user.name.clone().unwrap_or_else(|| "".to_string()),
                    AuditAction::UserCreate.to_string(),
                    info.ip,
                    info.user_agent,
                )
                .await?;

            txn.commit().await?;

            {
                let state = state.clone();
                let id = user.id;
                tokio::spawn(async move {
                    let webhook = state.db.get_user_creation_webhook().await?;
                    reqwest::Client::builder()
                        .build()?
                        .post(webhook)
                        .json(&UserCreationWebhook { id })
                        .send()
                        .await?;
                    anyhow::Ok(())
                });
            }

            user
        }
    };

    let mut claims = Claims::new_expires_in(&core::time::Duration::from_secs(86400 * 30))?;
    claims.add_additional("user_id", user.id.to_string())?;
    let token = pasetors::local::encrypt(&state.auth_token_key, &claims, None, None)?;
    let cookie = format!("{TOKEN_COOKIE_NAME}={token}; Path=/");
    let cookie = if let Some(hostname) = hostname.split(':').next() {
        if hostname.parse::<IpAddr>().is_err() {
            format!("{cookie}; Domain=.{hostname}")
        } else {
            cookie
        }
    } else {
        cookie
    };
    let mut headers = HeaderMap::new();
    headers.insert(SET_COOKIE, cookie.parse()?);

    Ok((
        headers,
        Redirect::temporary(query.next.as_deref().unwrap_or("/")),
    )
        .into_response())
}

pub(crate) async fn logout(Host(hostname): Host) -> Result<Response, ApiError> {
    let cookie =
        format!("{TOKEN_COOKIE_NAME}=deleted; Path=/; expires=Thu, 01 Jan 1970 00:00:00 GMT");
    let cookie = if let Some(hostname) = hostname.split(':').next() {
        if hostname.parse::<IpAddr>().is_err() {
            format!("{cookie}; Domain=.{hostname}")
        } else {
            cookie
        }
    } else {
        cookie
    };
    let mut headers = HeaderMap::new();
    headers.insert(SET_COOKIE, cookie.parse()?);
    Ok((headers, StatusCode::OK).into_response())
}
