use axum::{
    body::Body,
    extract::{Host, State, WebSocketUpgrade},
    http::Request,
    response::{IntoResponse, Response},
    routing::{any, delete, get, post, put},
    Router,
};
use axum_client_ip::SecureClientIpSource;
use axum_extra::{headers, TypedHeader};
use lapdev_db::entities;
use lapdev_proxy_http::forward::ProxyForward;
use lapdev_rpc::error::ApiError;

use crate::{
    account, admin, machine_type, organization, project,
    session::{logout, new_session, session_authorize},
    state::CoreState,
    websocket::handle_websocket,
    workspace,
};

static STATIC_DIR: include_dir::Dir =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/../lapdev-dashboard/dist");

fn private_routes() -> Router<CoreState> {
    Router::new()
        .route("/session", get(new_session))
        .route("/session", delete(logout))
        .route("/session/authorize", get(session_authorize))
        .route("/me", get(account::me))
        .route(
            "/me/organization/:org_id",
            put(account::set_current_organization),
        )
}

fn v1_api_routes(additional_router: Option<Router<CoreState>>) -> Router<CoreState> {
    let router = Router::new()
        .route("/cluster_info", get(admin::get_cluster_info))
        .route("/auth_providers", get(admin::get_auth_providers))
        .route("/hostnames", get(admin::get_hostnames))
        .route("/machine_types", get(machine_type::all_machine_types))
        .route("/join/:invitation_id", put(organization::join_organization))
        .route("/organizations", post(organization::create_organization))
        .route(
            "/organizations/:org_id",
            delete(organization::delete_organization),
        )
        .route(
            "/organizations/:org_id/name",
            put(organization::update_org_name),
        )
        .route(
            "/organizations/:org_id/auto_start_stop",
            put(organization::update_org_auto_start_stop),
        )
        .route(
            "/organizations/:org_id/members",
            get(organization::get_organization_members),
        )
        .route(
            "/organizations/:org_id/members/:user_id",
            put(organization::update_organization_member),
        )
        .route(
            "/organizations/:org_id/members/:user_id",
            delete(organization::delete_organization_member),
        )
        .route(
            "/organizations/:org_id/invitations",
            post(organization::create_user_invitation),
        )
        .route(
            "/organizations/:org_id/usage",
            get(organization::get_organization_usage),
        )
        .route(
            "/organizations/:org_id/audit_logs",
            get(organization::get_organization_audit_log),
        )
        .route(
            "/organizations/:org_id/quota",
            get(organization::get_organization_quota),
        )
        .route(
            "/organizations/:org_id/quota",
            put(organization::update_organization_quota),
        )
        .route(
            "/organizations/:org_id/projects",
            post(project::create_project),
        )
        .route(
            "/organizations/:org_id/projects",
            get(project::all_projects),
        )
        .route(
            "/organizations/:org_id/projects/:project_id",
            get(project::get_project),
        )
        .route(
            "/organizations/:org_id/projects/:project_id",
            delete(project::delete_project),
        )
        .route(
            "/organizations/:org_id/projects/:project_id/branches",
            get(project::get_project_branches),
        )
        .route(
            "/organizations/:org_id/projects/:project_id/prebuilds",
            get(project::get_project_prebuilds),
        )
        .route(
            "/organizations/:org_id/projects/:project_id/prebuilds",
            post(project::create_project_prebuild),
        )
        .route(
            "/organizations/:org_id/projects/:project_id/prebuilds/:prebuild_id",
            delete(project::delete_project_prebuild),
        )
        .route(
            "/organizations/:org_id/projects/:project_id/machine_type/:machine_type_id",
            put(project::update_project_machine_type),
        )
        .route(
            "/organizations/:org_id/projects/:project_id/env",
            get(project::get_project_env),
        )
        .route(
            "/organizations/:org_id/projects/:project_id/env",
            put(project::update_project_env),
        )
        .route(
            "/organizations/:org_id/workspaces",
            post(workspace::create_workspace),
        )
        .route(
            "/organizations/:org_id/workspaces",
            get(workspace::all_workspaces),
        )
        .route(
            "/organizations/:org_id/workspaces/:workspace_name",
            get(workspace::get_workspace),
        )
        .route(
            "/organizations/:org_id/workspaces/:workspace_name",
            delete(workspace::delete_workspace),
        )
        .route(
            "/organizations/:org_id/workspaces/:workspace_name/start",
            post(workspace::start_workspace),
        )
        .route(
            "/organizations/:org_id/workspaces/:workspace_name/stop",
            post(workspace::stop_workspace),
        )
        .route("/account/ssh_keys", post(account::create_ssh_key))
        .route("/account/ssh_keys", get(account::all_ssh_keys))
        .route("/account/ssh_keys/:key_id", delete(account::delete_ssh_key))
        .route("/admin/workspace_hosts", get(admin::get_workspace_hosts))
        .route("/admin/workspace_hosts", post(admin::create_workspace_host))
        .route(
            "/admin/workspace_hosts/:workspace_host_id",
            put(admin::update_workspace_host),
        )
        .route(
            "/admin/workspace_hosts/:workspace_host_id",
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
            "/admin/cpu_overcommit/:value",
            put(admin::update_cpu_overcommit),
        )
        .route(
            "/admin/machine_types",
            post(machine_type::create_machine_type),
        )
        .route(
            "/admin/machine_types/:machine_type_id",
            put(machine_type::update_machine_type),
        )
        .route(
            "/admin/machine_types/:machine_type_id",
            delete(machine_type::delete_machine_type),
        )
        .route("/admin/users", get(admin::get_cluster_users))
        .route("/admin/users/:user_id", put(admin::update_cluster_user));
    if let Some(additional) = additional_router {
        router.merge(additional)
    } else {
        router
    }
}

fn main_routes(additional_router: Option<Router<CoreState>>) -> Router<CoreState> {
    Router::new()
        .nest("/v1", v1_api_routes(additional_router))
        .nest("/private", private_routes())
}

pub async fn build_router(
    state: CoreState,
    additional_router: Option<Router<CoreState>>,
) -> Router<()> {
    Router::new()
        .route("/", any(handle_catch_all))
        .route("/*0", any(handle_catch_all))
        .nest("/api", main_routes(additional_router))
        .with_state(state)
        .layer(SecureClientIpSource::ConnectInfo.into_extension())
}

async fn handle_catch_all(
    Host(hostname): Host,
    websocket: Option<WebSocketUpgrade>,
    State(state): State<CoreState>,
    TypedHeader(cookie): TypedHeader<headers::Cookie>,
    req: Request<Body>,
) -> Result<Response, ApiError> {
    let path = req.uri().path();
    let path_query = req
        .uri()
        .path_and_query()
        .map(|v| v.as_str())
        .unwrap_or(path);
    if let Some((ws, workspace_host, port)) =
        lapdev_proxy_http::proxy::forward_workspace(hostname, &state.db).await
    {
        is_http_foward_allowed(&cookie, &state, &ws, port.as_ref()).await?;
        let port = port
            .map(|p| p.host_port as u16)
            .or_else(|| ws.ide_port.map(|p| p as u16));
        if let Some(forward) =
            lapdev_proxy_http::forward::handler(&workspace_host.host, path_query, websocket, port)
                .await
        {
            match forward {
                ProxyForward::Resp(resp) => return Ok(resp),
                ProxyForward::Proxy(uri) => {
                    // *req.uri_mut() = uri;
                    let headers = req.headers().clone();
                    let mut new_req = Request::builder()
                        .method(req.method())
                        .uri(uri)
                        .body(req.into_body())
                        .unwrap();
                    *new_req.headers_mut() = headers;
                    let resp = state.hyper_client.request(new_req).await?;
                    return Ok(resp.into_response());
                }
            }
        } else {
            return Err(ApiError::InternalError("can't foward http".to_string()));
        }
    }

    if let Some(websocket) = websocket {
        let uri = req.uri();
        let path = uri.path();
        let query = uri.query();
        let resp = handle_websocket(path, query, websocket, cookie, state).await?;
        return Ok(resp);
    }

    if let Some(f) = path.strip_prefix("/static/") {
        let accept_gzip = req
            .headers()
            .get("Accept-Encoding")
            .and_then(|h| h.to_str().ok())
            .map(|s| s.contains("gzip"))
            .unwrap_or(false);
        if let Some(file) = STATIC_DIR.get_file(f) {
            let content_type = if f.ends_with(".css") {
                Some("text/css")
            } else if f.ends_with(".js") {
                Some("text/javascript")
            } else if f.ends_with(".wasm") {
                Some("application/wasm")
            } else {
                None
            };
            if let Some(content_type) = content_type {
                if accept_gzip {
                    if let Some(file) = STATIC_DIR.get_file(format!("{f}.gz")) {
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

    if let Some(file) = STATIC_DIR.get_file("index.html") {
        return Ok(axum::response::Html::from(file.contents()).into_response());
    }

    Err(ApiError::InvalidRequest("Invalid Request".to_string()))
}

async fn is_http_foward_allowed(
    cookie: &headers::Cookie,
    state: &CoreState,
    ws: &entities::workspace::Model,
    port: Option<&entities::workspace_port::Model>,
) -> Result<(), ApiError> {
    let user = state.authenticate(cookie).await?;
    if ws.user_id == user.id {
        return Ok(());
    }
    if let Some(port) = port {
        if port.shared
            && state
                .db
                .get_organization_member(user.id, ws.organization_id)
                .await
                .is_ok()
        {
            return Ok(());
        }
    }

    Err(ApiError::Unauthorized)
}
