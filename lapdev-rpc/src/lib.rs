pub mod error;

use std::{
    io,
    time::{Duration, SystemTime},
};

use anyhow::{anyhow, Result};
use chrono::{DateTime, FixedOffset};
use error::ApiError;
use futures::{
    stream::{AbortHandle, Abortable},
    Sink, SinkExt, Stream, StreamExt, TryFutureExt, TryStreamExt,
};
use lapdev_common::{
    BuildTarget, ContainerInfo, CreateWorkspaceRequest, DeleteWorkspaceRequest, GitBranch,
    PrebuildInfo, ProjectRequest, RepoBuildInfo, RepoBuildOutput, RepoBuildResult, RepoContent,
    RunningWorkspace, StartWorkspaceRequest, StopWorkspaceRequest,
};
use serde::{Deserialize, Serialize};
use tarpc::transport::channel::UnboundedChannel;
use uuid::Uuid;

/// A tarpc message that can be either a request or a response.
#[derive(serde::Serialize, serde::Deserialize)]
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
    UnboundedChannel<tarpc::ClientMessage<Req1>, tarpc::Response<Resp1>>,
    UnboundedChannel<tarpc::Response<Resp2>, tarpc::ClientMessage<Req2>>,
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
    let (server, server_ret) = tarpc::transport::channel::unbounded();
    let (client, client_ret) = tarpc::transport::channel::unbounded();
    let (mut server_sink, server_stream) = server.split();
    let (mut client_sink, client_stream) = client.split();
    let (transport_sink, mut transport_stream) = transport.split();

    let (abort_handle, abort_registration) = AbortHandle::new_pair();

    {
        let abort_handle = abort_handle.clone();
        // Task for inbound message handling
        tokio::spawn(async move {
            let e: Result<()> = async move {
                while let Some(msg) = transport_stream.next().await {
                    match msg? {
                        TwoWayMessage::ClientMessage(req) => server_sink.send(req).await?,
                        TwoWayMessage::Response(resp) => client_sink.send(resp).await?,
                    }
                }
                Ok(())
            }
            .await;

            match e {
                Ok(()) => tracing::debug!("transport_stream done"),
                Err(e) => tracing::warn!("Error in inbound multiplexing: {}", e),
            }

            abort_handle.abort();
        });
    }

    let abortable_sink_channel = Abortable::new(
        futures::stream::select(
            server_stream.map_ok(TwoWayMessage::Response),
            client_stream.map_ok(TwoWayMessage::ClientMessage),
        )
        .map_err(|e| anyhow!(e)),
        abort_registration,
    );

    // Task for outbound message handling
    tokio::spawn(
        abortable_sink_channel
            .forward(transport_sink.sink_map_err(|e| anyhow!(e)))
            .inspect_ok(|_| tracing::debug!("transport_sink done"))
            .inspect_err(|e| tracing::warn!("Error in outbound multiplexing: {}", e)),
    );

    (server_ret, client_ret, abort_handle)
}

#[tarpc::service]
pub trait ConductorService {
    async fn update_workspace_last_inactivity(
        workspace_id: Uuid,
        last_inactivity: Option<DateTime<FixedOffset>>,
    );

    async fn update_build_repo_stdout(target: BuildTarget, line: String);

    async fn update_build_repo_stderr(target: BuildTarget, line: String);

    async fn running_workspaces() -> Result<Vec<RunningWorkspace>, ApiError>;
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
    context.deadline = SystemTime::now() + Duration::from_secs(86400);
    context
}
