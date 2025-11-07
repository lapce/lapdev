use std::{
    collections::HashMap,
    fmt,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use bytes::{Bytes, BytesMut};
use futures::{SinkExt, StreamExt};
use tokio::{
    io::{self, AsyncRead, AsyncWrite},
    sync::{mpsc, oneshot, Notify},
    task::JoinHandle,
    time::{Duration, MissedTickBehavior},
};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::{debug, error, trace};
use uuid::Uuid;

use crate::{
    error::TunnelError,
    message::{Protocol, Target, TunnelTarget, WireMessage},
    util::spawn_detached,
};

enum InnerCommand {
    InsertPending {
        tunnel_id: String,
        sender: oneshot::Sender<Result<(), TunnelError>>,
    },
    ResolvePending {
        tunnel_id: String,
        result: Result<(), TunnelError>,
    },
    RegisterStream {
        tunnel_id: String,
        sender: mpsc::UnboundedSender<Bytes>,
    },
    ForwardData {
        tunnel_id: String,
        payload: Vec<u8>,
    },
    CloseStream {
        tunnel_id: String,
    },
    FailAll {
        reason: String,
    },
}

struct Inner {
    send: mpsc::UnboundedSender<WireMessage>,
    command_tx: mpsc::UnboundedSender<InnerCommand>,
}

impl Inner {
    fn new(send: mpsc::UnboundedSender<WireMessage>) -> Self {
        let (command_tx, mut command_rx) = mpsc::unbounded_channel();

        tokio::spawn(async move {
            let mut pending: HashMap<String, oneshot::Sender<Result<(), TunnelError>>> =
                HashMap::new();
            let mut streams: HashMap<String, mpsc::UnboundedSender<Bytes>> = HashMap::new();

            while let Some(command) = command_rx.recv().await {
                match command {
                    InnerCommand::InsertPending { tunnel_id, sender } => {
                        pending.insert(tunnel_id, sender);
                    }
                    InnerCommand::ResolvePending { tunnel_id, result } => {
                        if let Some(sender) = pending.remove(&tunnel_id) {
                            let _ = sender.send(result);
                        }
                    }
                    InnerCommand::RegisterStream { tunnel_id, sender } => {
                        streams.insert(tunnel_id, sender);
                    }
                    InnerCommand::ForwardData { tunnel_id, payload } => {
                        match streams.get(&tunnel_id) {
                            Some(sender) => {
                                if sender.send(Bytes::from(payload)).is_err() {
                                    debug!("Failed to forward data for tunnel {}", tunnel_id);
                                }
                            }
                            None => {
                                debug!("Received data for unknown tunnel {}", tunnel_id);
                            }
                        }
                    }
                    InnerCommand::CloseStream { tunnel_id } => {
                        streams.remove(&tunnel_id);
                    }
                    InnerCommand::FailAll { reason } => {
                        for (_, sender) in pending.drain() {
                            let _ = sender.send(Err(TunnelError::Remote(reason.clone())));
                        }
                        streams.clear();
                    }
                }
            }

            for (_, sender) in pending.drain() {
                let _ = sender.send(Err(TunnelError::ConnectionClosed));
            }
            streams.clear();
        });

        Self { send, command_tx }
    }

    fn send_message(&self, message: WireMessage) -> Result<(), TunnelError> {
        self.send
            .send(message)
            .map_err(|_| TunnelError::ConnectionClosed)
    }

    async fn insert_pending(
        &self,
        tunnel_id: String,
        tx: oneshot::Sender<Result<(), TunnelError>>,
    ) {
        if let Err(err) = self.command_tx.send(InnerCommand::InsertPending {
            tunnel_id,
            sender: tx,
        }) {
            if let InnerCommand::InsertPending { sender, .. } = err.0 {
                let _ = sender.send(Err(TunnelError::ConnectionClosed));
            }
        }
    }

    async fn resolve_pending(&self, tunnel_id: &str, result: Result<(), TunnelError>) {
        if self
            .command_tx
            .send(InnerCommand::ResolvePending {
                tunnel_id: tunnel_id.to_string(),
                result,
            })
            .is_err()
        {
            debug!(
                "Failed to resolve pending tunnel {}; manager dropped",
                tunnel_id
            );
        }
    }

    async fn register_stream(&self, tunnel_id: String, tx: mpsc::UnboundedSender<Bytes>) {
        if self
            .command_tx
            .send(InnerCommand::RegisterStream {
                tunnel_id,
                sender: tx,
            })
            .is_err()
        {
            debug!("Failed to register stream; manager dropped");
        }
    }

    async fn forward_data(&self, tunnel_id: &str, payload: Vec<u8>) {
        if self
            .command_tx
            .send(InnerCommand::ForwardData {
                tunnel_id: tunnel_id.to_string(),
                payload,
            })
            .is_err()
        {
            debug!(
                "Failed to enqueue data for tunnel {}; manager dropped",
                tunnel_id
            );
        }
    }

    async fn close_stream(&self, tunnel_id: &str) {
        if self
            .command_tx
            .send(InnerCommand::CloseStream {
                tunnel_id: tunnel_id.to_string(),
            })
            .is_err()
        {
            debug!("Failed to close stream {}; manager dropped", tunnel_id);
        }
    }

    async fn fail_all(&self, reason: impl Into<String>) {
        if self
            .command_tx
            .send(InnerCommand::FailAll {
                reason: reason.into(),
            })
            .is_err()
        {
            debug!("Failed to fail pending tunnels; manager dropped");
        }
    }
}

const TUNNEL_HEARTBEAT_INTERVAL_SECS: u64 = 30;

/// Client capable of multiplexing TCP streams over an abstract transport.
pub struct TunnelClient {
    inner: Arc<Inner>,
    writer_task: JoinHandle<()>,
    reader_task: JoinHandle<()>,
    close_notify: Arc<Notify>,
}

impl fmt::Debug for TunnelClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TunnelClient").finish()
    }
}

impl TunnelClient {
    /// Establish a new tunnel client using any async byte stream.
    pub fn connect<S>(stream: S) -> Self
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let framed = Framed::new(stream, LengthDelimitedCodec::new());
        let (mut writer, mut reader) = framed.split();

        let (send_tx, mut send_rx) = mpsc::unbounded_channel::<WireMessage>();
        let inner = Arc::new(Inner::new(send_tx));
        let close_notify = Arc::new(Notify::new());

        let writer_task = tokio::spawn({
            let inner = Arc::clone(&inner);
            let notify = Arc::clone(&close_notify);
            async move {
                while let Some(message) = send_rx.recv().await {
                    match serde_json::to_vec(&message) {
                        Ok(payload) => {
                            if let Err(err) = writer.send(Bytes::from(payload)).await {
                                error!("Failed to send tunnel message: {}", err);
                                inner.fail_all(err.to_string()).await;
                                break;
                            }
                        }
                        Err(err) => {
                            error!("Failed to serialize tunnel message: {}", err);
                        }
                    }
                }

                if let Err(err) = writer.flush().await {
                    debug!("Failed to flush tunnel writer: {}", err);
                }

                notify.notify_waiters();
            }
        });

        let reader_inner = inner.clone();
        let reader_notify = Arc::clone(&close_notify);
        let reader_task = tokio::spawn(async move {
            while let Some(result) = reader.next().await {
                match result {
                    Ok(bytes) => {
                        handle_incoming(&reader_inner, bytes.freeze()).await;
                    }
                    Err(err) => {
                        error!("Tunnel transport receive error: {}", err);
                        reader_inner.fail_all(err.to_string()).await;
                        break;
                    }
                }
            }

            reader_inner.fail_all("connection closed").await;
            reader_notify.notify_waiters();
        });

        let heartbeat_inner = Arc::clone(&inner);
        let heartbeat_notify = Arc::clone(&close_notify);
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(Duration::from_secs(TUNNEL_HEARTBEAT_INTERVAL_SECS));
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    _ = heartbeat_notify.notified() => {
                        trace!("Tunnel heartbeat task exiting due to shutdown");
                        break;
                    }
                    _ = interval.tick() => {
                        if heartbeat_inner.send_message(WireMessage::Heartbeat).is_err() {
                            trace!("Tunnel heartbeat task stopping; connection closed");
                            break;
                        }
                    }
                }
            }
        });

        Self {
            inner,
            writer_task,
            reader_task,
            close_notify,
        }
    }

    /// Returns true if the underlying transport tasks have terminated.
    pub fn is_closed(&self) -> bool {
        self.writer_task.is_finished() || self.reader_task.is_finished()
    }

    /// Wait until the underlying transport has closed.
    pub async fn closed(&self) {
        if self.is_closed() {
            return;
        }
        self.close_notify.notified().await;
    }

    /// Open a TCP tunnel to the specified host and port.
    pub async fn connect_tcp(
        &self,
        target_host: impl Into<String>,
        target_port: u16,
    ) -> Result<TunnelTcpStream, TunnelError> {
        self.connect_with_protocol(Protocol::Tcp, TunnelTarget::new(target_host, target_port))
            .await
    }

    /// Open a UDP tunnel to the specified host and port.
    pub async fn connect_udp(
        &self,
        target_host: impl Into<String>,
        target_port: u16,
    ) -> Result<TunnelTcpStream, TunnelError> {
        self.connect_with_protocol(Protocol::Udp, TunnelTarget::new(target_host, target_port))
            .await
    }

    /// Connect to the endpoint and establish a single TCP tunnel, returning both the client and stream.
    pub async fn connect_single<S>(
        stream: S,
        target_host: impl Into<String>,
        target_port: u16,
    ) -> Result<(Self, TunnelTcpStream), TunnelError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let client = Self::connect(stream);
        let stream = client.connect_tcp(target_host, target_port).await?;
        Ok((client, stream))
    }

    async fn connect_with_protocol(
        &self,
        protocol: Protocol,
        target: TunnelTarget,
    ) -> Result<TunnelTcpStream, TunnelError> {
        let tunnel_id = format!("tunnel-{}", Uuid::new_v4());
        let (open_tx, open_rx) = oneshot::channel();
        let (data_tx, data_rx) = mpsc::unbounded_channel();

        self.inner.register_stream(tunnel_id.clone(), data_tx).await;
        self.inner.insert_pending(tunnel_id.clone(), open_tx).await;

        self.inner.send_message(WireMessage::Open {
            tunnel_id: tunnel_id.clone(),
            protocol,
            target: Target {
                host: target.host,
                port: target.port,
            },
        })?;

        match open_rx.await {
            Ok(Ok(())) => Ok(TunnelTcpStream::new(tunnel_id, self.inner.clone(), data_rx)),
            Ok(Err(err)) => {
                self.inner.close_stream(&tunnel_id).await;
                Err(err)
            }
            Err(_) => {
                self.inner.close_stream(&tunnel_id).await;
                Err(TunnelError::ConnectionClosed)
            }
        }
    }
}

impl Drop for TunnelClient {
    fn drop(&mut self) {
        let inner = self.inner.clone();
        spawn_detached(async move {
            inner.fail_all("client dropped").await;
        });
        self.writer_task.abort();
        self.reader_task.abort();
        self.close_notify.notify_waiters();
    }
}

/// Convenience wrapper owning both the tunnel client and the TCP stream.
pub struct TunnelTcpConnection {
    pub client: TunnelClient,
    pub stream: TunnelTcpStream,
}

impl TunnelTcpConnection {
    pub async fn connect<S>(
        transport: S,
        target_host: impl Into<String>,
        target_port: u16,
    ) -> Result<Self, TunnelError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (client, stream) =
            TunnelClient::connect_single(transport, target_host, target_port).await?;
        Ok(Self { client, stream })
    }
}

/// A stream-like object that behaves similarly to `TcpStream`, but forwards bytes over a tunnel.
pub struct TunnelTcpStream {
    tunnel_id: String,
    inner: Arc<Inner>,
    read_rx: mpsc::UnboundedReceiver<Bytes>,
    read_buffer: BytesMut,
    shutdown_sent: bool,
}

impl fmt::Debug for TunnelTcpStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TunnelTcpStream")
            .field("tunnel_id", &self.tunnel_id)
            .finish()
    }
}

impl TunnelTcpStream {
    fn new(tunnel_id: String, inner: Arc<Inner>, read_rx: mpsc::UnboundedReceiver<Bytes>) -> Self {
        Self {
            tunnel_id,
            inner,
            read_rx,
            read_buffer: BytesMut::with_capacity(8192),
            shutdown_sent: false,
        }
    }

    fn send_close(&mut self, reason: Option<String>) {
        if self.shutdown_sent {
            return;
        }
        self.shutdown_sent = true;
        let tunnel_id = self.tunnel_id.clone();
        let inner = self.inner.clone();
        spawn_detached(async move {
            inner.close_stream(&tunnel_id).await;
        });
        let message = WireMessage::Close {
            tunnel_id: self.tunnel_id.clone(),
            reason,
        };
        if let Err(err) = self.inner.send_message(message) {
            debug!(
                "Failed to send tunnel close for {}: {}",
                self.tunnel_id, err
            );
        }
    }
}

impl AsyncRead for TunnelTcpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if !self.read_buffer.is_empty() {
            let to_copy = std::cmp::min(buf.remaining(), self.read_buffer.len());
            buf.put_slice(&self.read_buffer.split_to(to_copy));
            return Poll::Ready(Ok(()));
        }

        match Pin::new(&mut self.read_rx).poll_recv(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(bytes)) => {
                if bytes.is_empty() {
                    return Poll::Ready(Ok(()));
                }

                self.read_buffer.extend_from_slice(&bytes);
                let to_copy = std::cmp::min(buf.remaining(), self.read_buffer.len());
                buf.put_slice(&self.read_buffer.split_to(to_copy));
                Poll::Ready(Ok(()))
            }
            Poll::Ready(None) => Poll::Ready(Ok(())),
        }
    }
}

impl AsyncWrite for TunnelTcpStream {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.shutdown_sent {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "tunnel is closed",
            )));
        }

        let payload = data.to_vec();
        match self.inner.send_message(WireMessage::Data {
            tunnel_id: self.tunnel_id.clone(),
            payload,
        }) {
            Ok(()) => Poll::Ready(Ok(data.len())),
            Err(err) => Poll::Ready(Err(io::Error::from(err))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.send_close(Some("shutdown".to_string()));
        Poll::Ready(Ok(()))
    }
}

async fn handle_incoming(inner: &Arc<Inner>, payload: Bytes) {
    match serde_json::from_slice::<WireMessage>(&payload) {
        Ok(WireMessage::OpenResult {
            tunnel_id,
            success,
            error,
        }) => {
            if success {
                inner.resolve_pending(&tunnel_id, Ok(())).await;
            } else {
                inner
                    .resolve_pending(
                        &tunnel_id,
                        Err(TunnelError::Remote(
                            error.unwrap_or_else(|| "remote open failed".to_string()),
                        )),
                    )
                    .await;
                inner.close_stream(&tunnel_id).await;
            }
        }
        Ok(WireMessage::Data { tunnel_id, payload }) => {
            inner.forward_data(&tunnel_id, payload).await;
        }
        Ok(WireMessage::Close { tunnel_id, reason }) => {
            if let Some(reason) = reason {
                inner
                    .resolve_pending(&tunnel_id, Err(TunnelError::Remote(reason.clone())))
                    .await;
            }
            inner.close_stream(&tunnel_id).await;
        }
        Ok(WireMessage::Heartbeat) => {
            // No-op; keep-alive frame.
        }
        Ok(WireMessage::Open { .. }) => {
            debug!("Received unexpected Open message from server");
        }
        Err(err) => {
            error!("Failed to parse tunnel message: {}", err);
        }
    }
}
