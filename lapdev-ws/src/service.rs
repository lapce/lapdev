use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyperlocal::Uri;
use lapdev_common::{
    BuildTarget, Container, ContainerInfo, CpuCore, CreateWorkspaceRequest, DeleteWorkspaceRequest,
    GitBranch, NewContainer, NewContainerEndpointSettings, NewContainerHostConfig,
    NewContainerNetwork, NewContainerNetworkingConfig, PrebuildInfo, ProjectRequest, RepoBuildInfo,
    RepoBuildOutput, RepoContent, RepoContentPosition, StartWorkspaceRequest, StopWorkspaceRequest,
};
use lapdev_guest_agent::{LAPDEV_CMDS, LAPDEV_IDE_CMDS, LAPDEV_SSH_PUBLIC_KEY};
use lapdev_rpc::{
    error::ApiError, ConductorServiceClient, InterWorkspaceService, InterWorkspaceServiceClient,
    WorkspaceService,
};
use tarpc::{context, tokio_serde::formats::Json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};
use uuid::Uuid;

use crate::server::{unix_client, WorkspaceServer, LAPDEV_WS_VERSION};

#[derive(Clone)]
pub struct WorkspaceRpcService {
    pub id: Uuid,
    pub server: WorkspaceServer,
    pub conductor_client: ConductorServiceClient,
}

impl WorkspaceService for WorkspaceRpcService {
    async fn ping(self, _context: tarpc::context::Context) -> String {
        "pong".to_string()
    }

    async fn version(self, _context: tarpc::context::Context) -> String {
        LAPDEV_WS_VERSION.to_string()
    }

    async fn get_project_branches(
        self,
        _context: tarpc::context::Context,
        req: ProjectRequest,
    ) -> Result<Vec<GitBranch>, ApiError> {
        self.server.project_branches(&req).await
    }

    async fn create_project(
        self,
        _context: tarpc::context::Context,
        req: ProjectRequest,
    ) -> Result<(), ApiError> {
        self.server
            .create_project(req.id, req.osuser, req.repo_url, req.auth)
            .await
    }

    async fn create_workspace(
        self,
        _context: tarpc::context::Context,
        ws_req: CreateWorkspaceRequest,
    ) -> Result<(), ApiError> {
        if ws_req.image.starts_with("ghcr.io") {
            let _ = self
                .server
                .pull_container_image(
                    &self.conductor_client,
                    &ws_req.osuser,
                    &ws_req.image,
                    &BuildTarget::Workspace {
                        id: ws_req.id,
                        name: ws_req.workspace_name.clone(),
                    },
                )
                .await;
        }
        let image_info = self
            .server
            .container_image_info(&ws_req.osuser, &ws_req.image)
            .await?;

        let client = unix_client();
        let uid = self.server.os_user_uid(&ws_req.osuser).await?;
        let socket = &format!("/run/user/{uid}/podman/podman.sock");
        if ws_req.create_network {
            let url = Uri::new(socket, "/networks/create");
            let body = serde_json::to_string(&NewContainerNetwork {
                name: ws_req.network_name.clone(),
            })?;
            let req = hyper::Request::builder()
                .method(hyper::Method::POST)
                .uri(url)
                .header("Content-Type", "application/json")
                .body(Full::<Bytes>::new(Bytes::from(body)))?;
            let resp = client.request(req).await?;
            let status = resp.status();
            let body = resp.collect().await?.to_bytes();
            if status != 201 {
                let err = String::from_utf8(body.to_vec())?;
                return Err(anyhow!(format!(
                    "create container network {} error: {err}",
                    ws_req.network_name
                ))
                .into());
            }

            {
                // workround for a bug in unbuntu 22.04
                // https://bugs.launchpad.net/ubuntu/+source/libpod/+bug/2024394
                let cni_file = format!(
                    "/home/{}/.config/cni/net.d/{}.conflist",
                    ws_req.osuser, ws_req.network_name
                );
                if let Ok(content) = tokio::fs::read_to_string(&cni_file).await {
                    if content.contains("\"cniVersion\": \"1.0.0\",") {
                        let content = content
                            .replace("\"cniVersion\": \"1.0.0\",", "\"cniVersion\": \"0.4.0\",");
                        let _ = tokio::fs::write(&cni_file, content).await;
                    }
                }
            }
        }

        let mut cmd = Vec::new();
        if let Some(c) = image_info.config.cmd.as_ref() {
            cmd = c.to_owned();
        }
        if let Some(entrypoint) = image_info.config.entrypoint.as_ref() {
            if !entrypoint.is_empty() {
                cmd = entrypoint.to_owned();
                if let Some(c) = image_info.config.cmd.as_ref() {
                    cmd.extend_from_slice(c);
                }
            }
        }

        let cmds = if !cmd.is_empty() { vec![cmd] } else { vec![] };
        let cmds = serde_json::to_string(&cmds)?;
        let ide_cmds = serde_json::to_string(&vec![
            "code-server",
            "--auth",
            "none",
            "--bind-addr",
            "0.0.0.0:30000",
            &format!("/workspaces/{}", ws_req.repo_name),
        ])?;

        let url = Uri::new(
            socket,
            &format!("/containers/create?name={}", ws_req.workspace_name),
        );
        let mut env: Vec<String> = ws_req
            .env
            .into_iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        env.extend_from_slice(&[
            format!("{LAPDEV_SSH_PUBLIC_KEY}={}", ws_req.ssh_public_key),
            format!("{LAPDEV_IDE_CMDS}={}", ide_cmds),
            format!("{LAPDEV_CMDS}={}", cmds),
        ]);

        let mut exposed_ports = image_info.config.exposed_ports.unwrap_or_default();
        exposed_ports.insert("22/tcp".to_string(), HashMap::new());
        exposed_ports.insert("30000/tcp".to_string(), HashMap::new());

        let mut body = NewContainer {
            hostname: ws_req.workspace_name.clone(),
            user: "root".to_string(),
            image: ws_req.image.clone(),
            entrypoint: vec!["/lapdev-guest-agent".to_string()],
            env,
            exposed_ports,
            working_dir: format!("/workspaces/{}", ws_req.repo_name),
            host_config: NewContainerHostConfig {
                publish_all_ports: true,
                binds: vec![format!(
                    "{}:/workspaces",
                    self.server
                        .workspace_folder(&ws_req.osuser, &ws_req.volume_name,)
                )],
                cpu_period: None,
                cpu_quota: None,
                cpuset_cpus: None,
                memory: ws_req.memory * 1024 * 1024 * 1024,
                network_mode: ws_req.network_name.clone(),
                cap_add: vec!["NET_RAW".to_string()],
                security_opt: Some(vec!["label=disable".to_string()]),
                storage_opt: None,
            },
            networking_config: NewContainerNetworkingConfig {
                endpoints_config: if let Some(service) = ws_req.service.as_ref() {
                    vec![(
                        ws_req.network_name.clone(),
                        NewContainerEndpointSettings {
                            aliases: vec![service.clone()],
                        },
                    )]
                    .into_iter()
                    .collect()
                } else {
                    HashMap::new()
                },
            },
        };

        match &ws_req.cpus {
            CpuCore::Dedicated(cores) => {
                body.host_config.cpuset_cpus = Some(
                    cores
                        .iter()
                        .map(|c| c.to_string())
                        .collect::<Vec<String>>()
                        .join(","),
                );
            }
            CpuCore::Shared(n) => {
                body.host_config.cpu_period = Some(100000);
                body.host_config.cpu_quota = Some(*n as i64 * 100000);
            }
        }

        let body = serde_json::to_string(&body)?;
        let req = hyper::Request::builder()
            .method(hyper::Method::POST)
            .uri(url)
            .header("Content-Type", "application/json")
            .body(Full::<Bytes>::new(Bytes::from(body)))?;
        let resp = client.request(req).await?;
        let status = resp.status();
        let body = resp.collect().await?.to_bytes();
        if status != 201 {
            let err = String::from_utf8(body.to_vec())?;
            return Err(anyhow!(format!(
                "create container {} error: {err}",
                ws_req.workspace_name
            ))
            .into());
        }

        let _container: Container = serde_json::from_slice(&body)?;

        Ok(())
    }

    async fn start_workspace(
        self,
        _context: context::Context,
        ws_req: StartWorkspaceRequest,
    ) -> Result<ContainerInfo, ApiError> {
        let uid = self.server.os_user_uid(&ws_req.osuser).await?;
        let socket = &format!("/run/user/{uid}/podman/podman.sock");
        let client = unix_client();
        let url = Uri::new(
            socket,
            &format!("/containers/{}/start", ws_req.workspace_name),
        );
        let req = hyper::Request::builder()
            .method(hyper::Method::POST)
            .uri(url)
            .body(Full::<Bytes>::new(Bytes::new()))?;
        let resp = client.request(req).await?;
        let status = resp.status();
        let body = resp.collect().await?.to_bytes();
        if status != 204 {
            let err = String::from_utf8(body.to_vec())?;
            return Err(anyhow!("start container error: {err}").into());
        }

        let url = Uri::new(
            socket,
            &format!("/containers/{}/json", ws_req.workspace_name),
        );
        let req = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri(url)
            .body(Full::<Bytes>::new(Bytes::new()))?;
        let resp = client.request(req).await?;
        let status = resp.status();
        let body = resp.collect().await?.to_bytes();
        if status != 200 {
            let err = String::from_utf8(body.to_vec())?;
            return Err(anyhow!("get container info error: {err}").into());
        }
        let info: ContainerInfo = serde_json::from_slice(&body)?;
        Ok(info)
    }

    async fn delete_workspace(
        self,
        _context: context::Context,
        req: DeleteWorkspaceRequest,
    ) -> Result<(), ApiError> {
        let uid = self.server.os_user_uid(&req.osuser).await?;
        let socket = format!("/run/user/{uid}/podman/podman.sock");

        let client = unix_client();
        {
            let url = Uri::new(&socket, &format!("/containers/{}/stop", req.workspace_name));
            let req = hyper::Request::builder()
                .method(hyper::Method::POST)
                .uri(url)
                .body(Full::<Bytes>::new(Bytes::new()))?;
            let resp = client.request(req).await?;
            let status = resp.status();
            if status != 204 && status != 304 && status != 404 {
                let body = resp.collect().await?.to_bytes();
                let err = String::from_utf8(body.to_vec())?;
                return Err(anyhow!("stop container error: {status} {err}").into());
            }
        }

        {
            let url = Uri::new(&socket, &format!("/containers/{}", req.workspace_name));
            let req = hyper::Request::builder()
                .method(hyper::Method::DELETE)
                .uri(url)
                .body(Full::<Bytes>::new(Bytes::new()))?;
            let resp = client.request(req).await?;
            let status = resp.status();
            if status != 204 && status != 404 {
                let body = resp.collect().await?.to_bytes();
                let err = String::from_utf8(body.to_vec())?;
                return Err(anyhow!("remove container error: {status} {err}").into());
            }
        }

        {
            let folder = self
                .server
                .workspace_folder(&req.osuser, &req.workspace_name);
            let _ = tokio::fs::remove_dir_all(&folder).await;
        }

        for image in req.images {
            if image.starts_with("ghcr.io") {
                // we don't need to save the default images
                continue;
            }
            let _ = self.server.delete_image(&req.osuser, &image).await;
        }

        if let Some(network) = req.network {
            self.server.delete_network(&req.osuser, &network).await?;
        }

        Ok(())
    }

    async fn stop_workspace(
        self,
        _context: context::Context,
        ws_req: StopWorkspaceRequest,
    ) -> Result<(), ApiError> {
        let uid = self.server.os_user_uid(&ws_req.osuser).await?;
        let socket = &format!("/run/user/{uid}/podman/podman.sock");
        let client = unix_client();
        let url = Uri::new(
            socket,
            &format!("/containers/{}/stop", ws_req.workspace_name),
        );
        let req = hyper::Request::builder()
            .method(hyper::Method::POST)
            .uri(url)
            .body(Full::<Bytes>::new(Bytes::new()))?;
        let resp = client.request(req).await?;
        let status = resp.status();
        if status != 204 && status != 304 {
            let body = resp.collect().await?.to_bytes();
            let err = String::from_utf8(body.to_vec())?;
            return Err(anyhow!("stop container error: {status} {err}").into());
        }

        Ok(())
    }

    async fn transfer_repo(
        self,
        _context: context::Context,
        info: RepoBuildInfo,
        repo: RepoContent,
    ) -> Result<(), ApiError> {
        let folder = self.server.build_repo_folder(&info);
        let path = PathBuf::from(&folder).join("repo.tar.zst");
        let mut file = if repo.position == RepoContentPosition::Initial {
            self.server.os_user_uid(&info.osuser).await?;
            Command::new("su")
                .arg("-")
                .arg(&info.osuser)
                .arg("-c")
                .arg(format!("mkdir -p {folder}"))
                .spawn()?
                .wait()
                .await?;
            tokio::fs::File::create(path).await?
        } else {
            tokio::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .await?
        };
        file.write_all(&repo.content).await?;
        Ok(())
    }

    async fn unarchive_repo(
        self,
        _context: context::Context,
        info: RepoBuildInfo,
    ) -> Result<(), ApiError> {
        let folder = PathBuf::from(self.server.build_repo_folder(&info));
        {
            let folder = folder.clone();
            let path = folder.join("repo.tar.zst");
            let local_path = path.clone();
            tokio::task::spawn_blocking(move || -> Result<()> {
                let archive = std::fs::File::open(&local_path)?;
                let tar = zstd::Decoder::new(archive)?;
                let mut archive = tar::Archive::new(tar);
                archive.unpack(&folder)?;
                Ok(())
            })
            .await??;
            tokio::fs::remove_file(&path).await?;
        }
        tokio::process::Command::new("chown")
            .arg("-R")
            .arg(format!("{}:{}", info.osuser, info.osuser))
            .arg(&folder)
            .output()
            .await?;
        Ok(())
    }

    async fn build_repo(self, _context: context::Context, info: RepoBuildInfo) -> RepoBuildOutput {
        let result = self.server.build_repo(&info, &self.conductor_client).await;

        match result {
            Ok(result) => return result,
            Err(e) => {
                if let ApiError::InternalError(e) = e {
                    tracing::error!("build repo {info:?} error: {e}");
                } else {
                    tracing::error!("build repo {info:?} error: {e}");
                }
            }
        };

        let image_name = self.server.get_default_image(&info).await;
        let image = format!("ghcr.io/lapce/lapdev-devcontainer-{image_name}:latest");
        tracing::debug!("build repo pick default image {image_name}");
        RepoBuildOutput::Image(image)
    }

    async fn copy_prebuild_image(
        self,
        _context: tarpc::context::Context,
        prebuild: PrebuildInfo,
        output: RepoBuildOutput,
        dest_osuser: String,
        target: BuildTarget,
    ) -> Result<(), ApiError> {
        let images = match output {
            RepoBuildOutput::Compose(services) => services.into_iter().map(|s| s.image).collect(),
            RepoBuildOutput::Image(tag) => vec![tag],
        };

        let prebuild_folder =
            PathBuf::from(self.server.prebuild_folder(&prebuild.osuser, prebuild.id));

        for image in images {
            if image.starts_with("ghcr.io") {
                // we don't need to save the default images
                continue;
            }
            if self
                .server
                .container_image_info(&dest_osuser, &image)
                .await
                .is_ok()
            {
                continue;
            }

            let dst_prebuild_path =
                PathBuf::from(self.server.prebuild_folder(&dest_osuser, prebuild.id));
            let dst_archive_path =
                dst_prebuild_path.join(format!("{}.tar", image.replace(':', "-")));
            if prebuild.osuser != dest_osuser {
                let archive_path = prebuild_folder.join(format!("{image}.tar"));
                let _ = tokio::fs::create_dir_all(&dst_prebuild_path).await;
                let output = tokio::process::Command::new("cp")
                    .arg(&archive_path)
                    .arg(&dst_archive_path)
                    .output()
                    .await?;
                if !output.status.success() {
                    return Err(anyhow!("can't copy prebuild image archive").into());
                }
                tokio::process::Command::new("chown")
                    .arg("-R")
                    .arg(format!("{dest_osuser}:{dest_osuser}"))
                    .arg(&dst_prebuild_path)
                    .output()
                    .await?;
            }
            let mut child = tokio::process::Command::new("su")
                .arg("-")
                .arg(&dest_osuser)
                .arg("-c")
                .arg(format!(
                    "podman image load -i {}",
                    dst_archive_path
                        .to_str()
                        .ok_or_else(|| anyhow!("archive path invalid"))?
                ))
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;
            let stderr_log = self
                .server
                .update_build_std_output(&self.conductor_client, &mut child, &target)
                .await;
            let status = child.wait().await?;
            if !status.success() {
                return Err(ApiError::InternalError(format!(
                    "container image not loaded succesfully: {:?}",
                    stderr_log.lock().await
                )));
            }
        }

        Ok(())
    }

    async fn copy_prebuild_content(
        self,
        _context: tarpc::context::Context,
        prebuild: PrebuildInfo,
        info: RepoBuildInfo,
    ) -> Result<(), ApiError> {
        let workspace_name = match info.target {
            BuildTarget::Workspace { name, .. } => name,
            BuildTarget::Prebuild { .. } => {
                return Err(ApiError::InternalError(
                    "copy prebuild content destiantion should be workspace".to_string(),
                ))
            }
        };
        let repo_folder = PathBuf::from(self.server.prebuild_folder(&prebuild.osuser, prebuild.id))
            .join(&info.repo_name);
        let workspace_folder = self.server.workspace_folder(&info.osuser, &workspace_name);
        tokio::fs::create_dir(&workspace_folder).await?;
        Command::new("cp")
            .arg("-r")
            .arg(&repo_folder)
            .arg(&workspace_folder)
            .spawn()?
            .wait()
            .await?;
        tokio::process::Command::new("chown")
            .arg("-R")
            .arg(format!("{}:{}", info.osuser, info.osuser))
            .arg(&workspace_folder)
            .output()
            .await?;
        Ok(())
    }

    async fn create_prebuild_archive(
        self,
        _context: tarpc::context::Context,
        output: RepoBuildOutput,
        prebuild: PrebuildInfo,
        repo_name: String,
    ) -> Result<(), ApiError> {
        let prebuild_folder =
            PathBuf::from(self.server.prebuild_folder(&prebuild.osuser, prebuild.id));
        let repo_folder = prebuild_folder.join(&repo_name);

        {
            let archive_path = prebuild_folder.join("repo.tar.zst");
            tokio::task::spawn_blocking(move || compress_folder(&repo_folder, &archive_path))
                .await??;
        }

        let images = match output {
            RepoBuildOutput::Compose(services) => services.into_iter().map(|s| s.image).collect(),
            RepoBuildOutput::Image(tag) => vec![tag],
        };

        for image in images {
            if image.starts_with("ghcr.io") {
                // we don't need to save the default images
                continue;
            }
            let archive_path = prebuild_folder.join(format!("{}.tar", image.replace(':', "-")));
            let mut child = Command::new("su")
                .arg("-")
                .arg(&prebuild.osuser)
                .arg("-c")
                .arg(format!(
                    "podman image save -o {} {image}",
                    archive_path
                        .to_str()
                        .ok_or_else(|| anyhow!("archive path invalid"))?
                ))
                .spawn()?;

            let status = child.wait().await?;
            if !status.success() {
                return Err(ApiError::InternalError(
                    "can't save prebuild image".to_string(),
                ));
            }
        }

        Ok(())
    }

    async fn delete_prebuild(
        self,
        _context: tarpc::context::Context,
        prebuild: PrebuildInfo,
        output: Option<RepoBuildOutput>,
    ) -> Result<(), ApiError> {
        let prebuild_folder = self.server.prebuild_folder(&prebuild.osuser, prebuild.id);
        if let Err(e) = tokio::fs::remove_dir_all(&prebuild_folder).await {
            tracing::error!("delete prebuild folder {prebuild_folder} error: {e:#}");
        }

        if let Some(output) = output {
            let images = match output {
                RepoBuildOutput::Compose(services) => {
                    services.into_iter().map(|s| s.image).collect()
                }
                RepoBuildOutput::Image(tag) => vec![tag],
            };

            for image in images {
                if image.starts_with("ghcr.io") {
                    // we don't need to save the default images
                    continue;
                }
                self.server.delete_image(&prebuild.osuser, &image).await?;
            }
        }

        Ok(())
    }

    async fn transfer_prebuild(
        self,
        _context: tarpc::context::Context,
        prebuild: PrebuildInfo,
        output: RepoBuildOutput,
        dst_host: String,
        dst_port: u16,
    ) -> Result<(), ApiError> {
        let conn =
            tarpc::serde_transport::tcp::connect((dst_host.clone(), dst_port), Json::default)
                .await
                .with_context(|| {
                    format!("ws service trying to connect to {dst_host}:{dst_port} failed")
                })?;
        let inter_ws_client =
            InterWorkspaceServiceClient::new(tarpc::client::Config::default(), conn).spawn();

        let mut files = vec!["repo.tar.zst".to_string()];
        let images = match output {
            RepoBuildOutput::Compose(services) => services.into_iter().map(|s| s.image).collect(),
            RepoBuildOutput::Image(tag) => vec![tag],
        };
        for image in images {
            if image.starts_with("ghcr.io") {
                // we don't need to save the default images
                continue;
            }
            files.push(format!("{}.tar", image.replace(':', "-")));
        }

        let folder = PathBuf::from(self.server.prebuild_folder(&prebuild.osuser, prebuild.id));
        for file_name in files {
            let mut buf = vec![0u8; 1024 * 1024];
            let mut file = tokio::fs::File::open(&folder.join(&file_name))
                .await
                .with_context(|| {
                    format!(
                        "transfer prebuild trying to open file {}",
                        folder.join(&file_name).to_string_lossy()
                    )
                })?;
            let mut initial = true;
            while let Ok(n) = file.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                inter_ws_client
                    .transfer_prebuild_archive(
                        context::current(),
                        prebuild.clone(),
                        file_name.clone(),
                        buf[..n].to_vec(),
                        !initial,
                    )
                    .await
                    .with_context(|| {
                        format!("inter ws transfer prebuld rpc to {dst_host}:{dst_port} failed")
                    })??;
                initial = false;
            }
        }
        Ok(())
    }

    async fn unarchive_prebuild(
        self,
        _context: tarpc::context::Context,
        prebuild: PrebuildInfo,
        repo_name: String,
    ) -> Result<(), ApiError> {
        let folder = PathBuf::from(self.server.prebuild_folder(&prebuild.osuser, prebuild.id));
        {
            let folder = folder.clone();
            let path = folder.join("repo.tar.zst");
            let repo_folder = folder.join(repo_name);
            tokio::task::spawn_blocking(move || -> Result<()> {
                let archive = std::fs::File::open(&path)?;
                let tar = zstd::Decoder::new(archive)?;
                let mut archive = tar::Archive::new(tar);
                archive.unpack(&repo_folder)?;
                Ok(())
            })
            .await??;
        }
        tokio::process::Command::new("chown")
            .arg("-R")
            .arg(format!("{}:{}", prebuild.osuser, prebuild.osuser))
            .arg(&folder)
            .output()
            .await?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct InterWorkspaceRpcService {
    pub server: WorkspaceServer,
}

impl InterWorkspaceService for InterWorkspaceRpcService {
    async fn transfer_prebuild_archive(
        self,
        _context: tarpc::context::Context,
        archive: PrebuildInfo,
        file: String,
        content: Vec<u8>,
        append: bool,
    ) -> Result<(), ApiError> {
        let folder = self.server.prebuild_folder(&archive.osuser, archive.id);
        let path = PathBuf::from(&folder).join(file);
        let mut file = if !append {
            self.server.os_user_uid(&archive.osuser).await?;
            Command::new("su")
                .arg("-")
                .arg(&archive.osuser)
                .arg("-c")
                .arg(format!("mkdir -p {folder}"))
                .spawn()?
                .wait()
                .await?;
            tokio::fs::File::create(path).await?
        } else {
            tokio::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .await?
        };
        file.write_all(&content).await?;
        Ok(())
    }
}

fn compress_folder(src_path: &Path, archive_path: &Path) -> Result<()> {
    let archive = std::fs::File::create(archive_path)?;
    let encoder = zstd::Encoder::new(archive, 0)?.auto_finish();
    let mut tar = tar::Builder::new(encoder);
    tar.append_dir_all(".", src_path)?;
    Ok(())
}
