use std::{collections::HashMap, sync::Arc};

use axum::extract::ws::WebSocket;
use lapdev_tunnel::{
    run_tunnel_server_with_connector, DynTunnelStream, TunnelClient, TunnelError, TunnelTarget,
};
use tokio::sync::RwLock;
use tracing::{info, warn};
use uuid::Uuid;

use crate::websocket_transport::WebSocketTransport;

pub struct DevboxTunnelRegistry {
    sessions: Arc<RwLock<HashMap<Uuid, Arc<TunnelClient>>>>,
}

impl DevboxTunnelRegistry {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn register_cli(&self, session_id: Uuid, socket: WebSocket) {
        let transport = WebSocketTransport::new(socket);
        let client = Arc::new(TunnelClient::connect(transport));

        {
            let mut sessions = self.sessions.write().await;
            if sessions.insert(session_id, client.clone()).is_some() {
                warn!(
                    "Replacing existing Devbox CLI tunnel for session {}",
                    session_id
                );
            } else {
                info!("Registered Devbox CLI tunnel for session {}", session_id);
            }
        }

        let sessions = Arc::clone(&self.sessions);
        tokio::spawn(async move {
            client.closed().await;
            let mut sessions = sessions.write().await;
            sessions.remove(&session_id);
            info!("Devbox CLI tunnel closed for session {}", session_id);
        });
    }

    pub async fn attach_sidecar(
        &self,
        session_id: Uuid,
        socket: WebSocket,
    ) -> Result<(), TunnelError> {
        let cli_client = {
            let sessions = self.sessions.read().await;
            sessions.get(&session_id).cloned()
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
