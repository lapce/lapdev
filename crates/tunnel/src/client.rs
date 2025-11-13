use std::{
    fmt, io,
    pin::Pin,
    task::{Context, Poll},
};

use lapdev_common::devbox::DirectChannelConfig;
use quinn::{Connection, Endpoint, RecvStream, SendStream, WriteError};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_tungstenite::WebSocketStream;

use crate::{
    direct::{connect_direct_tunnel, DirectEndpoint, QuicTransport},
    error::TunnelError,
    message::{self, Protocol, TunnelTarget},
    RelayEndpoint,
};

/// Client capable of opening TCP tunnels by mapping each request to a native QUIC stream.
pub struct TunnelClient {
    connection: Connection,
    mode: TunnelMode,
}

impl fmt::Debug for TunnelClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TunnelClient")
            .field("mode", &self.mode)
            .finish()
    }
}

impl TunnelClient {
    pub fn new_direct(connection: Connection) -> Self {
        Self {
            connection,
            mode: TunnelMode::Direct,
        }
    }

    pub fn new_relay(connection: Connection) -> Self {
        Self {
            connection,
            mode: TunnelMode::Relay,
        }
    }

    pub fn connect_with_mode(transport: QuicTransport, mode: TunnelMode) -> Self {
        Self {
            connection: transport.into_connection(),
            mode,
        }
    }

    pub async fn connect_tcp(
        &self,
        target_host: impl Into<String>,
        target_port: u16,
    ) -> Result<TunnelTcpStream, TunnelError> {
        self.connect_with_protocol(Protocol::Tcp, TunnelTarget::new(target_host, target_port))
            .await
    }

    pub async fn connect_udp(
        &self,
        target_host: impl Into<String>,
        target_port: u16,
    ) -> Result<TunnelTcpStream, TunnelError> {
        self.connect_with_protocol(Protocol::Udp, TunnelTarget::new(target_host, target_port))
            .await
    }

    pub async fn connect_single(
        transport: QuicTransport,
        target_host: impl Into<String>,
        target_port: u16,
    ) -> Result<(Self, TunnelTcpStream), TunnelError> {
        let client = Self::connect_with_mode(transport, TunnelMode::Relay);
        let stream = client.connect_tcp(target_host, target_port).await?;
        Ok((client, stream))
    }

    pub async fn connect_with_direct_or_relay<S, F, Fut, G>(
        direct: Option<&DirectChannelConfig>,
        direct_endpoint: &DirectEndpoint,
        ws_connector: F,
        on_direct_failure: G,
    ) -> Result<(Self, TunnelMode), TunnelError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<WebSocketStream<S>, TunnelError>>,
        G: FnOnce(&TunnelError),
    {
        if let Some(config) = direct {
            let direct_result = direct_endpoint.connect_tunnel(config).await;

            match direct_result {
                Ok(client) => {
                    tracing::info!("Established direct tunnel connection");
                    return Ok((client, TunnelMode::Direct));
                }
                Err(err) => {
                    on_direct_failure(&err);
                    tracing::info!(
                        error = %err,
                        "Direct tunnel attempt failed; falling back to relay transport"
                    );
                }
            }
        }

        let websocket = ws_connector().await?;
        let connection = RelayEndpoint::client_connection(websocket).await?;
        tracing::info!("Using relay tunnel connection over QUIC");
        Ok((TunnelClient::new_relay(connection), TunnelMode::Relay))
    }

    pub fn mode(&self) -> TunnelMode {
        self.mode
    }

    pub fn is_closed(&self) -> bool {
        self.connection.close_reason().is_some()
    }

    pub async fn closed(&self) {
        let _ = self.connection.clone().closed().await;
    }

    async fn connect_with_protocol(
        &self,
        protocol: Protocol,
        target: TunnelTarget,
    ) -> Result<TunnelTcpStream, TunnelError> {
        let (mut send, mut recv) = self
            .connection
            .clone()
            .open_bi()
            .await
            .map_err(to_transport_err)?;

        message::write_stream_open_request(&mut send, protocol, &target).await?;
        let response = message::read_stream_open_response(&mut recv).await?;
        if !response.success {
            let reason = response
                .error
                .unwrap_or_else(|| "remote open failed".to_string());
            return Err(TunnelError::Remote(reason));
        }

        Ok(TunnelTcpStream::new(send, recv))
    }
}

impl Drop for TunnelClient {
    fn drop(&mut self) {
        self.connection.close(0u32.into(), b"client dropped");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelMode {
    Direct,
    Relay,
}

/// Convenience wrapper owning both the tunnel client and an established TCP tunnel.
pub struct TunnelTcpConnection {
    pub client: TunnelClient,
    pub stream: TunnelTcpStream,
}

impl TunnelTcpConnection {
    pub fn new(client: TunnelClient, stream: TunnelTcpStream) -> Self {
        Self { client, stream }
    }
}

pub struct TunnelTcpStream {
    send: SendStream,
    recv: RecvStream,
}

impl fmt::Debug for TunnelTcpStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TunnelTcpStream").finish()
    }
}

impl Unpin for TunnelTcpStream {}

impl TunnelTcpStream {
    fn new(send: SendStream, recv: RecvStream) -> Self {
        Self { send, recv }
    }
}

impl AsyncRead for TunnelTcpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.recv).poll_read(cx, buf)
    }
}

impl AsyncWrite for TunnelTcpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        map_write_poll(Pin::new(&mut self.send).poll_write(cx, buf))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.send).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.send).poll_shutdown(cx)
    }
}

fn to_transport_err(err: impl Into<io::Error>) -> TunnelError {
    TunnelError::Transport(err.into())
}

fn map_write_poll<T>(poll: Poll<Result<T, WriteError>>) -> Poll<io::Result<T>> {
    match poll {
        Poll::Ready(Ok(value)) => Poll::Ready(Ok(value)),
        Poll::Ready(Err(err)) => Poll::Ready(Err(io::Error::other(err))),
        Poll::Pending => Poll::Pending,
    }
}
