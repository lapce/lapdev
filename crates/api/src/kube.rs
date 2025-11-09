use std::sync::Arc;

use axum::{
    extract::{ws::WebSocket, Path, State, WebSocketUpgrade},
    http::HeaderMap,
    response::Response,
};
use futures::StreamExt;
use lapdev_common::{
    kube::{KUBE_CLUSTER_TOKEN_HEADER, KUBE_ENVIRONMENT_TOKEN_HEADER},
    token::HashedToken,
};
use lapdev_kube::server::KubeClusterServer;
use lapdev_kube_rpc::{KubeClusterRpc, KubeManagerRpcClient};
use lapdev_rpc::{error::ApiError, spawn_twoway};
use lapdev_tunnel::{
    direct::QuicTransport, relay_client_addr, relay_server_addr, run_tunnel_server_with_connector,
    DynTunnelStream, TunnelClient, TunnelError, TunnelMode, TunnelTarget, WebSocketUdpSocket,
};
use secrecy::ExposeSecret;
use tarpc::{
    server::{BaseChannel, Channel},
    tokio_util::codec::LengthDelimitedCodec,
};
use uuid::Uuid;

use crate::{
    devbox_tunnels::split_axum_websocket, state::CoreState, websocket_transport::WebSocketTransport,
};

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
        state.kube_controller.kube_cluster_server_generation.clone(),
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
    let raw_token = headers
        .get(KUBE_CLUSTER_TOKEN_HEADER)
        .ok_or(ApiError::Unauthenticated)?
        .to_str()?
        .to_owned();
    let token = HashedToken::parse(&raw_token);
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
    Ok(handle_data_plane_websocket(
        websocket, state, cluster.id, raw_token,
    ))
}

fn handle_data_plane_websocket(
    websocket: WebSocketUpgrade,
    state: Arc<CoreState>,
    cluster_id: Uuid,
    token: String,
) -> Response {
    websocket
        .on_failed_upgrade(|e| tracing::error!("data plane websocket upgrade failed {e:?}"))
        .on_upgrade(move |socket| {
            let token = token.clone();
            async move {
                handle_data_plane_tunnel(socket, state, cluster_id, token).await;
            }
        })
}

async fn handle_data_plane_tunnel(
    socket: WebSocket,
    state: Arc<CoreState>,
    cluster_id: Uuid,
    token: String,
) {
    tracing::info!("Data plane tunnel established for cluster: {}", cluster_id);

    let (sink, stream) = split_axum_websocket(socket);
    let udp_socket =
        WebSocketUdpSocket::from_parts(sink, stream, relay_server_addr(), relay_client_addr());

    let transport = match QuicTransport::accept_udp_server(udp_socket, &token).await {
        Ok(transport) => transport,
        Err(err) => {
            tracing::warn!(
                cluster_id = %cluster_id,
                error = %err,
                "Failed to negotiate QUIC transport for data plane tunnel"
            );
            return;
        }
    };

    let tunnel_client = Arc::new(TunnelClient::connect_with_mode(
        transport,
        TunnelMode::Relay,
    ));

    let generation = state
        .kube_controller
        .tunnel_registry
        .register_client(cluster_id, Arc::clone(&tunnel_client))
        .await;

    tracing::info!(
        "Tunnel client registered for cluster {}; waiting for disconnect",
        cluster_id
    );

    tunnel_client.closed().await;

    tracing::info!("Data plane tunnel closed for cluster: {}", cluster_id);
    state
        .kube_controller
        .tunnel_registry
        .remove_client(cluster_id, generation)
        .await;
}

pub async fn sidecar_tunnel_websocket(
    Path((environment_id, workload_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
    State(state): State<Arc<CoreState>>,
) -> Result<Response, ApiError> {
    tracing::debug!(
        "Handling sidecar tunnel WebSocket for environment {} workload {}",
        environment_id,
        workload_id
    );

    // Get the environment auth token from headers
    let auth_token = extract_environment_token(&headers)?;

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
        "Sidecar authenticated for environment {} workload {} using auth token",
        environment.id,
        workload_id
    );

    let user_id = environment.user_id;
    let registry = state.devbox_tunnels.clone();
    let token_string = environment.auth_token.clone();

    Ok(websocket.on_upgrade(move |socket| {
        let registry = registry.clone();
        let token = token_string.clone();
        async move {
            if let Err(err) = registry.attach_sidecar(user_id, token, socket).await {
                tracing::warn!(
                    user_id = %user_id,
                    environment_id = %environment_id,
                    workload_id = %workload_id,
                    error = %err,
                    "Sidecar tunnel terminated with error"
                );
            }
        }
    }))
}

fn extract_environment_token<'a>(headers: &'a HeaderMap) -> Result<&'a str, ApiError> {
    headers
        .get(KUBE_ENVIRONMENT_TOKEN_HEADER)
        .ok_or(ApiError::Unauthenticated)?
        .to_str()
        .map_err(|_| ApiError::Unauthenticated)
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

    let cluster_id = environment.cluster_id;
    let auth_token = environment.auth_token.clone();
    let tunnel_registry = state.kube_controller.tunnel_registry.clone();

    Ok(websocket.on_upgrade(move |socket| {
        let registry = tunnel_registry.clone();
        let token = auth_token.clone();
        async move {
            match registry.get_client(cluster_id).await {
                Some(client) if !client.is_closed() => {
                    serve_devbox_proxy_tunnel(socket, client, token).await;
                }
                _ => {
                    tracing::warn!(
                        "No active cluster tunnel available for devbox proxy environment {} (cluster {})",
                        environment_id,
                        cluster_id
                    );
                }
            }
        }
    }))
}

async fn serve_devbox_proxy_tunnel(
    socket: WebSocket,
    cluster_client: Arc<TunnelClient>,
    token: String,
) {
    let (sink, stream) = split_axum_websocket(socket);
    let udp_socket =
        WebSocketUdpSocket::from_parts(sink, stream, relay_server_addr(), relay_client_addr());

    let transport = match QuicTransport::accept_udp_server(udp_socket, &token).await {
        Ok(transport) => transport,
        Err(err) => {
            tracing::warn!("Failed to negotiate QUIC relay for devbox proxy: {}", err);
            return;
        }
    };

    let connector_client = cluster_client.clone();

    let connector = move |target: TunnelTarget| {
        let client = connector_client.clone();
        async move {
            let TunnelTarget { host, port } = target;
            let stream = client.connect_tcp(host, port).await?;
            Ok::<DynTunnelStream, TunnelError>(Box::new(stream) as DynTunnelStream)
        }
    };

    if let Err(err) = run_tunnel_server_with_connector(transport, connector).await {
        tracing::warn!("Devbox proxy tunnel terminated with error: {}", err);
    }
}
