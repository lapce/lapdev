use std::{
    pin::Pin,
    task::{Context, Poll},
};

use bytes::Bytes;
use futures::{
    stream::{SplitSink, SplitStream},
    Sink, Stream, StreamExt,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
};
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream};
use tokio_util::io::{CopyToBytes, SinkWriter, StreamReader};

pub struct WebSocketStream(pub tokio_tungstenite::WebSocketStream<MaybeTlsStream<TcpStream>>);

impl Stream for WebSocketStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match futures_util::ready!(self.0.poll_next_unpin(cx)) {
                Some(Ok(msg)) => {
                    if let Message::Binary(msg) = msg {
                        return Poll::Ready(Some(Ok(msg)));
                    }
                }
                Some(Err(err)) => return Poll::Ready(Some(Err(std::io::Error::other(err)))),
                None => return Poll::Ready(None),
            }
        }
    }
}

impl Sink<Bytes> for WebSocketStream {
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

pub struct WebSocketTransport {
    writer: SinkWriter<CopyToBytes<SplitSink<WebSocketStream, Bytes>>>,
    reader: StreamReader<SplitStream<WebSocketStream>, Bytes>,
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

impl WebSocketTransport {
    pub fn new(stream: tokio_tungstenite::WebSocketStream<MaybeTlsStream<TcpStream>>) -> Self {
        let (sink, stream) = WebSocketStream(stream).split();
        let reader = StreamReader::new(stream);
        let writer = SinkWriter::new(CopyToBytes::new(sink));
        Self { reader, writer }
    }
}
