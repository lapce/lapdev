use std::{collections::HashMap, future::Future, sync::Arc};

use bytes::Bytes;
use futures::{future::BoxFuture, SinkExt, StreamExt};
use serde_json;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
    sync::{mpsc, watch},
};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::{debug, error, warn};

use crate::{
    error::TunnelError,
    message::{Protocol, Target, WireMessage},
    util::spawn_detached,
    TunnelTarget,
};

#[derive(Clone)]
struct ConnectionManager {
    command_tx: mpsc::UnboundedSender<ConnectionCommand>,
}

#[derive(Debug)]
enum ConnectionCommand {
    Register {
        tunnel_id: String,
        connection: ServerConnection,
    },
    ForwardData {
        tunnel_id: String,
        payload: Vec<u8>,
    },
    Terminate {
        tunnel_id: String,
    },
    ConnectionClosed {
        tunnel_id: String,
        reason: Option<String>,
    },
    Shutdown {
        reason: Option<String>,
    },
}

#[derive(Clone, Debug)]
struct ServerConnection {
    write_tx: mpsc::UnboundedSender<Bytes>,
    shutdown_tx: watch::Sender<bool>,
}

impl ConnectionManager {
    fn new(send: mpsc::UnboundedSender<WireMessage>) -> Self {
        let (command_tx, mut command_rx) = mpsc::unbounded_channel();

        tokio::spawn(async move {
            let mut connections: HashMap<String, ServerConnection> = HashMap::new();
            while let Some(command) = command_rx.recv().await {
                match command {
                    ConnectionCommand::Register {
                        tunnel_id,
                        connection,
                    } => {
                        connections.insert(tunnel_id, connection);
                    }
                    ConnectionCommand::ForwardData { tunnel_id, payload } => {
                        match connections.get(&tunnel_id) {
                            Some(conn) => {
                                if conn.write_tx.send(Bytes::from(payload)).is_err() {
                                    if let Some(conn) = connections.remove(&tunnel_id) {
                                        finalize_connection(
                                            &send,
                                            tunnel_id,
                                            conn,
                                            Some("connection writer dropped".to_string()),
                                        );
                                    }
                                }
                            }
                            None => {
                                debug!("Received data for unknown tunnel {}", tunnel_id);
                            }
                        }
                    }
                    ConnectionCommand::Terminate { tunnel_id } => {
                        if let Some(conn) = connections.remove(&tunnel_id) {
                            drop(conn.write_tx);
                            let _ = conn.shutdown_tx.send(true);
                        }
                    }
                    ConnectionCommand::ConnectionClosed { tunnel_id, reason } => {
                        if let Some(conn) = connections.remove(&tunnel_id) {
                            finalize_connection(&send, tunnel_id, conn, reason);
                        }
                    }
                    ConnectionCommand::Shutdown { reason } => {
                        let reason = reason.unwrap_or_else(|| "server shutdown".to_string());
                        for (tunnel_id, conn) in connections.drain() {
                            finalize_connection(&send, tunnel_id, conn, Some(reason.clone()));
                        }
                        break;
                    }
                }
            }

            for (tunnel_id, conn) in connections.drain() {
                finalize_connection(&send, tunnel_id, conn, Some("server shutdown".to_string()));
            }
        });

        Self { command_tx }
    }

    fn register(&self, tunnel_id: String, connection: ServerConnection) {
        if self
            .command_tx
            .send(ConnectionCommand::Register {
                tunnel_id,
                connection,
            })
            .is_err()
        {
            debug!("Connection manager dropped register command");
        }
    }

    fn forward_data(&self, tunnel_id: String, payload: Vec<u8>) {
        if self
            .command_tx
            .send(ConnectionCommand::ForwardData { tunnel_id, payload })
            .is_err()
        {
            debug!("Connection manager dropped data command");
        }
    }

    fn terminate(&self, tunnel_id: String) {
        if self
            .command_tx
            .send(ConnectionCommand::Terminate { tunnel_id })
            .is_err()
        {
            debug!("Connection manager dropped terminate command");
        }
    }

    fn connection_closed(&self, tunnel_id: String, reason: Option<String>) {
        if self
            .command_tx
            .send(ConnectionCommand::ConnectionClosed { tunnel_id, reason })
            .is_err()
        {
            debug!("Connection manager dropped close command");
        }
    }

    fn shutdown(&self, reason: Option<String>) {
        let _ = self.command_tx.send(ConnectionCommand::Shutdown { reason });
    }
}

fn finalize_connection(
    send: &mpsc::UnboundedSender<WireMessage>,
    tunnel_id: String,
    conn: ServerConnection,
    reason: Option<String>,
) {
    drop(conn.write_tx);
    let _ = conn.shutdown_tx.send(true);
    let _ = send.send(WireMessage::Close { tunnel_id, reason });
}

pub trait TunnelStream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T> TunnelStream for T where T: AsyncRead + AsyncWrite + Unpin + Send {}

pub type DynTunnelStream = Box<dyn TunnelStream>;

pub trait TunnelConnector: Send + Sync + 'static {
    fn connect(
        &self,
        target: TunnelTarget,
    ) -> BoxFuture<'static, Result<DynTunnelStream, TunnelError>>;
}

#[derive(Clone, Default)]
pub struct TcpConnector;

impl TunnelConnector for TcpConnector {
    fn connect(
        &self,
        target: TunnelTarget,
    ) -> BoxFuture<'static, Result<DynTunnelStream, TunnelError>> {
        Box::pin(async move {
            let address = format!("{}:{}", target.host, target.port);
            let stream = TcpStream::connect(address).await?;
            Ok(Box::new(stream) as DynTunnelStream)
        })
    }
}

impl<F, Fut> TunnelConnector for F
where
    F: Fn(TunnelTarget) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<DynTunnelStream, TunnelError>> + Send + 'static,
{
    fn connect(
        &self,
        target: TunnelTarget,
    ) -> BoxFuture<'static, Result<DynTunnelStream, TunnelError>> {
        Box::pin((self)(target))
    }
}

/// Run a tunnel server on top of any async byte stream.
pub async fn run_tunnel_server<S>(stream: S) -> Result<(), TunnelError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    run_tunnel_server_with_connector(stream, TcpConnector::default()).await
}

/// Run a tunnel server using a custom connector for handling inbound tunnel requests.
pub async fn run_tunnel_server_with_connector<S, C>(
    stream: S,
    connector: C,
) -> Result<(), TunnelError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    C: TunnelConnector,
{
    let connector: Arc<dyn TunnelConnector> = Arc::new(connector);
    run_tunnel_server_inner(stream, connector).await
}

async fn run_tunnel_server_inner<S>(
    stream: S,
    connector: Arc<dyn TunnelConnector>,
) -> Result<(), TunnelError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let framed = Framed::new(stream, LengthDelimitedCodec::new());
    let (mut writer, mut reader) = framed.split();

    let (send_tx, mut send_rx) = mpsc::unbounded_channel::<WireMessage>();
    let manager = ConnectionManager::new(send_tx.clone());

    let writer_task = tokio::spawn({
        let manager = manager.clone();
        async move {
            while let Some(message) = send_rx.recv().await {
                match serde_json::to_vec(&message) {
                    Ok(payload) => {
                        if let Err(err) = writer.send(Bytes::from(payload)).await {
                            error!("Failed to send tunnel frame: {}", err);
                            manager.shutdown(Some(err.to_string()));
                            break;
                        }
                    }
                    Err(err) => {
                        error!("Failed to serialize tunnel frame: {}", err);
                    }
                }
            }

            if let Err(err) = writer.flush().await {
                debug!("Failed to flush tunnel writer: {}", err);
            }
        }
    });

    let mut shutdown_reason: Option<String> = None;

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
                        &manager,
                        connector.clone(),
                        tunnel_id,
                        protocol,
                        target,
                    )
                    .await;
                }
                Ok(WireMessage::Data { tunnel_id, payload }) => {
                    handle_data(&manager, tunnel_id, payload);
                }
                Ok(WireMessage::Close { tunnel_id, .. }) => {
                    terminate_connection(&manager, &tunnel_id);
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
                shutdown_reason = Some(err.to_string());
                break;
            }
        }
    }

    let reason = shutdown_reason.unwrap_or_else(|| "server shutdown".to_string());
    manager.shutdown(Some(reason));
    drop(send_tx);
    let _ = writer_task.await;
    Ok(())
}

async fn handle_open(
    send: &mpsc::UnboundedSender<WireMessage>,
    manager: &ConnectionManager,
    connector: Arc<dyn TunnelConnector>,
    tunnel_id: String,
    protocol: Protocol,
    target: Target,
) {
    match protocol {
        Protocol::Tcp => {
            let tunnel_target = TunnelTarget::new(target.host.clone(), target.port);
            match connector.connect(tunnel_target).await {
                Ok(stream) => {
                    let (read_half, write_half) = tokio::io::split(stream);
                    let (write_tx, write_rx) = mpsc::unbounded_channel::<Bytes>();
                    let (shutdown_tx, _) = watch::channel(false);

                    manager.register(
                        tunnel_id.clone(),
                        ServerConnection {
                            write_tx: write_tx.clone(),
                            shutdown_tx: shutdown_tx.clone(),
                        },
                    );

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
                        tunnel_id.clone(),
                        manager.clone(),
                    );

                    spawn_conn_reader(
                        read_half,
                        shutdown_tx.subscribe(),
                        send.clone(),
                        tunnel_id,
                        manager.clone(),
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

fn handle_data(manager: &ConnectionManager, tunnel_id: String, payload: Vec<u8>) {
    manager.forward_data(tunnel_id, payload);
}

fn spawn_conn_writer<W>(
    mut write_half: W,
    mut data_rx: mpsc::UnboundedReceiver<Bytes>,
    mut shutdown_rx: watch::Receiver<bool>,
    tunnel_id: String,
    manager: ConnectionManager,
) where
    W: AsyncWrite + Unpin + Send + 'static,
{
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
            manager.connection_closed(tunnel_id, Some(reason));
        }
    });
}

fn spawn_conn_reader<R>(
    mut read_half: R,
    mut shutdown_rx: watch::Receiver<bool>,
    send: mpsc::UnboundedSender<WireMessage>,
    tunnel_id: String,
    manager: ConnectionManager,
) where
    R: AsyncRead + Unpin + Send + 'static,
{
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
            manager.connection_closed(tunnel_id, close_reason);
        }
    });
}

fn terminate_connection(manager: &ConnectionManager, tunnel_id: &str) {
    manager.terminate(tunnel_id.to_string());
}
