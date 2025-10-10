use std::sync::Arc;

use axum::{
    extract::{ws::WebSocket, Path, State, WebSocketUpgrade},
    http::HeaderMap,
    response::Response,
    Json,
};
use axum_extra::{headers, TypedHeader};
use futures::StreamExt;
use lapdev_devbox_rpc::{
    DevboxClientRpcClient, DevboxInterceptRpc, DevboxSessionInfo, DevboxSessionRpc, PortMapping,
    PortMappingOverride, StartInterceptRequest,
};
use lapdev_rpc::{error::ApiError, spawn_twoway};
use serde::{Deserialize, Serialize};
use tarpc::{
    server::{BaseChannel, Channel},
    tokio_util::codec::LengthDelimitedCodec,
};
use uuid::Uuid;

use crate::{state::CoreState, websocket_transport::WebSocketTransport};

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

    // Check for existing active session and notify it before displacing
    if let Some(old_handle) = state.active_devbox_sessions.read().await.get(&ctx.user.id) {
        tracing::info!(
            "Displacing existing session {} on device {} for user {}",
            old_handle.session_id,
            old_handle.device_name,
            ctx.user.id
        );

        // Notify old session that it's being displaced
        let _ = old_handle
            .notify_tx
            .send(crate::state::DevboxSessionNotification::Displaced {
                new_device_name: ctx.device_name.clone(),
            });
    }

    // Create or update devbox session
    let session = state
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
        "Created devbox session {} for user {} on device {}",
        session.id,
        ctx.user.id,
        ctx.device_name
    );

    Ok(handle_devbox_rpc_upgrade(
        websocket,
        state,
        session.id,
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
    let trans = WebSocketTransport::new(socket);
    let io = LengthDelimitedCodec::builder().new_framed(trans);
    let transport =
        tarpc::serde_transport::new(io, tarpc::tokio_serde::formats::Bincode::default());
    let (server_chan, client_chan, _abort_handle) = spawn_twoway(transport);

    // Create RPC client (for calling CLI methods)
    let rpc_client =
        DevboxClientRpcClient::new(tarpc::client::Config::default(), client_chan).spawn();

    // Create notification channel for session migration
    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel();

    // Register this session as active
    state.active_devbox_sessions.write().await.insert(
        user_id,
        crate::state::DevboxSessionHandle {
            session_id,
            device_name: device_name.clone(),
            notify_tx,
            rpc_client: rpc_client.clone(),
        },
    );

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
    state.active_devbox_sessions.write().await.remove(&user_id);
    notification_task.abort();

    // Mark session as revoked in database
    if let Err(e) = state.db.revoke_devbox_session(session_id).await {
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

/// RPC server implementation for DevboxSessionRpc
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

        // Fetch user info
        let user = self
            .state
            .db
            .get_user(self.user_id)
            .await
            .map_err(|e| format!("Failed to fetch user: {}", e))?
            .ok_or_else(|| "User not found".to_string())?;

        // Fetch session info
        let session = self
            .state
            .db
            .get_devbox_session(self.session_id)
            .await
            .map_err(|e| format!("Failed to fetch session: {}", e))?
            .ok_or_else(|| "Session not found".to_string())?;

        Ok(DevboxSessionInfo {
            session_id: session.id,
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

        // Fetch session to get active environment
        let session = self
            .state
            .db
            .get_devbox_session(self.session_id)
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

        // Update session's active environment
        self.state
            .db
            .update_devbox_session_active_environment(self.session_id, Some(environment_id))
            .await
            .map_err(|e| format!("Failed to update active environment: {}", e))?;

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

        // Get active intercepts for this environment
        let intercepts = self
            .state
            .db
            .get_active_intercepts_for_environment(environment_id)
            .await
            .map_err(|e| format!("Failed to fetch intercepts: {}", e))?;

        // Build response from intercepts
        let mut result = Vec::new();
        for intercept in intercepts {
            // Get workload info
            let workload = self
                .state
                .db
                .get_environment_workload(intercept.workload_id)
                .await
                .map_err(|e| format!("Failed to fetch workload: {}", e))?
                .ok_or_else(|| "Workload not found".to_string())?;

            // Parse port mappings from JSON
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
                device_name: String::new(), // Intercepts are no longer tied to specific sessions/devices
            });
        }

        Ok(result)
    }
}

/// Implementation for DevboxInterceptRpc - allows dashboard/CLI to control intercepts
pub struct DevboxInterceptRpcImpl {
    state: Arc<CoreState>,
    user_id: Uuid,
    organization_id: Uuid,
}

impl DevboxInterceptRpcImpl {
    pub fn new(state: Arc<CoreState>, user_id: Uuid, organization_id: Uuid) -> Self {
        Self {
            state,
            user_id,
            organization_id,
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

        // Get the active session handle to send RPC to CLI
        let rpc_client_opt = {
            let sessions = self.state.active_devbox_sessions.read().await;
            sessions
                .get(&self.user_id)
                .map(|handle| (handle.session_id, handle.rpc_client.clone()))
        };

        if let Some((session_id, rpc_client)) = rpc_client_opt {
            // Send start_intercept RPC to CLI
            tracing::info!(
                "Notifying CLI session {} to start intercept {}",
                session_id,
                intercept.id
            );

            let start_request = StartInterceptRequest {
                intercept_id: intercept.id,
                workload_id,
                workload_name: workload.name.clone(),
                namespace: workload.namespace.clone(),
                port_mappings: actual_port_mappings,
            };

            // Send RPC to CLI (fire and forget)
            tokio::spawn(async move {
                if let Err(e) = rpc_client
                    .start_intercept(tarpc::context::current(), start_request)
                    .await
                {
                    tracing::error!("Failed to notify CLI to start intercept: {}", e);
                }
            });
        } else {
            tracing::warn!(
                "No active WebSocket connection for user {}, intercept {} created but CLI not notified",
                self.user_id,
                intercept.id
            );
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

        // Notify CLI if session is still active
        let rpc_client_opt = {
            let sessions = self.state.active_devbox_sessions.read().await;
            sessions
                .get(&self.user_id)
                .map(|handle| (handle.session_id, handle.rpc_client.clone()))
        };

        if let Some((session_id, rpc_client)) = rpc_client_opt {
            // Send stop_intercept RPC to CLI
            tracing::info!(
                "Notifying CLI session {} to stop intercept {}",
                session_id,
                intercept_id
            );

            tokio::spawn(async move {
                if let Err(e) = rpc_client
                    .stop_intercept(tarpc::context::current(), intercept_id)
                    .await
                {
                    tracing::error!("Failed to notify CLI to stop intercept: {}", e);
                }
            });
        }

        Ok(())
    }
}
