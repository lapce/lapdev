use std::{
    collections::VecDeque,
    fmt, io,
    net::SocketAddr,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll},
};

use bytes::Bytes;
use futures::{future, Sink, SinkExt, Stream, StreamExt};
use futures_util::task::AtomicWaker;
use quinn::{
    udp::{RecvMeta, Transmit},
    AsyncUdpSocket, UdpPoller,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    pin,
    sync::Notify,
};
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};

use crate::{
    direct::{
        build_server_config, client_config, establish_client_connection, read_token,
        write_server_ack, QuicTransport,
    },
    error::TunnelError,
};

const DEFAULT_BUFFER_CAPACITY: usize = 256;

pub fn relay_client_addr() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 60000))
}

pub fn relay_server_addr() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 60001))
}

/// Establish a QUIC client over a WebSocket stream.
impl QuicTransport {
    pub async fn connect_websocket_client<S>(
        websocket: WebSocketStream<S>,
        token: &str,
    ) -> Result<Self, TunnelError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let socket = WebSocketUdpSocket::from_tungstenite(
            websocket,
            relay_client_addr(),
            relay_server_addr(),
        );
        Self::connect_udp_client(socket, token).await
    }

    pub async fn connect_udp_client(
        socket: Arc<WebSocketUdpSocket>,
        token: &str,
    ) -> Result<Self, TunnelError> {
        quic_client_from_udp(socket, token).await
    }

    pub async fn accept_websocket_server<S>(
        websocket: WebSocketStream<S>,
        expected_token: &str,
    ) -> Result<Self, TunnelError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let socket = WebSocketUdpSocket::from_tungstenite(
            websocket,
            relay_server_addr(),
            relay_client_addr(),
        );
        Self::accept_udp_server(socket, expected_token).await
    }

    pub async fn accept_udp_server(
        socket: Arc<WebSocketUdpSocket>,
        expected_token: &str,
    ) -> Result<Self, TunnelError> {
        quic_server_from_udp(socket, expected_token).await
    }
}

async fn quic_client_from_udp(
    socket: Arc<WebSocketUdpSocket>,
    token: &str,
) -> Result<QuicTransport, TunnelError> {
    let runtime = quinn::default_runtime()
        .ok_or_else(|| TunnelError::Transport(io::Error::other("no async runtime found")))?;
    let mut endpoint = quinn::Endpoint::new_with_abstract_socket(
        quinn::EndpointConfig::default(),
        None,
        socket,
        runtime,
    )
    .map_err(|err| TunnelError::Transport(io::Error::other(err)))?;
    endpoint.set_default_client_config(client_config());
    let connecting = endpoint
        .connect(relay_server_addr(), "lapdev-relay")
        .map_err(|err| TunnelError::Transport(io::Error::other(err)))?;
    let connection = connecting
        .await
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;
    establish_client_connection(connection, token).await
}

async fn quic_server_from_udp(
    socket: Arc<WebSocketUdpSocket>,
    expected_token: &str,
) -> Result<QuicTransport, TunnelError> {
    let runtime = quinn::default_runtime()
        .ok_or_else(|| TunnelError::Transport(io::Error::other("no async runtime found")))?;
    let endpoint = quinn::Endpoint::new_with_abstract_socket(
        quinn::EndpointConfig::default(),
        Some(build_server_config()?),
        socket,
        runtime,
    )
    .map_err(|err| TunnelError::Transport(io::Error::other(err)))?;

    let incoming = endpoint
        .accept()
        .await
        .ok_or(TunnelError::ConnectionClosed)?;

    let connecting = incoming
        .accept()
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;
    let connection = connecting
        .await
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;
    let (mut send, mut recv) = connection
        .accept_bi()
        .await
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;

    let presented = read_token(&mut recv).await?;
    if presented != expected_token {
        return Err(TunnelError::Remote("invalid direct credential".into()));
    }

    write_server_ack(&mut send).await?;
    send.finish()
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;
    Ok(QuicTransport::new(connection))
}

/// Adapter that exposes a WebSocket as a virtual UDP socket suitable for Quinn.
pub struct WebSocketUdpSocket {
    send_buffer: Arc<SendBuffer>,
    recv_buffer: Arc<RecvBuffer>,
    local_addr: SocketAddr,
    peer_addr: SocketAddr,
}

impl fmt::Debug for WebSocketUdpSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebSocketUdpSocket")
            .field("local_addr", &self.local_addr)
            .field("peer_addr", &self.peer_addr)
            .finish()
    }
}

impl WebSocketUdpSocket {
    pub fn from_tungstenite<S>(
        websocket: WebSocketStream<S>,
        local_addr: SocketAddr,
        peer_addr: SocketAddr,
    ) -> Arc<Self>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        Self::from_tungstenite_with_capacity(
            websocket,
            local_addr,
            peer_addr,
            DEFAULT_BUFFER_CAPACITY,
        )
    }

    pub fn from_tungstenite_with_capacity<S>(
        websocket: WebSocketStream<S>,
        local_addr: SocketAddr,
        peer_addr: SocketAddr,
        capacity: usize,
    ) -> Arc<Self>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (sink, stream) = split_tungstenite(websocket);
        Self::with_capacity_parts(sink, stream, local_addr, peer_addr, capacity)
    }

    pub fn from_parts<SinkT, StreamT>(
        sink: SinkT,
        stream: StreamT,
        local_addr: SocketAddr,
        peer_addr: SocketAddr,
    ) -> Arc<Self>
    where
        SinkT: Sink<Bytes, Error = io::Error> + Send + 'static,
        StreamT: Stream<Item = Result<Bytes, io::Error>> + Send + 'static,
    {
        Self::with_capacity_parts(sink, stream, local_addr, peer_addr, DEFAULT_BUFFER_CAPACITY)
    }

    pub fn with_capacity_parts<SinkT, StreamT>(
        sink: SinkT,
        stream: StreamT,
        local_addr: SocketAddr,
        peer_addr: SocketAddr,
        capacity: usize,
    ) -> Arc<Self>
    where
        SinkT: Sink<Bytes, Error = io::Error> + Send + 'static,
        StreamT: Stream<Item = Result<Bytes, io::Error>> + Send + 'static,
    {
        let send_buffer = Arc::new(SendBuffer::new(capacity));
        let recv_buffer = Arc::new(RecvBuffer::new(capacity));

        tokio::spawn(run_sender_generic(sink, Arc::clone(&send_buffer)));
        tokio::spawn(run_receiver_generic(stream, Arc::clone(&recv_buffer)));

        Arc::new(Self {
            send_buffer,
            recv_buffer,
            local_addr,
            peer_addr,
        })
    }
}

impl Drop for WebSocketUdpSocket {
    fn drop(&mut self) {
        self.send_buffer.close();
        self.recv_buffer.close();
    }
}

impl AsyncUdpSocket for WebSocketUdpSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(WebSocketUdpPoller {
            send_buffer: Arc::clone(&self.send_buffer),
        })
    }

    fn try_send(&self, transmit: &Transmit) -> io::Result<()> {
        if transmit.segment_size.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "segmented transmits are not supported",
            ));
        }

        let bytes = Bytes::copy_from_slice(transmit.contents);
        self.send_buffer.enqueue(bytes)
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [io::IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        self.recv_buffer
            .poll_recv(cx, bufs, meta, self.peer_addr, self.local_addr)
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.local_addr)
    }

    fn max_transmit_segments(&self) -> usize {
        1
    }

    fn max_receive_segments(&self) -> usize {
        1
    }

    fn may_fragment(&self) -> bool {
        false
    }
}

struct WebSocketUdpPoller {
    send_buffer: Arc<SendBuffer>,
}

impl fmt::Debug for WebSocketUdpPoller {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebSocketUdpPoller").finish()
    }
}

impl UdpPoller for WebSocketUdpPoller {
    fn poll_writable(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.send_buffer.has_capacity() {
            Poll::Ready(Ok(()))
        } else {
            self.send_buffer.register_waker(cx);
            if self.send_buffer.has_capacity() {
                Poll::Ready(Ok(()))
            } else if self.send_buffer.is_closed() {
                Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::ConnectionReset,
                    "send buffer closed",
                )))
            } else {
                Poll::Pending
            }
        }
    }
}

async fn run_sender_generic<SinkT>(sink: SinkT, buffer: Arc<SendBuffer>)
where
    SinkT: Sink<Bytes, Error = io::Error> + Send + 'static,
{
    pin!(sink);

    while let Some(datagram) = buffer.next_datagram().await {
        if SinkExt::send(&mut sink, datagram).await.is_err() {
            break;
        }
    }

    buffer.close();
    let _ = SinkExt::close(&mut sink).await;
}

async fn run_receiver_generic<StreamT>(stream: StreamT, buffer: Arc<RecvBuffer>)
where
    StreamT: Stream<Item = Result<Bytes, io::Error>> + Send + 'static,
{
    pin!(stream);

    while let Some(result) = StreamExt::next(&mut stream).await {
        match result {
            Ok(data) => {
                if buffer.push(data).await.is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    buffer.close();
}

fn split_tungstenite<S>(
    websocket: WebSocketStream<S>,
) -> (
    impl Sink<Bytes, Error = io::Error> + Send + 'static,
    impl Stream<Item = Result<Bytes, io::Error>> + Send + 'static,
)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (sink, stream) = websocket.split();

    let sink = sink
        .sink_map_err(|err| io::Error::other(err))
        .with(|bytes: Bytes| future::ready(Ok(Message::Binary(bytes))));

    let stream = stream.filter_map(|msg| {
        future::ready(match msg {
            Ok(Message::Binary(data)) => Some(Ok(data)),
            Ok(Message::Close(_)) => None,
            Ok(_) => None,
            Err(err) => Some(Err(io::Error::other(err))),
        })
    });

    (sink, stream)
}

struct SendBuffer {
    queue: Mutex<VecDeque<Bytes>>,
    capacity: usize,
    data_ready: Notify,
    space_waker: AtomicWaker,
    closed: AtomicBool,
}

impl SendBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            queue: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
            data_ready: Notify::new(),
            space_waker: AtomicWaker::new(),
            closed: AtomicBool::new(false),
        }
    }

    fn enqueue(&self, bytes: Bytes) -> io::Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "send buffer closed",
            ));
        }

        let mut queue = self.queue.lock().unwrap();
        if queue.len() >= self.capacity {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "send buffer full",
            ));
        }
        queue.push_back(bytes);
        drop(queue);
        self.data_ready.notify_one();
        Ok(())
    }

    async fn next_datagram(&self) -> Option<Bytes> {
        loop {
            if let Some(datagram) = self.dequeue() {
                return Some(datagram);
            }

            if self.closed.load(Ordering::SeqCst) {
                return None;
            }

            self.data_ready.notified().await;
        }
    }

    fn dequeue(&self) -> Option<Bytes> {
        let mut queue = self.queue.lock().unwrap();
        let popped = queue.pop_front();
        if popped.is_some() {
            self.space_waker.wake();
        }
        popped
    }

    fn has_capacity(&self) -> bool {
        let queue = self.queue.lock().unwrap();
        queue.len() < self.capacity
    }

    fn register_waker(&self, cx: &mut Context<'_>) {
        self.space_waker.register(cx.waker());
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.data_ready.notify_waiters();
        self.space_waker.wake();
    }
}

struct RecvBuffer {
    queue: Mutex<VecDeque<Bytes>>,
    capacity: usize,
    space_available: Notify,
    data_waker: AtomicWaker,
    closed: AtomicBool,
}

impl RecvBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            queue: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
            space_available: Notify::new(),
            data_waker: AtomicWaker::new(),
            closed: AtomicBool::new(false),
        }
    }

    async fn push(&self, bytes: Bytes) -> io::Result<()> {
        let mut pending = Some(bytes);

        loop {
            if self.closed.load(Ordering::SeqCst) {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionReset,
                    "receive buffer closed",
                ));
            }

            if let Some(data) = pending.take() {
                let mut queue = self.queue.lock().unwrap();
                if queue.len() < self.capacity {
                    queue.push_back(data);
                    drop(queue);
                    self.data_waker.wake();
                    return Ok(());
                }
                pending = Some(data);
                drop(queue);
            }

            self.space_available.notified().await;
        }
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [io::IoSliceMut<'_>],
        meta: &mut [RecvMeta],
        peer_addr: SocketAddr,
        local_addr: SocketAddr,
    ) -> Poll<io::Result<usize>> {
        if bufs.is_empty() || meta.is_empty() {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "no receive buffers provided",
            )));
        }

        if let Some(bytes) = self.dequeue() {
            let target = bufs[0].as_mut();
            if target.len() < bytes.len() {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "buffer too small for datagram",
                )));
            }
            target[..bytes.len()].copy_from_slice(&bytes);
            meta[0] = RecvMeta {
                addr: peer_addr,
                len: bytes.len(),
                stride: bytes.len(),
                ecn: None,
                dst_ip: Some(local_addr.ip()),
            };
            return Poll::Ready(Ok(1));
        }

        if self.closed.load(Ordering::SeqCst) {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "websocket closed",
            )));
        }

        self.data_waker.register(cx.waker());
        Poll::Pending
    }

    fn dequeue(&self) -> Option<Bytes> {
        let mut queue = self.queue.lock().unwrap();
        let popped = queue.pop_front();
        if popped.is_some() {
            self.space_available.notify_one();
        }
        popped
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.space_available.notify_waiters();
        self.data_waker.wake();
    }
}
