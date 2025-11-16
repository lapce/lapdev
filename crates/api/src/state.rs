use std::{
    collections::{BTreeMap, HashMap},
    str::FromStr,
    sync::Arc,
};

use anyhow::{anyhow, Context, Result};
use axum::{body::Body, extract::FromRequestParts, http::request::Parts, RequestPartsExt};
use axum_client_ip::ClientIp;
use axum_extra::{
    headers::{self, Cookie, HeaderMapExt, UserAgent},
    TypedHeader,
};
use chrono::{DateTime, Utc};
use hyper_util::{client::legacy::connect::HttpConnector, rt::TokioExecutor};
use lapdev_common::{
    devbox::DirectTunnelConfig,
    kube::{AppCatalogStatusEvent, ClusterStatusEvent, EnvironmentWorkloadStatusEvent},
    utils::resolve_api_host,
    UserRole, LAPDEV_AUTH_STATE_COOKIE, LAPDEV_AUTH_TOKEN_COOKIE, LAPDEV_BASE_HOSTNAME,
};
use lapdev_conductor::{scheduler::LAPDEV_CPU_OVERCOMMIT, Conductor};
use lapdev_db::api::DbApi;
use lapdev_devbox_rpc::PortMapping;
use lapdev_enterprise::license::LAPDEV_ENTERPRISE_LICENSE;
use lapdev_kube_rpc::DevboxRouteConfig;
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
use tokio::{
    sync::{broadcast, RwLock},
    time::{sleep, Duration},
};
use tokio_rustls::rustls::sign::CertifiedKey;
use uuid::Uuid;

use crate::{
    auth::{Auth, AuthConfig},
    cert::{load_cert, CertStore},
    devbox_tunnels::DevboxTunnelRegistry,
    github::GithubClient,
    kube_controller::KubeController,
};

use crate::environment_events::EnvironmentLifecycleEvent;

pub const OAUTH_STATE_COOKIE: &str = LAPDEV_AUTH_STATE_COOKIE;
pub const TOKEN_COOKIE_NAME: &str = LAPDEV_AUTH_TOKEN_COOKIE;
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

/// Temporary storage for CLI authentication tokens during browser login flow
pub struct PendingCliAuth {
    pub token: String,
    pub expires_at: DateTime<Utc>,
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
    // Devbox tunnel registry for session-level bridging
    pub devbox_tunnels: Arc<DevboxTunnelRegistry>,
    // Pending CLI authentication tokens (session_id -> token)
    pub pending_cli_auth: Arc<RwLock<HashMap<Uuid, PendingCliAuth>>>,
    // Active devbox sessions (user_id -> DevboxSessionHandle)
    pub active_devbox_sessions: Arc<RwLock<HashMap<Uuid, BTreeMap<Uuid, DevboxSessionHandle>>>>,
    // Lifecycle notifications for kube environments
    pub environment_events: broadcast::Sender<EnvironmentLifecycleEvent>,
    pub environment_workload_events: broadcast::Sender<EnvironmentWorkloadStatusEvent>,
    pub cluster_events: broadcast::Sender<ClusterStatusEvent>,
    pub app_catalog_events: broadcast::Sender<AppCatalogStatusEvent>,
}

/// Handle for an active devbox session
#[derive(Clone)]
pub struct DevboxSessionHandle {
    pub session_id: Uuid,
    pub device_name: String,
    pub notify_tx: tokio::sync::mpsc::UnboundedSender<DevboxSessionNotification>,
    pub rpc_client: lapdev_devbox_rpc::DevboxClientRpcClient,
    pub connection_id: Uuid,
}

/// Notifications that can be sent to active devbox sessions
#[derive(Debug, Clone)]
pub enum DevboxSessionNotification {
    Displaced { new_device_name: String },
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
        let (environment_events, _) = broadcast::channel(128);
        let (environment_workload_events, _) = broadcast::channel(256);
        let (cluster_events, _) = broadcast::channel(128);
        let (app_catalog_events, _) = broadcast::channel(128);
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
            kube_controller: KubeController::new(db, environment_events.clone()),
            devbox_tunnels: Arc::new(DevboxTunnelRegistry::new()),
            pending_cli_auth: Arc::new(RwLock::new(HashMap::new())),
            active_devbox_sessions: Arc::new(RwLock::new(HashMap::new())),
            environment_events,
            environment_workload_events: environment_workload_events.clone(),
            cluster_events,
            app_catalog_events,
        };

        state.spawn_background_tasks();

        state
    }

    fn spawn_background_tasks(&self) {
        {
            let state = self.clone();
            tokio::spawn(async move {
                if let Err(e) = state.monitor_config_updates().await {
                    tracing::error!("api monitor config updates error: {e}");
                }
            });
        }

        {
            let state = self.clone();
            tokio::spawn(async move {
                state.cleanup_pending_cli_auth_loop().await;
            });
        }

        Self::spawn_listener("environment", self.clone(), |s| async move {
            s.monitor_environment_events().await
        });
        Self::spawn_listener("environment_workload", self.clone(), |s| async move {
            s.monitor_environment_workload_events().await
        });
        Self::spawn_listener("cluster", self.clone(), |s| async move {
            s.monitor_cluster_events().await
        });
        Self::spawn_listener("app_catalog", self.clone(), |s| async move {
            s.monitor_app_catalog_events().await
        });
    }

    fn spawn_listener<F, Fut>(name: &'static str, state: CoreState, factory: F)
    where
        F: Fn(CoreState) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send,
    {
        tokio::spawn(async move {
            loop {
                match factory(state.clone()).await {
                    Ok(_) => break,
                    Err(e) => {
                        tracing::error!(
                            listener = name,
                            error = %e,
                            "listener error; retrying in 5s"
                        );
                        sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        });
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

    async fn cleanup_pending_cli_auth_loop(&self) {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            let now = Utc::now();
            let mut pending = self.pending_cli_auth.write().await;
            pending.retain(|_, auth| auth.expires_at > now);
            let removed = pending.len();
            if removed > 0 {
                tracing::debug!("Cleaned up {} expired CLI auth tokens", removed);
            }
        }
    }

    async fn monitor_environment_events(&self) -> Result<()> {
        let pool = self
            .db
            .pool
            .clone()
            .ok_or_else(|| anyhow!("db doesn't have pg pool"))?;
        let mut listener = sqlx::postgres::PgListener::connect_with(&pool).await?;
        listener.listen("environment_lifecycle").await?;
        tracing::info!("environment lifecycle listener started");
        loop {
            let notification = listener.recv().await?;
            match serde_json::from_str::<EnvironmentLifecycleEvent>(notification.payload()) {
                Ok(event) => {
                    let _ = self.environment_events.send(event);
                }
                Err(err) => {
                    tracing::error!(
                        payload = notification.payload(),
                        error = ?err,
                        "failed to deserialize environment lifecycle notification"
                    );
                }
            }
        }
    }

    async fn monitor_environment_workload_events(&self) -> Result<()> {
        let pool = self
            .db
            .pool
            .clone()
            .ok_or_else(|| anyhow!("db doesn't have pg pool"))?;
        let mut listener = sqlx::postgres::PgListener::connect_with(&pool).await?;
        listener.listen("environment_workload_status").await?;
        tracing::info!("environment workload listener started");
        loop {
            let notification = listener.recv().await?;
            match serde_json::from_str::<EnvironmentWorkloadStatusEvent>(notification.payload()) {
                Ok(event) => {
                    let _ = self.environment_workload_events.send(event);
                }
                Err(err) => {
                    tracing::error!(
                        payload = notification.payload(),
                        error = ?err,
                        "failed to deserialize environment workload status notification"
                    );
                }
            }
        }
    }

    async fn monitor_cluster_events(&self) -> Result<()> {
        let pool = self
            .db
            .pool
            .clone()
            .ok_or_else(|| anyhow!("db doesn't have pg pool"))?;
        let mut listener = sqlx::postgres::PgListener::connect_with(&pool).await?;
        listener.listen("cluster_status").await?;
        tracing::info!("cluster status listener started");
        loop {
            let notification = listener.recv().await?;
            match serde_json::from_str::<ClusterStatusEvent>(notification.payload()) {
                Ok(event) => {
                    let _ = self.cluster_events.send(event);
                }
                Err(err) => {
                    tracing::error!(
                        payload = notification.payload(),
                        error = ?err,
                        "failed to deserialize cluster status notification"
                    );
                }
            }
        }
    }

    async fn monitor_app_catalog_events(&self) -> Result<()> {
        let pool = self
            .db
            .pool
            .clone()
            .ok_or_else(|| anyhow!("db doesn't have pg pool"))?;
        let mut listener = sqlx::postgres::PgListener::connect_with(&pool).await?;
        listener.listen("app_catalog_status").await?;
        tracing::info!("app catalog status listener started");
        loop {
            let notification = listener.recv().await?;
            match serde_json::from_str::<AppCatalogStatusEvent>(notification.payload()) {
                Ok(event) => {
                    let _ = self.app_catalog_events.send(event);
                }
                Err(err) => {
                    tracing::error!(
                        payload = notification.payload(),
                        error = ?err,
                        "failed to deserialize app catalog status notification"
                    );
                }
            }
        }
    }

    pub async fn websocket_base_url(&self) -> String {
        let host = if let Ok(value) = std::env::var("LAPDEV_API_HOST") {
            resolve_api_host(Some(value.as_str()))
        } else {
            let stored = self
                .conductor
                .hostnames
                .read()
                .await
                .get("")
                .cloned()
                .unwrap_or_default();
            let stored_ref = if stored.trim().is_empty() {
                None
            } else {
                Some(stored.as_str())
            };
            resolve_api_host(stored_ref)
        };

        CoreState::normalize_ws_base(&host)
    }

    fn normalize_ws_base(candidate: &str) -> String {
        let trimmed = candidate.trim();
        let with_scheme = if trimmed.starts_with("http://")
            || trimmed.starts_with("https://")
            || trimmed.starts_with("ws://")
            || trimmed.starts_with("wss://")
        {
            trimmed.to_string()
        } else {
            format!("https://{}", trimmed)
        };

        if with_scheme.starts_with("http://") {
            with_scheme.replacen("http://", "ws://", 1)
        } else if with_scheme.starts_with("https://") {
            with_scheme.replacen("https://", "wss://", 1)
        } else {
            with_scheme
        }
    }

    pub async fn push_devbox_routes(&self, user_id: Uuid, environment_id: Uuid) {
        match self
            .build_devbox_route_snapshot(user_id, environment_id)
            .await
        {
            Ok((cluster_id, base_environment_id, routes)) => {
                if let Err(err) = self
                    .kube_controller
                    .set_devbox_routes(
                        cluster_id,
                        base_environment_id.unwrap_or(environment_id),
                        routes,
                    )
                    .await
                {
                    tracing::warn!(
                        environment_id = %environment_id,
                        error = %err,
                        "Failed to push devbox routes to kube-manager"
                    );
                }
            }
            Err(err) => {
                tracing::warn!(
                    environment_id = %environment_id,
                    error = %err,
                    "Failed to build devbox route snapshot"
                );
            }
        }
    }

    pub async fn push_devbox_route_for_intercept(
        &self,
        environment: lapdev_db_entities::kube_environment::Model,
        intercept: lapdev_db_entities::kube_devbox_workload_intercept::Model,
    ) {
        let base = self.websocket_base_url().await;
        let base_trimmed = base.trim_end_matches('/').to_string();

        match self
            .route_workload_id_for_intercept(&intercept)
            .await
            .and_then(|route_workload_id| {
                Self::build_devbox_route_config_from_intercept(
                    base_trimmed.as_str(),
                    &environment,
                    &intercept,
                    route_workload_id,
                )
            })
        {
            Ok(route) => {
                let (target_environment_id, _) = Self::route_environment_targets(&environment);
                if let Err(err) = self
                    .kube_controller
                    .set_devbox_route(environment.cluster_id, target_environment_id, route)
                    .await
                {
                    tracing::warn!(
                        environment_id = %environment.id,
                        workload_id = %intercept.workload_id,
                        intercept_id = %intercept.id,
                        error = %err,
                        "Failed to push devbox route for intercept"
                    );
                }
            }
            Err(err) => {
                tracing::warn!(
                    environment_id = %environment.id,
                    intercept_id = %intercept.id,
                    error = %err,
                    "Failed to build devbox route for intercept"
                );
            }
        }
    }

    pub async fn clear_devbox_route_for_intercept(
        &self,
        environment: lapdev_db_entities::kube_environment::Model,
        intercept: lapdev_db_entities::kube_devbox_workload_intercept::Model,
    ) {
        let (target_environment_id, branch_environment_id) =
            Self::route_environment_targets(&environment);

        let route_workload_id = match self
            .route_workload_id_for_intercept(&intercept)
            .await
        {
            Ok(id) => id,
            Err(err) => {
                tracing::warn!(
                    environment_id = %environment.id,
                    intercept_id = %intercept.id,
                    workload_id = %intercept.workload_id,
                    error = %err,
                    "Failed to resolve workload for devbox route removal"
                );
                return;
            }
        };

        if let Err(err) = self
            .kube_controller
            .remove_devbox_route(
                environment.cluster_id,
                target_environment_id,
                route_workload_id,
                branch_environment_id,
            )
            .await
        {
            tracing::warn!(
                environment_id = %environment.id,
                workload_id = %intercept.workload_id,
                intercept_id = %intercept.id,
                error = %err,
                "Failed to clear devbox route for intercept"
            );
        }
    }

    pub async fn clear_devbox_routes_for_environment(&self, environment_id: Uuid) {
        match self
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(|e| format!("Failed to fetch environment: {e}"))
        {
            Ok(Some(environment)) => {
                if let Err(err) = self
                    .kube_controller
                    .clear_devbox_routes(
                        environment.cluster_id,
                        environment.base_environment_id.unwrap_or(environment_id),
                        environment.base_environment_id.map(|_| environment_id),
                    )
                    .await
                {
                    tracing::warn!(
                        environment_id = %environment_id,
                        error = %err,
                        "Failed to clear devbox routes for environment"
                    );
                }
            }
            Ok(None) => {
                tracing::debug!(
                    environment_id = %environment_id,
                    "Environment not found while clearing devbox routes"
                );
            }
            Err(err) => {
                tracing::warn!(
                    environment_id = %environment_id,
                    error = %err,
                    "Failed to clear devbox routes for environment"
                );
            }
        }
    }

    async fn build_devbox_route_snapshot(
        &self,
        user_id: Uuid,
        environment_id: Uuid,
    ) -> Result<(Uuid, Option<Uuid>, HashMap<Uuid, DevboxRouteConfig>), String> {
        let environment = self
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(|e| format!("Failed to fetch environment: {e}"))?
            .ok_or_else(|| "Environment not found".to_string())?;

        let intercepts = self
            .db
            .get_active_intercepts_for_environment(environment_id)
            .await
            .map_err(|e| format!("Failed to fetch intercepts: {e}"))?;

        let base = self.websocket_base_url().await;
        let base_trimmed = base.trim_end_matches('/');

        let mut routes: HashMap<Uuid, DevboxRouteConfig> = HashMap::new();

        for intercept in intercepts {
            if intercept.user_id != user_id {
                continue;
            }

            let route_workload_id = self
                .route_workload_id_for_intercept(&intercept)
                .await?;

            let route = Self::build_devbox_route_config_from_intercept(
                base_trimmed,
                &environment,
                &intercept,
                route_workload_id,
            )?;

            routes.insert(route_workload_id, route);
        }

        Ok((
            environment.cluster_id,
            environment.base_environment_id,
            routes,
        ))
    }

    fn build_devbox_route_config_from_intercept(
        base_trimmed: &str,
        environment: &lapdev_db_entities::kube_environment::Model,
        intercept: &lapdev_db_entities::kube_devbox_workload_intercept::Model,
        route_workload_id: Uuid,
    ) -> Result<DevboxRouteConfig, String> {
        let port_mappings = Self::intercept_port_map(intercept)?;
        let websocket_url = format!(
            "{}/api/v1/kube/sidecar/tunnel/{}/{}",
            base_trimmed, environment.id, intercept.workload_id
        );

        Ok(DevboxRouteConfig {
            intercept_id: intercept.id,
            workload_id: route_workload_id,
            auth_token: environment.auth_token.clone(),
            websocket_url,
            path_pattern: "/*".to_string(),
            branch_environment_id: environment.base_environment_id.map(|_| environment.id),
            created_at_epoch_seconds: Some(intercept.created_at.timestamp()),
            expires_at_epoch_seconds: intercept.stopped_at.map(|dt| dt.timestamp()),
            port_mappings,
        })
    }

    fn intercept_port_map(
        intercept: &lapdev_db_entities::kube_devbox_workload_intercept::Model,
    ) -> Result<HashMap<u16, u16>, String> {
        let value: serde_json::Value = intercept.port_mappings.clone().into();
        let mappings: Vec<PortMapping> = serde_json::from_value(value).map_err(|e| {
            format!(
                "Failed to parse port mappings for intercept {}: {e}",
                intercept.id
            )
        })?;

        let mut port_map = HashMap::with_capacity(mappings.len());
        for mapping in &mappings {
            port_map.insert(mapping.workload_port, mapping.local_port);
        }

        Ok(port_map)
    }

    async fn route_workload_id_for_intercept(
        &self,
        intercept: &lapdev_db_entities::kube_devbox_workload_intercept::Model,
    ) -> Result<Uuid, String> {
        let workload = self
            .db
            .get_environment_workload(intercept.workload_id)
            .await
            .map_err(|e| {
                format!(
                    "Failed to load workload {} for intercept {}: {e}",
                    intercept.workload_id, intercept.id
                )
            })?
            .ok_or_else(|| {
                format!(
                    "Workload {} not found for intercept {}",
                    intercept.workload_id, intercept.id
                )
            })?;

        Ok(workload.base_workload_id.unwrap_or(workload.id))
    }

    fn route_environment_targets(
        environment: &lapdev_db_entities::kube_environment::Model,
    ) -> (Uuid, Option<Uuid>) {
        (
            environment.base_environment_id.unwrap_or(environment.id),
            environment.base_environment_id.map(|_| environment.id),
        )
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
