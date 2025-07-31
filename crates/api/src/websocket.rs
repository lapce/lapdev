use std::sync::Arc;

use axum::{
    extract::{
        ws::{Message, WebSocket},
        WebSocketUpgrade,
    },
    response::{IntoResponse, Response},
};
use axum_extra::headers;
use futures_util::StreamExt;
use hyper::StatusCode;
use lapdev_rpc::error::ApiError;
use uuid::Uuid;

use crate::state::CoreState;

pub async fn handle_websocket(
    path: &str,
    query: Option<&str>,
    websocket: WebSocketUpgrade,
    cookies: headers::Cookie,
    state: Arc<CoreState>,
) -> Result<Response, ApiError> {
    if path == "/ws" {
        let ws_name = query
            .ok_or(ApiError::InvalidRequest("no workspace name".to_string()))?
            .split('=')
            .last()
            .ok_or_else(|| ApiError::InvalidRequest("no workspace name".to_string()))?;

        let user = state.authenticate(&cookies).await?;
        let ws = state
            .db
            .get_workspace_by_name(ws_name)
            .await
            .map_err(|_| ApiError::InvalidRequest("invalid workspace name".to_string()))?;
        if ws.user_id != user.id {
            return Err(ApiError::Unauthorized);
        }

        let resp = websocket
            .on_upgrade(move |mut socket| async move {
                handle_workspace_updates(ws.id, &mut socket, &state).await;
                state.conductor.cleanup_workspace_updates(ws.id).await;
            })
            .into_response();
        return Ok(resp);
    } else if path == "/all_workspaces_ws" {
        let user = state.authenticate(&cookies).await?;
        let resp = websocket
            .on_upgrade(move |mut socket| async move {
                handle_all_workspaces_update(&user, &mut socket, &state).await;
                state.conductor.cleanup_all_workspace_updates(user.id).await;
            })
            .into_response();
        return Ok(resp);
    }

    Ok(StatusCode::NOT_FOUND.into_response())
}

async fn handle_workspace_updates(ws_id: Uuid, socket: &mut WebSocket, state: &CoreState) {
    let mut rx = state.conductor.workspace_updates(ws_id).await;
    loop {
        tokio::select! {
            incoming = socket.recv() => {
                if incoming.is_none() {
                    println!("websocket closed by client");
                    return;
                }
            }
            msg = rx.next() => {
                if let Some(msg) = msg {
                    if let Ok(c) = serde_json::to_string(&msg) {
                        let _ = socket.send(Message::Text(c.into())).await;
                    }
                } else {
                    println!("workspace update messages stopped");
                    return;
                }
            }
        }
    }
}

async fn handle_all_workspaces_update(
    user: &lapdev_db_entities::user::Model,
    socket: &mut WebSocket,
    state: &CoreState,
) {
    let mut rx = state.conductor.all_workspace_updates(user.id).await;
    loop {
        tokio::select! {
            incoming = socket.recv() => {
                if incoming.is_none() {
                    println!("websocket closed by client");
                    return;
                }
            }
            msg = rx.next() => {
                if let Some(msg) = msg {
                    if let Ok(c) = serde_json::to_string(&msg) {
                        let _ = socket.send(Message::Text(c.into())).await;
                    }
                } else {
                    println!("workspace update messages stopped");
                    return;
                }
            }
        }
    }
}
