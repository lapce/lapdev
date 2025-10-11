use std::{collections::HashMap, sync::Arc};

use bytes::Bytes;
use serde_json;
use futures::{SinkExt, StreamExt};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
    sync::{mpsc, watch, Mutex},
};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::{debug, error, warn};

use crate::{
    error::TunnelError,
    message::{Protocol, Target, WireMessage},
    util::spawn_detached,
};

type Connections = Arc<Mutex<HashMap<String, ServerConnection>>>;

#[derive(Clone)]
struct ServerConnection {
    write_tx: mpsc::UnboundedSender<Bytes>,
    shutdown_tx: watch::Sender<bool>,
}

/// Run a tunnel server on top of any async byte stream.
pub async fn run_tunnel_server<S>(stream: S) -> Result<(), TunnelError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let framed = Framed::new(stream, LengthDelimitedCodec::new());
    let (mut writer, mut reader) = framed.split();

    let (send_tx, mut send_rx) = mpsc::unbounded_channel::<WireMessage>();
    let connections: Connections = Arc::new(Mutex::new(HashMap::new()));

    let writer_task = tokio::spawn({
        let connections = connections.clone();
        async move {
            while let Some(message) = send_rx.recv().await {
                match serde_json::to_vec(&message) {
                    Ok(payload) => {
                        if let Err(err) = writer.send(Bytes::from(payload)).await {
                            error!("Failed to send tunnel frame: {}", err);
                            break;
                        }
                    }
                    Err(err) => {
                        error!("Failed to serialize tunnel frame: {}", err);
                    }
                }
            }

            // Ensure all connections are torn down when writer exits.
            let mut map = connections.lock().await;
            for (tunnel_id, conn) in map.drain() {
                drop(conn.write_tx);
                let _ = conn.shutdown_tx.send(true);
                let payload = serde_json::to_vec(&WireMessage::Close {
                    tunnel_id,
                    reason: Some("server shutdown".to_string()),
                })
                .unwrap_or_default();
                let _ = writer.send(Bytes::from(payload)).await;
            }
        }
    });

    while let Some(frame) = reader.next().await {
        match frame {
            Ok(bytes) => match serde_json::from_slice::<WireMessage>(&bytes) {
                Ok(WireMessage::Open {
                    tunnel_id,
                    protocol,
                    target,
                }) => {
                    handle_open(
                        &send_tx,
                        &connections,
                        tunnel_id,
                        protocol,
                        target,
                    )
                    .await;
                }
                Ok(WireMessage::Data { tunnel_id, payload }) => {
                    handle_data(&connections, tunnel_id, payload).await;
                }
                Ok(WireMessage::Close { tunnel_id, .. }) => {
                    terminate_connection(&connections, &tunnel_id).await;
                }
                Ok(WireMessage::OpenResult { .. }) => {
                    warn!("Server received unexpected OpenResult message");
                }
                Err(err) => {
                    error!("Failed to parse tunnel frame: {}", err);
                }
            },
            Err(err) => {
                error!("Tunnel server receive error: {}", err);
                break;
            }
        }
    }

    drop(send_tx);
    let _ = writer_task.await;
    Ok(())
}

async fn handle_open(
    send: &mpsc::UnboundedSender<WireMessage>,
    connections: &Connections,
    tunnel_id: String,
    protocol: Protocol,
    target: Target,
) {
    match protocol {
        Protocol::Tcp => {
            let address = format!("{}:{}", target.host, target.port);
            match TcpStream::connect(address).await {
                Ok(stream) => {
                    let (read_half, write_half) = stream.into_split();
                    let (write_tx, write_rx) = mpsc::unbounded_channel::<Bytes>();
                    let (shutdown_tx, _) = watch::channel(false);

                    {
                        let mut map = connections.lock().await;
                        map.insert(
                            tunnel_id.clone(),
                            ServerConnection {
                                write_tx: write_tx.clone(),
                                shutdown_tx: shutdown_tx.clone(),
                            },
                        );
                    }

                    if send
                        .send(WireMessage::OpenResult {
                            tunnel_id: tunnel_id.clone(),
                            success: true,
                            error: None,
                        })
                        .is_err()
                    {
                        debug!("Client dropped before acknowledging open");
                    }

                    spawn_conn_writer(
                        write_half,
                        write_rx,
                        shutdown_tx.subscribe(),
                        send.clone(),
                        tunnel_id.clone(),
                        connections.clone(),
                    );

                    spawn_conn_reader(
                        read_half,
                        shutdown_tx.subscribe(),
                        send.clone(),
                        tunnel_id,
                        connections.clone(),
                    );
                }
                Err(err) => {
                    let _ = send.send(WireMessage::OpenResult {
                        tunnel_id,
                        success: false,
                        error: Some(err.to_string()),
                    });
                }
            }
        }
        Protocol::Udp => {
            let _ = send.send(WireMessage::OpenResult {
                tunnel_id,
                success: false,
                error: Some("UDP tunneling not supported".to_string()),
            });
        }
    }
}

async fn handle_data(
    connections: &Connections,
    tunnel_id: String,
    payload: Vec<u8>,
) {
    let write_tx = {
        let map = connections.lock().await;
        map.get(&tunnel_id).map(|conn| conn.write_tx.clone())
    };

    if let Some(tx) = write_tx {
        if tx.send(Bytes::from(payload)).is_err() {
            debug!("Failed to dispatch data to tunnel {}", tunnel_id);
        }
    } else {
        debug!("Received data for unknown tunnel {}", tunnel_id);
    }
}

fn spawn_conn_writer(
    mut write_half: tokio::net::tcp::OwnedWriteHalf,
    mut data_rx: mpsc::UnboundedReceiver<Bytes>,
    mut shutdown_rx: watch::Receiver<bool>,
    send: mpsc::UnboundedSender<WireMessage>,
    tunnel_id: String,
    connections: Connections,
) {
    spawn_detached(async move {
        let mut close_reason: Option<String> = None;
        loop {
            tokio::select! {
                _ = shutdown_rx.changed(), if *shutdown_rx.borrow() => {
                    break;
                }
                maybe = data_rx.recv() => {
                    match maybe {
                        Some(bytes) => {
                            if let Err(err) = write_half.write_all(&bytes).await {
                                close_reason = Some(err.to_string());
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
        }

        if let Some(reason) = close_reason {
            if let Some(conn) = remove_connection(&connections, &tunnel_id).await {
                drop(conn.write_tx);
                let _ = conn.shutdown_tx.send(true);
            }
            let _ = send.send(WireMessage::Close {
                tunnel_id,
                reason: Some(reason),
            });
        }
    });
}

fn spawn_conn_reader(
    mut read_half: tokio::net::tcp::OwnedReadHalf,
    mut shutdown_rx: watch::Receiver<bool>,
    send: mpsc::UnboundedSender<WireMessage>,
    tunnel_id: String,
    connections: Connections,
) {
    spawn_detached(async move {
        let mut buffer = vec![0u8; 8192];
        let mut send_close = false;
        let mut close_reason: Option<String> = None;

        loop {
            tokio::select! {
                _ = shutdown_rx.changed(), if *shutdown_rx.borrow() => {
                    break;
                }
                result = read_half.read(&mut buffer) => {
                    match result {
                        Ok(0) => {
                            send_close = true;
                            break;
                        }
                        Ok(n) => {
                            if send.send(WireMessage::Data {
                                tunnel_id: tunnel_id.clone(),
                                payload: buffer[..n].to_vec(),
                            }).is_err() {
                                break;
                            }
                        }
                        Err(err) => {
                            send_close = true;
                            close_reason = Some(err.to_string());
                            break;
                        }
                    }
                }
            }
        }

        if send_close {
            if let Some(conn) = remove_connection(&connections, &tunnel_id).await {
                drop(conn.write_tx);
                let _ = conn.shutdown_tx.send(true);
            }
            let _ = send.send(WireMessage::Close {
                tunnel_id,
                reason: close_reason,
            });
        }
    });
}

async fn remove_connection(
    connections: &Connections,
    tunnel_id: &str,
) -> Option<ServerConnection> {
    let mut map = connections.lock().await;
    map.remove(tunnel_id)
}

async fn terminate_connection(connections: &Connections, tunnel_id: &str) {
    if let Some(conn) = remove_connection(connections, tunnel_id).await {
        drop(conn.write_tx);
        let _ = conn.shutdown_tx.send(true);
    }
}
