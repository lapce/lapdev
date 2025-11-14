use std::{
    collections::{BTreeMap, HashMap},
    io,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use axum::extract::ws::{CloseFrame, Message as AxumMessage, Utf8Bytes, WebSocket};
use bytes::Bytes;
use futures::{future, Sink, SinkExt, Stream, StreamExt};
use lapdev_tunnel::{
    relay_client_addr, relay_server_addr, run_tunnel_server_with_connector, DynTunnelStream,
    RelayEndpoint, TunnelClient, TunnelError, TunnelTarget, WebSocketUdpSocket,
};
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite::{self as ws, Message as TungMessage};
use tracing::{info, warn};
use uuid::Uuid;

pub struct DevboxTunnelRegistry {
    sessions_by_user: Arc<RwLock<HashMap<Uuid, BTreeMap<u64, Arc<TunnelClient>>>>>,
    generation_counter: AtomicU64,
}

impl DevboxTunnelRegistry {
    pub fn new() -> Self {
        Self {
            sessions_by_user: Arc::new(RwLock::new(HashMap::new())),
            generation_counter: AtomicU64::new(1),
        }
    }

    fn next_generation(&self) -> u64 {
        self.generation_counter.fetch_add(1, Ordering::SeqCst)
    }

    pub async fn register_cli(&self, user_id: Uuid, token: String, socket: WebSocket) {
        let (sink, stream) = split_axum_websocket(socket);
        let udp_socket =
            WebSocketUdpSocket::from_parts(sink, stream, relay_server_addr(), relay_client_addr());

        let connection = match RelayEndpoint::udp_server_connection(udp_socket).await {
            Ok(connection) => connection,
            Err(err) => {
                warn!(
                    user_id = %user_id,
                    error = %err,
                    "Failed to perform QUIC handshake for CLI tunnel"
                );
                return;
            }
        };

        let client = Arc::new(TunnelClient::new_relay(connection));
        let generation = self.next_generation();

        {
            let mut sessions = self.sessions_by_user.write().await;
            let entry = sessions.entry(user_id).or_insert_with(BTreeMap::new);
            if entry.insert(generation, client.clone()).is_some() {
                warn!(
                    "Overwrote Devbox CLI tunnel generation {} for user {}",
                    generation, user_id
                );
            }
            info!(
                "Registered Devbox CLI tunnel for user {} generation {}; active entries {}",
                user_id,
                generation,
                entry.len()
            );
        }

        let sessions = Arc::clone(&self.sessions_by_user);
        tokio::spawn(async move {
            client.closed().await;
            let mut sessions = sessions.write().await;
            if let Some(entry) = sessions.get_mut(&user_id) {
                if entry.remove(&generation).is_some() {
                    info!(
                        "Devbox CLI tunnel closed for user {} generation {}; remaining {}",
                        user_id,
                        generation,
                        entry.len()
                    );
                    if entry.is_empty() {
                        sessions.remove(&user_id);
                    }
                } else {
                    info!(
                        "Skip removing Devbox CLI tunnel for user {} due to generation mismatch (requested {})",
                        user_id, generation
                    );
                }
            } else {
                info!(
                    "Devbox CLI tunnel cleanup found no user {} for generation {}",
                    user_id, generation
                );
            }
        });
    }

    pub async fn attach_sidecar(
        &self,
        user_id: Uuid,
        socket: WebSocket,
    ) -> Result<(), TunnelError> {
        let cli_client = {
            let sessions = self.sessions_by_user.read().await;
            sessions.get(&user_id).and_then(|entry| {
                entry
                    .values()
                    .rev()
                    .find(|client| !client.is_closed())
                    .cloned()
            })
        };

        let Some(cli_client) = cli_client else {
            warn!(
                "Received sidecar tunnel for user {} with no active CLI tunnel",
                user_id
            );
            return Err(TunnelError::Remote("no active CLI tunnel".to_string()));
        };

        let (sink, stream) = split_axum_websocket(socket);
        let udp_socket =
            WebSocketUdpSocket::from_parts(sink, stream, relay_server_addr(), relay_client_addr());

        let connection = RelayEndpoint::udp_server_connection(udp_socket).await?;
        let connector = move |target: TunnelTarget| {
            let cli_client = cli_client.clone();
            async move {
                let stream = cli_client
                    .connect_tcp(target.host.clone(), target.port)
                    .await?;
                Ok::<DynTunnelStream, TunnelError>(Box::new(stream) as DynTunnelStream)
            }
        };

        run_tunnel_server_with_connector(connection, connector).await
    }
}

pub(crate) fn split_axum_websocket(
    socket: WebSocket,
) -> (
    impl Sink<TungMessage, Error = io::Error> + Send + 'static,
    impl Stream<Item = Result<Bytes, io::Error>> + Send + 'static,
) {
    let (sink, stream) = socket.split();

    let sink = sink
        .sink_map_err(|err| io::Error::other(err))
        .with(|message: TungMessage| future::ready(map_tungstenite_to_axum(message)));

    let stream = stream.filter_map(|msg| {
        future::ready(match msg {
            Ok(AxumMessage::Binary(data)) => Some(Ok(data)),
            Ok(AxumMessage::Close(_)) => None,
            Ok(_) => None,
            Err(err) => Some(Err(io::Error::other(err))),
        })
    });

    (sink, stream)
}

fn map_tungstenite_to_axum(message: TungMessage) -> io::Result<AxumMessage> {
    match message {
        ws::Message::Text(text) => {
            let bytes: Bytes = text.into();
            let utf8 = Utf8Bytes::try_from(bytes)
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
            Ok(AxumMessage::Text(utf8))
        }
        ws::Message::Binary(data) => Ok(AxumMessage::Binary(data)),
        ws::Message::Ping(data) => Ok(AxumMessage::Ping(data)),
        ws::Message::Pong(data) => Ok(AxumMessage::Pong(data)),
        ws::Message::Close(frame) => {
            let mapped = frame.map(|close| CloseFrame {
                code: close.code.into(),
                reason: Utf8Bytes::from(close.reason.to_string()),
            });
            Ok(AxumMessage::Close(mapped))
        }
        ws::Message::Frame(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame messages are not supported",
        )),
    }
}

impl Default for DevboxTunnelRegistry {
    fn default() -> Self {
        Self::new()
    }
}
