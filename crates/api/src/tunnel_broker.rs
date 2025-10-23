use std::collections::HashMap;

use axum::extract::ws::WebSocket;
use tokio::{
    io,
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use tracing::{info, warn};
use uuid::Uuid;

use crate::websocket_transport::WebSocketTransport;

#[derive(Default)]
struct InterceptWaitingEntry {
    devbox: Option<PendingEndpoint>,
    devbox_workload_id: Option<Uuid>,
    sidecars: HashMap<Uuid, PendingEndpoint>,
}

#[derive(Default)]
struct ProxySessionEntry {
    devbox: Option<PendingEndpoint>,
    active_environment: Option<Uuid>,
}

#[derive(Default)]
struct ProxyEnvironmentEntry {
    proxy: Option<PendingEndpoint>,
    active_session: Option<Uuid>,
}

struct ActiveBridgeEntry {
    environment_id: Uuid,
    handle: JoinHandle<()>,
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

enum BrokerCommand {
    RegisterIntercept {
        session_id: Uuid,
        workload_id: Option<Uuid>,
        kind: InterceptEndpointKind,
        socket: WebSocket,
        notify: oneshot::Sender<()>,
    },
    RegisterProxyClient {
        session_id: Uuid,
        environment_id: Uuid,
        socket: WebSocket,
        notify: oneshot::Sender<()>,
    },
    RegisterProxy {
        environment_id: Uuid,
        socket: WebSocket,
        notify: oneshot::Sender<()>,
    },
    UpdateSessionEnvironment {
        session_id: Uuid,
        environment_id: Option<Uuid>,
    },
    ProxyBridgeClosed {
        session_id: Uuid,
        environment_id: Uuid,
    },
}

pub struct TunnelBroker {
    commands: mpsc::UnboundedSender<BrokerCommand>,
}

impl TunnelBroker {
    pub fn new() -> Self {
        let (commands, mut rx) = mpsc::unbounded_channel();
        let command_tx = commands.clone();

        tokio::spawn(async move {
            let mut intercept_sessions: HashMap<Uuid, InterceptWaitingEntry> = HashMap::new();
            let mut proxy_sessions: HashMap<Uuid, ProxySessionEntry> = HashMap::new();
            let mut proxy_environments: HashMap<Uuid, ProxyEnvironmentEntry> = HashMap::new();
            let mut active_bridges: HashMap<Uuid, ActiveBridgeEntry> = HashMap::new();
            let command_sender = command_tx;

            while let Some(command) = rx.recv().await {
                match command {
                    BrokerCommand::RegisterIntercept {
                        session_id,
                        workload_id,
                        kind,
                        socket,
                        notify,
                    } => Self::handle_register_intercept(
                        &mut intercept_sessions,
                        session_id,
                        workload_id,
                        kind,
                        socket,
                        notify,
                    ),
                    BrokerCommand::RegisterProxyClient {
                        session_id,
                        environment_id,
                        socket,
                        notify,
                    } => Self::handle_register_proxy_client(
                        &command_sender,
                        &mut proxy_sessions,
                        &mut proxy_environments,
                        &mut active_bridges,
                        session_id,
                        environment_id,
                        socket,
                        notify,
                    ),
                    BrokerCommand::RegisterProxy {
                        environment_id,
                        socket,
                        notify,
                    } => Self::handle_register_proxy(
                        &command_sender,
                        &mut proxy_sessions,
                        &mut proxy_environments,
                        &mut active_bridges,
                        environment_id,
                        socket,
                        notify,
                    ),
                    BrokerCommand::UpdateSessionEnvironment {
                        session_id,
                        environment_id,
                    } => Self::handle_update_session_environment(
                        &command_sender,
                        &mut proxy_sessions,
                        &mut proxy_environments,
                        &mut active_bridges,
                        session_id,
                        environment_id,
                    ),
                    BrokerCommand::ProxyBridgeClosed {
                        session_id,
                        environment_id,
                    } => Self::handle_proxy_bridge_closed(
                        &mut proxy_sessions,
                        &mut proxy_environments,
                        &mut active_bridges,
                        session_id,
                        environment_id,
                    ),
                }
            }
        });

        Self { commands }
    }

    pub async fn register_devbox(
        &self,
        session_id: Uuid,
        workload_id: Option<Uuid>,
        socket: WebSocket,
    ) {
        let (notify_tx, notify_rx) = oneshot::channel();
        if self
            .commands
            .send(BrokerCommand::RegisterIntercept {
                session_id,
                workload_id,
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

    pub async fn register_sidecar(&self, session_id: Uuid, workload_id: Uuid, socket: WebSocket) {
        let (notify_tx, notify_rx) = oneshot::channel();
        if self
            .commands
            .send(BrokerCommand::RegisterIntercept {
                session_id,
                workload_id: Some(workload_id),
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

    pub async fn set_session_environment(&self, session_id: Uuid, environment_id: Option<Uuid>) {
        if self
            .commands
            .send(BrokerCommand::UpdateSessionEnvironment {
                session_id,
                environment_id,
            })
            .is_err()
        {
            warn!("Tunnel broker command channel closed while updating session environment");
        }
    }

    fn spawn_intercept_bridge(
        session_id: Uuid,
        workload_id: Option<Uuid>,
        devbox_socket: WebSocket,
        devbox_notify: Option<oneshot::Sender<()>>,
        sidecar_socket: WebSocket,
        sidecar_notify: Option<oneshot::Sender<()>>,
    ) {
        tokio::spawn(async move {
            match workload_id {
                Some(workload) => {
                    info!(
                        "Bridging intercept tunnel session {} workload {}",
                        session_id, workload
                    );
                }
                None => {
                    info!("Bridging intercept tunnel session {}", session_id);
                }
            }

            let mut devbox_transport = WebSocketTransport::new(devbox_socket);
            let mut sidecar_transport = WebSocketTransport::new(sidecar_socket);

            if let Err(err) =
                io::copy_bidirectional(&mut devbox_transport, &mut sidecar_transport).await
            {
                match workload_id {
                    Some(workload) => {
                        warn!(
                            "Intercept tunnel session {} workload {} ended with error: {}",
                            session_id, workload, err
                        );
                    }
                    None => {
                        warn!(
                            "Intercept tunnel session {} ended with error: {}",
                            session_id, err
                        );
                    }
                }
            } else {
                match workload_id {
                    Some(workload) => {
                        info!(
                            "Intercept tunnel session {} workload {} closed",
                            session_id, workload
                        );
                    }
                    None => {
                        info!("Intercept tunnel session {} closed", session_id);
                    }
                }
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
        commands: mpsc::UnboundedSender<BrokerCommand>,
        environment_id: Uuid,
        session_id: Uuid,
        devbox_socket: WebSocket,
        devbox_notify: Option<oneshot::Sender<()>>,
        proxy_socket: WebSocket,
        proxy_notify: Option<oneshot::Sender<()>>,
    ) -> JoinHandle<()> {
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

            let _ = commands.send(BrokerCommand::ProxyBridgeClosed {
                session_id,
                environment_id,
            });
        })
    }

    fn handle_register_intercept(
        sessions: &mut HashMap<Uuid, InterceptWaitingEntry>,
        session_id: Uuid,
        workload_id: Option<Uuid>,
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
                entry.devbox_workload_id = workload_id;
            }
            InterceptEndpointKind::Sidecar => {
                let Some(workload_id) = workload_id else {
                    warn!(
                        "Sidecar endpoint for session {} missing workload id; dropping connection",
                        session_id
                    );
                    return;
                };

                if entry
                    .sidecars
                    .insert(workload_id, PendingEndpoint::new(socket, notify))
                    .is_some()
                {
                    warn!(
                        "Duplicate sidecar endpoint for session {} workload {} - replacing stale connection",
                        session_id, workload_id
                    );
                }
            }
        }

        Self::try_pair_intercept(sessions, session_id);
    }

    fn try_pair_intercept(sessions: &mut HashMap<Uuid, InterceptWaitingEntry>, session_id: Uuid) {
        let (devbox, workload_id, sidecar, remove_entry) = {
            let Some(entry) = sessions.get_mut(&session_id) else {
                return;
            };

            let Some(devbox) = entry.devbox.take() else {
                return;
            };

            let workload_id = entry.devbox_workload_id.take();

            let (selected_workload, sidecar) = match workload_id {
                Some(workload_id) => match entry.sidecars.remove(&workload_id) {
                    Some(endpoint) => (Some(workload_id), endpoint),
                    None => {
                        // No matching workload yet; restore and wait.
                        entry.devbox = Some(devbox);
                        entry.devbox_workload_id = Some(workload_id);
                        return;
                    }
                },
                None => {
                    let Some((&workload_id, _)) = entry.sidecars.iter().next() else {
                        // No sidecars waiting; restore devbox endpoint.
                        entry.devbox = Some(devbox);
                        return;
                    };
                    let endpoint = entry
                        .sidecars
                        .remove(&workload_id)
                        .expect("sidecar entry present");
                    (Some(workload_id), endpoint)
                }
            };

            let remove_entry = entry.devbox.is_none() && entry.sidecars.is_empty();

            (devbox, selected_workload, sidecar, remove_entry)
        };

        if remove_entry {
            sessions.remove(&session_id);
        }

        let (devbox_socket, devbox_notify) = devbox.split();
        let (sidecar_socket, sidecar_notify) = sidecar.split();
        Self::spawn_intercept_bridge(
            session_id,
            workload_id,
            devbox_socket,
            Some(devbox_notify),
            sidecar_socket,
            Some(sidecar_notify),
        );
    }

    fn handle_register_proxy_client(
        commands: &mpsc::UnboundedSender<BrokerCommand>,
        sessions: &mut HashMap<Uuid, ProxySessionEntry>,
        environments: &mut HashMap<Uuid, ProxyEnvironmentEntry>,
        active_bridges: &mut HashMap<Uuid, ActiveBridgeEntry>,
        session_id: Uuid,
        environment_id: Uuid,
        socket: WebSocket,
        notify: oneshot::Sender<()>,
    ) {
        let entry = sessions.entry(session_id).or_default();
        if entry
            .devbox
            .replace(PendingEndpoint::new(socket, notify))
            .is_some()
        {
            warn!(
                "Duplicate devbox proxy client endpoint for session {} - replacing stale connection",
                session_id
            );
            Self::abort_active_bridge(active_bridges, environments, session_id);
        }

        Self::update_session_environment_internal(
            sessions,
            environments,
            active_bridges,
            session_id,
            Some(environment_id),
        );

        Self::attempt_proxy_pair(commands, sessions, environments, active_bridges, session_id);
    }

    fn handle_register_proxy(
        commands: &mpsc::UnboundedSender<BrokerCommand>,
        sessions: &mut HashMap<Uuid, ProxySessionEntry>,
        environments: &mut HashMap<Uuid, ProxyEnvironmentEntry>,
        active_bridges: &mut HashMap<Uuid, ActiveBridgeEntry>,
        environment_id: Uuid,
        socket: WebSocket,
        notify: oneshot::Sender<()>,
    ) {
        let active_session = {
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
            entry.active_session
        };

        if let Some(session_id) = active_session {
            Self::attempt_proxy_pair(commands, sessions, environments, active_bridges, session_id);
        }
    }

    fn handle_update_session_environment(
        commands: &mpsc::UnboundedSender<BrokerCommand>,
        sessions: &mut HashMap<Uuid, ProxySessionEntry>,
        environments: &mut HashMap<Uuid, ProxyEnvironmentEntry>,
        active_bridges: &mut HashMap<Uuid, ActiveBridgeEntry>,
        session_id: Uuid,
        environment_id: Option<Uuid>,
    ) {
        Self::update_session_environment_internal(
            sessions,
            environments,
            active_bridges,
            session_id,
            environment_id,
        );

        Self::attempt_proxy_pair(commands, sessions, environments, active_bridges, session_id);
    }

    fn handle_proxy_bridge_closed(
        sessions: &mut HashMap<Uuid, ProxySessionEntry>,
        environments: &mut HashMap<Uuid, ProxyEnvironmentEntry>,
        active_bridges: &mut HashMap<Uuid, ActiveBridgeEntry>,
        session_id: Uuid,
        environment_id: Uuid,
    ) {
        if let Some(active) = active_bridges.remove(&session_id) {
            info!(
                "Proxy tunnel between session {} and environment {} closed",
                session_id, active.environment_id
            );
            if active.environment_id != environment_id {
                warn!(
                    "Proxy bridge closed event environment mismatch: expected {}, received {}",
                    active.environment_id, environment_id
                );
            }
        } else {
            info!(
                "Proxy tunnel closed for session {} environment {}",
                session_id, environment_id
            );
        }

        if let Some(session_entry) = sessions.get(&session_id) {
            if session_entry.active_environment != Some(environment_id) {
                if let Some(env_entry) = environments.get_mut(&environment_id) {
                    if env_entry.active_session == Some(session_id) {
                        env_entry.active_session = None;
                    }
                }
            }
        }
    }

    fn update_session_environment_internal(
        sessions: &mut HashMap<Uuid, ProxySessionEntry>,
        environments: &mut HashMap<Uuid, ProxyEnvironmentEntry>,
        active_bridges: &mut HashMap<Uuid, ActiveBridgeEntry>,
        session_id: Uuid,
        environment_id: Option<Uuid>,
    ) {
        let entry = sessions.entry(session_id).or_default();
        if entry.active_environment == environment_id {
            return;
        }

        if let Some(active) = active_bridges.remove(&session_id) {
            info!(
                "Aborting proxy tunnel for session {} (environment {})",
                session_id, active.environment_id
            );
            active.handle.abort();
            if let Some(env_entry) = environments.get_mut(&active.environment_id) {
                if env_entry.active_session == Some(session_id) {
                    env_entry.active_session = None;
                }
            }
        }

        if let Some(old_env) = entry.active_environment.take() {
            if let Some(env_entry) = environments.get_mut(&old_env) {
                if env_entry.active_session == Some(session_id) {
                    env_entry.active_session = None;
                }
            }
        }

        entry.active_environment = environment_id;

        if let Some(new_env) = environment_id {
            let previous_session = {
                let env_entry = environments.entry(new_env).or_default();
                let previous = env_entry.active_session;
                env_entry.active_session = Some(session_id);
                previous
            };

            if let Some(previous_session) = previous_session {
                if previous_session != session_id {
                    Self::abort_active_bridge(active_bridges, environments, previous_session);
                }
            }
        }
    }

    fn abort_active_bridge(
        active_bridges: &mut HashMap<Uuid, ActiveBridgeEntry>,
        environments: &mut HashMap<Uuid, ProxyEnvironmentEntry>,
        session_id: Uuid,
    ) {
        if let Some(active) = active_bridges.remove(&session_id) {
            info!(
                "Aborting existing proxy bridge for session {} environment {}",
                session_id, active.environment_id
            );
            active.handle.abort();
            if let Some(env_entry) = environments.get_mut(&active.environment_id) {
                if env_entry.active_session == Some(session_id) {
                    env_entry.active_session = None;
                }
            }
        }
    }

    fn attempt_proxy_pair(
        commands: &mpsc::UnboundedSender<BrokerCommand>,
        sessions: &mut HashMap<Uuid, ProxySessionEntry>,
        environments: &mut HashMap<Uuid, ProxyEnvironmentEntry>,
        active_bridges: &mut HashMap<Uuid, ActiveBridgeEntry>,
        session_id: Uuid,
    ) {
        if active_bridges.contains_key(&session_id) {
            return;
        }

        let Some(session_entry) = sessions.get_mut(&session_id) else {
            return;
        };

        let Some(environment_id) = session_entry.active_environment else {
            return;
        };

        let Some(env_entry) = environments.get_mut(&environment_id) else {
            return;
        };

        if env_entry.active_session != Some(session_id) {
            return;
        }

        if session_entry.devbox.is_none() || env_entry.proxy.is_none() {
            return;
        }

        let devbox = session_entry
            .devbox
            .take()
            .expect("devbox endpoint missing despite earlier check");
        let proxy = env_entry
            .proxy
            .take()
            .expect("proxy endpoint missing despite earlier check");

        let (devbox_socket, devbox_notify) = devbox.split();
        let (proxy_socket, proxy_notify) = proxy.split();

        let handle = Self::spawn_proxy_bridge(
            commands.clone(),
            environment_id,
            session_id,
            devbox_socket,
            Some(devbox_notify),
            proxy_socket,
            Some(proxy_notify),
        );

        active_bridges.insert(
            session_id,
            ActiveBridgeEntry {
                environment_id,
                handle,
            },
        );
    }
}
