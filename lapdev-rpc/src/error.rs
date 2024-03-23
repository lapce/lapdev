use axum::{http::StatusCode, response::IntoResponse, Json};
use lapdev_common::QuotaResult;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub enum ApiError {
    Unauthenticated,
    Unauthorized,
    EnterpriseInvalid,
    RepositoryInvalid(String),
    InvalidRequest(String),
    InternalError(String),
    NoAuthToken,
    NoAvailableWorkspaceHost,
    InvalidAuthToken,
    QuotaReached(QuotaResult),
}

impl<E> From<E> for ApiError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        let err: anyhow::Error = err.into();
        Self::InternalError(format!("{err:#}"))
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use ApiError::*;
        let err = match self {
            NoAuthToken => "No auth token in the request",
            InvalidAuthToken => "Invalid auth token in the request",
            Unauthenticated => "Not authenticated",
            Unauthorized => "Not authorized",
            EnterpriseInvalid => "Doesn't have enterprise license or enterprise license is invalid",
            InvalidRequest(s) => s,
            InternalError(_) => "Internal Server Error",
            QuotaReached(result) => {
                return f.write_str(&format!(
                    "{} {} Quota Reached {}/{}",
                    result.kind, result.level, result.existing, result.quota
                ))
            }
            NoAvailableWorkspaceHost => "No avaialble workspace host",
            RepositoryInvalid(reason) => reason,
        };
        f.write_str(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        use ApiError::*;
        if let InternalError(e) = &self {
            tracing::error!("internal server error: {e:#}");
        }
        let status = match self {
            Unauthenticated => StatusCode::FORBIDDEN,
            Unauthorized | EnterpriseInvalid => StatusCode::UNAUTHORIZED,
            RepositoryInvalid(_)
            | InvalidRequest(_)
            | NoAuthToken
            | InvalidAuthToken
            | QuotaReached(_)
            | NoAvailableWorkspaceHost => StatusCode::BAD_REQUEST,
            InternalError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (
            status,
            Json(serde_json::json!({
                "error": self.to_string(),
            })),
        )
            .into_response()
    }
}
