use std::{collections::HashMap, str::FromStr, sync::Arc};

use anyhow::{anyhow, Context, Result};
use axum::{body::Body, extract::FromRequestParts, http::request::Parts, RequestPartsExt};
use axum_client_ip::ClientIp;
use axum_extra::{
    headers::{self, Cookie, HeaderMapExt, UserAgent},
    TypedHeader,
};
use hyper_util::{client::legacy::connect::HttpConnector, rt::TokioExecutor};
use lapdev_common::{UserRole, LAPDEV_BASE_HOSTNAME};
use lapdev_conductor::{scheduler::LAPDEV_CPU_OVERCOMMIT, Conductor};
use lapdev_db::api::DbApi;
use lapdev_enterprise::license::LAPDEV_ENTERPRISE_LICENSE;
use lapdev_rpc::error::ApiError;
use pasetors::{
    claims::ClaimsValidationRules,
    keys::SymmetricKey,
    token::{TrustedToken, UntrustedToken},
    version4::V4,
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Deserialize;
use sqlx::postgres::PgNotification;
use tokio_rustls::rustls::sign::CertifiedKey;
use uuid::Uuid;

use crate::{
    auth::{Auth, AuthConfig},
    cert::{load_cert, CertStore},
    github::GithubClient,
    kube_controller::KubeController,
    session::OAUTH_STATE_COOKIE,
};

pub const TOKEN_COOKIE_NAME: &str = "token";
pub const LAPDEV_CERTS: &str = "lapdev-certs";

pub type HyperClient = hyper_util::client::legacy::Client<HttpConnector, Body>;

pub struct RequestInfo {
    pub ip: Option<String>,
    pub user_agent: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ConfigUpdatePayload {
    name: String,
    value: String,
}

impl<S: Send + Sync> FromRequestParts<S> for RequestInfo {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let ip = parts
            .extract::<ClientIp>()
            .await
            .ok()
            .map(|ClientIp(ip)| ip.to_string());
        let user_agent = parts.extract::<TypedHeader<UserAgent>>().await.ok();
        let user_agent = user_agent.map(|u| u.to_string());
        Ok(Self { user_agent, ip })
    }
}

#[derive(Clone)]
pub struct CoreState {
    pub conductor: Conductor,
    pub github_client: GithubClient,
    pub hyper_client: Arc<HyperClient>,
    pub db: DbApi,
    pub auth: Arc<Auth>,
    pub auth_token_key: Arc<SymmetricKey<V4>>,
    pub certs: CertStore,
    // actuall ssh proxy port
    pub ssh_proxy_port: u16,
    // ssh proxy port to display in front end
    pub ssh_proxy_display_port: u16,
    pub static_dir: Arc<Option<include_dir::Dir<'static>>>,
    // Kubernetes controller
    pub kube_controller: KubeController,
}

impl CoreState {
    pub async fn new(
        conductor: Conductor,
        ssh_proxy_port: u16,
        ssh_proxy_display_port: u16,
        static_dir: Option<include_dir::Dir<'static>>,
    ) -> Self {
        let github_client = GithubClient::new();
        let key = conductor.db.load_api_auth_token_key().await;
        let auth = Auth::new(&conductor.db).await;
        let certs = load_certs(&conductor.db).await.unwrap_or_default();

        let hyper_client: HyperClient =
            hyper_util::client::legacy::Client::<(), ()>::builder(TokioExecutor::new())
                .build(HttpConnector::new());

        let db = conductor.db.clone();
        let state = Self {
            db: db.clone(),
            conductor,
            github_client,
            auth: Arc::new(auth),
            auth_token_key: Arc::new(key),
            certs: Arc::new(std::sync::RwLock::new(Arc::new(certs))),
            ssh_proxy_port,
            ssh_proxy_display_port,
            hyper_client: Arc::new(hyper_client),
            static_dir: Arc::new(static_dir),
            kube_controller: KubeController::new(db),
        };

        {
            let state = state.clone();
            tokio::spawn(async move {
                if let Err(e) = state.monitor_config_updates().await {
                    tracing::error!("api monitor config updates error: {e}");
                }
            });
        }

        state
    }

    async fn monitor_config_updates(&self) -> Result<()> {
        let pool = self
            .db
            .pool
            .clone()
            .ok_or_else(|| anyhow!("db doesn't have pg pool"))?;
        let mut listener = sqlx::postgres::PgListener::connect_with(&pool).await?;
        listener.listen("config_update").await?;
        loop {
            let notification = listener.recv().await?;
            if let Err(e) = self.handle_config_update_notification(notification).await {
                tracing::error!("handle config update notification error: {e:#}");
            }
        }
    }

    async fn handle_config_update_notification(&self, notification: PgNotification) -> Result<()> {
        let payload: ConfigUpdatePayload = serde_json::from_str(notification.payload())
            .with_context(|| format!("trying to deserialize payload {}", notification.payload()))?;
        if payload.name == AuthConfig::GITHUB.client_id
            || payload.name == AuthConfig::GITHUB.client_secret
            || payload.name == AuthConfig::GITLAB.client_id
            || payload.name == AuthConfig::GITLAB.client_secret
        {
            self.auth.resync(&self.db).await;
        } else if payload.name == LAPDEV_ENTERPRISE_LICENSE {
            self.conductor.enterprise.license.resync_license().await;
        } else if payload.name == LAPDEV_CPU_OVERCOMMIT {
            if let Ok(v) = payload.value.parse::<usize>() {
                *self.conductor.cpu_overcommit.write().await = v.max(1);
            }
        } else if payload.name == LAPDEV_BASE_HOSTNAME {
            *self.conductor.hostnames.write().await = self
                .conductor
                .enterprise
                .get_hostnames()
                .await
                .unwrap_or_default();
        } else if payload.name == LAPDEV_CERTS {
            if let Ok(certs) = load_certs(&self.db).await {
                if let Ok(mut current) = self.certs.write() {
                    *current = Arc::new(certs);
                }
            }
        }
        Ok(())
    }

    fn cookie_token(&self, cookie: &headers::Cookie, name: &str) -> Result<TrustedToken, ApiError> {
        let token = cookie.get(name).ok_or(ApiError::Unauthenticated)?;
        let untrusted_token =
            UntrustedToken::try_from(token).map_err(|_| ApiError::Unauthenticated)?;
        let token = pasetors::local::decrypt(
            &self.auth_token_key,
            &untrusted_token,
            &ClaimsValidationRules::new(),
            None,
            None,
        )
        .map_err(|_| ApiError::Unauthenticated)?;
        Ok(token)
    }

    pub fn auth_state_token(&self, cookie: &headers::Cookie) -> Result<TrustedToken, ApiError> {
        self.cookie_token(cookie, OAUTH_STATE_COOKIE)
    }

    pub fn token(&self, cookie: &headers::Cookie) -> Result<TrustedToken, ApiError> {
        self.cookie_token(cookie, TOKEN_COOKIE_NAME)
    }

    pub async fn require_enterprise(&self) -> Result<(), ApiError> {
        if self.conductor.enterprise.has_valid_license().await {
            return Ok(());
        }
        Err(ApiError::EnterpriseInvalid)
    }

    async fn authorize_org(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
    ) -> Result<
        (
            lapdev_db_entities::user::Model,
            lapdev_db_entities::organization_member::Model,
        ),
        ApiError,
    > {
        let user = self.authenticate_raw(headers).await?;
        let member = self
            .db
            .get_organization_member(user.id, org_id)
            .await
            .map_err(|_| ApiError::Unauthorized)?;
        Ok((user, member))
    }

    pub async fn authorize(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        required_role: Option<UserRole>,
    ) -> Result<lapdev_db_entities::user::Model, ApiError> {
        let (user, member) = self.authorize_org(headers, org_id).await?;

        if let Some(required_role) = required_role {
            let member_role =
                UserRole::from_str(&member.role).map_err(|_| ApiError::Unauthorized)?;

            match required_role {
                UserRole::Owner => {
                    if member_role != UserRole::Owner {
                        return Err(ApiError::DetailedUnauthorized(
                            "Owner role required".to_string(),
                        ));
                    }
                }
                UserRole::Admin => {
                    if member_role != UserRole::Owner && member_role != UserRole::Admin {
                        return Err(ApiError::DetailedUnauthorized(
                            "Admin or Owner role required".to_string(),
                        ));
                    }
                }
                UserRole::Member => {
                    // Any role is sufficient for Member access
                }
            }
        }

        Ok(user)
    }

    pub async fn authenticate_raw(
        &self,
        headers: &axum::http::HeaderMap,
    ) -> Result<lapdev_db_entities::user::Model, ApiError> {
        let cookie = headers
            .typed_get::<Cookie>()
            .ok_or(ApiError::Unauthenticated)?;
        self.authenticate(&cookie).await
    }

    pub async fn authenticate(
        &self,
        cookie: &headers::Cookie,
    ) -> Result<lapdev_db_entities::user::Model, ApiError> {
        let token = self.token(cookie)?;
        let user_id: Uuid = token
            .payload_claims()
            .and_then(|c| c.get_claim("user_id"))
            .and_then(|v| serde_json::from_value(v.to_owned()).ok())
            .ok_or(ApiError::Unauthenticated)?;
        let user = lapdev_db_entities::user::Entity::find_by_id(user_id)
            .filter(lapdev_db_entities::user::Column::DeletedAt.is_null())
            .one(&self.db.conn)
            .await?
            .ok_or(ApiError::Unauthenticated)?;
        Ok(user)
    }

    pub async fn authenticate_cluster_admin(
        &self,
        cookie: &headers::Cookie,
    ) -> Result<lapdev_db_entities::user::Model, ApiError> {
        let user = self.authenticate(cookie).await?;
        if !user.cluster_admin {
            return Err(ApiError::Unauthorized);
        }
        Ok(user)
    }

    pub async fn get_project(
        &self,
        cookie: &headers::Cookie,
        org_id: Uuid,
        project_id: Uuid,
    ) -> Result<
        (
            lapdev_db_entities::user::Model,
            lapdev_db_entities::project::Model,
        ),
        ApiError,
    > {
        let user = self.authenticate(cookie).await?;
        self.db
            .get_organization_member(user.id, org_id)
            .await
            .map_err(|_| ApiError::Unauthorized)?;
        let project = self
            .db
            .get_project(project_id)
            .await
            .map_err(|_| ApiError::InvalidRequest("project doesn't exist".to_string()))?;
        if project.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }
        Ok((user, project))
    }

}

async fn load_certs(db: &DbApi) -> Result<HashMap<String, Arc<CertifiedKey>>> {
    let certs = db.get_config(LAPDEV_CERTS).await?;
    let certs: Vec<(String, String)> = serde_json::from_str(&certs)?;

    let mut final_certs = HashMap::new();
    for (cert, key) in certs {
        if let Ok((dns_names, cert)) = load_cert(&cert, &key) {
            for dns_name in dns_names {
                final_certs.insert(dns_name, Arc::new(cert.clone()));
            }
        }
    }
    Ok(final_certs)
}

