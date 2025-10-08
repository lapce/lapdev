use anyhow::Result;
use axum_extra::headers::{self, authorization::Bearer};
use lapdev_db_entities::user;
use lapdev_rpc::error::ApiError;
use pasetors::claims::{Claims, ClaimsValidationRules};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::state::CoreState;

/// Context for devbox/CLI authenticated requests
pub struct DevboxContext {
    pub user: user::Model,
    pub organization_id: Uuid,
    pub device_name: String,
    pub token_claims: Claims,
}

impl CoreState {
    /// Authenticate CLI request using Bearer token (PASETO)
    /// Same pattern as browser session authentication - stateless!
    pub async fn authenticate_bearer(
        &self,
        auth_header: &headers::Authorization<Bearer>,
    ) -> Result<DevboxContext, ApiError> {
        let token = auth_header.token();

        // 1. Parse and decrypt PASETO token (same as browser cookies)
        let untrusted_token = pasetors::token::UntrustedToken::try_from(token)
            .map_err(|_| ApiError::Unauthenticated)?;
        let validated = pasetors::local::decrypt(
            &self.auth_token_key,
            &untrusted_token,
            &ClaimsValidationRules::new(),
            None,
            None,
        )
        .map_err(|_| ApiError::Unauthenticated)?;

        let claims = validated
            .payload_claims()
            .ok_or(ApiError::Unauthenticated)?;

        // 2. Extract and verify claims
        let user_id: Uuid = claims
            .get_claim("user_id")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .ok_or(ApiError::Unauthenticated)?;

        let organization_id: Uuid = claims
            .get_claim("organization_id")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .ok_or(ApiError::Unauthenticated)?;

        let session_type: String = claims
            .get_claim("session_type")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        if session_type != "cli" {
            return Err(ApiError::Unauthenticated);
        }

        let device_name: String = claims
            .get_claim("device_name")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .ok_or(ApiError::Unauthenticated)?;

        // 3. Load user (same as browser sessions - no session table lookup!)
        let user = user::Entity::find_by_id(user_id)
            .filter(user::Column::DeletedAt.is_null())
            .one(&self.db.conn)
            .await
            .map_err(|e| ApiError::InternalError(e.to_string()))?
            .ok_or(ApiError::Unauthenticated)?;

        Ok(DevboxContext {
            user,
            organization_id,
            device_name,
            token_claims: claims.clone(),
        })
    }
}
