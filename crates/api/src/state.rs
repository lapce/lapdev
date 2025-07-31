use std::{collections::HashMap, str::FromStr, sync::Arc};
use tokio::sync::RwLock;

use anyhow::{anyhow, Context, Result};
use axum::{body::Body, extract::FromRequestParts, http::request::Parts, RequestPartsExt};
use axum_client_ip::ClientIp;
use axum_extra::{
    headers::{self, Cookie, HeaderMapExt, UserAgent},
    TypedHeader,
};
use chrono::Utc;
use hyper_util::{client::legacy::connect::HttpConnector, rt::TokioExecutor};
use lapdev_api_hrpc::HrpcService;
use lapdev_common::{
    hrpc::HrpcError,
    kube::{
        CreateKubeClusterResponse, K8sProvider, K8sProviderKind, KubeClusterInfo,
        KubeClusterStatus, KubeWorkload, KubeWorkloadList, PaginationParams,
    },
    token::PlainToken,
    UserRole, LAPDEV_BASE_HOSTNAME,
};
use lapdev_conductor::{scheduler::LAPDEV_CPU_OVERCOMMIT, Conductor};
use lapdev_db::api::DbApi;
use lapdev_enterprise::license::LAPDEV_ENTERPRISE_LICENSE;
use lapdev_kube::server::KubeClusterServer;
use lapdev_rpc::error::ApiError;
use pasetors::{
    claims::ClaimsValidationRules,
    keys::SymmetricKey,
    token::{TrustedToken, UntrustedToken},
    version4::V4,
};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter, TransactionTrait,
};
use secrecy::ExposeSecret;
use serde::Deserialize;
use sqlx::postgres::PgNotification;
use tokio_rustls::rustls::sign::CertifiedKey;
use uuid::Uuid;

use crate::{
    auth::{Auth, AuthConfig},
    cert::{load_cert, CertStore},
    github::GithubClient,
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
    // KubeManager connections per cluster
    pub kube_cluster_servers: Arc<RwLock<HashMap<Uuid, Vec<KubeClusterServer>>>>,
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

        let state = Self {
            db: conductor.db.clone(),
            conductor,
            github_client,
            auth: Arc::new(auth),
            auth_token_key: Arc::new(key),
            certs: Arc::new(std::sync::RwLock::new(Arc::new(certs))),
            ssh_proxy_port,
            ssh_proxy_display_port,
            hyper_client: Arc::new(hyper_client),
            static_dir: Arc::new(static_dir),
            kube_cluster_servers: Arc::new(RwLock::new(HashMap::new())),
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

    pub async fn get_random_kube_cluster_server(
        &self,
        cluster_id: Uuid,
    ) -> Option<KubeClusterServer> {
        let servers = self.kube_cluster_servers.read().await;
        servers.get(&cluster_id)?.last().cloned()
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

impl HrpcService for CoreState {
    async fn all_k8s_providers(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
    ) -> Result<Vec<K8sProvider>, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;
        let providers = self
            .db
            .get_all_k8s_providers(org_id)
            .await
            .map_err(ApiError::from)?
            .into_iter()
            .filter_map(|p| {
                Some(K8sProvider {
                    id: p.id,
                    name: p.name,
                    provider: K8sProviderKind::from_str(&p.provider).ok()?,
                })
            })
            .collect();
        Ok(providers)
    }

    async fn create_k8s_gcp_provider(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        name: String,
        key: String,
    ) -> Result<(), HrpcError> {
        let user = self
            .authorize(headers, org_id, Some(UserRole::Admin))
            .await?;

        lapdev_db_entities::k8s_provider::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            created_at: ActiveValue::Set(Utc::now().into()),
            name: ActiveValue::Set(name),
            provider: ActiveValue::Set(K8sProviderKind::GCP.to_string()),
            config: ActiveValue::Set(key),
            created_by: ActiveValue::Set(user.id),
            organization_id: ActiveValue::Set(org_id),
            deleted_at: ActiveValue::Set(None),
        }
        .insert(&self.db.conn)
        .await
        .map_err(ApiError::from)?;

        Ok(())
    }

    async fn all_kube_clusters(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
    ) -> Result<Vec<KubeClusterInfo>, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;
        let clusters = self
            .db
            .get_all_kube_clusters(org_id)
            .await
            .map_err(ApiError::from)?
            .into_iter()
            .map(|c| KubeClusterInfo {
                cluster_id: Some(c.id.to_string()),
                cluster_name: Some(c.name),
                cluster_version: c.cluster_version.unwrap_or("Unknown".to_string()),
                node_count: 0, // TODO: Get actual node count from kube-manager
                available_cpu: "N/A".to_string(), // TODO: Get actual CPU from kube-manager
                available_memory: "N/A".to_string(), // TODO: Get actual memory from kube-manager
                provider: None, // TODO: Get provider info
                region: c.region,
                status: c
                    .status
                    .as_deref()
                    .and_then(|s| KubeClusterStatus::from_str(s).ok())
                    .unwrap_or(KubeClusterStatus::NotReady),
            })
            .collect();
        Ok(clusters)
    }

    async fn create_kube_cluster(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        name: String,
    ) -> Result<CreateKubeClusterResponse, HrpcError> {
        let user = self
            .authorize(headers, org_id, Some(UserRole::Admin))
            .await?;

        let cluster_id = Uuid::new_v4();
        let token = PlainToken::generate();
        let hashed_token = token.hashed();
        let token_name = format!("{name}-default");

        // Use a transaction to ensure both cluster and token are created atomically
        let txn = self.db.conn.begin().await.map_err(ApiError::from)?;

        // Create the cluster
        lapdev_db_entities::kube_cluster::ActiveModel {
            id: ActiveValue::Set(cluster_id),
            created_at: ActiveValue::Set(Utc::now().into()),
            name: ActiveValue::Set(name),
            cluster_version: ActiveValue::Set(None),
            status: ActiveValue::Set(Some(KubeClusterStatus::Provisioning.to_string())),
            region: ActiveValue::Set(None),
            created_by: ActiveValue::Set(user.id),
            organization_id: ActiveValue::Set(org_id),
            deleted_at: ActiveValue::Set(None),
            last_reported_at: ActiveValue::Set(None),
        }
        .insert(&txn)
        .await
        .map_err(ApiError::from)?;

        // Create the cluster token
        lapdev_db_entities::kube_cluster_token::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()), // UUID primary key
            created_at: ActiveValue::Set(Utc::now().into()),
            deleted_at: ActiveValue::Set(None),
            last_used_at: ActiveValue::Set(None),
            cluster_id: ActiveValue::Set(cluster_id), // Note: typo in entity field name
            created_by: ActiveValue::Set(user.id),
            name: ActiveValue::Set(token_name),
            token: ActiveValue::Set(hashed_token.expose_secret().to_vec()),
        }
        .insert(&txn)
        .await
        .map_err(ApiError::from)?;

        txn.commit().await.map_err(ApiError::from)?;

        Ok(CreateKubeClusterResponse {
            cluster_id,
            token: token.expose_secret().to_string(),
        })
    }

    async fn get_workloads(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
        namespace: Option<String>,
        pagination: Option<PaginationParams>,
    ) -> Result<KubeWorkloadList, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;

        // Verify cluster belongs to the organization
        let cluster = self
            .db
            .get_kube_cluster(cluster_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Cluster not found".to_string()))?;

        if cluster.organization_id != org_id {
            return Err(ApiError::Unauthorized.into());
        }

        // Get a connected KubeClusterServer for this cluster
        let server = self
            .get_random_kube_cluster_server(cluster_id)
            .await
            .ok_or_else(|| {
                ApiError::InvalidRequest("No connected KubeManager for this cluster".to_string())
            })?;

        let pagination = pagination.unwrap_or(PaginationParams {
            cursor: None,
            limit: 20,
        });

        // Call KubeManager to get workloads
        match server
            .rpc_client
            .get_workloads(tarpc::context::current(), namespace, Some(pagination))
            .await
        {
            Ok(Ok(workload_list)) => Ok(workload_list),
            Ok(Err(e)) => Err(ApiError::InvalidRequest(format!("KubeManager error: {}", e)).into()),
            Err(e) => Err(ApiError::InvalidRequest(format!("Connection error: {}", e)).into()),
        }
    }

    async fn get_workload_details(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
        name: String,
        namespace: String,
    ) -> Result<Option<KubeWorkload>, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;

        // Verify cluster belongs to the organization
        let cluster = self
            .db
            .get_kube_cluster(cluster_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Cluster not found".to_string()))?;

        if cluster.organization_id != org_id {
            return Err(ApiError::Unauthorized.into());
        }

        // Get a connected KubeClusterServer for this cluster
        let server = self
            .get_random_kube_cluster_server(cluster_id)
            .await
            .ok_or_else(|| {
                ApiError::InvalidRequest("No connected KubeManager for this cluster".to_string())
            })?;

        // Call KubeManager to get workload details
        match server
            .rpc_client
            .get_workload_details(tarpc::context::current(), name, namespace)
            .await
        {
            Ok(Ok(workload_details)) => Ok(workload_details),
            Ok(Err(e)) => Err(ApiError::InvalidRequest(format!("KubeManager error: {}", e)).into()),
            Err(e) => Err(ApiError::InvalidRequest(format!("Connection error: {}", e)).into()),
        }
    }
}
