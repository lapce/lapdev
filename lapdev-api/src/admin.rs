use std::{collections::HashMap, str::FromStr};

use anyhow::anyhow;
use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    Json,
};
use axum_extra::{headers::Cookie, TypedHeader};
use chrono::Utc;
use hyper::StatusCode;
use lapdev_common::{
    AuthProvider, ClusterInfo, ClusterUser, ClusterUserResult, EnterpriseLicense, MachineType,
    NewLicense, NewLicenseKey, NewWorkspaceHost, OauthSettings, UpdateClusterUser,
    UpdateWorkspaceHost, WorkspaceHost, WorkspaceHostStatus, LAPDEV_BASE_HOSTNAME,
};
use lapdev_conductor::scheduler::{self, LAPDEV_CPU_OVERCOMMIT};
use lapdev_db::entities;
use lapdev_rpc::error::ApiError;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    auth::AuthConfig,
    state::{CoreState, LAPDEV_CERTS},
};

pub async fn update_license(
    TypedHeader(cookie): TypedHeader<Cookie>,
    State(state): State<CoreState>,
    Json(new_license_key): Json<NewLicenseKey>,
) -> Result<Json<EnterpriseLicense>, ApiError> {
    state.authenticate_cluster_admin(&cookie).await?;
    let license = state
        .conductor
        .enterprise
        .license
        .update_license(&new_license_key.key)
        .await?;
    Ok(Json(license))
}

pub async fn get_license(
    TypedHeader(cookie): TypedHeader<Cookie>,
    State(state): State<CoreState>,
) -> Result<Json<EnterpriseLicense>, ApiError> {
    state.authenticate_cluster_admin(&cookie).await?;
    let license = state
        .conductor
        .enterprise
        .license
        .license
        .read()
        .await
        .clone()
        .ok_or_else(|| ApiError::InvalidRequest("No Valid Enterprise License".to_string()))?;
    Ok(Json(license))
}

pub async fn new_license(
    State(state): State<CoreState>,
    Json(new_license): Json<NewLicense>,
) -> Result<String, ApiError> {
    let license = state
        .conductor
        .enterprise
        .license
        .sign_new_license(
            &new_license.secret,
            new_license.expires_at,
            new_license.users,
            new_license.hostname,
        )
        .map_err(|e| ApiError::InvalidRequest(e.to_string()))?;
    Ok(license)
}

pub async fn get_workspace_hosts(
    TypedHeader(cookie): TypedHeader<Cookie>,
    State(state): State<CoreState>,
) -> Result<Json<Vec<WorkspaceHost>>, ApiError> {
    state.authenticate_cluster_admin(&cookie).await?;
    let hosts = state.db.get_all_workspace_hosts().await?;
    let hosts = hosts
        .into_iter()
        .filter_map(|h| {
            Some(WorkspaceHost {
                id: h.id,
                host: h.host,
                port: h.port,
                status: WorkspaceHostStatus::from_str(&h.status).ok()?,
                cpu: h.cpu,
                memory: h.memory,
                disk: h.disk,
                available_dedicated_cpu: h.available_dedicated_cpu,
                available_shared_cpu: h.available_shared_cpu,
                available_memory: h.available_memory,
                available_disk: h.available_disk,
                region: h.region,
                zone: h.zone,
            })
        })
        .collect();
    Ok(Json(hosts))
}

pub async fn create_workspace_host(
    TypedHeader(cookie): TypedHeader<Cookie>,
    State(state): State<CoreState>,
    Json(new_workspace_host): Json<NewWorkspaceHost>,
) -> Result<Json<WorkspaceHost>, ApiError> {
    state.authenticate_cluster_admin(&cookie).await?;
    if state
        .db
        .get_workspace_host_by_host(&new_workspace_host.host)
        .await?
        .is_some()
    {
        return Err(ApiError::InvalidRequest(format!(
            "Workspace host {} already exists",
            new_workspace_host.host
        )));
    }
    let host = entities::workspace_host::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        deleted_at: ActiveValue::Set(None),
        host: ActiveValue::Set(new_workspace_host.host),
        port: ActiveValue::Set(6123),
        inter_port: ActiveValue::Set(6122),
        status: ActiveValue::Set(WorkspaceHostStatus::New.to_string()),
        cpu: ActiveValue::Set(new_workspace_host.cpu as i32),
        memory: ActiveValue::Set(new_workspace_host.memory as i32),
        disk: ActiveValue::Set(new_workspace_host.disk as i32),
        available_shared_cpu: ActiveValue::Set(new_workspace_host.cpu as i32),
        available_dedicated_cpu: ActiveValue::Set(new_workspace_host.cpu as i32),
        available_memory: ActiveValue::Set(new_workspace_host.memory as i32),
        available_disk: ActiveValue::Set(new_workspace_host.disk as i32),
        region: ActiveValue::Set(new_workspace_host.region),
        zone: ActiveValue::Set(new_workspace_host.zone),
    }
    .insert(&state.db.conn)
    .await?;
    Ok(Json(WorkspaceHost {
        id: host.id,
        host: host.host,
        port: host.port,
        status: WorkspaceHostStatus::from_str(&host.status)?,
        cpu: host.cpu,
        memory: host.memory,
        disk: host.disk,
        available_dedicated_cpu: host.available_dedicated_cpu,
        available_shared_cpu: host.available_shared_cpu,
        available_memory: host.available_memory,
        available_disk: host.available_disk,
        region: host.region,
        zone: host.zone,
    }))
}

pub async fn update_workspace_host(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path(id): Path<Uuid>,
    State(state): State<CoreState>,
    Json(update_workspace_host): Json<UpdateWorkspaceHost>,
) -> Result<Json<WorkspaceHost>, ApiError> {
    state.authenticate_cluster_admin(&cookie).await?;
    let host = entities::workspace_host::ActiveModel {
        id: ActiveValue::Set(id),
        cpu: ActiveValue::Set(update_workspace_host.cpu as i32),
        memory: ActiveValue::Set(update_workspace_host.memory as i32),
        disk: ActiveValue::Set(update_workspace_host.disk as i32),
        region: ActiveValue::Set(update_workspace_host.region),
        zone: ActiveValue::Set(update_workspace_host.zone),
        ..Default::default()
    }
    .update(&state.db.conn)
    .await?;
    let txn = state.db.conn.begin().await?;
    scheduler::recalcuate_workspce_host(&txn, &host, state.conductor.cpu_overcommit().await)
        .await?;
    txn.commit().await?;

    Ok(Json(WorkspaceHost {
        id: host.id,
        host: host.host,
        port: host.port,
        status: WorkspaceHostStatus::from_str(&host.status)?,
        cpu: host.cpu,
        memory: host.memory,
        disk: host.disk,
        available_dedicated_cpu: host.available_dedicated_cpu,
        available_shared_cpu: host.available_shared_cpu,
        available_memory: host.available_memory,
        available_disk: host.available_disk,
        region: host.region,
        zone: host.zone,
    }))
}

pub async fn delete_workspace_host(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path(id): Path<Uuid>,
    State(state): State<CoreState>,
) -> Result<Response, ApiError> {
    state.authenticate_cluster_admin(&cookie).await?;
    let now = Utc::now();
    if entities::workspace::Entity::find()
        .filter(entities::workspace::Column::DeletedAt.is_null())
        .filter(entities::workspace::Column::HostId.eq(id))
        .one(&state.db.conn)
        .await?
        .is_some()
    {
        return Err(ApiError::InvalidRequest(
            "You can't delete a workspace host with workspaces on it".to_string(),
        ));
    };
    entities::workspace_host::ActiveModel {
        id: ActiveValue::Set(id),
        deleted_at: ActiveValue::Set(Some(now.into())),
        ..Default::default()
    }
    .update(&state.db.conn)
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn get_oauth(
    TypedHeader(cookie): TypedHeader<Cookie>,
    State(state): State<CoreState>,
) -> Result<Json<OauthSettings>, ApiError> {
    state.authenticate_cluster_admin(&cookie).await?;
    let setting = OauthSettings {
        github_client_id: state
            .db
            .get_config(AuthConfig::GITHUB.client_id)
            .await
            .unwrap_or_default(),
        github_client_secret: state
            .db
            .get_config(AuthConfig::GITHUB.client_secret)
            .await
            .unwrap_or_default(),
        gitlab_client_id: state
            .db
            .get_config(AuthConfig::GITLAB.client_id)
            .await
            .unwrap_or_default(),
        gitlab_client_secret: state
            .db
            .get_config(AuthConfig::GITLAB.client_secret)
            .await
            .unwrap_or_default(),
    };
    Ok(Json(setting))
}

pub async fn update_oauth(
    TypedHeader(cookie): TypedHeader<Cookie>,
    State(state): State<CoreState>,
    Json(update_oauth): Json<OauthSettings>,
) -> Result<Response, ApiError> {
    let is_cluster_initiated = {
        let txn = state.db.conn.begin().await?;
        let is_cluster_initiated = state.db.is_cluster_initiated(&txn).await;
        txn.commit().await?;
        is_cluster_initiated
    };
    if is_cluster_initiated {
        // we don't need authentication if cluster is not initiated
        state.authenticate_cluster_admin(&cookie).await?;
    }
    state
        .db
        .update_config(AuthConfig::GITHUB.client_id, &update_oauth.github_client_id)
        .await?;
    state
        .db
        .update_config(
            AuthConfig::GITHUB.client_secret,
            &update_oauth.github_client_secret,
        )
        .await?;
    state
        .db
        .update_config(AuthConfig::GITLAB.client_id, &update_oauth.gitlab_client_id)
        .await?;
    state
        .db
        .update_config(
            AuthConfig::GITLAB.client_secret,
            &update_oauth.gitlab_client_secret,
        )
        .await?;
    state.auth.resync(&state.db).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn get_certs(
    TypedHeader(cookie): TypedHeader<Cookie>,
    State(state): State<CoreState>,
) -> Result<Json<Vec<(String, String)>>, ApiError> {
    state.authenticate_cluster_admin(&cookie).await?;
    let certs = state.db.get_config(LAPDEV_CERTS).await?;
    let certs: Vec<(String, String)> = serde_json::from_str(&certs)?;
    Ok(Json(certs))
}

pub async fn update_certs(
    TypedHeader(cookie): TypedHeader<Cookie>,
    State(state): State<CoreState>,
    Json(update_body): Json<Vec<(String, String)>>,
) -> Result<StatusCode, ApiError> {
    state.authenticate_cluster_admin(&cookie).await?;
    let certs = serde_json::to_string(&update_body)?;
    state.db.update_config(LAPDEV_CERTS, &certs).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_auth_providers(
    State(state): State<CoreState>,
) -> Result<Json<Vec<AuthProvider>>, ApiError> {
    let providers: Vec<AuthProvider> = state
        .auth
        .clients
        .read()
        .await
        .keys()
        .map(|k| k.to_owned())
        .collect();
    Ok(Json(providers))
}

pub async fn get_cluster_info(
    State(state): State<CoreState>,
) -> Result<Json<ClusterInfo>, ApiError> {
    let providers: Vec<AuthProvider> = state
        .auth
        .clients
        .read()
        .await
        .keys()
        .map(|k| k.to_owned())
        .collect();
    let machine_types = state.db.get_all_machine_types().await?;
    let machine_types: Vec<MachineType> = machine_types
        .into_iter()
        .map(|m| MachineType {
            id: m.id,
            name: m.name,
            shared: m.shared,
            cpu: m.cpu as usize,
            memory: m.memory as usize,
            disk: m.disk as usize,
            cost_per_second: m.cost_per_second as usize,
        })
        .collect();
    let has_enterprise = state.conductor.enterprise.has_valid_license().await;
    Ok(Json(ClusterInfo {
        auth_providers: providers,
        machine_types,
        has_enterprise,
        hostnames: state.conductor.hostnames.read().await.clone(),
        ssh_proxy_port: state.ssh_proxy_port,
    }))
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FilterQuery {
    filter: Option<String>,
    page_size: Option<u64>,
    page: Option<u64>,
}

pub async fn get_cluster_users(
    TypedHeader(cookie): TypedHeader<Cookie>,
    State(state): State<CoreState>,
    Query(filter_query): Query<FilterQuery>,
) -> Result<Json<ClusterUserResult>, ApiError> {
    state.authenticate_cluster_admin(&cookie).await?;
    let mut query =
        entities::user::Entity::find().filter(entities::user::Column::DeletedAt.is_null());
    if let Some(filter) = filter_query.filter {
        query = query.filter(entities::user::Column::Name.like(format!("%{filter}%")));
    }
    let page_size = filter_query.page_size.unwrap_or(10);
    let page = filter_query.page.unwrap_or(0);
    let result = query.paginate(&state.db.conn, page_size);
    let items_and_pages = result.num_items_and_pages().await?;
    let users = result.fetch_page(page).await?;
    let users = users
        .into_iter()
        .filter_map(|user| {
            Some(ClusterUser {
                id: user.id,
                auth_provider: AuthProvider::from_str(&user.provider).ok()?,
                avatar_url: user.avatar_url,
                name: user.name,
                email: user.email,
                cluster_admin: user.cluster_admin,
                created_at: user.created_at,
            })
        })
        .collect();
    Ok(Json(ClusterUserResult {
        users,
        num_pages: items_and_pages.number_of_pages,
        total_items: items_and_pages.number_of_items,
        page,
        page_size,
    }))
}

pub async fn update_cluster_user(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path(user_id): Path<Uuid>,
    State(state): State<CoreState>,
    Json(update_cluter_user): Json<UpdateClusterUser>,
) -> Result<StatusCode, ApiError> {
    state.authenticate_cluster_admin(&cookie).await?;
    let user = state
        .db
        .get_user(user_id)
        .await?
        .ok_or_else(|| ApiError::InvalidRequest("user doesn't exist".to_string()))?;
    if !update_cluter_user.cluster_admin {
        // check if it's the last cluster admin if we're trying to change it to false
        if entities::user::Entity::find()
            .filter(entities::user::Column::DeletedAt.is_null())
            .filter(entities::user::Column::ClusterAdmin.eq(true))
            .filter(entities::user::Column::Id.ne(user.id))
            .one(&state.db.conn)
            .await?
            .is_none()
        {
            return Err(ApiError::InvalidRequest(
                "You can't turn off the last cluster admin".to_string(),
            ));
        }
    }
    entities::user::ActiveModel {
        id: ActiveValue::Set(user.id),
        cluster_admin: ActiveValue::Set(update_cluter_user.cluster_admin),
        ..Default::default()
    }
    .update(&state.db.conn)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_cpu_overcommit(
    TypedHeader(cookie): TypedHeader<Cookie>,
    State(state): State<CoreState>,
) -> Result<String, ApiError> {
    state.require_enterprise().await?;
    state.authenticate_cluster_admin(&cookie).await?;
    let value = state.conductor.cpu_overcommit().await;
    Ok(value.to_string())
}

pub async fn update_cpu_overcommit(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path(value): Path<usize>,
    State(state): State<CoreState>,
) -> Result<StatusCode, ApiError> {
    state.require_enterprise().await?;
    state.authenticate_cluster_admin(&cookie).await?;
    if value < 2 {
        return Err(ApiError::InvalidRequest(
            "Cpu overcommit must be greater than 1".to_string(),
        ));
    }
    *state.conductor.cpu_overcommit.write().await = value;
    state
        .db
        .update_config(LAPDEV_CPU_OVERCOMMIT, &value.to_string())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_hostnames(
    State(state): State<CoreState>,
) -> Result<Json<HashMap<String, String>>, ApiError> {
    let mut hostnames = state
        .conductor
        .enterprise
        .get_hostnames()
        .await
        .unwrap_or_default();

    if hostnames.is_empty() {
        hostnames.insert("".to_string(), "".to_string());
    }

    Ok(Json(hostnames))
}

pub async fn update_hostnames(
    TypedHeader(cookie): TypedHeader<Cookie>,
    State(state): State<CoreState>,
    Json(update): Json<Vec<(String, String)>>,
) -> Result<StatusCode, ApiError> {
    state.authenticate_cluster_admin(&cookie).await?;
    let mut hostnames = state
        .conductor
        .enterprise
        .get_hostnames()
        .await
        .unwrap_or_default();
    if hostnames.is_empty() {
        hostnames.insert("".to_string(), "".to_string());
    }

    for (region, hostname) in update {
        let region = region.trim();
        if !region.is_empty() {
            state.require_enterprise().await?;
        }

        let hostname = hostname.trim().to_lowercase();

        if let Some(license) = state
            .conductor
            .enterprise
            .license
            .license
            .read()
            .await
            .as_ref()
        {
            let hostname = hostname
                .split(':')
                .next()
                .ok_or_else(|| anyhow!("can't find hostname split"))?;
            let hostname_is_valid = hostname == license.hostname
                || hostname
                    .strip_suffix(&format!(".{}", license.hostname))
                    .map(|p| !p.contains('.'))
                    .unwrap_or(false);

            if !hostname_is_valid {
                return Err(ApiError::InvalidRequest(format!(
                    "the hostname needs to be {} or a subdomain of {}",
                    license.hostname, license.hostname
                )));
            }
        }

        if let Some(value) = hostnames.get_mut(region) {
            *value = hostname;
        }
    }

    state
        .db
        .update_config(LAPDEV_BASE_HOSTNAME, &serde_json::to_string(&hostnames)?)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}
