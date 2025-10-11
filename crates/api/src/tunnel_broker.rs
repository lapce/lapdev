use std::{collections::HashMap, sync::Arc};

use axum::extract::ws::WebSocket;
use tokio::{
    io,
    sync::{oneshot, Mutex},
};
use tracing::{info, warn};
use uuid::Uuid;

use crate::websocket_transport::WebSocketTransport;

#[derive(Default)]
struct WaitingEntry {
    devbox: Option<PendingEndpoint>,
    sidecar: Option<PendingEndpoint>,
}

struct PendingEndpoint {
    socket: Option<WebSocket>,
    notify: Option<oneshot::Sender<()>>,
}

impl PendingEndpoint {
    fn new(socket: WebSocket, notify: oneshot::Sender<()>) -> Self {
        Self {
            socket: Some(socket),
            notify: Some(notify),
        }
    }

    fn take_socket(&mut self) -> Option<WebSocket> {
        self.socket.take()
    }

    fn take_notify(&mut self) -> Option<oneshot::Sender<()>> {
        self.notify.take()
    }
}

#[derive(Clone, Copy)]
enum EndpointKind {
    Devbox,
    Sidecar,
}

pub struct TunnelBroker {
    sessions: Arc<Mutex<HashMap<Uuid, WaitingEntry>>>,
}

impl TunnelBroker {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn register_devbox(&self, session_id: Uuid, socket: WebSocket) {
        self.register_endpoint(session_id, EndpointKind::Devbox, socket)
            .await;
    }

    pub async fn register_sidecar(&self, session_id: Uuid, socket: WebSocket) {
        self.register_endpoint(session_id, EndpointKind::Sidecar, socket)
            .await;
    }

    async fn register_endpoint(&self, session_id: Uuid, kind: EndpointKind, socket: WebSocket) {
        let (notify_tx, notify_rx) = oneshot::channel();
        let mut bridge_pair = None;

        {
            let mut sessions = self.sessions.lock().await;
            let entry = sessions.entry(session_id).or_default();

            match kind {
                EndpointKind::Devbox => {
                    if entry.devbox.is_some() {
                        warn!("Duplicate devbox endpoint for session {}", session_id);
                        let _ = notify_tx.send(());
                        return;
                    }
                    entry.devbox = Some(PendingEndpoint::new(socket, notify_tx));
                }
                EndpointKind::Sidecar => {
                    if entry.sidecar.is_some() {
                        warn!("Duplicate sidecar endpoint for session {}", session_id);
                        let _ = notify_tx.send(());
                        return;
                    }
                    entry.sidecar = Some(PendingEndpoint::new(socket, notify_tx));
                }
            }

            if entry.devbox.is_some() && entry.sidecar.is_some() {
                let mut entry = sessions.remove(&session_id).unwrap_or_default();
                let mut devbox = entry.devbox.take().unwrap();
                let mut sidecar = entry.sidecar.take().unwrap();
                let devbox_socket = devbox.take_socket().unwrap();
                let sidecar_socket = sidecar.take_socket().unwrap();
                let devbox_notify = devbox.take_notify();
                let sidecar_notify = sidecar.take_notify();
                bridge_pair = Some((
                    session_id,
                    devbox_socket,
                    devbox_notify,
                    sidecar_socket,
                    sidecar_notify,
                ));
            }
        }

        if let Some((session_id, devbox_socket, devbox_notify, sidecar_socket, sidecar_notify)) =
            bridge_pair
        {
            self.spawn_bridge(
                session_id,
                devbox_socket,
                devbox_notify,
                sidecar_socket,
                sidecar_notify,
            );
        }

        let _ = notify_rx.await;
    }

    fn spawn_bridge(
        &self,
        session_id: Uuid,
        devbox_socket: WebSocket,
        devbox_notify: Option<oneshot::Sender<()>>,
        sidecar_socket: WebSocket,
        sidecar_notify: Option<oneshot::Sender<()>>,
    ) {
        tokio::spawn(async move {
            info!("Bridging tunnel session {}", session_id);

            let mut devbox_transport = WebSocketTransport::new(devbox_socket);
            let mut sidecar_transport = WebSocketTransport::new(sidecar_socket);

            if let Err(err) =
                io::copy_bidirectional(&mut devbox_transport, &mut sidecar_transport).await
            {
                warn!("Tunnel session {} ended with error: {}", session_id, err);
            } else {
                info!("Tunnel session {} closed", session_id);
            }

            if let Some(tx) = devbox_notify {
                let _ = tx.send(());
            }
            if let Some(tx) = sidecar_notify {
                let _ = tx.send(());
            }
        });
    }
}
