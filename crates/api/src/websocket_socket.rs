use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use axum::extract::ws::{Message, WebSocket};
use bytes::Bytes;
use futures::{Sink, Stream};
use futures_util::StreamExt;
use pin_project::pin_project;

/// Adapter that exposes an Axum `WebSocket` as a binary sink/stream.
#[pin_project]
pub struct AxumBinarySocket {
    #[pin]
    inner: WebSocket,
}

impl AxumBinarySocket {
    pub fn new(socket: WebSocket) -> Self {
        Self { inner: socket }
    }
}

impl Stream for AxumBinarySocket {
    type Item = Result<Bytes, io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        loop {
            match futures::ready!(this.inner.as_mut().poll_next_unpin(cx)) {
                Some(Ok(Message::Binary(data))) => return Poll::Ready(Some(Ok(data))),
                Some(Ok(Message::Close(_))) => return Poll::Ready(None),
                Some(Ok(_)) => continue,
                Some(Err(err)) => return Poll::Ready(Some(Err(io::Error::other(err)))),
                None => return Poll::Ready(None),
            }
        }
    }
}

impl Sink<Bytes> for AxumBinarySocket {
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project()
            .inner
            .poll_ready(cx)
            .map_err(io::Error::other)
    }

    fn start_send(self: Pin<&mut Self>, item: Bytes) -> Result<(), Self::Error> {
        self.project()
            .inner
            .start_send(Message::Binary(item))
            .map_err(io::Error::other)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project()
            .inner
            .poll_flush(cx)
            .map_err(io::Error::other)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project()
            .inner
            .poll_close(cx)
            .map_err(io::Error::other)
    }
}
