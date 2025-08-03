use std::sync::Arc;

use axum::{
    extract::{ws::WebSocket, State, WebSocketUpgrade},
    http::HeaderMap,
    response::Response,
};
use futures::StreamExt;
use lapdev_common::{kube::KUBE_CLUSTER_TOKEN_HEADER, token::HashedToken};
use lapdev_kube::server::KubeClusterServer;
use lapdev_kube_rpc::{KubeClusterRpc, KubeManagerRpcClient};
use lapdev_rpc::{error::ApiError, spawn_twoway};
use secrecy::ExposeSecret;
use tarpc::{
    server::{BaseChannel, Channel},
    tokio_util::codec::LengthDelimitedCodec,
};
use uuid::Uuid;

use crate::{state::CoreState, websocket_transport::WebSocketTransport};

pub async fn kube_cluster_websocket(
    websocket: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<Arc<CoreState>>,
) -> Result<Response, ApiError> {
    println!("now handle kube_cluster_websocket");
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

    println!("now handle cluster websocket");
    Ok(handle_cluster_websocket(websocket, state, cluster.id))
}

fn handle_cluster_websocket(
    websocket: WebSocketUpgrade,
    state: Arc<CoreState>,
    cluster_id: Uuid,
) -> Response {
    websocket
        .on_failed_upgrade(|e| println!("websocket upgrade failed {e:?}"))
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
