pub mod error;

use pin_project::pin_project;
use std::{
    fmt, io,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant},
};

use anyhow::{anyhow, Result};
use chrono::{DateTime, FixedOffset};
use error::ApiError;
use futures::{
    stream::{AbortHandle, Abortable},
    Sink, SinkExt, Stream, StreamExt, TryFutureExt,
};
use lapdev_common::{
    BuildTarget, ContainerInfo, CreateWorkspaceRequest, DeleteWorkspaceRequest, GitBranch,
    HostWorkspace, PrebuildInfo, ProjectRequest, RepoBuildInfo, RepoBuildOutput, RepoBuildResult,
    RepoContent, StartWorkspaceRequest, StopWorkspaceRequest,
};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::PollSender;
use uuid::Uuid;

/// A tarpc message that can be either a request or a response.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum TwoWayMessage<Req, Resp> {
    ClientMessage(tarpc::ClientMessage<Req>),
    Response(tarpc::Response<Resp>),
}

/// Returns two transports that multiplex over the given transport.
/// The first transport can be used by a server: it receives requests and sends back responses.
/// The second transport can be used by a client: it sends requests and receives back responses.
#[allow(clippy::complexity)]
pub fn spawn_twoway<Req1, Resp1, Req2, Resp2, T>(
    transport: T,
) -> (
    ChannelTransport<tarpc::ClientMessage<Req1>, tarpc::Response<Resp1>>,
    ChannelTransport<tarpc::Response<Resp2>, tarpc::ClientMessage<Req2>>,
    AbortHandle,
)
where
    T: Stream<Item = Result<TwoWayMessage<Req1, Resp2>, io::Error>>,
    T: Sink<TwoWayMessage<Req2, Resp1>, Error = io::Error>,
    T: Unpin + Send + 'static,
    Req1: Send + 'static,
    Resp1: Send + 'static,
    Req2: Send + 'static,
    Resp2: Send + 'static,
{
    let (mut transport_sink, mut transport_stream) = transport.split();

    let (abort_handle, abort_registration) = AbortHandle::new_pair();
    let (server_in_tx, server_in_rx) =
        mpsc::channel::<Result<tarpc::ClientMessage<Req1>, io::Error>>(RPC_CHANNEL_CAPACITY);
    let (client_in_tx, client_in_rx) =
        mpsc::channel::<Result<tarpc::Response<Resp2>, io::Error>>(RPC_CHANNEL_CAPACITY);
    let (server_out_tx, mut server_out_rx) =
        mpsc::channel::<tarpc::Response<Resp1>>(RPC_CHANNEL_CAPACITY);
    let (client_out_tx, mut client_out_rx) =
        mpsc::channel::<tarpc::ClientMessage<Req2>>(RPC_CHANNEL_CAPACITY);

    let (server_transport, client_transport) = (
        ChannelTransport::new(server_in_rx, PollSender::new(server_out_tx)),
        ChannelTransport::new(client_in_rx, PollSender::new(client_out_tx)),
    );

    {
        let abort_on_exit = abort_handle.clone();
        let loop_abort = abort_handle.clone();
        let server_in_tx = server_in_tx;
        let client_in_tx = client_in_tx;
        tokio::spawn(async move {
            let e: Result<()> = async move {
                while !loop_abort.is_aborted() {
                    match transport_stream.next().await {
                        Some(Ok(TwoWayMessage::ClientMessage(req))) => {
                            if server_in_tx.send(Ok(req)).await.is_err() {
                                break;
                            }
                        }
                        Some(Ok(TwoWayMessage::Response(resp))) => {
                            if client_in_tx.send(Ok(resp)).await.is_err() {
                                break;
                            }
                        }
                        Some(Err(err)) => {
                            let cloned = clone_io_error(&err);
                            let _ = server_in_tx.send(Err(err)).await;
                            let _ = client_in_tx.send(Err(cloned)).await;
                            break;
                        }
                        None => break,
                    }
                }
                Ok(())
            }
            .await;

            match e {
                Ok(()) => tracing::debug!("transport_stream done"),
                Err(e) => tracing::warn!("Error in inbound multiplexing: {}", e),
            }

            abort_on_exit.abort();
        });
    }

    let outbound = async move {
        loop {
            tokio::select! {
                Some(resp) = server_out_rx.recv() => {
                    if let Err(e) = transport_sink.send(TwoWayMessage::Response(resp)).await.map_err(|e| anyhow!(e)) {
                        return Err(e);
                    }
                }
                Some(msg) = client_out_rx.recv() => {
                    if let Err(e) = transport_sink.send(TwoWayMessage::ClientMessage(msg)).await.map_err(|e| anyhow!(e)) {
                        return Err(e);
                    }
                }
                else => break,
            }
        }

        transport_sink.sink_map_err(|e| anyhow!(e)).close().await
    };

    tokio::spawn(
        Abortable::new(outbound, abort_registration)
            .inspect_ok(|_| tracing::debug!("transport_sink done"))
            .inspect_err(|e| tracing::warn!("Error in outbound multiplexing: {}", e)),
    );

    (server_transport, client_transport, abort_handle)
}

const RPC_CHANNEL_CAPACITY: usize = 32;

fn clone_io_error(err: &io::Error) -> io::Error {
    io::Error::new(err.kind(), err.to_string())
}

#[derive(Debug)]
pub enum TwoWayTransportError {
    Closed,
    Io(io::Error),
}

impl fmt::Display for TwoWayTransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => write!(f, "transport closed"),
            Self::Io(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for TwoWayTransportError {}

impl From<io::Error> for TwoWayTransportError {
    fn from(err: io::Error) -> Self {
        TwoWayTransportError::Io(err)
    }
}

#[pin_project]
pub struct ChannelTransport<In, Out> {
    receiver: mpsc::Receiver<Result<In, io::Error>>,
    #[pin]
    sender: PollSender<Out>,
}

impl<In, Out> ChannelTransport<In, Out> {
    fn new(receiver: mpsc::Receiver<Result<In, io::Error>>, sender: PollSender<Out>) -> Self {
        Self { receiver, sender }
    }
}

impl<In, Out> Stream for ChannelTransport<In, Out> {
    type Item = Result<In, TwoWayTransportError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        match this.receiver.poll_recv(cx) {
            Poll::Ready(Some(Ok(item))) => Poll::Ready(Some(Ok(item))),
            Poll::Ready(Some(Err(err))) => Poll::Ready(Some(Err(TwoWayTransportError::Io(err)))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<In, Out> Sink<Out> for ChannelTransport<In, Out>
where
    Out: Send,
{
    type Error = TwoWayTransportError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project()
            .sender
            .poll_ready(cx)
            .map_err(|_| TwoWayTransportError::Closed)
    }

    fn start_send(self: Pin<&mut Self>, item: Out) -> Result<(), Self::Error> {
        self.project()
            .sender
            .start_send(item)
            .map_err(|_| TwoWayTransportError::Closed)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project()
            .sender
            .poll_flush(cx)
            .map_err(|_| TwoWayTransportError::Closed)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project()
            .sender
            .poll_close(cx)
            .map_err(|_| TwoWayTransportError::Closed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{SinkExt, StreamExt};
    use tarpc::{context, trace};
    use tokio::io::duplex;
    use tokio_util::codec::LengthDelimitedCodec;

    #[tokio::test]
    async fn spawn_twoway_moves_messages_between_halves() {
        let (io_a, io_b) = duplex(1024);
        let framed_a = LengthDelimitedCodec::builder().new_framed(io_a);
        let framed_b = LengthDelimitedCodec::builder().new_framed(io_b);
        let transport_a = tarpc::serde_transport::new::<
            _,
            TwoWayMessage<String, String>,
            TwoWayMessage<String, String>,
            _,
        >(framed_a, tarpc::tokio_serde::formats::Bincode::default());
        let transport_b = tarpc::serde_transport::new::<
            _,
            TwoWayMessage<String, String>,
            TwoWayMessage<String, String>,
            _,
        >(framed_b, tarpc::tokio_serde::formats::Bincode::default());
        let (mut server, mut client, abort_handle) =
            spawn_twoway::<String, String, String, String, _>(transport_a);
        let (mut transport_sink, mut transport_stream) = transport_b.split();

        let cancel: tarpc::ClientMessage<String> = tarpc::ClientMessage::Cancel {
            trace_context: trace::Context {
                trace_id: trace::TraceId::from(1u128),
                span_id: trace::SpanId::from(2u64),
                sampling_decision: trace::SamplingDecision::Sampled,
            },
            request_id: 7,
        };
        transport_sink
            .send(TwoWayMessage::ClientMessage(cancel))
            .await
            .unwrap();
        match server.next().await.unwrap().unwrap() {
            tarpc::ClientMessage::Cancel { request_id, .. } => assert_eq!(request_id, 7),
            other => panic!("unexpected client message: {:?}", other),
        }

        let inbound_response = tarpc::Response {
            request_id: 9,
            message: Ok("pong".to_string()),
        };
        transport_sink
            .send(TwoWayMessage::Response(inbound_response.clone()))
            .await
            .unwrap();
        let received = client.next().await.unwrap().unwrap();
        assert_eq!(received.request_id, 9);

        let outbound_request = tarpc::ClientMessage::Request(tarpc::Request {
            context: context::current(),
            id: 11,
            message: "ping".to_string(),
        });
        client.send(outbound_request).await.unwrap();
        match transport_stream.next().await.unwrap().unwrap() {
            TwoWayMessage::ClientMessage(tarpc::ClientMessage::Request(sent)) => {
                assert_eq!(sent.id, 11);
                assert_eq!(sent.message, "ping");
            }
            msg => panic!("unexpected outbound message: {:?}", msg),
        }

        let outbound_response = tarpc::Response {
            request_id: 13,
            message: Ok("ok".to_string()),
        };
        server.send(outbound_response).await.unwrap();
        match transport_stream.next().await.unwrap().unwrap() {
            TwoWayMessage::Response(sent) => assert_eq!(sent.request_id, 13),
            msg => panic!("unexpected outbound response: {:?}", msg),
        }

        abort_handle.abort();
    }
}

#[tarpc::service]
pub trait ConductorService {
    async fn update_workspace_last_inactivity(
        workspace_id: Uuid,
        last_inactivity: Option<DateTime<FixedOffset>>,
    );

    async fn update_build_repo_stdout(target: BuildTarget, line: String);

    async fn update_build_repo_stderr(target: BuildTarget, line: String);

    async fn running_workspaces_on_host() -> Result<Vec<HostWorkspace>, ApiError>;

    async fn auto_delete_inactive_workspaces();
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Workspace {
    pub image: String,
}

#[tarpc::service]
pub trait WorkspaceService {
    async fn ping() -> String;

    async fn version() -> String;

    async fn create_workspace(req: CreateWorkspaceRequest) -> Result<(), ApiError>;

    async fn start_workspace(req: StartWorkspaceRequest) -> Result<ContainerInfo, ApiError>;

    async fn delete_workspace(req: DeleteWorkspaceRequest) -> Result<(), ApiError>;

    async fn stop_workspace(req: StopWorkspaceRequest) -> Result<(), ApiError>;

    async fn create_project(req: ProjectRequest) -> Result<(), ApiError>;

    async fn get_project_branches(req: ProjectRequest) -> Result<Vec<GitBranch>, ApiError>;

    async fn transfer_repo(info: RepoBuildInfo, repo: RepoContent) -> Result<(), ApiError>;

    async fn unarchive_repo(info: RepoBuildInfo) -> Result<(), ApiError>;

    async fn build_repo(info: RepoBuildInfo) -> RepoBuildResult;

    async fn rebuild_repo(info: RepoBuildInfo) -> RepoBuildResult;

    async fn create_prebuild_archive(
        output: RepoBuildOutput,
        prebuild: PrebuildInfo,
        repo_name: String,
    ) -> Result<(), ApiError>;

    async fn copy_prebuild_image(
        prebuild: PrebuildInfo,
        output: RepoBuildOutput,
        dest_osuser: String,
        target: BuildTarget,
    ) -> Result<(), ApiError>;

    async fn copy_prebuild_content(
        prebuild: PrebuildInfo,
        info: RepoBuildInfo,
    ) -> Result<(), ApiError>;

    async fn delete_prebuild(
        prebuild: PrebuildInfo,
        output: Option<RepoBuildOutput>,
    ) -> Result<(), ApiError>;

    async fn transfer_prebuild(
        prebuild: PrebuildInfo,
        output: RepoBuildOutput,
        dst_host: String,
        dst_port: u16,
    ) -> Result<(), ApiError>;

    async fn unarchive_prebuild(prebuild: PrebuildInfo, repo_name: String) -> Result<(), ApiError>;
}

#[tarpc::service]
pub trait InterWorkspaceService {
    async fn transfer_prebuild_archive(
        archive: PrebuildInfo,
        file: String,
        content: Vec<u8>,
        append: bool,
    ) -> Result<(), ApiError>;
}

pub fn long_running_context() -> tarpc::context::Context {
    let mut context = tarpc::context::current();
    context.deadline = Instant::now() + Duration::from_secs(86400);
    context
}
