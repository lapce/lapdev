use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use axum::extract::ws::WebSocket;
use lapdev_tunnel::{
    run_tunnel_server_with_connector, DynTunnelStream, TunnelClient, TunnelError, TunnelTarget,
};
use tokio::sync::RwLock;
use tracing::{info, warn};
use uuid::Uuid;

use crate::websocket_transport::WebSocketTransport;

pub struct DevboxTunnelRegistry {
    sessions: Arc<RwLock<HashMap<Uuid, BTreeMap<u64, Arc<TunnelClient>>>>>,
    generation_counter: AtomicU64,
}

impl DevboxTunnelRegistry {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            generation_counter: AtomicU64::new(1),
        }
    }

    fn next_generation(&self) -> u64 {
        self.generation_counter.fetch_add(1, Ordering::SeqCst)
    }

    pub async fn register_cli(&self, session_id: Uuid, socket: WebSocket) {
        let transport = WebSocketTransport::new(socket);
        let client = Arc::new(TunnelClient::connect(transport));
        let generation = self.next_generation();

        {
            let mut sessions = self.sessions.write().await;
            let entry = sessions.entry(session_id).or_insert_with(BTreeMap::new);
            if entry.insert(generation, client.clone()).is_some() {
                warn!(
                    "Overwrote Devbox CLI tunnel generation {} for session {}",
                    generation, session_id
                );
            }
            info!(
                "Registered Devbox CLI tunnel for session {} generation {}; active entries {}",
                session_id,
                generation,
                entry.len()
            );
        }

        let sessions = Arc::clone(&self.sessions);
        tokio::spawn(async move {
            client.closed().await;
            let mut sessions = sessions.write().await;
            if let Some(entry) = sessions.get_mut(&session_id) {
                if entry.remove(&generation).is_some() {
                    info!(
                        "Devbox CLI tunnel closed for session {} generation {}; remaining {}",
                        session_id,
                        generation,
                        entry.len()
                    );
                    if entry.is_empty() {
                        sessions.remove(&session_id);
                    }
                } else {
                    info!(
                        "Skip removing Devbox CLI tunnel for session {} due to generation mismatch (requested {})",
                        session_id,
                        generation
                    );
                }
            } else {
                info!(
                    "Devbox CLI tunnel cleanup found no session {} for generation {}",
                    session_id, generation
                );
            }
        });
    }

    pub async fn attach_sidecar(
        &self,
        session_id: Uuid,
        socket: WebSocket,
    ) -> Result<(), TunnelError> {
        let cli_client = {
            let sessions = self.sessions.read().await;
            sessions.get(&session_id).and_then(|entry| {
                entry
                    .values()
                    .rev()
                    .find(|client| !client.is_closed())
                    .cloned()
            })
        };

        let Some(cli_client) = cli_client else {
            warn!(
                "Received sidecar tunnel for session {} with no active CLI tunnel",
                session_id
            );
            return Err(TunnelError::Remote("no active CLI tunnel".to_string()));
        };

        let transport = WebSocketTransport::new(socket);
        let connector = move |target: TunnelTarget| {
            let cli_client = cli_client.clone();
            async move {
                let stream = cli_client
                    .connect_tcp(target.host.clone(), target.port)
                    .await?;
                Ok::<DynTunnelStream, TunnelError>(Box::new(stream) as DynTunnelStream)
            }
        };

        run_tunnel_server_with_connector(transport, connector).await
    }
}

impl Default for DevboxTunnelRegistry {
    fn default() -> Self {
        Self::new()
    }
}
