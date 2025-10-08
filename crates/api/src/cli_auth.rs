use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use axum_extra::{headers, TypedHeader};
use chrono::Utc;
use lapdev_rpc::error::ApiError;
use pasetors::claims::Claims;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::state::CoreState;

#[derive(Debug, Deserialize)]
pub struct GenerateTokenRequest {
    pub session_id: Uuid,
    pub device_name: String,
}

#[derive(Debug, Deserialize)]
pub struct CliTokenQuery {
    pub session_id: Uuid,
}

#[derive(Debug, Serialize)]
pub struct CliTokenResponse {
    pub token: String,
    pub expires_in: u32, // seconds (30 days = 2592000)
}

/// POST /api/v1/auth/cli/generate-token - Generate CLI token for already logged-in users
/// This is called by the Leptos frontend when user is already authenticated
pub async fn generate_cli_token(
    TypedHeader(cookie): TypedHeader<headers::Cookie>,
    State(state): State<Arc<CoreState>>,
    Json(req): Json<GenerateTokenRequest>,
) -> Result<Json<CliTokenResponse>, ApiError> {
    // Authenticate the browser session
    let user = state.authenticate(&cookie).await?;

    // Create PASETO token for CLI
    let mut cli_claims = Claims::new_expires_in(&core::time::Duration::from_secs(86400 * 30))?;
    cli_claims.add_additional("user_id", user.id.to_string())?;
    cli_claims.add_additional("organization_id", user.current_organization.to_string())?;
    cli_claims.add_additional("session_type", "cli")?;
    cli_claims.add_additional("device_name", req.device_name.clone())?;
    let cli_token = pasetors::local::encrypt(&state.auth_token_key, &cli_claims, None, None)?;

    // Store in pending_cli_auth map (5-min TTL)
    state.pending_cli_auth.write().await.insert(
        req.session_id,
        crate::state::PendingCliAuth {
            token: cli_token,
            expires_at: Utc::now() + chrono::Duration::seconds(300),
        },
    );

    Ok(Json(CliTokenResponse {
        token: "generated".to_string(), // Don't return actual token, CLI will poll for it
        expires_in: 2592000,
    }))
}

/// GET /api/v1/auth/cli/token - Poll for CLI auth token
/// Returns 404 if token not ready, or the token if authentication completed
pub async fn cli_token_poll(
    Query(params): Query<CliTokenQuery>,
    State(state): State<Arc<CoreState>>,
) -> Result<Response, ApiError> {
    let pending = state.pending_cli_auth.read().await;

    if let Some(auth) = pending.get(&params.session_id) {
        if auth.expires_at > Utc::now() {
            // Remove from pending (single use)
            let token = auth.token.clone();
            drop(pending);
            state
                .pending_cli_auth
                .write()
                .await
                .remove(&params.session_id);

            return Ok(Json(CliTokenResponse {
                token,
                expires_in: 2592000, // 30 days
            })
            .into_response());
        }
    }

    Ok(StatusCode::NOT_FOUND.into_response())
}
