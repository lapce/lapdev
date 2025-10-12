use std::{
    pin::Pin,
    task::{Context, Poll},
};

use axum::extract::ws::{Message, WebSocket};
use bytes::Bytes;
use futures::{
    stream::{SplitSink, SplitStream},
    Sink, Stream, StreamExt,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::io::{CopyToBytes, SinkWriter, StreamReader};

/// Adapter that exposes an Axum `WebSocket` as an `AsyncRead`/`AsyncWrite` transport.
pub struct WebSocketTransport {
    writer: SinkWriter<CopyToBytes<SplitSink<SocketStream, Bytes>>>,
    reader: StreamReader<SplitStream<SocketStream>, Bytes>,
}

impl WebSocketTransport {
    pub fn new(socket: WebSocket) -> Self {
        let (sink, stream) = SocketStream(socket).split();
        let reader = StreamReader::new(stream);
        let writer = SinkWriter::new(CopyToBytes::new(sink));
        Self { reader, writer }
    }
}

impl AsyncRead for WebSocketTransport {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        Pin::new(&mut this.reader).poll_read(cx, buf)
    }
}

impl AsyncWrite for WebSocketTransport {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        let this = self.get_mut();
        Pin::new(&mut this.writer).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        let this = self.get_mut();
        Pin::new(&mut this.writer).poll_flush(cx)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        let this = self.get_mut();
        Pin::new(&mut this.writer).poll_shutdown(cx)
    }
}

struct SocketStream(WebSocket);

impl Stream for SocketStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match futures_util::ready!(self.0.poll_next_unpin(cx)) {
                Some(Ok(Message::Binary(data))) => {
                    return Poll::Ready(Some(Ok(data)));
                }
                Some(Ok(Message::Close(_))) => {
                    return Poll::Ready(None);
                }
                Some(Ok(_)) => continue, // Ignore non-binary frames.
                Some(Err(err)) => {
                    return Poll::Ready(Some(Err(std::io::Error::other(err))));
                }
                None => {
                    return Poll::Ready(None);
                }
            }
        }
    }
}

impl Sink<Bytes> for SocketStream {
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
