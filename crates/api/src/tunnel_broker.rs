use std::collections::HashMap;

use axum::extract::ws::WebSocket;
use tokio::{
    io,
    sync::{mpsc, oneshot},
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
    socket: WebSocket,
    notify: oneshot::Sender<()>,
}

impl PendingEndpoint {
    fn new(socket: WebSocket, notify: oneshot::Sender<()>) -> Self {
        Self { socket, notify }
    }

    fn split(self) -> (WebSocket, oneshot::Sender<()>) {
        (self.socket, self.notify)
    }
}

#[derive(Clone, Copy)]
enum InterceptEndpointKind {
    Devbox,
    Sidecar,
}

pub struct TunnelBroker {
    commands: mpsc::UnboundedSender<BrokerCommand>,
}

enum BrokerCommand {
    RegisterIntercept {
        session_id: Uuid,
        kind: InterceptEndpointKind,
        socket: WebSocket,
        notify: oneshot::Sender<()>,
    },
    RegisterProxyClient {
        environment_id: Uuid,
        session_id: Uuid,
        socket: WebSocket,
        notify: oneshot::Sender<()>,
    },
    RegisterProxy {
        environment_id: Uuid,
        socket: WebSocket,
        notify: oneshot::Sender<()>,
    },
}

impl TunnelBroker {
    pub fn new() -> Self {
        let (commands, mut rx) = mpsc::unbounded_channel();

        tokio::spawn(async move {
            let mut intercept_sessions: HashMap<Uuid, InterceptWaitingEntry> = HashMap::new();
            let mut proxy_environments: HashMap<Uuid, ProxyWaitingEntry> = HashMap::new();

            while let Some(command) = rx.recv().await {
                match command {
                    BrokerCommand::RegisterIntercept {
                        session_id,
                        kind,
                        socket,
                        notify,
                    } => Self::handle_register_intercept(
                        &mut intercept_sessions,
                        session_id,
                        kind,
                        socket,
                        notify,
                    ),
                    BrokerCommand::RegisterProxyClient {
                        environment_id,
                        session_id,
                        socket,
                        notify,
                    } => Self::handle_register_proxy_client(
                        &mut proxy_environments,
                        environment_id,
                        session_id,
                        socket,
                        notify,
                    ),
                    BrokerCommand::RegisterProxy {
                        environment_id,
                        socket,
                        notify,
                    } => Self::handle_register_proxy(
                        &mut proxy_environments,
                        environment_id,
                        socket,
                        notify,
                    ),
                }
            }
        });

        Self { commands }
    }

    pub async fn register_devbox(&self, session_id: Uuid, socket: WebSocket) {
        let (notify_tx, notify_rx) = oneshot::channel();
        if self
            .commands
            .send(BrokerCommand::RegisterIntercept {
                session_id,
                kind: InterceptEndpointKind::Devbox,
                socket,
                notify: notify_tx,
            })
            .is_err()
        {
            warn!("Tunnel broker command channel closed while registering devbox endpoint");
            return;
        }
        let _ = notify_rx.await;
    }

    pub async fn register_sidecar(&self, session_id: Uuid, socket: WebSocket) {
        let (notify_tx, notify_rx) = oneshot::channel();
        if self
            .commands
            .send(BrokerCommand::RegisterIntercept {
                session_id,
                kind: InterceptEndpointKind::Sidecar,
                socket,
                notify: notify_tx,
            })
            .is_err()
        {
            warn!("Tunnel broker command channel closed while registering sidecar endpoint");
            return;
        }
        let _ = notify_rx.await;
    }

    pub async fn register_devbox_proxy_client(
        &self,
        environment_id: Uuid,
        session_id: Uuid,
        socket: WebSocket,
    ) {
        let (notify_tx, notify_rx) = oneshot::channel();
        if self
            .commands
            .send(BrokerCommand::RegisterProxyClient {
                environment_id,
                session_id,
                socket,
                notify: notify_tx,
            })
            .is_err()
        {
            warn!("Tunnel broker command channel closed while registering proxy client endpoint");
            return;
        }
        let _ = notify_rx.await;
    }

    pub async fn register_devbox_proxy(&self, environment_id: Uuid, socket: WebSocket) {
        let (notify_tx, notify_rx) = oneshot::channel();
        if self
            .commands
            .send(BrokerCommand::RegisterProxy {
                environment_id,
                socket,
                notify: notify_tx,
            })
            .is_err()
        {
            warn!("Tunnel broker command channel closed while registering proxy endpoint");
            return;
        }
        let _ = notify_rx.await;
    }

    fn spawn_intercept_bridge(
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

    fn handle_register_intercept(
        sessions: &mut HashMap<Uuid, InterceptWaitingEntry>,
        session_id: Uuid,
        kind: InterceptEndpointKind,
        socket: WebSocket,
        notify: oneshot::Sender<()>,
    ) {
        let entry = sessions.entry(session_id).or_default();
        match kind {
            InterceptEndpointKind::Devbox => {
                if entry
                    .devbox
                    .replace(PendingEndpoint::new(socket, notify))
                    .is_some()
                {
                    warn!(
                        "Duplicate devbox endpoint for session {} - replacing stale connection",
                        session_id
                    );
                }
            }
            InterceptEndpointKind::Sidecar => {
                if entry
                    .sidecar
                    .replace(PendingEndpoint::new(socket, notify))
                    .is_some()
                {
                    warn!(
                        "Duplicate sidecar endpoint for session {} - replacing stale connection",
                        session_id
                    );
                }
            }
        }

        let ready = entry.devbox.is_some() && entry.sidecar.is_some();
        if !ready {
            return;
        }

        if let Some(entry) = sessions.remove(&session_id) {
            let InterceptWaitingEntry { devbox, sidecar } = entry;
            if let (Some(devbox), Some(sidecar)) = (devbox, sidecar) {
                let (devbox_socket, devbox_notify) = devbox.split();
                let (sidecar_socket, sidecar_notify) = sidecar.split();
                Self::spawn_intercept_bridge(
                    session_id,
                    devbox_socket,
                    Some(devbox_notify),
                    sidecar_socket,
                    Some(sidecar_notify),
                );
            }
        }
    }

    fn handle_register_proxy_client(
        environments: &mut HashMap<Uuid, ProxyWaitingEntry>,
        environment_id: Uuid,
        session_id: Uuid,
        socket: WebSocket,
        notify: oneshot::Sender<()>,
    ) {
        let entry = environments.entry(environment_id).or_default();
        entry.session_id = Some(session_id);
        if entry
            .devbox
            .replace(PendingEndpoint::new(socket, notify))
            .is_some()
        {
            warn!(
                "Duplicate devbox proxy client endpoint for environment {} (session {})",
                environment_id, session_id
            );
        }

        if entry.proxy.is_none() {
            return;
        }

        if let Some(entry) = environments.remove(&environment_id) {
            let ProxyWaitingEntry {
                devbox,
                proxy,
                session_id: stored_session,
            } = entry;

            if let (Some(devbox), Some(proxy)) = (devbox, proxy) {
                let (devbox_socket, devbox_notify) = devbox.split();
                let (proxy_socket, proxy_notify) = proxy.split();
                let session_id = stored_session.unwrap_or_else(|| {
                    warn!(
                        "Missing session id while bridging environment {}; defaulting to {}",
                        environment_id, session_id
                    );
                    session_id
                });
                Self::spawn_proxy_bridge(
                    environment_id,
                    session_id,
                    devbox_socket,
                    Some(devbox_notify),
                    proxy_socket,
                    Some(proxy_notify),
                );
            }
        }
    }

    fn handle_register_proxy(
        environments: &mut HashMap<Uuid, ProxyWaitingEntry>,
        environment_id: Uuid,
        socket: WebSocket,
        notify: oneshot::Sender<()>,
    ) {
        let entry = environments.entry(environment_id).or_default();
        if entry
            .proxy
            .replace(PendingEndpoint::new(socket, notify))
            .is_some()
        {
            warn!(
                "Duplicate devbox proxy endpoint for environment {}",
                environment_id
            );
        }

        if entry.devbox.is_none() {
            return;
        }

        if let Some(entry) = environments.remove(&environment_id) {
            let ProxyWaitingEntry {
                devbox,
                proxy,
                session_id,
            } = entry;

            if let (Some(devbox), Some(proxy)) = (devbox, proxy) {
                let (devbox_socket, devbox_notify) = devbox.split();
                let (proxy_socket, proxy_notify) = proxy.split();
                let session_id = session_id.unwrap_or_else(|| {
                    warn!(
                        "Missing session id while bridging environment {}; using zero UUID",
                        environment_id
                    );
                    Uuid::nil()
                });
                Self::spawn_proxy_bridge(
                    environment_id,
                    session_id,
                    devbox_socket,
                    Some(devbox_notify),
                    proxy_socket,
                    Some(proxy_notify),
                );
            }
        }
    }
}
