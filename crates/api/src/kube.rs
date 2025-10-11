use std::sync::Arc;

use axum::{
    extract::{ws::WebSocket, Path, State, WebSocketUpgrade},
    http::HeaderMap,
    response::Response,
};
use futures::{SinkExt, StreamExt};
use lapdev_common::{kube::KUBE_CLUSTER_TOKEN_HEADER, token::HashedToken};
use lapdev_kube::server::KubeClusterServer;
use lapdev_kube_rpc::{
    ClientTunnelFrame, ClientTunnelMessage, KubeClusterRpc, KubeManagerRpcClient,
    ServerTunnelFrame, ServerTunnelMessage,
};
use lapdev_rpc::{error::ApiError, spawn_twoway};
use secrecy::ExposeSecret;
use tarpc::{
    server::{BaseChannel, Channel},
    tokio_util::codec::LengthDelimitedCodec,
};
use uuid::Uuid;

use crate::{state::CoreState, websocket_transport::WebSocketTransport};

pub async fn kube_cluster_rpc_websocket(
    websocket: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<Arc<CoreState>>,
) -> Result<Response, ApiError> {
    tracing::debug!("now handle kube_cluster_websocket");
    let token = headers
        .get(KUBE_CLUSTER_TOKEN_HEADER)
        .ok_or(ApiError::Unauthenticated)?
        .to_str()?;
    let token = HashedToken::parse(token);
    let token = state
        .db
        .get_kube_token(token.expose_secret())
        .await?
        .ok_or(ApiError::Unauthenticated)?;

    state.db.update_kube_token_last_used(token.id).await?;
    let cluster = state
        .db
        .get_kube_cluster(token.cluster_id)
        .await?
        .ok_or_else(|| {
            ApiError::InternalError(format!("Cluster {} doesn't exist", token.cluster_id))
        })?;

    tracing::debug!("now handle cluster websocket");
    Ok(handle_cluster_websocket(websocket, state, cluster.id))
}

fn handle_cluster_websocket(
    websocket: WebSocketUpgrade,
    state: Arc<CoreState>,
    cluster_id: Uuid,
) -> Response {
    websocket
        .on_failed_upgrade(|e| tracing::error!("websocket upgrade failed {e:?}"))
        .on_upgrade(move |socket| async move {
            handle_cluster_rpc(socket, state, cluster_id).await;
        })
}

async fn handle_cluster_rpc(socket: WebSocket, state: Arc<CoreState>, cluster_id: Uuid) {
    let trans = WebSocketTransport::new(socket);
    let io = LengthDelimitedCodec::builder().new_framed(trans);
    let transport =
        tarpc::serde_transport::new(io, tarpc::tokio_serde::formats::Bincode::default());
    let (server_chan, client_chan, _) = spawn_twoway(transport);
    let rpc_client =
        KubeManagerRpcClient::new(tarpc::client::Config::default(), client_chan).spawn();
    let rpc_server = KubeClusterServer::new(
        cluster_id,
        rpc_client,
        state.db.clone(),
        state.kube_controller.kube_cluster_servers.clone(),
        state.kube_controller.tunnel_registry.clone(),
    );

    let fut = {
        let rpc_server = rpc_server.clone();
        tokio::spawn(async move {
            BaseChannel::with_defaults(server_chan)
                .execute(rpc_server.serve())
                .for_each(|resp| async move {
                    tokio::spawn(resp);
                })
                .await;
        })
    };

    rpc_server.register().await;
    let _ = fut.await;
    rpc_server.unregister().await;
}

pub async fn kube_data_plane_websocket(
    websocket: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<Arc<CoreState>>,
) -> Result<Response, ApiError> {
    tracing::debug!("Handling data plane WebSocket connection");
    let token = headers
        .get(KUBE_CLUSTER_TOKEN_HEADER)
        .ok_or(ApiError::Unauthenticated)?
        .to_str()?;
    let token = HashedToken::parse(token);
    let token = state
        .db
        .get_kube_token(token.expose_secret())
        .await?
        .ok_or(ApiError::Unauthenticated)?;

    state.db.update_kube_token_last_used(token.id).await?;
    let cluster = state
        .db
        .get_kube_cluster(token.cluster_id)
        .await?
        .ok_or_else(|| {
            ApiError::InternalError(format!("Cluster {} doesn't exist", token.cluster_id))
        })?;

    tracing::debug!("Handling data plane WebSocket for cluster: {}", cluster.id);
    Ok(handle_data_plane_websocket(websocket, state, cluster.id))
}

fn handle_data_plane_websocket(
    websocket: WebSocketUpgrade,
    state: Arc<CoreState>,
    cluster_id: Uuid,
) -> Response {
    websocket
        .on_failed_upgrade(|e| tracing::error!("data plane websocket upgrade failed {e:?}"))
        .on_upgrade(move |socket| async move {
            handle_data_plane_tunnel(socket, state, cluster_id).await;
        })
}

async fn handle_data_plane_tunnel(socket: WebSocket, state: Arc<CoreState>, cluster_id: Uuid) {
    tracing::info!("Data plane tunnel established for cluster: {}", cluster_id);

    let (ws_sender, mut ws_receiver) = socket.split();
    let tunnel_registry = state.kube_controller.tunnel_registry.clone();

    // Create channels for communication
    let (outgoing_tx, mut outgoing_rx) =
        tokio::sync::mpsc::unbounded_channel::<ServerTunnelMessage>();

    // Register the sender with the tunnel registry
    tunnel_registry
        .register_tunnel_sender(cluster_id, outgoing_tx)
        .await;

    // Task to handle outgoing messages (both external messages and responses)
    let outgoing_task = {
        tokio::spawn(async move {
            let mut ws_sender = ws_sender;
            loop {
                match outgoing_rx.recv().await {
                    Some(message) => {
                        let frame = ServerTunnelFrame {
                            message,
                            timestamp: chrono::Utc::now(),
                            message_id: Uuid::new_v4(),
                        };

                        match frame.serialize() {
                            Ok(data) => {
                                if let Err(e) = ws_sender
                                    .send(axum::extract::ws::Message::Binary(data.into()))
                                    .await
                                {
                                    tracing::error!("Failed to send server message: {}", e);
                                    break;
                                }
                            }
                            Err(e) => {
                                tracing::error!("Failed to serialize server message: {}", e);
                            }
                        }
                    }
                    None => {
                        break; // Channel closed
                    }
                }
            }
            tracing::debug!("Outgoing message task ended for cluster: {}", cluster_id);
        })
    };

    // Handle incoming tunnel messages from KubeManager
    let incoming_task = {
        let tunnel_registry = tunnel_registry.clone();
        tokio::spawn(async move {
            while let Some(msg) = ws_receiver.next().await {
                match msg {
                    Ok(axum::extract::ws::Message::Binary(data)) => {
                        // Try deserializing as ClientTunnelFrame (messages FROM KubeManager)
                        match ClientTunnelFrame::deserialize(&data) {
                            Ok(frame) => {
                                tracing::debug!("Received client message: {:?}", frame.message);
                                let message = frame.message;
                                let message_for_logs = message.clone();
                                tunnel_registry.handle_client_message(message).await;

                                // Handle client messages from KubeManager
                                match message_for_logs {
                                    ClientTunnelMessage::ConnectionOpened {
                                        tunnel_id,
                                        local_addr,
                                    } => {
                                        tracing::info!(
                                            "Data plane: Connection opened for tunnel {} at {}",
                                            tunnel_id,
                                            local_addr
                                        );
                                    }
                                    ClientTunnelMessage::ConnectionFailed {
                                        tunnel_id,
                                        error,
                                        error_code,
                                    } => {
                                        tracing::warn!("Data plane: Connection failed for tunnel {} - {} ({:?})", 
                                                     tunnel_id, error, error_code);
                                    }
                                    ClientTunnelMessage::ConnectionClosed {
                                        tunnel_id,
                                        bytes_transferred,
                                    } => {
                                        tracing::info!("Data plane: Connection closed for tunnel {}, {} bytes transferred", 
                                                     tunnel_id, bytes_transferred);
                                    }
                                    ClientTunnelMessage::Data {
                                        tunnel_id,
                                        payload,
                                        sequence_num: _,
                                    } => {
                                        tracing::debug!(
                                            "Data plane: Received {} bytes from tunnel {}",
                                            payload.len(),
                                            tunnel_id
                                        );
                                        // Forward data from KubeManager to client
                                    }
                                    ClientTunnelMessage::Pong { timestamp } => {
                                        tracing::debug!(
                                            "Data plane: Received pong with timestamp {}",
                                            timestamp
                                        );
                                    }
                                    ClientTunnelMessage::TunnelStats {
                                        active_connections,
                                        total_connections,
                                        bytes_sent,
                                        bytes_received,
                                        connection_errors,
                                    } => {
                                        tracing::debug!("Data plane: Tunnel stats - active: {}, total: {}, sent: {}, received: {}, errors: {}", 
                                                      active_connections, total_connections, bytes_sent, bytes_received, connection_errors);
                                    }
                                    ClientTunnelMessage::CloseConnection { tunnel_id, reason } => {
                                        tracing::info!("Data plane: KubeManager requested connection close for tunnel {} ({:?})", 
                                                     tunnel_id, reason);
                                        // Handle connection close request from KubeManager
                                    }
                                    ClientTunnelMessage::Authenticate {
                                        cluster_id,
                                        auth_token: _auth_token,
                                        tunnel_capabilities,
                                    } => {
                                        tracing::info!("Data plane: Authentication request from cluster {} with capabilities: {:?}", 
                                                     cluster_id, tunnel_capabilities);
                                        // Handle authentication
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::error!("Failed to deserialize client tunnel frame: {}", e);
                            }
                        }
                    }
                    Ok(axum::extract::ws::Message::Close(_)) => {
                        tracing::info!("Data plane WebSocket closed for cluster: {}", cluster_id);
                        break;
                    }
                    Err(e) => {
                        tracing::error!("Data plane WebSocket error: {}", e);
                        break;
                    }
                    _ => {
                        // Ignore other message types (text, ping, pong)
                    }
                }
            }
            tracing::debug!(
                "Incoming message handling ended for cluster: {}",
                cluster_id
            );
        })
    };

    // Wait for either task to complete
    tokio::select! {
        _ = incoming_task => {
            tracing::debug!("Incoming message handling ended for cluster: {}", cluster_id);
        }
        _ = outgoing_task => {
            tracing::debug!("Outgoing message handling ended for cluster: {}", cluster_id);
        }
    }

    // Clean up the tunnel registry
    tunnel_registry.remove_tunnel(cluster_id).await;
    tracing::info!("Data plane tunnel closed for cluster: {}", cluster_id);
}

pub async fn sidecar_tunnel_websocket(
    Path((environment_id, session_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
    State(state): State<Arc<CoreState>>,
) -> Result<Response, ApiError> {
    tracing::debug!(
        "Handling sidecar tunnel WebSocket for environment {} session {}",
        environment_id,
        session_id
    );

    // Get the environment auth token from headers
    const KUBE_ENVIRONMENT_TOKEN_HEADER: &str = "X-Lapdev-Environment-Token";
    let auth_token = headers
        .get(KUBE_ENVIRONMENT_TOKEN_HEADER)
        .ok_or(ApiError::Unauthenticated)?
        .to_str()
        .map_err(|_| ApiError::Unauthenticated)?;

    // Get the environment and validate auth token
    let environment = state
        .db
        .get_kube_environment(environment_id)
        .await?
        .ok_or(ApiError::Unauthenticated)?;

    // Validate the auth token matches
    if auth_token != environment.auth_token {
        return Err(ApiError::Unauthenticated);
    }

    tracing::debug!(
        "Sidecar authenticated for environment {} using auth token",
        environment.id
    );

    let broker = state.tunnel_broker.clone();

    Ok(websocket.on_upgrade(move |socket| async move {
        broker.register_sidecar(session_id, socket).await;
    }))
}

pub async fn devbox_proxy_tunnel_websocket(
    Path(environment_id): Path<Uuid>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
    State(state): State<Arc<CoreState>>,
) -> Result<Response, ApiError> {
    tracing::debug!(
        "Handling devbox proxy tunnel WebSocket for environment {}",
        environment_id
    );

    // Get the environment auth token from headers
    const KUBE_ENVIRONMENT_TOKEN_HEADER: &str = "X-Lapdev-Environment-Token";
    let auth_token = headers
        .get(KUBE_ENVIRONMENT_TOKEN_HEADER)
        .ok_or(ApiError::Unauthenticated)?
        .to_str()
        .map_err(|_| ApiError::Unauthenticated)?;

    // Get the environment and validate auth token
    let environment = state
        .db
        .get_kube_environment(environment_id)
        .await?
        .ok_or(ApiError::Unauthenticated)?;

    // Validate the auth token matches
    if auth_token != environment.auth_token {
        return Err(ApiError::Unauthenticated);
    }

    tracing::info!(
        "Devbox proxy authenticated for environment {} using auth token",
        environment.id
    );

    // TODO: Store the devbox-proxy WebSocket connection for this environment
    // This will be used to forward connections from devboxes to in-cluster services
    // For now, just keep the connection alive
    Ok(websocket.on_upgrade(move |socket| async move {
        tracing::info!("Devbox proxy WebSocket connection established for environment {}", environment_id);

        // Keep the connection alive by reading messages
        use futures::StreamExt;
        let (mut _sender, mut receiver) = socket.split();

        while let Some(msg) = receiver.next().await {
            match msg {
                Ok(axum::extract::ws::Message::Close(_)) => {
                    tracing::info!("Devbox proxy closed connection for environment {}", environment_id);
                    break;
                }
                Err(e) => {
                    tracing::error!("Devbox proxy WebSocket error for environment {}: {}", environment_id, e);
                    break;
                }
                _ => {}
            }
        }

        tracing::info!("Devbox proxy WebSocket connection closed for environment {}", environment_id);
    }))
}
