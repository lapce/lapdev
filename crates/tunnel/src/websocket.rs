use std::{
    pin::Pin,
    task::{Context, Poll},
};

use bytes::Bytes;
use futures::{
    stream::{SplitSink, SplitStream},
    Sink, Stream, StreamExt,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};
use tokio_util::io::{CopyToBytes, SinkWriter, StreamReader};

/// Adapter that exposes a WebSocket as an AsyncRead/AsyncWrite transport.
pub struct WebSocketTransport<S> {
    writer: SinkWriter<CopyToBytes<SplitSink<SocketStream<S>, Bytes>>>,
    reader: StreamReader<SplitStream<SocketStream<S>>, Bytes>,
}

impl<S> WebSocketTransport<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    pub fn new(socket: WebSocketStream<S>) -> Self {
        let (sink, stream) = SocketStream(socket).split();
        let reader = StreamReader::new(stream);
        let writer = SinkWriter::new(CopyToBytes::new(sink));
        Self { reader, writer }
    }
}

impl<S> AsyncRead for WebSocketTransport<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        Pin::new(&mut this.reader).poll_read(cx, buf)
    }
}

impl<S> AsyncWrite for WebSocketTransport<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        Pin::new(&mut this.writer).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        Pin::new(&mut this.writer).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        Pin::new(&mut this.writer).poll_shutdown(cx)
    }
}

struct SocketStream<S>(WebSocketStream<S>);

impl<S> Stream for SocketStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match futures_util::ready!(self.0.poll_next_unpin(cx)) {
                Some(Ok(Message::Binary(data))) => {
                    return Poll::Ready(Some(Ok(Bytes::from(data))));
                }
                Some(Ok(Message::Text(_))) => {
                    // Ignore non-binary frames.
                    continue;
                }
                Some(Ok(Message::Close(_))) => return Poll::Ready(None),
                Some(Ok(_)) => continue,
                Some(Err(err)) => {
                    return Poll::Ready(Some(Err(std::io::Error::other(err))));
                }
                None => return Poll::Ready(None),
            }
        }
    }
}

impl<S> Sink<Bytes> for SocketStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    type Error = std::io::Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.0)
            .poll_ready(cx)
            .map_err(std::io::Error::other)
    }

    fn start_send(mut self: Pin<&mut Self>, item: Bytes) -> Result<(), Self::Error> {
        Pin::new(&mut self.0)
            .start_send(Message::Binary(item))
            .map_err(std::io::Error::other)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.0)
            .poll_flush(cx)
            .map_err(std::io::Error::other)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.0)
            .poll_close(cx)
            .map_err(std::io::Error::other)
    }
}
