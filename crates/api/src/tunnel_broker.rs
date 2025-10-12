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
struct InterceptWaitingEntry {
    devbox: Option<PendingEndpoint>,
    sidecar: Option<PendingEndpoint>,
}

#[derive(Default)]
struct ProxyWaitingEntry {
    devbox: Option<PendingEndpoint>,
    proxy: Option<PendingEndpoint>,
    session_id: Option<Uuid>,
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
enum InterceptEndpointKind {
    Devbox,
    Sidecar,
}

pub struct TunnelBroker {
    intercept_sessions: Arc<Mutex<HashMap<Uuid, InterceptWaitingEntry>>>,
    proxy_environments: Arc<Mutex<HashMap<Uuid, ProxyWaitingEntry>>>,
}

impl TunnelBroker {
    pub fn new() -> Self {
        Self {
            intercept_sessions: Arc::new(Mutex::new(HashMap::new())),
            proxy_environments: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn register_devbox(&self, session_id: Uuid, socket: WebSocket) {
        self.register_intercept_endpoint(session_id, InterceptEndpointKind::Devbox, socket)
            .await;
    }

    pub async fn register_sidecar(&self, session_id: Uuid, socket: WebSocket) {
        self.register_intercept_endpoint(session_id, InterceptEndpointKind::Sidecar, socket)
            .await;
    }

    pub async fn register_devbox_proxy_client(
        &self,
        environment_id: Uuid,
        session_id: Uuid,
        socket: WebSocket,
    ) {
        let (notify_tx, notify_rx) = oneshot::channel();
        let mut bridge_pair = None;

        {
            let mut environments = self.proxy_environments.lock().await;
            let entry = environments.entry(environment_id).or_default();

            if entry.devbox.is_some() {
                warn!(
                    "Duplicate devbox proxy client endpoint for environment {} (session {})",
                    environment_id, session_id
                );
                let _ = notify_tx.send(());
                return;
            }

            entry.session_id = Some(session_id);
            entry.devbox = Some(PendingEndpoint::new(socket, notify_tx));

            if entry.proxy.is_some() {
                let mut entry = environments.remove(&environment_id).unwrap_or_default();
                let mut devbox = entry.devbox.take().unwrap();
                let mut proxy = entry.proxy.take().unwrap();
                let devbox_socket = devbox.take_socket().unwrap();
                let proxy_socket = proxy.take_socket().unwrap();
                let devbox_notify = devbox.take_notify();
                let proxy_notify = proxy.take_notify();
                let session_id = entry.session_id.unwrap_or_else(|| {
                    warn!(
                        "Missing session id while bridging environment {}; defaulting to {}",
                        environment_id, session_id
                    );
                    session_id
                });
                bridge_pair = Some((
                    environment_id,
                    session_id,
                    devbox_socket,
                    devbox_notify,
                    proxy_socket,
                    proxy_notify,
                ));
            }
        }

        if let Some((
            environment_id,
            session_id,
            devbox_socket,
            devbox_notify,
            proxy_socket,
            proxy_notify,
        )) = bridge_pair
        {
            self.spawn_proxy_bridge(
                environment_id,
                session_id,
                devbox_socket,
                devbox_notify,
                proxy_socket,
                proxy_notify,
            );
        }

        let _ = notify_rx.await;
    }

    pub async fn register_devbox_proxy(&self, environment_id: Uuid, socket: WebSocket) {
        let (notify_tx, notify_rx) = oneshot::channel();
        let mut bridge_pair = None;

        {
            let mut environments = self.proxy_environments.lock().await;
            let entry = environments.entry(environment_id).or_default();

            if entry.proxy.is_some() {
                warn!(
                    "Duplicate devbox proxy endpoint for environment {}",
                    environment_id
                );
                let _ = notify_tx.send(());
                return;
            }

            entry.proxy = Some(PendingEndpoint::new(socket, notify_tx));

            if entry.devbox.is_some() {
                let mut entry = environments.remove(&environment_id).unwrap_or_default();
                let mut devbox = entry.devbox.take().unwrap();
                let mut proxy = entry.proxy.take().unwrap();
                let devbox_socket = devbox.take_socket().unwrap();
                let proxy_socket = proxy.take_socket().unwrap();
                let devbox_notify = devbox.take_notify();
                let proxy_notify = proxy.take_notify();
                let session_id = entry.session_id.unwrap_or_else(|| {
                    warn!(
                        "Missing session id while bridging environment {}; using zero UUID",
                        environment_id
                    );
                    Uuid::nil()
                });
                bridge_pair = Some((
                    environment_id,
                    session_id,
                    devbox_socket,
                    devbox_notify,
                    proxy_socket,
                    proxy_notify,
                ));
            }
        }

        if let Some((
            environment_id,
            session_id,
            devbox_socket,
            devbox_notify,
            proxy_socket,
            proxy_notify,
        )) = bridge_pair
        {
            self.spawn_proxy_bridge(
                environment_id,
                session_id,
                devbox_socket,
                devbox_notify,
                proxy_socket,
                proxy_notify,
            );
        }

        let _ = notify_rx.await;
    }

    async fn register_intercept_endpoint(
        &self,
        session_id: Uuid,
        kind: InterceptEndpointKind,
        socket: WebSocket,
    ) {
        let (notify_tx, notify_rx) = oneshot::channel();
        let mut bridge_pair = None;

        {
            let mut sessions = self.intercept_sessions.lock().await;
            let entry = sessions.entry(session_id).or_default();

            match kind {
                InterceptEndpointKind::Devbox => {
                    if entry.devbox.is_some() {
                        warn!("Duplicate devbox endpoint for session {}", session_id);
                        let _ = notify_tx.send(());
                        return;
                    }
                    entry.devbox = Some(PendingEndpoint::new(socket, notify_tx));
                }
                InterceptEndpointKind::Sidecar => {
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
            self.spawn_intercept_bridge(
                session_id,
                devbox_socket,
                devbox_notify,
                sidecar_socket,
                sidecar_notify,
            );
        }

        let _ = notify_rx.await;
    }

    fn spawn_intercept_bridge(
        &self,
        session_id: Uuid,
        devbox_socket: WebSocket,
        devbox_notify: Option<oneshot::Sender<()>>,
        sidecar_socket: WebSocket,
        sidecar_notify: Option<oneshot::Sender<()>>,
    ) {
        tokio::spawn(async move {
            info!("Bridging intercept tunnel session {}", session_id);

            let mut devbox_transport = WebSocketTransport::new(devbox_socket);
            let mut sidecar_transport = WebSocketTransport::new(sidecar_socket);

            if let Err(err) =
                io::copy_bidirectional(&mut devbox_transport, &mut sidecar_transport).await
            {
                warn!(
                    "Intercept tunnel session {} ended with error: {}",
                    session_id, err
                );
            } else {
                info!("Intercept tunnel session {} closed", session_id);
            }

            if let Some(tx) = devbox_notify {
                let _ = tx.send(());
            }
            if let Some(tx) = sidecar_notify {
                let _ = tx.send(());
            }
        });
    }

    fn spawn_proxy_bridge(
        &self,
        environment_id: Uuid,
        session_id: Uuid,
        devbox_socket: WebSocket,
        devbox_notify: Option<oneshot::Sender<()>>,
        proxy_socket: WebSocket,
        proxy_notify: Option<oneshot::Sender<()>>,
    ) {
        tokio::spawn(async move {
            info!(
                "Bridging proxy tunnel for environment {} session {}",
                environment_id, session_id
            );

            let mut devbox_transport = WebSocketTransport::new(devbox_socket);
            let mut proxy_transport = WebSocketTransport::new(proxy_socket);

            if let Err(err) =
                io::copy_bidirectional(&mut devbox_transport, &mut proxy_transport).await
            {
                warn!(
                    "Proxy tunnel for environment {} session {} ended with error: {}",
                    environment_id, session_id, err
                );
            } else {
                info!(
                    "Proxy tunnel for environment {} session {} closed",
                    environment_id, session_id
                );
            }

            if let Some(tx) = devbox_notify {
                let _ = tx.send(());
            }
            if let Some(tx) = proxy_notify {
                let _ = tx.send(());
            }
        });
    }
}
