use std::{collections::BTreeMap, sync::Arc};

use axum::{
    extract::{ws::WebSocket, Path, Query, State, WebSocketUpgrade},
    http::HeaderMap,
    response::Response,
    Json,
};
use axum_extra::{headers, TypedHeader};
use futures::StreamExt;
use lapdev_common::devbox::{DirectCandidateSet, DirectChannelConfig, DirectCredential};
use lapdev_devbox_rpc::{
    DevboxClientRpcClient, DevboxInterceptRpc, DevboxSessionInfo, DevboxSessionRpc, PortMapping,
};
use lapdev_rpc::{error::ApiError, spawn_twoway};
use lapdev_tunnel::{
    direct::QuicTransport, relay_client_addr, relay_server_addr, run_tunnel_server_with_connector,
    websocket_serde_transport_from_socket, DynTunnelStream, TunnelError, TunnelTarget,
    WebSocketUdpSocket,
};
use serde::{Deserialize, Serialize};
use tarpc::server::{BaseChannel, Channel};
use uuid::Uuid;

use crate::{
    devbox_tunnels::split_axum_websocket, state::CoreState, websocket_socket::AxumBinarySocket,
};

#[derive(Debug, Serialize)]
pub struct WhoamiResponse {
    pub user_id: Uuid,
    pub email: String,
    pub name: Option<String>,
    pub organization_id: Uuid,
    pub device_name: String,
    pub authenticated_at: Option<String>,
    pub expires_at: Option<String>,
}

/// GET /api/v1/devbox/whoami - Get current CLI session info
pub async fn devbox_whoami(
    State(state): State<Arc<CoreState>>,
    TypedHeader(auth): TypedHeader<headers::Authorization<headers::authorization::Bearer>>,
) -> Result<Json<WhoamiResponse>, ApiError> {
    let ctx = state.authenticate_bearer(&auth).await?;

    // Extract timestamps from token claims if available
    let authenticated_at = ctx
        .token_claims
        .get_claim("iat")
        .and_then(|v| serde_json::from_value::<String>(v.clone()).ok());

    let expires_at = ctx
        .token_claims
        .get_claim("exp")
        .and_then(|v| serde_json::from_value::<String>(v.clone()).ok());

    Ok(Json(WhoamiResponse {
        user_id: ctx.user.id,
        email: ctx.user.email.unwrap_or_default(),
        name: ctx.user.name,
        organization_id: ctx.organization_id,
        device_name: ctx.device_name,
        authenticated_at,
        expires_at,
    }))
}

/// WebSocket endpoint for devbox RPC connections (control plane)
pub async fn devbox_rpc_websocket(
    websocket: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<Arc<CoreState>>,
) -> Result<Response, ApiError> {
    tracing::debug!("Handling devbox RPC WebSocket connection");

    // Extract Bearer token from Authorization header
    let auth_header = headers
        .get("Authorization")
        .ok_or(ApiError::Unauthenticated)?
        .to_str()
        .map_err(|_| ApiError::Unauthenticated)?;

    if !auth_header.starts_with("Bearer ") {
        return Err(ApiError::Unauthenticated);
    }

    let token = &auth_header[7..]; // Skip "Bearer "

    // Authenticate using the same method as regular devbox requests
    let bearer = headers::Authorization::bearer(token).map_err(|_| ApiError::Unauthenticated)?;
    let ctx = state.authenticate_bearer(&bearer).await?;

    tracing::info!(
        "Devbox RPC WebSocket authenticated for user {} ({})",
        ctx.user.id,
        ctx.device_name
    );

    // Extract expiration time from token claims
    let expires_at = ctx
        .token_claims
        .get_claim("exp")
        .and_then(|v| serde_json::from_value::<String>(v.clone()).ok())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(|| chrono::Utc::now() + chrono::Duration::days(30));

    // Check for existing active sessions and notify them before displacing
    if let Some(existing_handles) = state
        .active_devbox_sessions
        .read()
        .await
        .get(&ctx.user.id)
        .map(|entries| entries.values().cloned().collect::<Vec<_>>())
    {
        for old_handle in existing_handles {
            tracing::info!(
                "Displacing existing session {} on device {} for user {}",
                old_handle.session_id,
                old_handle.device_name,
                ctx.user.id
            );

            let _ = old_handle
                .notify_tx
                .send(crate::state::DevboxSessionNotification::Displaced {
                    new_device_name: ctx.device_name.clone(),
                });
        }
    }

    // Create or update devbox session
    state
        .db
        .create_or_update_devbox_session(
            ctx.user.id,
            token,
            ctx.device_name.clone(),
            expires_at.into(),
        )
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to create session: {}", e)))?;

    tracing::info!(
        "Created devbox session for user {} on device {}",
        ctx.user.id,
        ctx.device_name
    );

    Ok(handle_devbox_rpc_upgrade(
        websocket,
        state,
        ctx.session_id,
        ctx.user.id,
        ctx.organization_id,
        ctx.device_name,
    ))
}

fn handle_devbox_rpc_upgrade(
    websocket: WebSocketUpgrade,
    state: Arc<CoreState>,
    session_id: Uuid,
    user_id: Uuid,
    organization_id: Uuid,
    device_name: String,
) -> Response {
    websocket
        .on_failed_upgrade(|e| tracing::error!("devbox RPC websocket upgrade failed {e:?}"))
        .on_upgrade(move |socket| async move {
            handle_devbox_rpc(
                socket,
                state,
                session_id,
                user_id,
                organization_id,
                device_name,
            )
            .await;
        })
}

async fn handle_devbox_rpc(
    socket: WebSocket,
    state: Arc<CoreState>,
    session_id: Uuid,
    user_id: Uuid,
    organization_id: Uuid,
    device_name: String,
) {
    let socket = AxumBinarySocket::new(socket);
    let transport = websocket_serde_transport_from_socket(
        socket,
        tarpc::tokio_serde::formats::Bincode::default(),
    );
    let (server_chan, client_chan, _abort_handle) = spawn_twoway(transport);

    // Create RPC client (for calling CLI methods)
    let rpc_client =
        DevboxClientRpcClient::new(tarpc::client::Config::default(), client_chan).spawn();

    // Create notification channel for session migration
    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel();

    // Register this session as active
    let connection_id = Uuid::new_v4();
    {
        let mut sessions = state.active_devbox_sessions.write().await;
        sessions
            .entry(user_id)
            .or_insert_with(BTreeMap::new)
            .insert(
                connection_id,
                crate::state::DevboxSessionHandle {
                    session_id,
                    device_name: device_name.clone(),
                    notify_tx,
                    rpc_client: rpc_client.clone(),
                    connection_id,
                },
            );
    }

    // Create RPC server (for CLI to call our methods)
    let rpc_server = DevboxSessionRpcServer::new(
        session_id,
        user_id,
        organization_id,
        device_name.clone(),
        rpc_client.clone(),
        state.clone(),
    );

    tracing::info!(
        "Starting DevboxSessionRpc server for session {} user {} device {}",
        session_id,
        user_id,
        device_name
    );

    // Run RPC server and notification listener concurrently
    let rpc_client_for_notifications = rpc_client.clone();
    let notification_task = tokio::spawn(async move {
        while let Some(notification) = notify_rx.recv().await {
            match notification {
                crate::state::DevboxSessionNotification::Displaced { new_device_name } => {
                    tracing::info!(
                        "Session {} displaced by new login from {}",
                        session_id,
                        new_device_name
                    );
                    // Notify the CLI that it's been displaced
                    let _ = rpc_client_for_notifications
                        .session_displaced(tarpc::context::current(), new_device_name)
                        .await;
                }
            }
        }
    });

    // Run the RPC server
    BaseChannel::with_defaults(server_chan)
        .execute(rpc_server.serve())
        .for_each(|resp| async move {
            tokio::spawn(resp);
        })
        .await;

    // Cleanup: unregister session and stop notification listener
    {
        let mut sessions = state.active_devbox_sessions.write().await;
        if let Some(entries) = sessions.get_mut(&user_id) {
            entries.remove(&connection_id);
            if entries.is_empty() {
                sessions.remove(&user_id);
            }
        }
    }
    notification_task.abort();

    if let Ok(Some(session)) = state.db.get_active_devbox_session(user_id).await {
        if let Some(environment_id) = session.active_environment_id {
            let state_clone = state.clone();
            tokio::spawn(async move {
                state_clone
                    .clear_devbox_routes_for_environment(environment_id)
                    .await;
            });
        }
    }

    // Mark session as revoked in database
    if let Err(e) = state.db.revoke_active_devbox_session(user_id).await {
        tracing::error!(
            "Failed to revoke session {} on disconnect: {}",
            session_id,
            e
        );
    }

    tracing::info!(
        "DevboxSessionRpc server stopped for session {} user {} device {} (session revoked)",
        session_id,
        user_id,
        device_name
    );
}

#[derive(Default, Deserialize)]
pub struct DevboxInterceptTunnelParams {
    _workload_id: Option<Uuid>,
}

/// WebSocket endpoint for devbox intercept tunnels (server side - receives connections from in-cluster services)
pub async fn devbox_intercept_tunnel_websocket(
    Path(requested_user_id): Path<Uuid>,
    Query(_params): Query<DevboxInterceptTunnelParams>,
    websocket: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<Arc<CoreState>>,
) -> Result<Response, ApiError> {
    tracing::debug!("Handling devbox tunnel WebSocket connection");

    let auth_header = headers
        .get("Authorization")
        .ok_or(ApiError::Unauthenticated)?
        .to_str()
        .map_err(|_| ApiError::Unauthenticated)?;

    if !auth_header.starts_with("Bearer ") {
        return Err(ApiError::Unauthenticated);
    }

    let token = &auth_header[7..];

    let session = state
        .db
        .get_devbox_session_by_token_hash(token)
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to load devbox session: {}", e)))?
        .ok_or(ApiError::Unauthenticated)?;

    if session.user_id != requested_user_id {
        tracing::warn!(
            "Devbox user mismatch: token user {} vs path {}",
            session.user_id,
            requested_user_id
        );
        return Err(ApiError::Unauthenticated);
    }

    state
        .db
        .update_devbox_session_last_used(session.user_id)
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to update session usage: {}", e)))?;

    tracing::info!(
        "Devbox tunnel WebSocket authenticated for user {} ({})",
        session.user_id,
        session.device_name
    );

    let user_id = session.user_id;
    let registry = state.devbox_tunnels.clone();

    let token_string = token.to_string();

    Ok(websocket.on_upgrade(move |socket| {
        let registry = registry.clone();
        async move {
            registry.register_cli(user_id, token_string, socket).await;
        }
    }))
}

/// WebSocket endpoint for devbox client tunnels (client side - connects to in-cluster services via devbox-proxy)
pub async fn devbox_client_tunnel_websocket(
    Path(requested_user_id): Path<Uuid>,
    websocket: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<Arc<CoreState>>,
) -> Result<Response, ApiError> {
    tracing::debug!("Handling devbox client tunnel WebSocket connection");

    let auth_header = headers
        .get("Authorization")
        .ok_or(ApiError::Unauthenticated)?
        .to_str()
        .map_err(|_| ApiError::Unauthenticated)?;

    if !auth_header.starts_with("Bearer ") {
        return Err(ApiError::Unauthenticated);
    }

    let token = &auth_header[7..];

    let session = state
        .db
        .get_devbox_session_by_token_hash(token)
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to load devbox session: {}", e)))?
        .ok_or(ApiError::Unauthenticated)?;

    if session.user_id != requested_user_id {
        tracing::warn!(
            "Devbox user mismatch: token user {} vs path {}",
            session.user_id,
            requested_user_id
        );
        return Err(ApiError::Unauthenticated);
    }

    state
        .db
        .update_devbox_session_last_used(session.user_id)
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to update session usage: {}", e)))?;

    tracing::info!(
        "Devbox client tunnel WebSocket authenticated for user {} ({})",
        session.user_id,
        session.device_name
    );

    let user_id = session.user_id;

    let environment_id = session
        .active_environment_id
        .ok_or_else(|| ApiError::InvalidRequest("No active environment selected".to_string()))?;

    let environment = state.db.get_kube_environment(environment_id).await?;

    tracing::info!(
        "Devbox client tunnel for user {} targeting environment {}",
        user_id,
        environment_id
    );
    let cluster_id = environment.map(|e| e.cluster_id);

    let tunnel_registry = state.kube_controller.tunnel_registry.clone();
    let token_string = token.to_string();

    Ok(websocket.on_upgrade(move |socket| {
        let registry = tunnel_registry.clone();
        let cluster_id = cluster_id;
        let user_id = user_id;
        let token = token_string.clone();
        async move {
            let (sink, stream) = split_axum_websocket(socket);
            let udp_socket = WebSocketUdpSocket::from_parts(
                sink,
                stream,
                relay_server_addr(),
                relay_client_addr(),
            );

            let connector = move |target: TunnelTarget| {
                let registry = registry.clone();
                async move {
                    let Some(cluster_id) = cluster_id else {
                        return Err(TunnelError::Remote("active environment is not set".into()));
                    };

                    match registry.get_client(cluster_id).await {
                        Some(cluster_client) if !cluster_client.is_closed() => {
                            let TunnelTarget { host, port } = target;
                            let stream = cluster_client.connect_tcp(host, port).await?;
                            Ok::<DynTunnelStream, TunnelError>(Box::new(stream) as DynTunnelStream)
                        }
                        Some(_) => {
                            tracing::warn!(
                                user_id = %user_id,
                                cluster_id = %cluster_id,
                                "Cluster tunnel is closed; rejecting devbox client request"
                            );
                            Err(TunnelError::Remote("cluster tunnel is closed".into()))
                        }
                        None => {
                            tracing::warn!(
                                user_id = %user_id,
                                cluster_id = %cluster_id,
                                "No cluster tunnel available for devbox client request"
                            );
                            Err(TunnelError::Remote("cluster tunnel unavailable".into()))
                        }
                    }
                }
            };

            match QuicTransport::accept_udp_server(udp_socket, &token).await {
                Ok(transport) => {
                    if let Err(err) = run_tunnel_server_with_connector(transport, connector).await {
                        tracing::warn!(
                            user_id = %user_id,
                            error = %err,
                            "Devbox client tunnel terminated with error"
                        );
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        user_id = %user_id,
                        error = %err,
                        "Failed to establish QUIC relay for devbox client tunnel"
                    );
                }
            }
        }
    }))
}

#[derive(Clone)]
struct DevboxSessionRpcServer {
    session_id: Uuid,
    user_id: Uuid,
    #[allow(dead_code)]
    organization_id: Uuid,
    #[allow(dead_code)]
    device_name: String,
    #[allow(dead_code)]
    client_rpc: DevboxClientRpcClient,
    state: Arc<CoreState>,
}

impl DevboxSessionRpcServer {
    fn new(
        session_id: Uuid,
        user_id: Uuid,
        organization_id: Uuid,
        device_name: String,
        client_rpc: DevboxClientRpcClient,
        state: Arc<CoreState>,
    ) -> Self {
        Self {
            session_id,
            user_id,
            organization_id,
            device_name,
            client_rpc,
            state,
        }
    }
}

impl DevboxSessionRpc for DevboxSessionRpcServer {
    async fn whoami(self, _context: tarpc::context::Context) -> Result<DevboxSessionInfo, String> {
        tracing::info!("whoami called for session {}", self.session_id);

        let user = self
            .state
            .db
            .get_user(self.user_id)
            .await
            .map_err(|e| format!("Failed to fetch user: {}", e))?
            .ok_or_else(|| "User not found".to_string())?;

        let session = self
            .state
            .db
            .get_active_devbox_session(self.user_id)
            .await
            .map_err(|e| format!("Failed to fetch session: {}", e))?
            .ok_or_else(|| "Session not found".to_string())?;

        if let Some(environment_id) = session.active_environment_id {
            let state = self.state.clone();
            let user_id = self.user_id;
            tokio::spawn(async move {
                state.push_devbox_routes(user_id, environment_id).await;
            });
        }

        Ok(DevboxSessionInfo {
            user_id: self.user_id,
            email: user.email.unwrap_or_default(),
            device_name: session.device_name,
            created_at: session.created_at.with_timezone(&chrono::Utc),
            expires_at: session.expires_at.with_timezone(&chrono::Utc),
            last_used_at: session.last_used_at.with_timezone(&chrono::Utc),
        })
    }

    async fn get_active_environment(
        self,
        _context: tarpc::context::Context,
    ) -> Result<Option<lapdev_devbox_rpc::DevboxEnvironmentInfo>, String> {
        tracing::info!(
            "get_active_environment called for session {}",
            self.session_id
        );

        let session = self
            .state
            .db
            .get_active_devbox_session(self.user_id)
            .await
            .map_err(|e| format!("Failed to fetch session: {}", e))?
            .ok_or_else(|| "Session not found".to_string())?;

        if let Some(env_id) = session.active_environment_id {
            let environment = self
                .state
                .db
                .get_kube_environment(env_id)
                .await
                .map_err(|e| format!("Failed to fetch environment: {}", e))?
                .ok_or_else(|| "Environment not found".to_string())?;

            let cluster = self
                .state
                .db
                .get_kube_cluster(environment.cluster_id)
                .await
                .map_err(|e| format!("Failed to fetch cluster: {}", e))?
                .ok_or_else(|| "Cluster not found".to_string())?;

            Ok(Some(lapdev_devbox_rpc::DevboxEnvironmentInfo {
                environment_id: env_id,
                cluster_name: cluster.name,
                namespace: environment.namespace,
            }))
        } else {
            Ok(None)
        }
    }

    async fn update_device_name(
        self,
        _context: tarpc::context::Context,
        device_name: String,
    ) -> Result<(), String> {
        tracing::info!("update_device_name called for session {}", self.session_id);

        self.state
            .db
            .update_devbox_session_device_name(self.user_id, device_name.clone())
            .await
            .map_err(|e| format!("Failed to update device name: {}", e))?;

        {
            let mut sessions = self.state.active_devbox_sessions.write().await;
            if let Some(entries) = sessions.get_mut(&self.user_id) {
                if let Some(handle) = entries
                    .values_mut()
                    .find(|handle| handle.session_id == self.session_id)
                {
                    handle.device_name = device_name.clone();
                }
            }
        }

        tracing::info!(
            "Updated device name for session {} to {}",
            self.session_id,
            device_name
        );

        Ok(())
    }

    async fn heartbeat(self, _context: tarpc::context::Context) -> Result<(), String> {
        tracing::trace!("Heartbeat received for session {}", self.session_id);
        Ok(())
    }

    async fn publish_direct_candidates(
        self,
        _context: tarpc::context::Context,
        update: DirectCandidateSet,
    ) -> Result<DirectChannelConfig, String> {
        let credential = DirectCredential {
            token: Uuid::new_v4().to_string(),
            expires_at: chrono::Utc::now() + chrono::Duration::minutes(10),
        };

        let config = DirectChannelConfig {
            credential,
            devbox_candidates: update.candidates,
            sidecar_candidates: Vec::new(),
            server_certificate: update.server_certificate,
        };

        self.state
            .set_direct_channel_config(self.user_id, config.clone())
            .await;

        Ok(config)
    }

    async fn set_active_environment(
        self,
        _context: tarpc::context::Context,
        environment_id: Uuid,
    ) -> Result<(), String> {
        tracing::info!(
            "set_active_environment called for session {} with environment {}",
            self.session_id,
            environment_id
        );

        let previous_environment = self
            .state
            .db
            .get_active_devbox_session(self.user_id)
            .await
            .map_err(|e| format!("Failed to fetch session: {}", e))?
            .and_then(|session| session.active_environment_id);

        self.state
            .db
            .update_devbox_session_active_environment(self.user_id, Some(environment_id))
            .await
            .map_err(|e| format!("Failed to update active environment: {}", e))?;

        // Fetch environment details to notify client
        let environment = self
            .state
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(|e| format!("Failed to fetch environment: {}", e))?
            .ok_or_else(|| "Environment not found".to_string())?;

        let cluster = self
            .state
            .db
            .get_kube_cluster(environment.cluster_id)
            .await
            .map_err(|e| format!("Failed to fetch cluster: {}", e))?
            .ok_or_else(|| "Cluster not found".to_string())?;

        // Notify client about the environment change
        let env_info = lapdev_devbox_rpc::DevboxEnvironmentInfo {
            environment_id: environment.id,
            cluster_name: cluster.name,
            namespace: environment.namespace.clone(),
        };

        // Fire and forget - don't wait for client response
        let client_rpc = self.client_rpc.clone();
        tracing::info!(
            "Spawning task to notify CLI about environment change to {} ({})",
            env_info.cluster_name,
            env_info.namespace
        );
        tokio::spawn(async move {
            tracing::info!("Calling environment_changed RPC on client...");
            match client_rpc
                .environment_changed(tarpc::context::current(), Some(env_info.clone()))
                .await
            {
                Ok(_) => {
                    tracing::info!(
                        "Successfully notified client of environment change to {}",
                        env_info.namespace
                    );
                }
                Err(e) => {
                    tracing::warn!("Failed to notify client of environment change: {}", e);
                }
            }
        });

        let state = self.state.clone();
        let user_id = self.user_id;
        tokio::spawn(async move {
            state.push_devbox_routes(user_id, environment_id).await;
        });

        if let Some(prev_env) = previous_environment.filter(|prev| *prev != environment_id) {
            let state = self.state.clone();
            tokio::spawn(async move {
                state.clear_devbox_routes_for_environment(prev_env).await;
            });
        }

        Ok(())
    }

    async fn list_workload_intercepts(
        self,
        _context: tarpc::context::Context,
        environment_id: Uuid,
    ) -> Result<Vec<lapdev_devbox_rpc::WorkloadInterceptInfo>, String> {
        tracing::info!(
            "list_workload_intercepts called for session {} environment {}",
            self.session_id,
            environment_id
        );

        let session = self
            .state
            .db
            .get_active_devbox_session(self.user_id)
            .await
            .map_err(|e| format!("Failed to fetch session: {}", e))?
            .ok_or_else(|| "Session not found".to_string())?;

        let intercepts = self
            .state
            .db
            .get_active_intercepts_for_environment(environment_id)
            .await
            .map_err(|e| format!("Failed to fetch intercepts: {}", e))?;

        let mut result = Vec::new();
        for intercept in intercepts {
            let workload = self
                .state
                .db
                .get_environment_workload(intercept.workload_id)
                .await
                .map_err(|e| format!("Failed to fetch workload: {}", e))?
                .ok_or_else(|| "Workload not found".to_string())?;

            let port_mappings: Vec<lapdev_devbox_rpc::PortMapping> =
                serde_json::from_value(serde_json::Value::from(intercept.port_mappings.clone()))
                    .map_err(|e| format!("Failed to parse port mappings: {}", e))?;

            result.push(lapdev_devbox_rpc::WorkloadInterceptInfo {
                intercept_id: intercept.id,
                workload_id: workload.id,
                workload_name: workload.name,
                namespace: workload.namespace,
                port_mappings,
                created_at: intercept.created_at.with_timezone(&chrono::Utc),
                device_name: session.device_name.clone(),
            });
        }

        Ok(result)
    }

    async fn list_services(
        self,
        _context: tarpc::context::Context,
        environment_id: Uuid,
    ) -> Result<Vec<lapdev_devbox_rpc::ServiceInfo>, String> {
        tracing::info!(
            "list_services called for session {} environment {}",
            self.session_id,
            environment_id
        );

        // Get environment and verify user has access
        let environment = self
            .state
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(|e| format!("Failed to fetch environment: {}", e))?
            .ok_or_else(|| "Environment not found".to_string())?;

        // Verify user has access to this environment
        if environment.user_id != self.user_id {
            return Err("Unauthorized: You don't have access to this environment".to_string());
        }

        // Get all services for the environment
        let services = self
            .state
            .db
            .get_environment_services(environment_id)
            .await
            .map_err(|e| format!("Failed to fetch services: {}", e))?;

        let mut result = Vec::new();
        for service in services {
            // Convert KubeServicePort to ServicePort
            let ports: Vec<lapdev_devbox_rpc::ServicePort> = service
                .ports
                .iter()
                .map(|p| lapdev_devbox_rpc::ServicePort {
                    name: p.name.clone(),
                    port: p.port as u16,
                    protocol: p.protocol.clone().unwrap_or_else(|| "TCP".to_string()),
                })
                .collect();

            result.push(lapdev_devbox_rpc::ServiceInfo {
                name: service.name,
                namespace: service.namespace,
                ports,
            });
        }

        Ok(result)
    }

    async fn request_direct_client_config(
        self,
        _context: tarpc::context::Context,
        environment_id: Uuid,
    ) -> Result<Option<DirectChannelConfig>, String> {
        let environment = self
            .state
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(|e| format!("Failed to fetch environment: {}", e))?
            .ok_or_else(|| "Environment not found".to_string())?;

        if environment.user_id != self.user_id {
            return Err("Unauthorized".to_string());
        }

        match self
            .state
            .kube_controller
            .request_devbox_direct_config(
                environment.cluster_id,
                self.user_id,
                environment.id,
                environment.namespace.clone(),
            )
            .await
        {
            Ok(config) => Ok(config),
            Err(err) => {
                tracing::warn!(
                    environment_id = %environment_id,
                    user_id = %self.user_id,
                    error = %err,
                    "Failed to fetch direct devbox config; falling back to relay"
                );
                Ok(None)
            }
        }
    }
}

/// Implementation for DevboxInterceptRpc - allows dashboard/CLI to control intercepts
pub struct DevboxInterceptRpcImpl {
    state: Arc<CoreState>,
    user_id: Uuid,
    _organization_id: Uuid,
}

impl DevboxInterceptRpcImpl {
    pub fn new(state: Arc<CoreState>, user_id: Uuid, organization_id: Uuid) -> Self {
        Self {
            state,
            user_id,
            _organization_id: organization_id,
        }
    }
}

impl DevboxInterceptRpc for DevboxInterceptRpcImpl {
    async fn start_workload_intercept(
        self,
        _context: tarpc::context::Context,
        workload_id: Uuid,
        port_mappings: Vec<lapdev_devbox_rpc::PortMappingOverride>,
    ) -> Result<Uuid, String> {
        tracing::info!(
            "start_workload_intercept called for user {} workload {}",
            self.user_id,
            workload_id
        );

        // Get workload to find environment and validate access
        let workload = self
            .state
            .db
            .get_environment_workload(workload_id)
            .await
            .map_err(|e| format!("Failed to fetch workload: {}", e))?
            .ok_or_else(|| "Workload not found".to_string())?;

        // Get environment to validate user has access
        let environment = self
            .state
            .db
            .get_kube_environment(workload.environment_id)
            .await
            .map_err(|e| format!("Failed to fetch environment: {}", e))?
            .ok_or_else(|| "Environment not found".to_string())?;

        // Verify user has access to this environment
        if environment.user_id != self.user_id {
            return Err("Unauthorized: You don't have access to this environment".to_string());
        }

        // Convert port mapping overrides to actual port mappings
        let mut actual_port_mappings = Vec::new();
        for override_mapping in &port_mappings {
            let local_port = override_mapping
                .local_port
                .unwrap_or(override_mapping.workload_port);
            actual_port_mappings.push(PortMapping {
                workload_port: override_mapping.workload_port,
                local_port,
                protocol: "TCP".to_string(),
            });
        }

        // Create intercept in database
        let intercept = self
            .state
            .db
            .create_workload_intercept(
                self.user_id,
                environment.id,
                workload_id,
                serde_json::to_value(&actual_port_mappings)
                    .map_err(|e| format!("Failed to serialize port mappings: {}", e))?,
            )
            .await
            .map_err(|e| format!("Failed to create intercept: {}", e))?;

        if self
            .state
            .active_devbox_sessions
            .read()
            .await
            .get(&self.user_id)
            .is_some()
        {
            let state = self.state.clone();
            let environment = environment.clone();
            let intercept_clone = intercept.clone();
            tokio::spawn(async move {
                state
                    .push_devbox_route_for_intercept(environment, intercept_clone)
                    .await;
            });
        }

        Ok(intercept.id)
    }

    async fn stop_workload_intercept(
        self,
        _context: tarpc::context::Context,
        intercept_id: Uuid,
    ) -> Result<(), String> {
        tracing::info!(
            "stop_workload_intercept called for user {} intercept {}",
            self.user_id,
            intercept_id
        );

        // Get intercept to validate ownership
        let intercept = self
            .state
            .db
            .get_workload_intercept(intercept_id)
            .await
            .map_err(|e| format!("Failed to fetch intercept: {}", e))?
            .ok_or_else(|| "Intercept not found".to_string())?;

        // Verify intercept belongs to this user
        if intercept.user_id != self.user_id {
            return Err("Unauthorized: This intercept doesn't belong to you".to_string());
        }

        // Stop the intercept in database
        self.state
            .db
            .stop_workload_intercept(intercept_id)
            .await
            .map_err(|e| format!("Failed to stop intercept: {}", e))?;

        if self
            .state
            .active_devbox_sessions
            .read()
            .await
            .get(&self.user_id)
            .is_some()
        {
            match self
                .state
                .db
                .get_kube_environment(intercept.environment_id)
                .await
            {
                Ok(Some(environment)) => {
                    let state = self.state.clone();
                    let intercept_clone = intercept.clone();
                    tokio::spawn(async move {
                        state
                            .clear_devbox_route_for_intercept(environment, intercept_clone)
                            .await;
                    });
                }
                Ok(None) => {
                    tracing::warn!(
                        environment_id = %intercept.environment_id,
                        intercept_id = %intercept.id,
                        "Environment not found while clearing devbox route after intercept stop"
                    );
                }
                Err(err) => {
                    tracing::warn!(
                        environment_id = %intercept.environment_id,
                        intercept_id = %intercept.id,
                        error = %err,
                        "Failed to load environment while clearing devbox route after intercept stop"
                    );
                }
            }
        }

        Ok(())
    }
}
