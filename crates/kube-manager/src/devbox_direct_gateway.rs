use std::{
    collections::HashMap,
    io,
    net::{SocketAddr, ToSocketAddrs},
    sync::Arc,
    time::Duration,
};

use chrono::{DateTime, Utc};
use lapdev_common::devbox::{DirectChannelConfig, DirectCredential};
use lapdev_tunnel::direct::{
    collect_candidates, CandidateOptions, DirectConnection, DirectCredentialStore,
    DirectQuicServer, DirectServerOptions,
};
use lapdev_tunnel::{run_tunnel_server_with_connector, TunnelConnector, TunnelError, TunnelTarget};
use tokio::{
    net::TcpStream,
    sync::RwLock,
    task::JoinHandle,
    time::{sleep, MissedTickBehavior},
};
use tracing::{info, warn};
use uuid::Uuid;

use futures::future::BoxFuture;

#[derive(Clone)]
pub struct DevboxDirectGateway {
    server: Arc<DirectQuicServer>,
    credential_store: Arc<DirectCredentialStore>,
    sessions: Arc<RwLock<HashMap<String, DirectSession>>>,
    _accept_task: Arc<JoinHandle<()>>,
    _cleanup_task: Arc<JoinHandle<()>>,
}

#[derive(Clone)]
struct DirectSession {
    user_id: Uuid,
    environment_id: Uuid,
    namespace: String,
    expires_at: DateTime<Utc>,
}

const DEFAULT_STUN_ENDPOINTS: &[&str] = &[
    "stun.l.google.com:19302",
    "stun1.l.google.com:19302",
    "stun2.l.google.com:19302",
];

impl DevboxDirectGateway {
    pub async fn new() -> Result<Self, TunnelError> {
        let options = DirectServerOptions {
            stun_servers: Self::resolve_stun_endpoints(),
            ..Default::default()
        };
        let server =
            Arc::new(DirectQuicServer::bind_with_options(([0, 0, 0, 0], 0).into(), options).await?);
        let credential_store = server.credential_handle();
        let sessions = Arc::new(RwLock::new(HashMap::new()));
        let gateway = Self {
            server: Arc::clone(&server),
            credential_store: Arc::clone(&credential_store),
            sessions: Arc::clone(&sessions),
            _accept_task: Arc::new(Self::spawn_accept_loop(
                Arc::clone(&server),
                Arc::clone(&credential_store),
                Arc::clone(&sessions),
            )),
            _cleanup_task: Arc::new(Self::spawn_cleanup_loop(
                sessions,
                credential_store,
            )),
        };
        Ok(gateway)
    }

    fn spawn_accept_loop(
        server: Arc<DirectQuicServer>,
        credential_store: Arc<DirectCredentialStore>,
        sessions: Arc<RwLock<HashMap<String, DirectSession>>>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                match server.accept().await {
                    Ok(DirectConnection { transport, token }) => {
                        let session = {
                            let sessions_guard = sessions.read().await;
                            sessions_guard.get(&token).cloned()
                        };

                        let Some(session) = session else {
                            warn!("Direct devbox connection rejected due to unknown token");
                            let _ = credential_store.remove(&token).await;
                            continue;
                        };

                        let connector = NamespacedTcpConnector::new(session.namespace.clone());
                        info!(
                            user_id = %session.user_id,
                            environment_id = %session.environment_id,
                            "Accepted direct devbox tunnel"
                        );
                        tokio::spawn(async move {
                            if let Err(err) =
                                run_tunnel_server_with_connector(transport, connector).await
                            {
                                warn!(
                                    user_id = %session.user_id,
                                    environment_id = %session.environment_id,
                                    error = %err,
                                    "Direct devbox tunnel terminated"
                                );
                            }
                        });
                    }
                    Err(err) => {
                        warn!(error = %err, "Devbox direct gateway accept failed");
                        sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        })
    }

    fn spawn_cleanup_loop(
        sessions: Arc<RwLock<HashMap<String, DirectSession>>>,
        credential_store: Arc<DirectCredentialStore>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let now = Utc::now();
                let expired_tokens: Vec<String> = {
                    let sessions_guard = sessions.read().await;
                    sessions_guard
                        .iter()
                        .filter_map(|(token, session)| {
                            if session.expires_at <= now {
                                Some(token.clone())
                            } else {
                                None
                            }
                        })
                        .collect()
                };

                if expired_tokens.is_empty() {
                    continue;
                }

                let mut sessions_guard = sessions.write().await;
                for token in expired_tokens {
                    sessions_guard.remove(&token);
                    let _ = credential_store.remove(&token).await;
                }
            }
        })
    }

    fn resolve_stun_endpoints() -> Vec<SocketAddr> {
        DEFAULT_STUN_ENDPOINTS
            .iter()
            .filter_map(|endpoint| Self::resolve_stun_endpoint(endpoint).ok())
            .collect()
    }

    fn resolve_stun_endpoint(endpoint: &str) -> io::Result<SocketAddr> {
        endpoint
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "no address resolved"))
    }

    pub async fn issue_config(
        &self,
        user_id: Uuid,
        environment_id: Uuid,
        namespace: String,
    ) -> Result<DirectChannelConfig, String> {
        let token = Uuid::new_v4().to_string();
        let expires_at = Utc::now() + chrono::Duration::minutes(10);
        self.credential_store.insert(token.clone()).await;

        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(
                token.clone(),
                DirectSession {
                    user_id,
                    environment_id,
                    namespace: namespace.clone(),
                    expires_at,
                },
            );
        }

        let candidate_opts = CandidateOptions {
            preferred_port: Some(
                self.server
                    .local_addr()
                    .map_err(|e| format!("Failed to fetch server addr: {}", e))?
                    .port(),
            ),
            stun_observed_addr: self.server.observed_addr(),
            include_loopback: false,
            relay_candidates: Vec::new(),
        };
        let discovery = collect_candidates(&candidate_opts);

        Ok(DirectChannelConfig {
            credential: DirectCredential { token, expires_at },
            devbox_candidates: discovery.candidates.candidates,
            sidecar_candidates: Vec::new(),
        })
    }
}

struct NamespacedTcpConnector {
    namespace: String,
}

impl NamespacedTcpConnector {
    fn new(namespace: String) -> Self {
        Self { namespace }
    }

    fn host_allowed(namespace: &str, host: &str) -> bool {
        let ns = namespace.to_ascii_lowercase();
        let host = host.to_ascii_lowercase();
        host.ends_with(&format!(".{}.svc", ns))
            || host.ends_with(&format!(".{}.svc.cluster.local", ns))
            || host.ends_with(&format!(".{}", ns))
    }
}

impl TunnelConnector for NamespacedTcpConnector {
    fn connect(
        &self,
        target: TunnelTarget,
    ) -> BoxFuture<'static, Result<lapdev_tunnel::DynTunnelStream, TunnelError>> {
        let namespace = self.namespace.clone();
        Box::pin(async move {
            let TunnelTarget { host, port } = target;
            if !NamespacedTcpConnector::host_allowed(&namespace, &host) {
                warn!(host = %host, namespace = %namespace, "Rejected direct devbox target outside namespace");
                return Err(TunnelError::Remote("target namespace not permitted".into()));
            }
            let address = format!("{}:{}", host, port);
            let stream = TcpStream::connect(address).await?;
            Ok(Box::new(stream) as lapdev_tunnel::DynTunnelStream)
        })
    }
}
