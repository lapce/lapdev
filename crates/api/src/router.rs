use std::sync::Arc;

use axum::{
    body::Body,
    debug_handler,
    extract::{FromRequestParts, Path, Request, State, WebSocketUpgrade},
    http::{HeaderMap, Method, Uri},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{any, delete, get, post, put},
    Json, Router,
};
use axum_client_ip::ClientIpSource;
use axum_extra::{extract::Host, headers, TypedHeader};
use hyper::StatusCode;
use lapdev_api_hrpc::{HrpcService, HrpcServiceResponse};
use lapdev_common::WorkspaceStatus;
use lapdev_proxy_http::{forward::ProxyForward, proxy::WorkspaceForwardError};
use lapdev_rpc::error::ApiError;

use crate::{
    account, admin,
    kube::{kube_cluster_websocket, kube_data_plane_websocket},
    machine_type, organization, project,
    session::{logout, new_session, session_authorize},
    state::CoreState,
    websocket::handle_websocket,
    workspace,
};

static STATIC_DIR: include_dir::Dir =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/../dashboard/dist");
const PAGE_NOT_FOUND: &[u8] = include_bytes!("../pages/not_found.html");
const PAGE_NOT_AUTHORISED: &[u8] = include_bytes!("../pages/not_authorised.html");
const PAGE_NOT_RUNNING: &[u8] = include_bytes!("../pages/not_running.html");
const PAGE_NOT_FORWARDED: &[u8] = include_bytes!("../pages/not_forwarded.html");
const PAGE_CSS: &[u8] = include_bytes!("../pages/main.css");


fn private_routes() -> Router<Arc<CoreState>> {
    Router::new()
        .route("/session", get(new_session))
        .route("/session", delete(logout))
        .route("/session/authorize", get(session_authorize))
        .route("/me", get(account::me))
        .route(
            "/me/organization/{org_id}",
            put(account::set_current_organization),
        )
}

async fn not_found() -> impl IntoResponse {
    StatusCode::NOT_FOUND
}

fn v1_api_routes() -> Router<Arc<CoreState>> {
    Router::new()
        .route("/", any(not_found))
        .route("/{*wildcard}", any(not_found))
        .route("/cluster_info", get(admin::get_cluster_info))
        .route("/auth_providers", get(admin::get_auth_providers))
        .route("/hostnames", get(admin::get_hostnames))
        .route("/machine_types", get(machine_type::all_machine_types))
        .route(
            "/join/{invitation_id}",
            put(organization::join_organization),
        )
        .route("/kube/cluster/rpc", any(kube_cluster_websocket))
        .route("/kube/cluster/tunnel", any(kube_data_plane_websocket))
        .route("/organizations", post(organization::create_organization))
        .route(
            "/organizations/{org_id}",
            delete(organization::delete_organization),
        )
        .route(
            "/organizations/{org_id}/name",
            put(organization::update_org_name),
        )
        .route(
            "/organizations/{org_id}/auto_start_stop",
            put(organization::update_org_auto_start_stop),
        )
        .route(
            "/organizations/{org_id}/members",
            get(organization::get_organization_members),
        )
        .route(
            "/organizations/{org_id}/members/{user_id}",
            put(organization::update_organization_member),
        )
        .route(
            "/organizations/{org_id}/members/{user_id}",
            delete(organization::delete_organization_member),
        )
        .route(
            "/organizations/{org_id}/invitations",
            post(organization::create_user_invitation),
        )
        .route(
            "/organizations/{org_id}/usage",
            get(organization::get_organization_usage),
        )
        .route(
            "/organizations/{org_id}/audit_logs",
            get(organization::get_organization_audit_log),
        )
        .route(
            "/organizations/{org_id}/quota",
            get(organization::get_organization_quota),
        )
        .route(
            "/organizations/{org_id}/quota",
            put(organization::update_organization_quota),
        )
        .route(
            "/organizations/{org_id}/projects",
            post(project::create_project),
        )
        .route(
            "/organizations/{org_id}/projects",
            get(project::all_projects),
        )
        .route(
            "/organizations/{org_id}/projects/{project_id}",
            get(project::get_project),
        )
        .route(
            "/organizations/{org_id}/projects/{project_id}",
            delete(project::delete_project),
        )
        .route(
            "/organizations/{org_id}/projects/{project_id}/branches",
            get(project::get_project_branches),
        )
        .route(
            "/organizations/{org_id}/projects/{project_id}/prebuilds",
            get(project::get_project_prebuilds),
        )
        .route(
            "/organizations/{org_id}/projects/{project_id}/prebuilds",
            post(project::create_project_prebuild),
        )
        .route(
            "/organizations/{org_id}/projects/{project_id}/prebuilds/{prebuild_id}",
            delete(project::delete_project_prebuild),
        )
        .route(
            "/organizations/{org_id}/projects/{project_id}/machine_type/{machine_type_id}",
            put(project::update_project_machine_type),
        )
        .route(
            "/organizations/{org_id}/projects/{project_id}/env",
            get(project::get_project_env),
        )
        .route(
            "/organizations/{org_id}/projects/{project_id}/env",
            put(project::update_project_env),
        )
        .route(
            "/organizations/{org_id}/workspaces",
            post(workspace::create_workspace),
        )
        .route(
            "/organizations/{org_id}/workspaces",
            get(workspace::all_workspaces),
        )
        .route(
            "/organizations/{org_id}/workspaces/{workspace_name}",
            get(workspace::get_workspace),
        )
        .route(
            "/organizations/{org_id}/workspaces/{workspace_name}",
            delete(workspace::delete_workspace),
        )
        .route(
            "/organizations/{org_id}/workspaces/{workspace_name}/start",
            post(workspace::start_workspace),
        )
        .route(
            "/organizations/{org_id}/workspaces/{workspace_name}/stop",
            post(workspace::stop_workspace),
        )
        .route(
            "/organizations/{org_id}/workspaces/{workspace_name}/pin",
            post(workspace::pin_workspace),
        )
        .route(
            "/organizations/{org_id}/workspaces/{workspace_name}/unpin",
            post(workspace::unpin_workspace),
        )
        .route(
            "/organizations/{org_id}/workspaces/{workspace_name}/rebuild",
            post(workspace::rebuild_workspace),
        )
        .route(
            "/organizations/{org_id}/workspaces/{workspace_name}/ports",
            get(workspace::workspace_ports),
        )
        .route(
            "/organizations/{org_id}/workspaces/{workspace_name}/ports/{port}",
            put(workspace::update_workspace_port),
        )
        .route("/account/ssh_keys", post(account::create_ssh_key))
        .route("/account/ssh_keys", get(account::all_ssh_keys))
        .route(
            "/account/ssh_keys/{key_id}",
            delete(account::delete_ssh_key),
        )
        .route("/account/git_providers", get(account::get_git_providers))
        .route(
            "/account/git_providers/connect",
            put(account::connect_git_provider),
        )
        .route(
            "/account/git_providers/disconnect",
            put(account::disconnect_git_provider),
        )
        .route(
            "/account/git_providers/update_scope",
            put(account::update_scope),
        )
        .route("/admin/workspace_hosts", get(admin::get_workspace_hosts))
        .route("/admin/workspace_hosts", post(admin::create_workspace_host))
        .route(
            "/admin/workspace_hosts/{workspace_host_id}",
            put(admin::update_workspace_host),
        )
        .route(
            "/admin/workspace_hosts/{workspace_host_id}",
            delete(admin::delete_workspace_host),
        )
        .route("/admin/license", get(admin::get_license))
        .route("/admin/license", put(admin::update_license))
        .route("/admin/new_license", post(admin::new_license))
        .route("/admin/oauth", get(admin::get_oauth))
        .route("/admin/oauth", put(admin::update_oauth))
        .route("/admin/certs", get(admin::get_certs))
        .route("/admin/certs", put(admin::update_certs))
        .route("/admin/hostnames", put(admin::update_hostnames))
        .route("/admin/cpu_overcommit", get(admin::get_cpu_overcommit))
        .route(
            "/admin/cpu_overcommit/{value}",
            put(admin::update_cpu_overcommit),
        )
        .route(
            "/admin/machine_types",
            post(machine_type::create_machine_type),
        )
        .route(
            "/admin/machine_types/{machine_type_id}",
            put(machine_type::update_machine_type),
        )
        .route(
            "/admin/machine_types/{machine_type_id}",
            delete(machine_type::delete_machine_type),
        )
        .route("/admin/users", get(admin::get_cluster_users))
        .route("/admin/users/{user_id}", put(admin::update_cluster_user))
        .route(
            "/admin/organizations/{org_id}/clear",
            put(organization::clear_organization),
        )
}

fn main_routes() -> Router<Arc<CoreState>> {
    Router::new()
        .nest("/v1", v1_api_routes())
        .nest("/private", private_routes())
        .route("/rpc", any(handle_rpc))
}

pub fn add_forward_middleware(state: Arc<CoreState>, router: Router) -> Router {
    router
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            forward_middleware,
        ))
        .layer(ClientIpSource::ConnectInfo.into_extension())
}

pub fn build_router(state: Arc<CoreState>) -> Router {
    Router::new()
        .route("/", any(handle_catch_all))
        .route("/{*wildcard}", any(handle_catch_all))
        .route("/health-check", get(health_check))
        .nest("/api", main_routes())
        .with_state(state)
}

async fn health_check() {}

#[debug_handler]
async fn handle_catch_all(
    State(state): State<Arc<CoreState>>,
    // websocket: WebSocketUpgrade,
    TypedHeader(cookie): TypedHeader<headers::Cookie>,
    req: Request,
) -> Result<Response, ApiError> {
    let (parts, _) = req.into_parts();
    let mut parts_clone = parts.clone();
    let websocket = WebSocketUpgrade::from_request_parts(&mut parts_clone, &())
        .await
        .ok();

    let path = parts.uri.path();
    tracing::debug!("handle catch all got {path}");

    if let Some(websocket) = websocket {
        let uri = &parts.uri;
        let path = uri.path();
        let query = uri.query();
        let resp = handle_websocket(path, query, websocket, cookie, state).await?;
        return Ok(resp);
    }

    if let Some(f) = path.strip_prefix("/static/") {
        let accept_gzip = parts
            .headers
            .get("Accept-Encoding")
            .and_then(|h| h.to_str().ok())
            .map(|s| s.contains("gzip"))
            .unwrap_or(false);
        if let Some(file) = if let Some(d) = state.static_dir.as_ref() {
            d.get_file(f)
        } else {
            STATIC_DIR.get_file(f)
        } {
            let content_type = if f.ends_with(".css") {
                Some("text/css")
            } else if f.ends_with(".js") {
                Some("text/javascript")
            } else if f.ends_with(".wasm") {
                Some("application/wasm")
            } else if f.ends_with(".ico") {
                Some("image/vnd.microsoft.icon")
            } else {
                None
            };
            if let Some(content_type) = content_type {
                if accept_gzip {
                    if let Some(file) = if let Some(d) = state.static_dir.as_ref() {
                        d.get_file(format!("{f}.gz"))
                    } else {
                        STATIC_DIR.get_file(format!("{f}.gz"))
                    } {
                        return Ok((
                            [
                                (axum::http::header::CONTENT_TYPE, content_type),
                                (
                                    axum::http::header::CACHE_CONTROL,
                                    "public, max-age=31536000",
                                ),
                                (axum::http::header::CONTENT_ENCODING, "gzip"),
                            ],
                            file.contents(),
                        )
                            .into_response());
                    }
                }
                return Ok((
                    [
                        (axum::http::header::CONTENT_TYPE, content_type),
                        (
                            axum::http::header::CACHE_CONTROL,
                            "public, max-age=31536000",
                        ),
                    ],
                    file.contents(),
                )
                    .into_response());
            }
        }
    }

    if let Some(file) = if let Some(d) = state.static_dir.as_ref() {
        d.get_file("index.html")
    } else {
        STATIC_DIR.get_file("index.html")
    } {
        return Ok(axum::response::Html::from(file.contents()).into_response());
    }

    Err(ApiError::InvalidRequest("Invalid Request".to_string()))
}

async fn forward_middleware(
    State(state): State<Arc<CoreState>>,
    Host(hostname): Host,
    TypedHeader(cookie): TypedHeader<headers::Cookie>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let hostname = hostname
        .split(":")
        .next()
        .ok_or_else(|| ApiError::InternalError("can't split : from hostname".to_string()))?;
    let is_forward = state
        .conductor
        .hostnames
        .read()
        .await
        .values()
        .any(|v| v != hostname && hostname.ends_with(v));
    let (parts, body) = req.into_parts();
    let mut parts_clone = parts.clone();
    let websocket = WebSocketUpgrade::from_request_parts(&mut parts_clone, &())
        .await
        .ok();

    let path = parts.uri.path();
    let path_query = parts
        .uri
        .path_and_query()
        .map(|v| v.as_str())
        .unwrap_or(path);

    if !is_forward {
        if path == "/ws" || path == "/all_workspaces_ws" {
            if let Some(websocket) = websocket {
                let uri = &parts.uri;
                let path = uri.path();
                let query = uri.query();
                let resp = handle_websocket(path, query, websocket, cookie, state).await?;
                return Ok(resp);
            }
        }

        return Ok(next.run(Request::from_parts(parts, body)).await);
    }

    if path.starts_with("/error-page/lapdev-main.css") {
        return Ok((
            [
                (axum::http::header::CONTENT_TYPE, "text/css"),
                (
                    axum::http::header::CACHE_CONTROL,
                    "public, max-age=31536000",
                ),
            ],
            PAGE_CSS,
        )
            .into_response());
    }

    let user = state.authenticate(&cookie).await.ok();
    match lapdev_proxy_http::proxy::forward_workspace(hostname, &state.db, user.as_ref()).await {
        Ok((ws, port)) => {
            if ws.status != WorkspaceStatus::Running.to_string() {
                return Ok(axum::response::Html::from(PAGE_NOT_RUNNING).into_response());
            }

            let Some(workspace_host) = state.db.get_workspace_host(ws.host_id).await.ok().flatten()
            else {
                return Ok(axum::response::Html::from(PAGE_NOT_RUNNING).into_response());
            };

            let port = port
                .map(|p| p.host_port as u16)
                .or_else(|| ws.ide_port.map(|p| p as u16));
            if let Some(forward) = lapdev_proxy_http::forward::handler(
                &workspace_host.host,
                path_query,
                websocket,
                &parts.headers,
                port,
            )
            .await
            {
                match forward {
                    ProxyForward::Resp(resp) => Ok(resp),
                    ProxyForward::Proxy(uri) => {
                        // *req.uri_mut() = uri;
                        let headers = parts.headers.clone();
                        let mut new_req = Request::builder()
                            .method(parts.method)
                            .uri(uri)
                            .body(body)
                            .unwrap();
                        *new_req.headers_mut() = headers;
                        let resp = state.hyper_client.request(new_req).await?;
                        Ok(resp.into_response())
                    }
                }
            } else {
                Ok(axum::response::Html::from(PAGE_NOT_FOUND).into_response())
            }
        }
        Err(e) => {
            let b = match e {
                WorkspaceForwardError::WorkspaceNotFound => PAGE_NOT_FOUND,
                WorkspaceForwardError::PortNotForwarded => PAGE_NOT_FORWARDED,
                WorkspaceForwardError::InvalidHostname => PAGE_NOT_RUNNING,
                WorkspaceForwardError::Unauthorised => PAGE_NOT_AUTHORISED,
            };
            Ok(axum::response::Html::from(b).into_response())
        }
    }
}

async fn handle_rpc(
    State(state): State<Arc<CoreState>>,
    headers: axum::http::HeaderMap,
    body: String,
) -> Result<Json<HrpcServiceResponse>, ApiError> {
    let r = state.handle_rpc(&headers, &body).await?;
    Ok(Json(r))
}
