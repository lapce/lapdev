use std::{collections::HashMap, net::IpAddr, str::FromStr};

use axum::{
    extract::{Host, Query, State},
    http::{header::SET_COOKIE, HeaderMap},
    response::{IntoResponse, Redirect, Response},
    Json,
};
use axum_extra::{headers, TypedHeader};
use chrono::Utc;
use hyper::StatusCode;
use lapdev_common::{
    console::NewSessionResponse, AuditAction, AuditResourceKind, AuthProvider, ProviderUser,
};
use lapdev_db::entities;
use lapdev_rpc::error::ApiError;
use pasetors::claims::Claims;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter,
    TransactionTrait,
};
use serde::Deserialize;

use crate::state::{CoreState, RequestInfo, TOKEN_COOKIE_NAME};

const OAUTH_STATE: &str = "oauth_state";
const REDIRECT_URL: &str = "redirect_url";

#[derive(Debug, Deserialize)]
pub struct AuthRequest {
    code: String,
    state: String,
    provider: AuthProvider,
    next: Option<String>,
}

pub(crate) async fn new_session(
    Host(hostname): Host,
    Query(query): Query<HashMap<String, String>>,
    State(state): State<CoreState>,
) -> Result<Response, ApiError> {
    let next = query
        .get("next")
        .ok_or_else(|| ApiError::InvalidRequest("no next url in query string".to_string()))?;
    let host = query
        .get("host")
        .ok_or_else(|| ApiError::InvalidRequest("no host in query string".to_string()))?;
    let provider = query
        .get("provider")
        .ok_or_else(|| ApiError::InvalidRequest("no provider in query string".to_string()))?;
    let provider = AuthProvider::from_str(provider)
        .map_err(|_| ApiError::InvalidRequest(format!("provider {provider} is invalid")))?;

    let redirect_url =
        format!("{host}/api/private/session/authorize?provider={provider}&next={next}");
    let (url, csrf) = state.auth.authorize_url(provider, &redirect_url).await?;

    let mut claims = Claims::new()?;
    claims.add_additional(OAUTH_STATE, csrf.clone())?;
    claims.add_additional(REDIRECT_URL, redirect_url.clone())?;
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

    Ok((headers, Json(NewSessionResponse { url, state: csrf })).into_response())
}

pub(crate) async fn session_authorize(
    Host(hostname): Host,
    Query(query): Query<AuthRequest>,
    State(state): State<CoreState>,
    info: RequestInfo,
    TypedHeader(cookie): TypedHeader<headers::Cookie>,
) -> Result<Response, ApiError> {
    let token = state.token(&cookie)?;
    let claims = token.payload_claims().ok_or(ApiError::InvalidAuthToken)?;

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
            ProviderUser {
                id: user.id,
                login: user.username,
                email: user.email,
                name: user.name,
                avatar_url: user.avatar_url,
            }
        }
    };

    let user = match entities::user::Entity::find()
        .filter(entities::user::Column::Provider.eq(query.provider.to_string()))
        .filter(entities::user::Column::ProviderId.eq(provider_user.id))
        .filter(entities::user::Column::DeletedAt.is_null())
        .one(&state.db.conn)
        .await?
    {
        Some(user) => {
            entities::user::ActiveModel {
                id: ActiveValue::Set(user.id),
                provider_login: ActiveValue::Set(provider_user.login),
                access_token: ActiveValue::Set(token.secret().to_string()),
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
                    let count = entities::user::Entity::find()
                        .filter(entities::user::Column::DeletedAt.is_null())
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
