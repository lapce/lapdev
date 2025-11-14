use std::{
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use futures::future::BoxFuture;
use quinn::{Connection, RecvStream, SendStream, WriteError};
use tokio::{
    io::{self as tokio_io, AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
};
use tracing::{debug, warn};

use crate::{
    error::TunnelError,
    message::{self, Protocol, TunnelTarget},
};

const UNSUPPORTED_PROTOCOL: &str = "protocol not supported";

pub trait TunnelStream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T> TunnelStream for T where T: AsyncRead + AsyncWrite + Unpin + Send {}

pub type DynTunnelStream = Box<dyn TunnelStream>;

pub trait TunnelConnector: Send + Sync + 'static {
    fn connect(
        &self,
        target: TunnelTarget,
    ) -> BoxFuture<'static, Result<DynTunnelStream, TunnelError>>;
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

pub async fn run_tunnel_server(connection: Connection) -> Result<(), TunnelError> {
    run_tunnel_server_with_connector(connection, TcpConnector::default()).await
}

pub async fn run_tunnel_server_with_connector<C>(
    connection: Connection,
    connector: C,
) -> Result<(), TunnelError>
where
    C: TunnelConnector,
{
    run_tunnel_server_inner(connection, Arc::new(connector)).await
}

async fn run_tunnel_server_inner(
    connection: Connection,
    connector: Arc<dyn TunnelConnector>,
) -> Result<(), TunnelError> {
    loop {
        match connection.accept_bi().await {
            Ok((send, recv)) => {
                let connector = connector.clone();
                tokio::spawn(async move {
                    if let Err(err) = handle_stream(connector, send, recv).await {
                        debug!("tunnel stream ended: {}", err);
                    }
                });
            }
            Err(quinn::ConnectionError::ApplicationClosed(_))
            | Err(quinn::ConnectionError::ConnectionClosed(_)) => {
                return Ok(());
            }
            Err(err) => {
                return Err(TunnelError::Transport(io::Error::other(err)));
            }
        }
    }
}

async fn handle_stream(
    connector: Arc<dyn TunnelConnector>,
    mut send: SendStream,
    mut recv: RecvStream,
) -> Result<(), TunnelError> {
    let request = match message::read_stream_open_request(&mut recv).await {
        Ok(request) => request,
        Err(err) => {
            warn!("failed to parse stream request: {}", err);
            let _ =
                message::write_stream_open_response(&mut send, false, Some("bad request")).await;
            return Err(err);
        }
    };

    if request.protocol != Protocol::Tcp {
        message::write_stream_open_response(&mut send, false, Some(UNSUPPORTED_PROTOCOL)).await?;
        return Err(TunnelError::Remote(UNSUPPORTED_PROTOCOL.into()));
    }

    let target = request.target.clone();
    let stream = match connector.connect(target.clone()).await {
        Ok(stream) => stream,
        Err(err) => {
            let reason = err.to_string();
            message::write_stream_open_response(&mut send, false, Some(&reason)).await?;
            return Err(err);
        }
    };

    message::write_stream_open_response(&mut send, true, None).await?;
    pipe_streams(send, recv, stream).await
}

async fn pipe_streams(
    send: SendStream,
    recv: RecvStream,
    mut target: DynTunnelStream,
) -> Result<(), TunnelError> {
    let mut quic_stream = QuicServerStream::new(send, recv);

    tokio_io::copy_bidirectional(&mut quic_stream, &mut target)
        .await
        .map_err(|err| TunnelError::Transport(err))?;
    Ok(())
}

struct QuicServerStream {
    send: SendStream,
    recv: RecvStream,
}

impl QuicServerStream {
    fn new(send: SendStream, recv: RecvStream) -> Self {
        Self { send, recv }
    }
}

impl Unpin for QuicServerStream {}

impl AsyncRead for QuicServerStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.recv).poll_read(cx, buf)
    }
}

impl AsyncWrite for QuicServerStream {
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

fn map_write_poll<T>(poll: Poll<Result<T, WriteError>>) -> Poll<io::Result<T>> {
    match poll {
        Poll::Ready(Ok(value)) => Poll::Ready(Ok(value)),
        Poll::Ready(Err(err)) => Poll::Ready(Err(io::Error::other(err))),
        Poll::Pending => Poll::Pending,
    }
}
