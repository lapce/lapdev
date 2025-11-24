use std::{
    io,
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll},
};

use bytes::{Bytes, BytesMut};
use futures::{Sink, Stream};
use futures_util::StreamExt;
use pin_project::pin_project;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_serde::{Deserializer, Serializer};
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};

/// Builds a serde transport directly on top of a WebSocket stream.
pub fn websocket_serde_transport<S, Item, SinkItem, Codec>(
    socket: WebSocketStream<S>,
    codec: Codec,
) -> WebSocketTransport<SocketStream<S>, Item, SinkItem, Codec>
where
    S: AsyncRead + AsyncWrite + Unpin,
    Item: for<'de> Deserialize<'de>,
    SinkItem: Serialize,
    Codec: Serializer<SinkItem> + Deserializer<Item>,
{
    WebSocketTransport {
        inner: SocketStream(socket),
        codec,
        _marker: PhantomData,
    }
}

/// Builds a serde transport from any socket that transmits binary websocket frames.
pub fn websocket_serde_transport_from_socket<Socket, Item, SinkItem, Codec>(
    socket: Socket,
    codec: Codec,
) -> WebSocketTransport<Socket, Item, SinkItem, Codec>
where
    Socket: Stream<Item = Result<Bytes, io::Error>> + Sink<Bytes, Error = io::Error>,
    Socket: Unpin,
    Item: for<'de> Deserialize<'de>,
    SinkItem: Serialize,
    Codec: Serializer<SinkItem> + Deserializer<Item>,
{
    WebSocketTransport {
        inner: socket,
        codec,
        _marker: PhantomData,
    }
}

#[pin_project]
pub struct WebSocketTransport<Socket, Item, SinkItem, Codec> {
    #[pin]
    inner: Socket,
    #[pin]
    codec: Codec,
    _marker: PhantomData<(Item, SinkItem)>,
}

impl<Socket, Item, SinkItem, Codec, CodecError> Stream
    for WebSocketTransport<Socket, Item, SinkItem, Codec>
where
    Socket: Stream<Item = Result<Bytes, io::Error>> + Sink<Bytes, Error = io::Error>,
    Socket: Unpin,
    Item: for<'de> Deserialize<'de>,
    SinkItem: Serialize,
    Codec: Deserializer<Item, Error = CodecError> + Serializer<SinkItem, Error = CodecError>,
    CodecError: std::error::Error + Send + Sync + 'static,
{
    type Item = io::Result<Item>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        match futures::ready!(this.inner.as_mut().poll_next(cx)) {
            Some(Ok(bytes)) => {
                let buf = BytesMut::from(bytes.as_ref());
                match this.codec.as_mut().deserialize(&buf) {
                    Ok(item) => Poll::Ready(Some(Ok(item))),
                    Err(err) => Poll::Ready(Some(Err(io::Error::other(err)))),
                }
            }
            Some(Err(err)) => Poll::Ready(Some(Err(err))),
            None => Poll::Ready(None),
        }
    }
}

impl<Socket, Item, SinkItem, Codec, CodecError> Sink<SinkItem>
    for WebSocketTransport<Socket, Item, SinkItem, Codec>
where
    Socket: Stream<Item = Result<Bytes, io::Error>> + Sink<Bytes, Error = io::Error>,
    Socket: Unpin,
    Item: for<'de> Deserialize<'de>,
    SinkItem: Serialize,
    Codec: Deserializer<Item, Error = CodecError> + Serializer<SinkItem, Error = CodecError>,
    CodecError: std::error::Error + Send + Sync + 'static,
{
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project().inner.poll_ready(cx)
    }

    fn start_send(self: Pin<&mut Self>, item: SinkItem) -> io::Result<()> {
        let mut this = self.project();
        let bytes = this
            .codec
            .as_mut()
            .serialize(&item)
            .map_err(|err| io::Error::other(err))?;
        this.inner.start_send(bytes)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project().inner.poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project().inner.poll_close(cx)
    }
}

#[pin_project]
pub struct SocketStream<S>(#[pin] WebSocketStream<S>);

impl<S> Stream for SocketStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    type Item = Result<Bytes, io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut inner = self.project();
        loop {
            match futures::ready!(inner.0.poll_next_unpin(cx)) {
                Some(Ok(Message::Binary(data))) => {
                    return Poll::Ready(Some(Ok(data)));
                }
                Some(Ok(Message::Close(_))) => return Poll::Ready(None),
                Some(Ok(_)) => continue,
                Some(Err(err)) => {
                    return Poll::Ready(Some(Err(io::Error::other(err))));
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
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project().0.poll_ready(cx).map_err(io::Error::other)
    }

    fn start_send(self: Pin<&mut Self>, item: Bytes) -> io::Result<()> {
        self.project()
            .0
            .start_send(Message::Binary(item))
            .map_err(io::Error::other)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project().0.poll_flush(cx).map_err(io::Error::other)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project().0.poll_close(cx).map_err(io::Error::other)
    }
}
