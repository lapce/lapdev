use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use clap::Parser;
use docker_compose_types::{AdvancedBuildStep, BuildStep, Compose};
use fs4::tokio::AsyncFileExt;
use futures::StreamExt;
use git2::{Cred, FetchOptions, FetchPrune, RemoteCallbacks, Repository};
use http_body_util::{BodyExt, Full};
use hyperlocal::{UnixClientExt, UnixConnector, Uri};
use lapdev_common::{
    devcontainer::{
        DevContainerCmd, DevContainerConfig, DevContainerCwd, DevContainerLifeCycleCmd,
    },
    utils, BuildTarget, ContainerImageInfo, CpuCore, GitBranch, HostWorkspace, ProjectRequest,
    RepoBuildInfo, RepoBuildOutput, RepoBuildResult, RepoComposeService, RepobuildError,
};
use lapdev_rpc::{
    error::ApiError, spawn_twoway, ConductorServiceClient, InterWorkspaceService, WorkspaceService,
};
use netstat2::TcpState;
use serde::Deserialize;
use tarpc::{
    context::current,
    server::{BaseChannel, Channel},
    tokio_serde::formats::Json,
};
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::{Mutex, RwLock},
};
use uuid::Uuid;

use crate::{
    service::{InterWorkspaceRpcService, WorkspaceRpcService},
    watcher::FileWatcher,
};

pub const LAPDEV_WS_VERSION: &str = env!("CARGO_PKG_VERSION");
const INSTALL_SCRIPT: &[u8] = include_bytes!("../scripts/install_guest_agent.sh");
const LAPDEV_GUEST_AGENT: &[u8] = include_bytes!("../../target/release/lapdev-guest-agent");

#[derive(Clone, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
struct LapdevWsConfig {
    bind: Option<String>,
    ws_port: Option<u16>,
    inter_ws_port: Option<u16>,
    backup_host: Option<String>,
}

#[derive(Parser)]
#[clap(name = "lapdev-ws")]
#[clap(version = env!("CARGO_PKG_VERSION"))]
struct Cli {
    /// The config file path
    #[clap(short, long, action, value_hint = clap::ValueHint::AnyPath)]
    config_file: Option<PathBuf>,
    /// The folder for putting data
    #[clap(short, long, action, value_hint = clap::ValueHint::AnyPath)]
    data_folder: Option<PathBuf>,
}

impl Cli {
    fn data_folder(&self) -> PathBuf {
        self.data_folder
            .clone()
            .unwrap_or_else(|| PathBuf::from("/var/lib/lapdev-ws"))
    }
}

#[derive(Clone)]
pub struct WorkspaceServer {
    data_folder: PathBuf,
    pub rpcs: Arc<RwLock<Vec<WorkspaceRpcService>>>,
}

pub async fn start() {
    let cli = Cli::parse();
    let _result = setup_log(&cli.data_folder()).await;

    if let Err(e) = run(&cli).await {
        tracing::error!("lapdev-ws server run error: {e:#}");
    }
}

async fn run(cli: &Cli) -> Result<()> {
    let config_file = cli
        .config_file
        .clone()
        .unwrap_or_else(|| PathBuf::from("/etc/lapdev-ws.conf"));
    let config_content = tokio::fs::read_to_string(&config_file)
        .await
        .with_context(|| format!("can't read config file {}", config_file.to_string_lossy()))?;
    let config: LapdevWsConfig =
        toml::from_str(&config_content).with_context(|| "wrong config file format")?;
    let bind = config.bind.as_deref().unwrap_or("0.0.0.0");
    let ws_port = config.ws_port.unwrap_or(6123);
    let inter_ws_port = config.inter_ws_port.unwrap_or(6122);
    WorkspaceServer::new(cli.data_folder())
        .run(bind, ws_port, inter_ws_port, config.backup_host)
        .await
}

impl WorkspaceServer {
    fn new(data_folder: PathBuf) -> Self {
        Self {
            data_folder,
            rpcs: Default::default(),
        }
    }

    async fn run(
        &self,
        bind: &str,
        ws_port: u16,
        inter_ws_port: u16,
        backup_host: Option<String>,
    ) -> Result<()> {
        {
            Command::new("mkdir")
                .arg("-p")
                .arg(self.data_folder.join("workspaces"))
                .status()
                .await?;
            Command::new("mkdir")
                .arg("-p")
                .arg(self.data_folder.join("prebuilds"))
                .status()
                .await?;
            Command::new("mkdir")
                .arg("-p")
                .arg(self.data_folder.join("projects"))
                .status()
                .await?;
            Command::new("mkdir")
                .arg("-p")
                .arg(self.data_folder.join("osusers"))
                .status()
                .await?;

            let watcher = FileWatcher::new(&self.data_folder, backup_host)?;
            std::thread::spawn(move || {
                watcher.watch();
            });
        }

        {
            let server = self.clone();
            let bind = bind.to_string();
            tokio::spawn(async move {
                if let Err(e) = server.run_inter_ws_service(&bind, inter_ws_port).await {
                    tracing::error!("run iter ws service error: {e:#}");
                }
            });
        }

        let mut listener =
            tarpc::serde_transport::tcp::listen((bind, ws_port), Json::default).await?;

        {
            let server = self.clone();
            tokio::spawn(async move {
                server.run_tasks().await;
            });
        }

        while let Some(conn) = listener.next().await {
            if let Ok(conn) = conn {
                let peer_addr = conn.peer_addr();
                let (server_chan, client_chan, _) = spawn_twoway(conn);
                let conductor_client =
                    ConductorServiceClient::new(tarpc::client::Config::default(), client_chan)
                        .spawn();
                let server = self.clone();

                let id = Uuid::new_v4();
                let rpc = WorkspaceRpcService {
                    id,
                    server,
                    conductor_client,
                };
                self.rpcs.write().await.push(rpc.clone());

                let rpcs = self.rpcs.clone();
                tokio::spawn(async move {
                    BaseChannel::with_defaults(server_chan)
                        .execute(rpc.serve())
                        .for_each(|resp| async move {
                            tokio::spawn(resp);
                        })
                        .await;
                    tracing::info!("incoming conductor connection {peer_addr:?} stopped");
                    rpcs.write().await.retain(|rpc| rpc.id != id);
                });
            }
        }

        Ok(())
    }

    async fn run_tasks(&self) {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            if let Err(e) = self.run_auto_stop_delete_task().await {
                tracing::error!("run task error: {e:?}");
            }
            if let Err(e) = self.check_inactive_osusers().await {
                tracing::error!("check inactive osusers error: {e:?}");
            }

            let rpc = { self.rpcs.read().await.first().cloned() };
            if let Some(rpc) = rpc {
                if let Err(e) = rpc
                    .conductor_client
                    .auto_delete_inactive_workspaces(current())
                    .await
                {
                    tracing::error!("run auto delete inactive workspaces error: {e:?}");
                }
            }
        }
    }

    async fn run_auto_stop_delete_task(&self) -> Result<(), ApiError> {
        let rpc = { self.rpcs.read().await.first().cloned() };
        let rpc = rpc.ok_or_else(|| anyhow!("don't have any conductor connections"))?;
        let workspaces = rpc
            .conductor_client
            .running_workspaces_on_host(current())
            .await??;

        let mut active_ports = HashMap::new();
        for si in netstat2::iterate_sockets_info_without_pids(
            netstat2::AddressFamilyFlags::IPV4 | netstat2::AddressFamilyFlags::IPV6,
            netstat2::ProtocolFlags::TCP,
        )?
        .flatten()
        {
            if let netstat2::ProtocolSocketInfo::Tcp(si) = si.protocol_socket_info {
                if si.state == TcpState::Established {
                    active_ports.insert(si.local_port as i32, si.local_port as i32);
                }
            }
        }

        for workspace in &workspaces {
            if let Err(e) = self
                .run_atuo_stop_on_ws(&rpc, workspace, &active_ports)
                .await
            {
                tracing::error!("run auto stop on ws {workspace:?} error: {e:?}");
            }
        }

        Ok(())
    }

    async fn run_atuo_stop_on_ws(
        &self,
        rpc: &WorkspaceRpcService,
        workspace: &HostWorkspace,
        active_ports: &HashMap<i32, i32>,
    ) -> Result<(), ApiError> {
        let mut active = false;
        if let Some(port) = workspace.ssh_port {
            active |= active_ports.contains_key(&port);
        }
        if let Some(port) = workspace.ide_port {
            active |= active_ports.contains_key(&port);
        }

        if active {
            // we have activity on the workspace, so we set last_inactivity to none
            // if it's not
            if workspace.last_inactivity.is_some() {
                let _ = rpc
                    .conductor_client
                    .update_workspace_last_inactivity(current(), workspace.id, None)
                    .await;
            }
        } else {
            // we don't have activity, so if last_inactivity is none,
            // we make the current time as the last_inactivity
            if workspace.last_inactivity.is_none() {
                let _ = rpc
                    .conductor_client
                    .update_workspace_last_inactivity(
                        current(),
                        workspace.id,
                        Some(Utc::now().into()),
                    )
                    .await;
            }
        }
        Ok(())
    }

    async fn check_inactive_osusers(&self) -> Result<()> {
        let mut read_dir = tokio::fs::read_dir(&self.osuser_lock_base_path()).await?;
        while let Some(path) = read_dir.next_entry().await? {
            let file_name = path.file_name();
            let osuser = file_name
                .to_str()
                .ok_or_else(|| anyhow!("can't convert path to str"))?;
            if let Err(e) = self.check_inactive_osuser(osuser).await {
                tracing::error!("check inactive osuser {osuser} error: {e:?}");
            }
        }
        Ok(())
    }

    async fn check_inactive_osuser(&self, osuser: &str) -> Result<()> {
        let uid = self._os_user_uid(osuser).await?;

        let osuser_lock_path = self.osuser_lock_path(osuser);
        let mut f = File::open(&osuser_lock_path).await?;
        f.lock_exclusive_async().await?;
        let mut buffer = String::new();
        f.read_to_string(&mut buffer).await?;
        let last_activity = buffer.trim().parse::<u64>()?;
        let now = utils::unix_timestamp()?;
        if last_activity + 24 * 60 * 60 * 7 > now {
            return Ok(());
        }

        let workspace_base_folder = self.osuser_workspace_base_folder(osuser);
        let is_empty = tokio::fs::read_dir(&workspace_base_folder)
            .await?
            .next_entry()
            .await?
            .is_none();
        if !is_empty {
            return Ok(());
        }

        tracing::info!("now cleaning up osuser {osuser}");

        Command::new("loginctl")
            .arg("disable-linger")
            .arg(&uid)
            .output()
            .await?;

        Command::new("userdel").arg(osuser).output().await?;

        tokio::fs::remove_dir(&workspace_base_folder).await?;
        tokio::fs::remove_dir_all(format!("/home/{osuser}")).await?;

        f.unlock_async().await?;
        tokio::fs::remove_file(&osuser_lock_path).await?;

        Ok(())
    }

    async fn run_inter_ws_service(&self, bind: &str, inter_ws_port: u16) -> Result<()> {
        let mut listener =
            tarpc::serde_transport::tcp::listen((bind, inter_ws_port), Json::default).await?;
        listener.config_mut().max_frame_length(usize::MAX);
        listener
            // Ignore accept errors.
            .filter_map(|r| futures::future::ready(r.ok()))
            .map(tarpc::server::BaseChannel::with_defaults)
            // serve is generated by the service attribute. It takes as input any type implementing
            // the generated World trait.
            .map(|channel| {
                let server = InterWorkspaceRpcService {
                    server: self.clone(),
                };
                channel.execute(server.serve()).for_each(spawn)
            })
            // Max 10 channels.
            .buffer_unordered(100)
            .for_each(|_| async {})
            .await;
        Ok(())
    }

    pub async fn get_podman_socket(&self, osuser: &str) -> Result<PathBuf, ApiError> {
        let _ = self.os_user_uid(osuser).await?;
        let name = format!("/tmp/{osuser}-{}.sock", utils::rand_string(10));
        let _ = Command::new("su")
            .arg("-")
            .arg(osuser)
            .arg("-c")
            .arg(format!("podman system service --time=60 unix://{name}"))
            .spawn()?;

        let mut n = 0;
        loop {
            if tokio::fs::try_exists(&name).await.unwrap_or(false) {
                break;
            }
            if n > 10 {
                return Err(ApiError::InternalError(
                    "can't get podman socket".to_string(),
                ));
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            n += 1;
        }
        Ok(PathBuf::from(name))
    }

    async fn _os_user_uid(&self, osuser: &str) -> Result<String> {
        let output = Command::new("id").arg("-u").arg(osuser).output().await;
        if let Ok(output) = output {
            if output.status.success() {
                let uid = String::from_utf8(output.stdout)?.trim().to_string();
                return Ok(uid);
            }
        }
        Err(anyhow!("no user"))
    }

    pub async fn os_user_uid(&self, username: &str) -> Result<String, ApiError> {
        let mut f = File::create(self.osuser_lock_path(username)).await?;
        f.lock_exclusive_async().await?;
        let ts = utils::unix_timestamp()?.to_string();
        f.write_all(ts.as_bytes()).await?;

        if let Ok(uid) = self._os_user_uid(username).await {
            return Ok(uid);
        }

        if !Command::new("useradd")
            .arg(username)
            .arg("-d")
            .arg(format!("/home/{username}"))
            .arg("-m")
            .status()
            .await?
            .success()
        {
            return Err(anyhow!("can't do useradd {username}").into());
        }

        let _ = tokio::process::Command::new("usermod")
            .arg("-a")
            .arg("-G")
            .arg("video")
            .arg(username)
            .output()
            .await;
        let _ = tokio::process::Command::new("usermod")
            .arg("-a")
            .arg("-G")
            .arg("render")
            .arg(username)
            .output()
            .await;

        let workspace_base_folder = self.osuser_workspace_base_folder(username);
        Command::new("mkdir")
            .arg("-p")
            .arg(&workspace_base_folder)
            .spawn()?
            .wait()
            .await?;
        Command::new("chown")
            .arg("-R")
            .arg(format!("{username}:{username}"))
            .arg(&workspace_base_folder)
            .output()
            .await?;

        let prebuild_base_folder = self.osuser_prebuild_base_folder(username);
        Command::new("mkdir")
            .arg("-p")
            .arg(&prebuild_base_folder)
            .spawn()?
            .wait()
            .await?;

        let project_base_folder = self.osuser_project_base_folder(username);
        Command::new("mkdir")
            .arg("-p")
            .arg(&project_base_folder)
            .spawn()?
            .wait()
            .await?;

        let containers_config_folder = format!("/home/{username}/.config/containers");
        Command::new("su")
            .arg("-")
            .arg(username)
            .arg("-c")
            .arg(format!("mkdir -p {containers_config_folder}"))
            .spawn()?
            .wait()
            .await?;
        tokio::fs::write(
            format!("{containers_config_folder}/containers.conf"),
            r#"
[containers]
annotations=["run.oci.keep_original_groups=1",]
"#,
        )
        .await?;
        tokio::fs::write(
            format!("{containers_config_folder}/registries.conf"),
            r#"
unqualified-search-registries = ["docker.io"]
"#,
        )
        .await?;
        tokio::fs::write(
            format!("{containers_config_folder}/storage.conf"),
            r#"
[storage]
driver = "overlay"
"#,
        )
        .await?;
        Command::new("chown")
            .arg("-R")
            .arg(format!("{username}:{username}"))
            .arg(&containers_config_folder)
            .output()
            .await?;

        let uid = self._os_user_uid(username).await?;

        Command::new("loginctl")
            .arg("enable-linger")
            .arg(&uid)
            .output()
            .await?;

        Ok(uid)
    }

    pub async fn get_devcontainer(
        &self,
        folder: &Path,
    ) -> Result<Option<(DevContainerCwd, DevContainerConfig)>, ApiError> {
        let devcontainer_folder_path = folder.join(".devcontainer").join("devcontainer.json");
        let devcontainer_root_path = folder.join(".devcontainer.json");
        let (cwd, file_path) = if tokio::fs::try_exists(&devcontainer_folder_path)
            .await
            .unwrap_or(false)
        {
            (folder.join(".devcontainer"), devcontainer_folder_path)
        } else if tokio::fs::try_exists(&devcontainer_root_path)
            .await
            .unwrap_or(false)
        {
            (folder.to_path_buf(), devcontainer_root_path)
        } else {
            return Ok(None);
        };
        let content = tokio::fs::read_to_string(&file_path).await?;
        let config: DevContainerConfig = json5::from_str(&content)
            .map_err(|e| ApiError::RepositoryInvalid(format!("devcontainer.json invalid: {e}")))?;
        Ok(Some((cwd, config)))
    }

    async fn write_extra_dockerfile(
        &self,
        info: &RepoBuildInfo,
        temp_docker_file: &mut File,
        install_script_path: &Path,
        lapdev_guest_agent_path: &Path,
    ) -> Result<()> {
        temp_docker_file.write_all(b"\nUSER root\n").await?;
        temp_docker_file
            .write_all(b"COPY lapdev-guest-agent /lapdev-guest-agent\n")
            .await?;
        temp_docker_file
            .write_all(b"RUN chmod +x /lapdev-guest-agent\n")
            .await?;
        temp_docker_file
            .write_all(b"COPY install_guest_agent.sh /install_guest_agent.sh\n")
            .await?;
        temp_docker_file
            .write_all(b"RUN sh /install_guest_agent.sh\n")
            .await?;
        temp_docker_file
            .write_all(b"RUN rm /install_guest_agent.sh\n")
            .await?;

        {
            let mut install_script_file = tokio::fs::File::create(&install_script_path).await?;
            install_script_file.write_all(INSTALL_SCRIPT).await?;
        }

        {
            let mut file = tokio::fs::File::create(&lapdev_guest_agent_path).await?;
            file.write_all(LAPDEV_GUEST_AGENT).await?;
            file.flush().await?;
        }

        tokio::process::Command::new("chown")
            .arg(format!("{}:{}", info.osuser, info.osuser))
            .arg(install_script_path)
            .output()
            .await?;

        tokio::process::Command::new("chown")
            .arg(format!("{}:{}", info.osuser, info.osuser))
            .arg(lapdev_guest_agent_path)
            .output()
            .await?;

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn do_build_container_image(
        &self,
        conductor_client: &ConductorServiceClient,
        info: &RepoBuildInfo,
        cwd: &Path,
        context: &Path,
        dockerfile_content: &str,
        tag: &str,
        install_extra: bool,
    ) -> Result<ContainerImageInfo, ApiError> {
        let install_script_path = context.join("install_guest_agent.sh");
        let lapdev_guest_agent_path = context.join("lapdev-guest-agent");
        let temp = tempfile::NamedTempFile::new()?.into_temp_path();
        {
            let mut temp_docker_file = tokio::fs::File::create(&temp).await?;
            temp_docker_file
                .write_all(dockerfile_content.as_bytes())
                .await?;
            if install_extra {
                self.write_extra_dockerfile(
                    info,
                    &mut temp_docker_file,
                    &install_script_path,
                    &lapdev_guest_agent_path,
                )
                .await?;
            }
        }

        tokio::process::Command::new("chown")
            .arg(format!("{}:{}", info.osuser, info.osuser))
            .arg(&temp)
            .output()
            .await?;

        let build_args = info
            .env
            .iter()
            .map(|(name, value)| format!("--build-arg {name}={value}"))
            .collect::<Vec<String>>()
            .join(" ");

        let mut child = tokio::process::Command::new("su")
            .arg("-")
            .arg(&info.osuser)
            .arg("-c")
            .arg(format!(
                "cd {} && podman build --pull --no-cache {build_args} {} -m {}g -f {} -t {tag} {}",
                cwd.to_string_lossy(),
                match &info.cpus {
                    CpuCore::Dedicated(cpus) => {
                        format!(
                            "--cpuset-cpus {}",
                            cpus.iter()
                                .map(|c| c.to_string())
                                .collect::<Vec<String>>()
                                .join(",")
                        )
                    }
                    CpuCore::Shared(n) => {
                        format!("--cpu-period 100000 --cpu-quota {}", *n * 100000)
                    }
                },
                info.memory,
                temp.to_string_lossy(),
                context.to_string_lossy(),
            ))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let stderr_log = self
            .update_build_std_output(conductor_client, &mut child, &info.target)
            .await;
        let status = child.wait().await?;
        if !status.success() {
            return Err(ApiError::RepositoryBuildFailure(RepobuildError {
                msg: "Workspace image build failed.".to_string(),
                stderr: stderr_log.lock().await.clone(),
            }));
        }

        if install_extra {
            let _ = tokio::fs::remove_file(&install_script_path).await;
            let _ = tokio::fs::remove_file(&lapdev_guest_agent_path).await;
        }

        let image_info = self.container_image_info(&info.osuser, tag).await?;

        Ok(image_info)
    }

    pub async fn build_container_image_from_base(
        &self,
        conductor_client: &ConductorServiceClient,
        info: &RepoBuildInfo,
        cwd: &Path,
        image: &str,
        tag: &str,
    ) -> Result<ContainerImageInfo, ApiError> {
        let install_extra = !image
            .trim()
            .to_lowercase()
            .starts_with("ghcr.io/lapce/lapdev-devcontainer-");

        let context = cwd.to_path_buf();
        let dockerfile_content = format!("FROM {image}\n");

        self.do_build_container_image(
            conductor_client,
            info,
            cwd,
            &context,
            &dockerfile_content,
            tag,
            install_extra,
        )
        .await
    }

    pub async fn pull_container_image(
        &self,
        conductor_client: &ConductorServiceClient,
        osuser: &str,
        image: &str,
        target: &BuildTarget,
    ) -> Result<()> {
        let mut child = tokio::process::Command::new("su")
            .arg("-")
            .arg(osuser)
            .arg("-c")
            .arg(format!("podman pull {image}"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        self.update_build_std_output(conductor_client, &mut child, target)
            .await;
        let _ = child.wait().await;
        Ok(())
    }

    pub async fn container_image_info(
        &self,
        osuser: &str,
        image: &str,
    ) -> Result<ContainerImageInfo, ApiError> {
        let socket = self.get_podman_socket(osuser).await?;
        let url = Uri::new(socket, &format!("/images/{image}/json"));
        let client = unix_client();
        let req = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri(url)
            .body(Full::<Bytes>::new(Bytes::new()))?;
        let resp = client.request(req).await?;
        let status = resp.status();
        let body = resp.collect().await?.to_bytes();
        if status != 200 {
            let err = String::from_utf8(body.to_vec())?;
            return Err(anyhow!(err).into());
        }
        let image_info: ContainerImageInfo = serde_json::from_slice(&body)?;
        Ok(image_info)
    }

    pub async fn build_container_image(
        &self,
        conductor_client: &ConductorServiceClient,
        info: &RepoBuildInfo,
        cwd: &Path,
        build: &AdvancedBuildStep,
        tag: &str,
    ) -> Result<ContainerImageInfo, ApiError> {
        let context = cwd.join(&build.context);
        let dockerfile = build.dockerfile.as_deref().unwrap_or("Dockerfile");
        let dockerfile = context.join(dockerfile);

        let dockerfile_content = tokio::fs::read_to_string(dockerfile)
            .await
            .map_err(|e| ApiError::RepositoryInvalid(format!("Can't read dockerfile: {e}")))?;
        self.do_build_container_image(
            conductor_client,
            info,
            cwd,
            &context,
            &dockerfile_content,
            tag,
            true,
        )
        .await
    }

    fn compose_service_env(&self, service: &docker_compose_types::Service) -> Vec<String> {
        match &service.environment {
            docker_compose_types::Environment::List(list) => list.clone(),
            docker_compose_types::Environment::KvPair(pair) => pair
                .iter()
                .filter_map(|(key, value)| Some(format!("{key}={}", value.as_ref()?)))
                .collect(),
        }
    }

    async fn build_compose_service(
        &self,
        conductor_client: &ConductorServiceClient,
        info: &RepoBuildInfo,
        cwd: &Path,
        service: &docker_compose_types::Service,
        tag: &str,
    ) -> Result<ContainerImageInfo, ApiError> {
        if let Some(build) = &service.build_ {
            let build = match build {
                BuildStep::Simple(context) => AdvancedBuildStep {
                    context: context.to_string(),
                    ..Default::default()
                },
                BuildStep::Advanced(build) => build.to_owned(),
            };
            self.build_container_image(conductor_client, info, cwd, &build, tag)
                .await
        } else if let Some(image) = &service.image {
            self.build_container_image_from_base(conductor_client, info, cwd, image, tag)
                .await
        } else {
            return Err(ApiError::RepositoryInvalid(
                "Can't find image or build in this compose service".to_string(),
            ));
        }
    }

    pub async fn build_compose(
        &self,
        conductor_client: &ConductorServiceClient,
        info: &RepoBuildInfo,
        config: &DevContainerConfig,
        compose_file: &Path,
        tag: &str,
    ) -> Result<RepoBuildOutput, ApiError> {
        let content = tokio::fs::read_to_string(compose_file)
            .await
            .map_err(|e| ApiError::RepositoryInvalid(format!("Can't read compose file: {e}")))?;
        let compose: Compose = serde_yaml::from_str(&content)
            .map_err(|e| ApiError::RepositoryInvalid(format!("Can't parse compose file: {e}")))?;
        let cwd = compose_file
            .parent()
            .ok_or_else(|| anyhow!("compose file doens't have a parent directory"))?;
        let mut services = Vec::new();
        for (name, service) in compose.services.0 {
            if let Some(service) = service {
                let tag = format!("{tag}:{name}");
                let info = self
                    .build_compose_service(conductor_client, info, cwd, &service, &tag)
                    .await?;
                let env = self.compose_service_env(&service);
                services.push(RepoComposeService {
                    name,
                    image: tag,
                    env,
                    info,
                });
            }
        }
        Ok(RepoBuildOutput::Compose {
            services,
            ports_attributes: config.ports_attributes.clone(),
        })
    }

    pub fn repo_target_image_tag(&self, target: &BuildTarget) -> String {
        match target {
            BuildTarget::Workspace { name, .. } => name.clone(),
            BuildTarget::Prebuild { id, .. } => id.to_string(),
        }
    }

    pub async fn run_lifecycle_commands(
        &self,
        conductor_client: &ConductorServiceClient,
        repo: &RepoBuildInfo,
        output: &RepoBuildOutput,
        config: &DevContainerConfig,
    ) {
        if let Some(cmd) = config.on_create_command.as_ref() {
            let _ = self
                .run_lifecycle_command(conductor_client, repo, output, config, cmd)
                .await;
        }
        if let Some(cmd) = config.update_content_command.as_ref() {
            let _ = self
                .run_lifecycle_command(conductor_client, repo, output, config, cmd)
                .await;
        }
        if let Some(cmd) = config.post_create_command.as_ref() {
            let _ = self
                .run_lifecycle_command(conductor_client, repo, output, config, cmd)
                .await;
        }
    }

    async fn run_lifecycle_command(
        &self,
        conductor_client: &ConductorServiceClient,
        repo: &RepoBuildInfo,
        output: &RepoBuildOutput,
        config: &DevContainerConfig,
        cmd: &DevContainerLifeCycleCmd,
    ) -> Result<()> {
        match output {
            RepoBuildOutput::Compose { services, .. } => {
                let cmd = match cmd {
                    DevContainerLifeCycleCmd::Simple(cmd) => {
                        DevContainerCmd::Simple(cmd.to_string())
                    }
                    DevContainerLifeCycleCmd::Args(cmds) => DevContainerCmd::Args(cmds.to_owned()),
                    DevContainerLifeCycleCmd::Object(cmds) => {
                        for (service, cmd) in cmds {
                            if let Some(service) = services.iter().find(|s| &s.name == service) {
                                self.run_devcontainer_command(
                                    conductor_client,
                                    repo,
                                    &service.image,
                                    cmd,
                                )
                                .await?;
                            }
                        }
                        return Ok(());
                    }
                };
                // if it's a single command, then we run it on main compose service
                if let Some(service) = config.service.as_ref() {
                    if let Some(service) = services.iter().find(|s| &s.name == service) {
                        self.run_devcontainer_command(conductor_client, repo, &service.image, &cmd)
                            .await?;
                    }
                }
            }
            RepoBuildOutput::Image { image, .. } => {
                let cmd = match cmd {
                    DevContainerLifeCycleCmd::Simple(cmd) => {
                        DevContainerCmd::Simple(cmd.to_string())
                    }
                    DevContainerLifeCycleCmd::Args(cmds) => DevContainerCmd::Args(cmds.to_owned()),
                    DevContainerLifeCycleCmd::Object(_) => {
                        return Err(anyhow!("can't use object cmd for non compose"))
                    }
                };
                self.run_devcontainer_command(conductor_client, repo, image, &cmd)
                    .await?;
            }
        }
        Ok(())
    }

    pub fn osuser_lock_base_path(&self) -> String {
        format!("{}/osusers", self.data_folder.to_str().unwrap_or_default())
    }

    pub fn osuser_lock_path(&self, osuser: &str) -> String {
        format!("{}/{osuser}", self.osuser_lock_base_path())
    }

    pub fn osuser_workspace_base_folder(&self, osuser: &str) -> String {
        format!(
            "{}/workspaces/{osuser}",
            self.data_folder.to_str().unwrap_or_default()
        )
    }

    pub fn osuser_prebuild_base_folder(&self, osuser: &str) -> String {
        format!(
            "{}/prebuilds/{osuser}",
            self.data_folder.to_str().unwrap_or_default()
        )
    }

    pub fn osuser_project_base_folder(&self, osuser: &str) -> String {
        format!(
            "{}/projects/{osuser}",
            self.data_folder.to_str().unwrap_or_default()
        )
    }

    pub fn workspace_folder(&self, osuser: &str, workspace_name: &str) -> String {
        format!(
            "{}/{workspace_name}",
            self.osuser_workspace_base_folder(osuser)
        )
    }

    pub fn prebuild_folder(&self, osuser: &str, prebuild_id: Uuid) -> String {
        format!("{}/{prebuild_id}", self.osuser_prebuild_base_folder(osuser))
    }

    pub fn project_folder(&self, osuser: &str, project_id: Uuid) -> String {
        format!("{}/{project_id}", self.osuser_project_base_folder(osuser))
    }

    pub fn build_base_folder(&self, info: &RepoBuildInfo) -> String {
        match &info.target {
            BuildTarget::Workspace { name, .. } => self.workspace_folder(&info.osuser, name),
            BuildTarget::Prebuild { id, .. } => self.prebuild_folder(&info.osuser, *id),
        }
    }

    pub fn build_repo_folder(&self, info: &RepoBuildInfo) -> String {
        let target = match &info.target {
            BuildTarget::Workspace { name, .. } => self.workspace_folder(&info.osuser, name),
            BuildTarget::Prebuild { id, .. } => self.prebuild_folder(&info.osuser, *id),
        };
        format!("{target}/{}", info.repo_name)
    }

    pub async fn get_default_image(&self, info: &RepoBuildInfo) -> String {
        let repo_path = PathBuf::from(self.build_repo_folder(info));
        if tokio::fs::try_exists(repo_path.join("Cargo.toml"))
            .await
            .unwrap_or(false)
        {
            return "rust".to_string();
        }

        if tokio::fs::try_exists(repo_path.join("go.mod"))
            .await
            .unwrap_or(false)
        {
            return "go".to_string();
        }

        if tokio::fs::try_exists(repo_path.join("tsconfig.json"))
            .await
            .unwrap_or(false)
        {
            return "typescript".to_string();
        }

        if tokio::fs::try_exists(repo_path.join("package.json"))
            .await
            .unwrap_or(false)
        {
            return "javascript".to_string();
        }

        if tokio::fs::try_exists(repo_path.join("requirements.txt"))
            .await
            .unwrap_or(false)
            || tokio::fs::try_exists(repo_path.join("setup.py"))
                .await
                .unwrap_or(false)
        {
            return "python".to_string();
        }

        "universal".to_string()
    }

    pub async fn build_repo_result(
        &self,
        info: &RepoBuildInfo,
        result: Result<RepoBuildOutput, ApiError>,
    ) -> RepoBuildResult {
        let err = match result {
            Ok(output) => {
                return RepoBuildResult {
                    error: None,
                    output,
                }
            }
            Err(e) => {
                tracing::error!("build repo {info:?} error: {e:?}");
                e
            }
        };

        let image_name = self.get_default_image(info).await;
        let image = format!("ghcr.io/lapce/lapdev-devcontainer-{image_name}:latest");
        tracing::debug!("build repo pick default image {image_name}");
        let err = match err {
            ApiError::RepositoryBuildFailure(err) => err,
            ApiError::RepositoryInvalid(msg) => RepobuildError {
                msg,
                stderr: vec![],
            },
            _ => RepobuildError {
                msg: "Internal Server Error".to_string(),
                stderr: vec![],
            },
        };
        RepoBuildResult {
            error: Some(err),
            output: RepoBuildOutput::Image {
                image,
                info: ContainerImageInfo::default(),
                ports_attributes: Default::default(),
            },
        }
    }

    pub async fn build_repo(
        &self,
        info: &RepoBuildInfo,
        conductor_client: &ConductorServiceClient,
    ) -> Result<RepoBuildOutput, ApiError> {
        self.prepare_repo(info).await?;
        self.do_build_repo(info, conductor_client).await
    }

    pub async fn do_build_repo(
        &self,
        info: &RepoBuildInfo,
        conductor_client: &ConductorServiceClient,
    ) -> Result<RepoBuildOutput, ApiError> {
        let (cwd, config) = self
            .get_devcontainer(&PathBuf::from(self.build_repo_folder(info)))
            .await?
            .ok_or_else(|| {
                ApiError::RepositoryInvalid(
                    "Repo doesn't have devcontainer configured or invalid devcontainer config."
                        .to_string(),
                )
            })?;
        let tag = self.repo_target_image_tag(&info.target);
        let output = if let Some(compose_file) = &config.docker_compose_file {
            self.build_compose(
                conductor_client,
                info,
                &config,
                &cwd.join(compose_file),
                &tag,
            )
            .await?
        } else if let Some(build) = config.build.as_ref() {
            let build = AdvancedBuildStep {
                context: build.context.clone().unwrap_or(".".to_string()),
                dockerfile: build.dockerfile.to_owned(),
                ..Default::default()
            };
            let info = self
                .build_container_image(conductor_client, info, &cwd, &build, &tag)
                .await?;
            RepoBuildOutput::Image {
                image: tag.clone(),
                info,
                ports_attributes: config.ports_attributes.clone(),
            }
        } else if let Some(image) = config.image.as_ref() {
            let info = self
                .build_container_image_from_base(conductor_client, info, &cwd, image, &tag)
                .await?;
            RepoBuildOutput::Image {
                image: tag.clone(),
                info,
                ports_attributes: config.ports_attributes.clone(),
            }
        } else {
            return Err(ApiError::RepositoryInvalid(
                "devcontainer doesn't have any image information".to_string(),
            ));
        };
        self.run_lifecycle_commands(conductor_client, info, &output, &config)
            .await;
        Ok(output)
    }

    pub async fn prepare_repo(&self, info: &RepoBuildInfo) -> Result<(), ApiError> {
        self.os_user_uid(&info.osuser).await?;
        let build_dir = self.build_base_folder(info);
        let repo_dir = format!("{build_dir}/{}", info.repo_name);
        Command::new("mkdir")
            .arg("-p")
            .arg(&repo_dir)
            .spawn()?
            .wait()
            .await?;

        if let BuildTarget::Prebuild { id, project } = &info.target {
            let src_dir = self.project_folder(&info.osuser, *project);
            if !tokio::fs::try_exists(&src_dir).await.unwrap_or(false) {
                self.create_project(
                    *id,
                    info.osuser.clone(),
                    info.repo_url.clone(),
                    info.auth.clone(),
                )
                .await?;
            }
            tokio::process::Command::new("sh")
                .arg("-c")
                .arg(format!("cp -a {src_dir}/. {repo_dir}"))
                .status()
                .await?;
        } else {
            let auth = info.auth.clone();
            let repo_url = info.repo_url.clone();
            let repo_dir = PathBuf::from(repo_dir.clone());
            tokio::task::spawn_blocking(move || -> Result<()> {
                let mut opt = git2::FetchOptions::new();
                let mut cbs = RemoteCallbacks::new();
                cbs.credentials(|_, _, _| Cred::userpass_plaintext(&auth.0, &auth.1));
                opt.remote_callbacks(cbs);
                git2::build::RepoBuilder::new()
                    .fetch_options(opt)
                    .clone(&repo_url, &repo_dir)?;
                Ok(())
            })
            .await??;
        }

        if info.head != info.branch {
            let repo_dir = PathBuf::from(repo_dir.clone());
            let branch_name = info.branch.clone();
            tokio::task::spawn_blocking(move || {
                let repo = git2::Repository::open(repo_dir)?;
                let object = repo.revparse_single(&format!("origin/{branch_name}"))?;
                let commit = object
                    .as_commit()
                    .ok_or_else(|| anyhow!("branch can't turn into commit"))?;
                let branch = repo.branch(&branch_name, commit, false)?;
                let commit = branch.get().peel_to_commit()?;
                repo.checkout_tree(commit.as_object(), None)?;
                repo.set_head(&format!("refs/heads/{branch_name}"))?;
                Ok::<(), anyhow::Error>(())
            })
            .await??;
        };

        tokio::process::Command::new("chown")
            .arg("-R")
            .arg(format!("{}:{}", info.osuser, info.osuser))
            .arg(&build_dir)
            .output()
            .await?;

        Ok(())
    }

    async fn run_devcontainer_command(
        &self,
        conductor_client: &ConductorServiceClient,
        info: &RepoBuildInfo,
        image: &str,
        cmd: &DevContainerCmd,
    ) -> Result<()> {
        let repo_folder = self.build_repo_folder(info);

        let cmd = match cmd {
            DevContainerCmd::Simple(cmd) => cmd.to_string(),
            DevContainerCmd::Args(cmds) => cmds.join(" "),
        };
        let mut child = Command::new("su")
            .arg("-")
            .arg(&info.osuser)
            .arg("-c")
            .arg(format!(
                "podman run --rm {} -m {}g --security-opt label=disable -v {repo_folder}:/workspace -w /workspace --user root --entrypoint \"\" {image} {cmd}",
                match &info.cpus {
                    CpuCore::Dedicated(cpus) => {
                        format!(
                            "--cpuset-cpus {}",
                            cpus.iter()
                                .map(|c| c.to_string())
                                .collect::<Vec<String>>()
                                .join(",")
                        )
                    }
                    CpuCore::Shared(n) => {
                        format!("--cpu-period 100000 --cpu-quota {}", *n * 100000)
                    }
                },
                info.memory,
            ))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        self.update_build_std_output(conductor_client, &mut child, &info.target)
            .await;
        child.wait().await?;
        Ok(())
    }

    pub async fn update_build_std_output(
        &self,
        conductor_client: &ConductorServiceClient,
        child: &mut tokio::process::Child,
        target: &BuildTarget,
    ) -> Arc<Mutex<Vec<String>>> {
        if let Some(stdout) = child.stdout.take() {
            let conductor_client = conductor_client.clone();
            let target = target.clone();
            let mut reader = BufReader::new(stdout);
            tokio::spawn(async move {
                let mut line = String::new();
                while let Ok(n) = reader.read_line(&mut line).await {
                    if n > 0 {
                        let line = line.trim_end().to_string();
                        let _ = conductor_client
                            .update_build_repo_stdout(current(), target.clone(), line)
                            .await;
                    } else {
                        break;
                    }
                    line.clear();
                }
            });
        }

        let stderr_log = Arc::new(Mutex::new(Vec::new()));
        if let Some(stderr) = child.stderr.take() {
            let stderr_log = stderr_log.clone();
            let conductor_client = conductor_client.clone();
            let target = target.clone();
            let mut reader = BufReader::new(stderr);
            tokio::spawn(async move {
                let mut line = String::new();
                while let Ok(n) = reader.read_line(&mut line).await {
                    if n > 0 {
                        let line = line.trim_end().to_string();
                        let _ = conductor_client
                            .update_build_repo_stderr(current(), target.clone(), line.clone())
                            .await;
                        stderr_log.lock().await.push(line);
                    } else {
                        break;
                    }
                    line.clear();
                }
            });
        }
        stderr_log
    }

    pub async fn delete_image(&self, osuser: &str, image: &str) -> Result<(), ApiError> {
        let socket = self.get_podman_socket(osuser).await?;

        let client = unix_client();
        {
            let url = Uri::new(&socket, &format!("/images/{image}"));
            let req = hyper::Request::builder()
                .method(hyper::Method::DELETE)
                .uri(url)
                .body(Full::<Bytes>::new(Bytes::new()))?;
            let resp = client.request(req).await?;
            let status = resp.status();
            if status != 200 && status != 404 {
                let body = resp.collect().await?.to_bytes();
                let err = String::from_utf8(body.to_vec())?;
                return Err(ApiError::InternalError(format!(
                    "delete image error: {err}"
                )));
            }
        }

        Ok(())
    }

    pub async fn delete_network(&self, osuser: &str, network: &str) -> Result<(), ApiError> {
        let socket = self.get_podman_socket(osuser).await?;

        let client = unix_client();
        {
            let url = Uri::new(&socket, &format!("/networks/{network}"));
            let req = hyper::Request::builder()
                .method(hyper::Method::DELETE)
                .uri(url)
                .body(Full::<Bytes>::new(Bytes::new()))?;
            let resp = client.request(req).await?;
            let status = resp.status();
            if status != 204 && status != 404 {
                let body = resp.collect().await?.to_bytes();
                let err = String::from_utf8(body.to_vec())?;
                return Err(ApiError::InternalError(format!(
                    "delete network error: {err}"
                )));
            }
        }

        Ok(())
    }

    pub async fn create_project(
        &self,
        id: Uuid,
        osuser: String,
        repo_url: String,
        auth: (String, String),
    ) -> Result<(), ApiError> {
        self.os_user_uid(&osuser).await?;
        let project_folder = self.project_folder(&osuser, id);

        Command::new("mkdir")
            .arg("-p")
            .arg(&project_folder)
            .spawn()?
            .wait()
            .await?;

        {
            let project_folder = project_folder.clone();
            tokio::task::spawn_blocking(move || -> Result<()> {
                let mut opt = git2::FetchOptions::new();
                let mut cbs = RemoteCallbacks::new();
                cbs.credentials(|_, _, _| Cred::userpass_plaintext(&auth.0, &auth.1));
                opt.remote_callbacks(cbs);
                git2::build::RepoBuilder::new()
                    .fetch_options(opt)
                    .clone(&repo_url, &PathBuf::from(project_folder))?;
                Ok(())
            })
            .await??;
        }

        Ok(())
    }

    pub async fn project_branches(&self, req: &ProjectRequest) -> Result<Vec<GitBranch>, ApiError> {
        let project_folder = self.project_folder(&req.osuser, req.id);
        let auth = req.auth.clone();
        let branches = tokio::task::spawn_blocking(move || -> Result<Vec<GitBranch>, ApiError> {
            let path = PathBuf::from(project_folder);
            let repo = git2::Repository::open(path)?;

            let head = repo.head()?;
            let head_branch = head.shorthand().ok_or_else(|| anyhow!("no head branch"))?;

            if let Err(e) = repo_pull(&repo, auth) {
                let err = if let ApiError::InternalError(e) = e {
                    e.to_string()
                } else {
                    e.to_string()
                };
                tracing::debug!("repo pull error: {err}");
            }

            let mut branches = repo_branches(&repo)?;
            branches.sort_by_key(|b| (b.name.as_str() == head_branch, b.time));
            branches.reverse();
            Ok(branches)
        })
        .await??;

        Ok(branches)
    }
}

async fn spawn(fut: impl futures::Future<Output = ()> + Send + 'static) {
    tokio::spawn(fut);
}

pub fn unix_client(
) -> hyper_util::client::legacy::Client<UnixConnector, http_body_util::Full<hyper::body::Bytes>> {
    hyper_util::client::legacy::Client::unix()
}

async fn setup_log(
    data_folder: &Path,
) -> Result<tracing_appender::non_blocking::WorkerGuard, anyhow::Error> {
    let folder = data_folder.join("logs");
    if !tokio::fs::try_exists(&folder).await.unwrap_or(false) {
        tokio::fs::create_dir_all(&folder).await?;
        let _ = tokio::process::Command::new("chown")
            .arg("-R")
            .arg("/var/lib/lapdev/")
            .output()
            .await;
    }
    let file_appender = tracing_appender::rolling::Builder::new()
        .max_log_files(30)
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("lapdev-ws.log")
        .build(folder)?;
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    let var = std::env::var("RUST_LOG").unwrap_or_default();
    let var = format!("error,lapdev_ws=info,lapdev_rpc=info,lapdev_common=info,{var}");
    let filter = tracing_subscriber::EnvFilter::builder().parse_lossy(var);
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter(filter)
        .with_writer(non_blocking)
        .init();
    Ok(guard)
}

fn repo_branches(repo: &Repository) -> Result<Vec<GitBranch>> {
    let mut branches = Vec::new();
    for (branch, _) in repo.branches(None)?.flatten() {
        let reference = branch.get();
        if reference.is_remote() {
            let commit = reference.peel_to_commit()?;
            let name = reference.shorthand().unwrap_or("");
            let name = name.strip_prefix("origin/").unwrap_or(name);
            let summary = commit.summary().unwrap_or("");
            let time = DateTime::from_timestamp(commit.time().seconds(), 0)
                .ok_or_else(|| anyhow!("invalid timestamp"))?;
            if name != "HEAD" {
                branches.push(GitBranch {
                    name: name.to_string(),
                    summary: summary.to_string(),
                    commit: commit.id().to_string(),
                    time: time.into(),
                });
            }
        }
    }
    Ok(branches)
}

fn repo_pull(repo: &Repository, auth: (String, String)) -> Result<(), ApiError> {
    let mut remote = repo
        .find_remote("origin")
        .or_else(|_| repo.remote_anonymous("origin"))?;
    let mut fetch_options = FetchOptions::new();
    let mut cbs = RemoteCallbacks::new();
    cbs.credentials(|_, _, _| Cred::userpass_plaintext(&auth.0, &auth.1));
    fetch_options.prune(FetchPrune::On);
    fetch_options.remote_callbacks(cbs);
    remote.fetch(&[] as &[&str], Some(&mut fetch_options), None)?;

    let head = repo.head()?;
    let head_branch = head.shorthand().ok_or_else(|| anyhow!("no head branch"))?;

    let fetch_head = repo.find_reference("FETCH_HEAD")?;
    let fetch_commit = repo.reference_to_annotated_commit(&fetch_head)?;
    let analysis = repo.merge_analysis(&[&fetch_commit])?;
    if analysis.0.is_fast_forward() {
        let refname = format!("refs/heads/{}", head_branch);
        match repo.find_reference(&refname) {
            Ok(mut r) => {
                let name = match r.name() {
                    Some(s) => s.to_string(),
                    None => String::from_utf8_lossy(r.name_bytes()).to_string(),
                };
                let msg = format!(
                    "Fast-Forward: Setting {} to id: {}",
                    name,
                    fetch_commit.id()
                );
                r.set_target(fetch_commit.id(), &msg)?;
                repo.set_head(&name)?;
                repo.checkout_head(Some(
                    git2::build::CheckoutBuilder::default()
                        // For some reason the force is required to make the working directory actually get updated
                        // I suspect we should be adding some logic to handle dirty working directory states
                        // but this is just an example so maybe not.
                        .force(),
                ))?;
            }
            Err(_) => {
                // The branch doesn't exist so just set the reference to the
                // commit directly. Usually this is because you are pulling
                // into an empty repository.
                repo.reference(
                    &refname,
                    fetch_commit.id(),
                    true,
                    &format!("Setting {} to {}", head_branch, fetch_commit.id()),
                )?;
                repo.set_head(&refname)?;
                repo.checkout_head(Some(
                    git2::build::CheckoutBuilder::default()
                        .allow_conflicts(true)
                        .conflict_style_merge(true)
                        .force(),
                ))?;
            }
        };
    }
    Ok(())
}
