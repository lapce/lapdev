use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use data_encoding::BASE64_MIME;
use futures::{channel::mpsc::UnboundedReceiver, stream::AbortHandle, SinkExt, StreamExt};
use git2::{Cred, FetchOptions, FetchPrune, RemoteCallbacks, Repository};
use lapdev_common::{
    utils::rand_string, AuditAction, AuditResourceKind, BuildTarget, CreateWorkspaceRequest,
    DeleteWorkspaceRequest, GitBranch, NewProject, NewProjectResponse, NewWorkspace,
    NewWorkspaceResponse, PrebuildInfo, PrebuildStatus, PrebuildUpdateEvent, RepoBuildInfo,
    RepoBuildOutput, RepoContent, RepoContentPosition, RepoSource, StartWorkspaceRequest,
    StopWorkspaceRequest, UsageResourceKind, WorkspaceStatus, WorkspaceUpdateEvent,
    LAPDEV_DEFAULT_OSUSER,
};
use lapdev_common::{PrebuildReplicaStatus, WorkspaceHostStatus};
use lapdev_db::{api::DbApi, entities};
use lapdev_enterprise::enterprise::Enterprise;
use lapdev_rpc::{
    error::ApiError, long_running_context, spawn_twoway, ConductorService, WorkspaceServiceClient,
};
use russh::keys::{
    key::{KeyPair, PublicKey, SignatureHash},
    pkcs8, PublicKeyBase64,
};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
    TransactionTrait,
};
use serde::Deserialize;
use sqlx::postgres::PgNotification;
use tarpc::{
    context,
    server::{BaseChannel, Channel},
    tokio_serde::formats::Bincode,
};
use tokio::{
    io::AsyncReadExt,
    sync::{Mutex, RwLock},
};
use uuid::Uuid;

use crate::{
    rpc::ConductorRpc,
    scheduler::{self, LAPDEV_CPU_OVERCOMMIT},
};

#[derive(Clone, Default)]
pub struct WorkspaceUpdate {
    pub seen: Vec<WorkspaceUpdateEvent>,
    pub subscribers: Vec<futures::channel::mpsc::UnboundedSender<WorkspaceUpdateEvent>>,
}

#[derive(Clone, Default)]
pub struct PrebuildUpdate {
    pub seen: Vec<PrebuildUpdateEvent>,
    pub subscribers: Vec<futures::channel::mpsc::UnboundedSender<PrebuildUpdateEvent>>,
}

#[derive(Clone, Default)]
pub struct PrebuildReplicaUpdate {
    pub subscribers: Vec<futures::channel::mpsc::UnboundedSender<PrebuildReplicaStatus>>,
}

#[derive(Debug, Deserialize)]
struct TableUpdatePayload {
    id: Uuid,
    #[allow(dead_code)]
    table: String,
    #[allow(dead_code)]
    action_type: String,
}

#[derive(Debug, Deserialize)]
struct StatusUpdatePayload {
    id: Uuid,
    table: String,
    status: String,
    user_id: Option<Uuid>,
}

struct WorkspaceHostInfo {
    model: entities::workspace_host::Model,
    latency: Option<u128>,
}

#[derive(Debug, Clone)]
pub struct RepoDetails {
    pub url: String,
    pub name: String,
    pub branch: String,
    pub commit: String,
    pub project: Option<entities::project::Model>,
    pub auth: (String, String),
    // head branch name
    pub head: String,
    // all branches and their commit id
    pub branches: Vec<(String, String)>,
}

#[derive(Clone)]
pub struct Conductor {
    version: String,
    rpc_aborts: Arc<Mutex<HashMap<Uuid, AbortHandle>>>,
    rpcs: Arc<Mutex<HashMap<Uuid, WorkspaceServiceClient>>>,
    ws_hosts: Arc<Mutex<HashMap<Uuid, WorkspaceHostInfo>>>,
    region: Arc<RwLock<String>>,
    pub hostnames: Arc<RwLock<HashMap<String, String>>>,
    pub cpu_overcommit: Arc<RwLock<usize>>,
    // updates for a single prebuild, including the image building outputs
    pub prebuild_updates: Arc<Mutex<HashMap<Uuid, PrebuildUpdate>>>,
    // updates for a prebuild replica
    pub prebuild_replica_updates: Arc<Mutex<HashMap<Uuid, PrebuildReplicaUpdate>>>,
    // updates for a single workspace, including the image building outputs
    pub ws_updates: Arc<Mutex<HashMap<Uuid, WorkspaceUpdate>>>,
    // all workpaces updates for an account
    #[allow(clippy::complexity)]
    pub all_workspace_updates: Arc<
        Mutex<HashMap<Uuid, Vec<futures::channel::mpsc::UnboundedSender<(Uuid, WorkspaceStatus)>>>>,
    >,
    pub enterprise: Arc<Enterprise>,
    pub db: DbApi,
}

impl Conductor {
    pub async fn new(version: &str, db: DbApi) -> Result<Self> {
        tokio::fs::create_dir_all("/var/lib/lapdev/projects/")
            .await
            .with_context(|| "trying to create /var/lib/lapdev/projects/")?;
        let cpu_overcommit = db
            .get_config(LAPDEV_CPU_OVERCOMMIT)
            .await
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(4)
            .max(1);

        let enterprise = Arc::new(Enterprise::new(db.clone()).await?);
        let hostnames = enterprise.get_hostnames().await.unwrap_or_default();
        let conductor = Self {
            version: version.to_string(),
            rpcs: Default::default(),
            rpc_aborts: Default::default(),
            prebuild_updates: Default::default(),
            prebuild_replica_updates: Default::default(),
            ws_hosts: Default::default(),
            ws_updates: Default::default(),
            region: Default::default(),
            hostnames: Arc::new(RwLock::new(hostnames)),
            cpu_overcommit: Arc::new(RwLock::new(cpu_overcommit)),
            all_workspace_updates: Default::default(),
            enterprise,
            db,
        };

        {
            let conductor = conductor.clone();
            tokio::spawn(async move {
                conductor.monitor_workspace_hosts().await;
            });
        }

        {
            let conductor = conductor.clone();
            tokio::spawn(async move {
                if let Err(e) = conductor.monitor_status_updates().await {
                    tracing::error!("conductor monitor status updates error: {e}");
                }
            });
        }

        {
            let conductor = conductor.clone();
            tokio::spawn(async move {
                conductor.monitor_auto_start_stop().await;
            });
        }

        Ok(conductor)
    }

    async fn listen_table_update(&self) -> Result<()> {
        let pool = self
            .db
            .pool
            .clone()
            .ok_or_else(|| anyhow!("db doesn't have pg pool"))?;
        let mut listener = sqlx::postgres::PgListener::connect_with(&pool).await?;
        listener.listen("table_update").await?;
        loop {
            let notification = listener.recv().await?;
            let _ = self
                .handle_workspace_host_update_notification(notification)
                .await;
        }
    }

    async fn handle_workspace_host_update_notification(
        &self,
        notification: PgNotification,
    ) -> Result<()> {
        let payload: TableUpdatePayload = serde_json::from_str(notification.payload())?;
        let workspace_host = self
            .db
            .get_workspace_host(payload.id)
            .await?
            .ok_or_else(|| anyhow!("can't find workspace host"))?;

        if workspace_host.deleted_at.is_some() {
            // the workspace host was deleted
            self.ws_hosts.lock().await.remove(&workspace_host.id);
            self.rpcs.lock().await.remove(&workspace_host.id);
            if let Some(abort) = self.rpc_aborts.lock().await.remove(&workspace_host.id) {
                abort.abort();
            }
        } else {
            let mut ws_hosts = self.ws_hosts.lock().await;
            if let Some(info) = ws_hosts.get_mut(&workspace_host.id) {
                info.model = workspace_host;
            } else {
                let id = workspace_host.id;
                let host = workspace_host.host.clone();
                let port = workspace_host.port as u16;
                ws_hosts.insert(
                    id,
                    WorkspaceHostInfo {
                        model: workspace_host,
                        latency: None,
                    },
                );
                let conductor = self.clone();
                tokio::spawn(async move {
                    conductor.connect_workspace_host(id, host, port).await;
                });
            }
        }

        self.decide_current_region().await;

        Ok(())
    }

    async fn monitor_workspace_hosts(&self) {
        {
            let conductor = self.clone();
            tokio::spawn(async move {
                if let Err(e) = conductor.listen_table_update().await {
                    println!("listen table update error: {e}");
                } else {
                    println!("listen table update exited");
                }
            });
        }

        let mut ws_hosts = self.ws_hosts.lock().await;
        let hosts = self.get_workspace_hosts().await;

        for workspace_host in hosts {
            if !ws_hosts.contains_key(&workspace_host.id) {
                let id = workspace_host.id;
                let host = workspace_host.host.clone();
                let port = workspace_host.port as u16;
                ws_hosts.insert(
                    id,
                    WorkspaceHostInfo {
                        model: workspace_host,
                        latency: None,
                    },
                );
                let conductor = self.clone();
                tokio::spawn(async move {
                    conductor.connect_workspace_host(id, host, port).await;
                });
            }
        }
    }

    async fn monitor_status_updates(&self) -> Result<()> {
        let pool = self
            .db
            .pool
            .clone()
            .ok_or_else(|| anyhow!("db doesn't have pg pool"))?;
        let mut listener = sqlx::postgres::PgListener::connect_with(&pool).await?;
        listener.listen("status_update").await?;
        loop {
            let notification = listener.recv().await?;
            if let Err(e) = self.handle_status_update_notification(notification).await {
                tracing::error!("handle status update notification error: {e:#}");
            }
        }
    }

    async fn handle_status_update_notification(&self, notification: PgNotification) -> Result<()> {
        tracing::debug!(
            "status update notification payload: {}",
            notification.payload()
        );
        let payload: StatusUpdatePayload = serde_json::from_str(notification.payload())
            .with_context(|| format!("trying to deserialize payload {}", notification.payload()))?;
        match payload.table.as_str() {
            "workspace" => {
                let status = WorkspaceStatus::from_str(&payload.status).with_context(|| {
                    format!("trying to deserialize workspace status {}", payload.status)
                })?;
                self.add_workspace_update_event(
                    payload.user_id,
                    payload.id,
                    WorkspaceUpdateEvent::Status(status),
                )
                .await;
            }
            "prebuild" => {
                let status = PrebuildStatus::from_str(&payload.status).with_context(|| {
                    format!("trying to deserialize prebuild status {}", payload.status)
                })?;
                self.add_prebuild_update_event(payload.id, PrebuildUpdateEvent::Status(status))
                    .await;
            }
            "prebuild_replica" => {
                let status =
                    PrebuildReplicaStatus::from_str(&payload.status).with_context(|| {
                        format!(
                            "trying to deserialize prebuild replica status {}",
                            payload.status
                        )
                    })?;
                self.add_prebuild_replica_update_event(payload.id, status)
                    .await;
            }
            _ => {
                return Err(anyhow!("status update table {} not handled", payload.table));
            }
        }
        Ok(())
    }

    async fn get_workspace_hosts(&self) -> Vec<entities::workspace_host::Model> {
        entities::workspace_host::Entity::find()
            .filter(entities::workspace_host::Column::DeletedAt.is_null())
            .all(&self.db.conn)
            .await
            .unwrap_or_default()
    }

    async fn connect_workspace_host(&self, id: Uuid, host: String, port: u16) {
        loop {
            {
                if !self.ws_hosts.lock().await.contains_key(&id) {
                    // this means the workspace host server is removed,
                    // so we don't connect to it anymore.
                    return;
                }
            }

            let _ = self.connect_workspace_host_once(id, &host, port).await;

            {
                self.rpcs.lock().await.remove(&id);
                self.rpc_aborts.lock().await.remove(&id);
            }

            let _ = tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
    }

    async fn decide_current_region(&self) {
        if !self.enterprise.has_valid_license().await {
            return;
        }

        let mut region_latencies = HashMap::new();
        for (_, ws_host) in self.ws_hosts.lock().await.iter() {
            if let Some(latency) = ws_host.latency {
                if let Some(existing) = region_latencies.get_mut(&ws_host.model.region) {
                    if latency < *existing {
                        *existing = latency;
                    }
                } else {
                    region_latencies.insert(ws_host.model.region.clone(), latency);
                }
            }
        }

        if let Some((region, _)) = region_latencies.iter().min_by_key(|(_, l)| **l) {
            let mut current_region = self.region.write().await;
            if &*current_region != region {
                tracing::info!("change current region from {current_region:?} to {region:?}");
                *current_region = region.to_owned();
            }
        }
    }

    async fn connect_workspace_host_once(&self, id: Uuid, host: &str, port: u16) -> Result<()> {
        tracing::debug!("start to connect to workspace host {host}:{port}");
        let conn = tarpc::serde_transport::tcp::connect((host, port), Bincode::default).await?;
        let (server_chan, client_chan, abort_handle) = spawn_twoway(conn);
        let ws_client =
            WorkspaceServiceClient::new(tarpc::client::Config::default(), client_chan).spawn();
        {
            self.rpcs.lock().await.insert(id, ws_client.clone());
            self.rpc_aborts
                .lock()
                .await
                .insert(id, abort_handle.clone());
        }

        {
            let ws_client = ws_client.clone();
            let conductor = self.clone();
            tokio::spawn(async move {
                let start = std::time::Instant::now();
                if let Ok(pong) = ws_client.ping(context::current()).await {
                    if pong == "pong" {
                        let latency = start.elapsed().as_millis();
                        {
                            if let Some(info) = conductor.ws_hosts.lock().await.get_mut(&id) {
                                info.latency = Some(latency);
                            }
                        }

                        conductor.decide_current_region().await;
                    }
                }
            });
        }

        let rpc = ConductorRpc {
            ws_host_id: id,
            conductor: self.clone(),
        };

        let rpc_future = tokio::spawn(
            BaseChannel::with_defaults(server_chan)
                .execute(rpc.serve())
                .for_each(|resp| async move {
                    tokio::spawn(resp);
                }),
        );

        let ws_version = ws_client.version(context::current()).await;
        if ws_version.as_deref().unwrap_or_default() != self.version {
            abort_handle.abort();
            let ws_host = self
                .db
                .get_workspace_host(id)
                .await?
                .ok_or_else(|| anyhow!("can't find workspace host in db"))?;
            if ws_host.status != WorkspaceHostStatus::VersionMismatch.to_string() {
                entities::workspace_host::ActiveModel {
                    id: ActiveValue::Set(id),
                    status: ActiveValue::Set(WorkspaceHostStatus::VersionMismatch.to_string()),
                    ..Default::default()
                }
                .update(&self.db.conn)
                .await?;
                tracing::error!(
                    "version mismatch on lapdev ({}), and lapdev-ws ({ws_version:?})",
                    self.version
                );
            }
        } else {
            entities::workspace_host::ActiveModel {
                id: ActiveValue::Set(id),
                status: ActiveValue::Set(WorkspaceHostStatus::Active.to_string()),
                ..Default::default()
            }
            .update(&self.db.conn)
            .await?;

            let _ = rpc_future.await;
            tracing::debug!("workspace host connection ended");

            entities::workspace_host::ActiveModel {
                id: ActiveValue::Set(id),
                status: ActiveValue::Set(WorkspaceHostStatus::Inactive.to_string()),
                ..Default::default()
            }
            .update(&self.db.conn)
            .await?;
        }

        Ok(())
    }

    async fn monitor_auto_start_stop(&self) {
        loop {
            let orgs = if self.enterprise.license.has_valid().await {
                self.enterprise
                    .auto_start_stop
                    .get_organization_auto_stop()
                    .await
                    .unwrap_or_default()
            } else {
                vec![]
            };

            if orgs.is_empty() {
                tokio::time::sleep(Duration::from_secs(60)).await;
            } else {
                for org in orgs {
                    let workspaces = self
                        .enterprise
                        .auto_start_stop
                        .organization_auto_stop_workspaces(org)
                        .await
                        .unwrap_or_default();
                    for workspace in workspaces {
                        tracing::info!(
                            "stop workspace {} because of auto stop timeout",
                            workspace.name
                        );
                        let _ = self.stop_workspace(workspace, None, None).await;
                    }
                }
            }
        }
    }

    fn format_repo_url(&self, repo: &str) -> String {
        let repo = repo.trim();
        let repo = if !repo.starts_with("http://")
            && !repo.starts_with("https://")
            && !repo.starts_with("ssh://")
        {
            format!("https://{repo}")
        } else {
            repo.to_string()
        };
        repo.strip_suffix('/')
            .map(|r| r.to_string())
            .unwrap_or(repo)
    }

    async fn get_raw_repo_details(
        &self,
        repo_url: &str,
        branch: Option<&str>,
        auth: (String, String),
    ) -> Result<RepoDetails, ApiError> {
        let (head, branches) = {
            let local_repo_url = repo_url.to_string();
            let repo_url = repo_url.to_string();
            let auth = auth.clone();
            tokio::task::spawn_blocking(move || {
                let repo = git2::Repository::init("/var/lib/lapdev")?;
                let mut remote = repo.remote_anonymous(&repo_url)?;

                let mut cbs = RemoteCallbacks::new();
                cbs.credentials(move |_, _, _| Cred::userpass_plaintext(&auth.0, &auth.1));
                let connection = remote.connect_auth(git2::Direction::Fetch, Some(cbs), None)?;

                let mut head = None;
                let mut branches = Vec::new();
                for remote in connection.list()?.iter() {
                    let name = remote.name();
                    if name == "HEAD" {
                        head = Some(remote.oid().to_string());
                    }
                    if let Some(branch) = name.strip_prefix("refs/heads/") {
                        branches.push((branch.to_string(), remote.oid().to_string()))
                    }
                }
                let head = head.and_then(|head| {
                    branches
                        .iter()
                        .find(|(_, c)| c == &head)
                        .map(|(b, _)| b.to_string())
                });
                Ok::<(Option<String>, Vec<(String, String)>), ApiError>((head, branches))
            })
            .await?
            .map_err(|e| {
                let err = if let ApiError::InternalError(e) = e {
                    e.to_string()
                } else {
                    e.to_string()
                };
                tracing::warn!("can't open repo {local_repo_url}: {err}");
                ApiError::RepositoryInvalid(
                    "Repository URL invalid or we don't have access to it".to_string(),
                )
            })?
        };
        let head = head.ok_or_else(|| {
            ApiError::RepositoryInvalid("repo doesn't have default branch".to_string())
        })?;

        let branch = branch
            .map(|b| b.to_string())
            .unwrap_or_else(|| head.clone());
        let Some(commit) = branches
            .iter()
            .find(|(b, _)| b == &branch)
            .map(|(_, c)| c.to_string())
        else {
            return Err(ApiError::RepositoryInvalid(format!(
                "repository does't have branch {branch}"
            )));
        };

        let path = repo_url
            .split('/')
            .last()
            .ok_or_else(|| ApiError::RepositoryInvalid("invalid repo path".to_string()))?;
        let repo_name = path.strip_suffix(".git").unwrap_or(path).to_string();

        Ok(RepoDetails {
            url: repo_url.to_string(),
            name: repo_name,
            branch,
            auth,
            commit,
            project: None,
            head,
            branches,
        })
    }

    pub async fn create_project(
        &self,
        user: entities::user::Model,
        org_id: Uuid,
        project: NewProject,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<NewProjectResponse, ApiError> {
        let repo = self.format_repo_url(&project.repo);
        let repo = self
            .get_raw_repo_details(
                &repo,
                None,
                (user.provider_login.clone(), user.access_token.clone()),
            )
            .await?;

        let txn = self.db.conn.begin().await?;
        if let Some(quota) = self
            .enterprise
            .check_create_project_quota(&txn, org_id, user.id)
            .await?
        {
            return Err(ApiError::QuotaReached(quota));
        }

        let id = uuid::Uuid::new_v4();
        let now = Utc::now();
        let project = entities::project::ActiveModel {
            id: ActiveValue::Set(id),
            name: ActiveValue::Set(repo.name.clone()),
            created_at: ActiveValue::Set(now.into()),
            repo_url: ActiveValue::Set(repo.url.clone()),
            repo_name: ActiveValue::Set(repo.name.clone()),
            organization_id: ActiveValue::Set(org_id),
            created_by: ActiveValue::Set(user.id),
            machine_type_id: ActiveValue::Set(project.machine_type_id),
            ..Default::default()
        };
        let project = project.insert(&txn).await?;

        self.enterprise
            .insert_audit_log(
                &txn,
                now.into(),
                user.id,
                project.organization_id,
                AuditResourceKind::Project.to_string(),
                project.id,
                project.name.clone(),
                AuditAction::PrebuildCreate.to_string(),
                ip,
                user_agent,
            )
            .await?;
        txn.commit().await?;

        let path = PathBuf::from(format!("/var/lib/lapdev/projects/{id}"));
        let url = repo.url.clone();
        tokio::fs::create_dir_all(&path).await?;
        clone_repo(url, path, repo.auth.clone()).await?;

        Ok(NewProjectResponse {
            id: project.id,
            name: project.name,
        })
    }

    pub async fn project_branches(
        &self,
        user_id: Uuid,
        project: &entities::project::Model,
        auth: (String, String),
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<Vec<GitBranch>, ApiError> {
        let project_id = project.id;
        let (previous_branches, branches) = tokio::task::spawn_blocking(
            move || -> Result<(Vec<GitBranch>, Vec<GitBranch>), ApiError> {
                let path = PathBuf::from(format!("/var/lib/lapdev/projects/{project_id}"));
                let repo = git2::Repository::open(path)?;

                let previous_branches = repo_branches(&repo)?;

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
                Ok((previous_branches, branches))
            },
        )
        .await??;

        // find deleted branches
        {
            let mut deleted_branches = Vec::new();
            let branches: HashMap<String, String> = branches
                .iter()
                .map(|b| (b.name.clone(), b.name.clone()))
                .collect();
            for p in previous_branches {
                if !branches.contains_key(&p.name) {
                    deleted_branches.push(p.name.clone());
                }
            }
            if !deleted_branches.is_empty() {
                let conductor = self.clone();
                let project = project.to_owned();
                tokio::spawn(async move {
                    conductor
                        .check_deleted_project_branchs(
                            user_id,
                            &project,
                            deleted_branches,
                            ip,
                            user_agent,
                        )
                        .await;
                });
            }
        }

        Ok(branches)
    }

    #[allow(clippy::too_many_arguments)]
    async fn do_create_project_prebuild(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        project: &entities::project::Model,
        prebuild: &entities::prebuild::Model,
        repo: RepoDetails,
        machine_type: &entities::machine_type::Model,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<RepoBuildOutput, ApiError> {
        {
            let conductor = self.clone();
            let project = project.to_owned();
            let current_prebuild = prebuild.id;
            let branch = prebuild.branch.clone();
            tokio::spawn(async move {
                if let Ok(prebuilds) = entities::prebuild::Entity::find()
                    .filter(entities::prebuild::Column::DeletedAt.is_null())
                    .filter(entities::prebuild::Column::ProjectId.eq(project.id))
                    .filter(entities::prebuild::Column::Branch.eq(branch))
                    .all(&conductor.db.conn)
                    .await
                {
                    for prebuild in prebuilds {
                        if prebuild.id != current_prebuild {
                            let ip = ip.clone();
                            let user_agent = user_agent.clone();
                            let _ = conductor
                                .check_unused_prebuild(
                                    org_id, user_id, &project, &prebuild, ip, user_agent,
                                )
                                .await;
                        }
                    }
                }
            });
        }

        let temp_repo_dir = tempfile::tempdir()?;
        self.prepare_repo(&repo, temp_repo_dir.path()).await?;

        let ws_client = { self.rpcs.lock().await.get(&prebuild.host_id).cloned() }
            .ok_or_else(|| anyhow!("can't find the prebuild host rpc client"))?;
        let env = repo
            .project
            .as_ref()
            .and_then(|project| project.env.as_ref())
            .and_then(|env| serde_json::from_str::<Vec<(String, String)>>(env).ok())
            .unwrap_or_default();
        let info = RepoBuildInfo {
            target: BuildTarget::Prebuild(prebuild.id),
            repo_name: repo.name.clone(),
            env,
            osuser: prebuild.osuser.clone(),
            cpus: serde_json::from_str(&prebuild.cores)?,
            memory: machine_type.memory as usize,
        };
        self.transfer_repo(&ws_client, temp_repo_dir.path(), info.clone())
            .await?;
        let output = self
            .build_repo(&ws_client, info.clone(), temp_repo_dir.path())
            .await;
        ws_client
            .create_prebuild_archive(
                long_running_context(),
                output.clone(),
                PrebuildInfo {
                    id: prebuild.id,
                    osuser: prebuild.osuser.clone(),
                },
                repo.name.clone(),
            )
            .await??;
        Ok(output)
    }

    pub async fn create_project_prebuild(
        &self,
        user: &entities::user::Model,
        project: &entities::project::Model,
        ws: Option<&entities::workspace::Model>,
        repo: &RepoDetails,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<entities::prebuild::Model, ApiError> {
        let id = uuid::Uuid::new_v4();

        let osuser = if self.db.is_container_isolated().await.unwrap_or(false) {
            format!("org-{}", project.organization_id)
        } else {
            LAPDEV_DEFAULT_OSUSER.to_string()
        };

        let machine_type_id = ws
            .map(|ws| ws.machine_type_id)
            .unwrap_or(project.machine_type_id);
        let machine_type = self
            .db
            .get_machine_type(machine_type_id)
            .await?
            .ok_or_else(|| ApiError::InternalError("Can't find machine type".to_string()))?;

        let now = Utc::now();
        let txn = self.db.conn.begin().await?;

        let (host_id, cores, host) = if let Some(ws) = ws {
            // if the prebuild initiated by creating a workspace
            // we'll use the resource of the workspace
            let cores: Vec<usize> = serde_json::from_str(&ws.cores)?;
            (ws.host_id, cores, None)
        } else {
            let shared_cpu = if self.enterprise.has_valid_license().await {
                machine_type.shared
            } else {
                // only enterprise support shared cpu
                false
            };
            let (host, cores) = scheduler::pick_workspce_host(
                &txn,
                None,
                Vec::new(),
                shared_cpu,
                machine_type.cpu as usize,
                machine_type.memory as usize,
                machine_type.disk as usize,
                self.region().await,
                self.cpu_overcommit().await,
            )
            .await
            .map_err(|_| ApiError::NoAvailableWorkspaceHost)?;
            (host.id, cores, Some(host))
        };

        let usage = self
            .enterprise
            .usage
            .new_usage(
                &txn,
                now.into(),
                project.organization_id,
                Some(user.id),
                UsageResourceKind::Prebuild,
                id,
                format!("{} {}", project.name, repo.branch),
                machine_type.id,
                machine_type.cost_per_second,
            )
            .await?;

        let result = entities::prebuild::ActiveModel {
            id: ActiveValue::Set(id),
            deleted_at: ActiveValue::Set(None),
            created_at: ActiveValue::Set(now.into()),
            project_id: ActiveValue::Set(project.id),
            user_id: ActiveValue::Set(Some(user.id)),
            osuser: ActiveValue::Set(osuser.clone()),
            branch: ActiveValue::Set(repo.branch.clone()),
            commit: ActiveValue::Set(repo.commit.clone()),
            status: ActiveValue::Set(PrebuildStatus::Building.to_string()),
            host_id: ActiveValue::Set(host_id),
            cores: ActiveValue::Set(serde_json::to_string(&cores)?),
            by_workspace: ActiveValue::Set(ws.is_some()),
            build_output: ActiveValue::Set(None),
        }
        .insert(&txn)
        .await;
        let prebuild = match result {
            Ok(model) => model,
            Err(e) => {
                if let Some(sea_orm::SqlErr::UniqueConstraintViolation(_)) = e.sql_err() {
                    return Err(ApiError::InvalidRequest(
                        "prebuild already exists".to_string(),
                    ));
                }
                return Err(e)?;
            }
        };

        self.enterprise
            .insert_audit_log(
                &txn,
                now.into(),
                user.id,
                project.organization_id,
                AuditResourceKind::Prebuild.to_string(),
                prebuild.id,
                format!(
                    "{} {} {}",
                    project.name,
                    prebuild.branch,
                    &prebuild.commit[..7]
                ),
                AuditAction::PrebuildCreate.to_string(),
                ip.clone(),
                user_agent.clone(),
            )
            .await?;

        if let Some(host) = host {
            scheduler::recalcuate_workspce_host(&txn, &host, self.cpu_overcommit().await).await?;
        }
        txn.commit().await?;
        let local_prebuild = prebuild.clone();

        let conductor = self.clone();
        let repo = repo.clone();
        let user_id = user.id;
        let org_id = project.organization_id;
        let project = project.to_owned();
        tokio::spawn(async move {
            let mut update_prebuild = entities::prebuild::ActiveModel {
                id: ActiveValue::Set(prebuild.id),
                ..Default::default()
            };
            let status = match conductor
                .do_create_project_prebuild(
                    org_id,
                    user_id,
                    &project,
                    &prebuild,
                    repo,
                    &machine_type,
                    ip,
                    user_agent,
                )
                .await
            {
                Ok(output) => {
                    update_prebuild.build_output =
                        ActiveValue::Set(serde_json::to_string(&output).ok());
                    PrebuildStatus::Ready
                }
                Err(e) => {
                    let err = if let ApiError::InternalError(e) = e {
                        e
                    } else {
                        e.to_string()
                    };
                    tracing::error!("create prebuild filed: {err}");
                    PrebuildStatus::Failed
                }
            };
            let now = Utc::now();
            update_prebuild.status = ActiveValue::Set(status.to_string());
            let txn = conductor.db.conn.begin().await?;
            let host = conductor
                .db
                .get_workspace_host_with_lock(&txn, prebuild.host_id)
                .await?;
            update_prebuild.update(&txn).await?;
            if let Some(host) = host {
                scheduler::recalcuate_workspce_host(&txn, &host, conductor.cpu_overcommit().await)
                    .await?;
            }
            conductor
                .enterprise
                .usage
                .end_usage(&txn, usage.id, now.into())
                .await?;
            txn.commit().await?;

            Ok::<(), anyhow::Error>(())
        });

        Ok(local_prebuild)
    }

    fn archive_repo(repo_path: &Path, archive_path: &Path) -> Result<(), ApiError> {
        let archive = std::fs::File::create(archive_path)?;
        let encoder = zstd::Encoder::new(archive, 0)?.auto_finish();
        let mut tar = tar::Builder::new(encoder);
        tar.append_dir_all(".", repo_path)?;
        Ok(())
    }

    fn generate_key_pair(&self) -> Result<(String, String)> {
        let key = KeyPair::generate_rsa(4096, SignatureHash::SHA2_512)
            .ok_or_else(|| anyhow!("can't generate ssh key pair"))?;
        let id_rsa = encode_pkcs8_pem(&key)?;
        let public_key = key.clone_public_key()?;
        let public_key = format!(
            "{} {}",
            match public_key {
                PublicKey::RSA { .. } | PublicKey::EC { .. } => "ssh-rsa",
                PublicKey::Ed25519(_) => "ssh-ed25519",
            },
            public_key.public_key_base64()
        );
        Ok((id_rsa, public_key))
    }

    async fn region(&self) -> String {
        self.region.read().await.clone()
    }

    pub async fn cpu_overcommit(&self) -> usize {
        *self.cpu_overcommit.read().await
    }

    #[allow(clippy::too_many_arguments)]
    async fn create_workspace_model(
        &self,
        org: &entities::organization::Model,
        user: &entities::user::Model,
        repo: &RepoDetails,
        machine_type: &entities::machine_type::Model,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<entities::workspace::Model, ApiError> {
        let name = format!("{}-{}", repo.name, rand_string(12));
        let (id_rsa, public_key) = self.generate_key_pair()?;
        let osuser = self.get_osuser(user).await;

        let txn = self.db.conn.begin().await?;
        if let Some(quota) = self
            .enterprise
            .check_create_workspace_quota(&txn, org.id, user.id)
            .await?
        {
            return Err(ApiError::QuotaReached(quota));
        }

        let shared_cpu = if self.enterprise.has_valid_license().await {
            machine_type.shared
        } else {
            // only enterprise support shared cpu
            false
        };

        let prebuild = if let Some(project) = repo.project.as_ref() {
            self.db
                .get_prebuild_by_branch_and_commit_with_lock(
                    &txn,
                    project.id,
                    &repo.branch,
                    &repo.commit,
                )
                .await?
        } else {
            None
        };

        let replica_hosts = if let Some(prebuild) = prebuild.as_ref() {
            entities::prebuild_replica::Entity::find()
                .filter(entities::prebuild_replica::Column::DeletedAt.is_null())
                .filter(entities::prebuild_replica::Column::PrebuildId.eq(prebuild.id))
                .all(&txn)
                .await?
                .iter()
                .map(|r| r.host_id)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let (host, cores) = scheduler::pick_workspce_host(
            &txn,
            prebuild.as_ref().map(|p| p.host_id),
            replica_hosts,
            shared_cpu,
            machine_type.cpu as usize,
            machine_type.memory as usize,
            machine_type.disk as usize,
            self.region().await,
            self.cpu_overcommit().await,
        )
        .await
        .map_err(|_| ApiError::NoAvailableWorkspaceHost)?;
        let workspace_id = Uuid::new_v4();
        let now = Utc::now();
        let ws = entities::workspace::ActiveModel {
            id: ActiveValue::Set(workspace_id),
            deleted_at: ActiveValue::Set(None),
            name: ActiveValue::Set(name.clone()),
            created_at: ActiveValue::Set(now.into()),
            status: ActiveValue::Set(WorkspaceStatus::New.to_string()),
            repo_url: ActiveValue::Set(repo.url.clone()),
            repo_name: ActiveValue::Set(repo.name.clone()),
            branch: ActiveValue::Set(repo.branch.clone()),
            commit: ActiveValue::Set(repo.commit.clone()),
            organization_id: ActiveValue::Set(org.id),
            user_id: ActiveValue::Set(user.id),
            project_id: ActiveValue::Set(repo.project.as_ref().map(|p| p.id)),
            host_id: ActiveValue::Set(host.id),
            osuser: ActiveValue::Set(osuser),
            ssh_private_key: ActiveValue::Set(id_rsa),
            ssh_public_key: ActiveValue::Set(public_key),
            cores: ActiveValue::Set(serde_json::to_string(&cores)?),
            ssh_port: ActiveValue::Set(None),
            ide_port: ActiveValue::Set(None),
            prebuild_id: ActiveValue::Set(prebuild.as_ref().map(|p| p.host_id)),
            service: ActiveValue::Set(None),
            usage_id: ActiveValue::Set(None),
            machine_type_id: ActiveValue::Set(machine_type.id),
            last_inactivity: ActiveValue::Set(None),
            auto_stop: ActiveValue::Set(org.auto_stop),
            auto_start: ActiveValue::Set(org.auto_start),
            env: ActiveValue::Set(repo.project.as_ref().and_then(|p| p.env.clone())),
            build_output: ActiveValue::Set(None),
            is_compose: ActiveValue::Set(false),
            compose_parent: ActiveValue::Set(None),
        };
        let ws = ws.insert(&txn).await?;
        self.enterprise
            .insert_audit_log(
                &txn,
                now.into(),
                user.id,
                org.id,
                AuditResourceKind::Workspace.to_string(),
                workspace_id,
                ws.name.clone(),
                AuditAction::WorkspaceCreate.to_string(),
                ip,
                user_agent,
            )
            .await?;

        scheduler::recalcuate_workspce_host(&txn, &host, self.cpu_overcommit().await).await?;

        txn.commit().await?;
        Ok(ws)
    }

    async fn transfer_repo(
        &self,
        ws_client: &WorkspaceServiceClient,
        temp_repo_dir: &Path,
        info: RepoBuildInfo,
    ) -> Result<(), ApiError> {
        let archive_dir = tempfile::tempdir()?;
        let archive_path = archive_dir.path().join("repo.tar.zst");
        {
            let temp_repo_dir = temp_repo_dir.to_path_buf();
            let archive_path = archive_path.clone();
            tokio::task::spawn_blocking(move || Self::archive_repo(&temp_repo_dir, &archive_path))
                .await??;
        }

        let mut buf = vec![0u8; 1024 * 1024];
        let mut file = tokio::fs::File::open(&archive_path).await?;
        let mut initial = true;

        tracing::debug!("now start to transfer repo");
        while let Ok(n) = file.read(&mut buf).await {
            if n == 0 {
                break;
            }
            ws_client
                .transfer_repo(
                    context::current(),
                    info.clone(),
                    RepoContent {
                        content: buf[..n].to_vec(),
                        position: if initial {
                            RepoContentPosition::Initial
                        } else {
                            RepoContentPosition::Append
                        },
                    },
                )
                .await??;
            initial = false;
        }

        tracing::debug!("transfer repo is done, now unarchive it");
        ws_client
            .unarchive_repo(context::current(), info.clone())
            .await??;

        Ok(())
    }

    async fn prepare_repo(&self, repo: &RepoDetails, temp_repo_dir: &Path) -> Result<(), ApiError> {
        if let Some(project) = repo.project.as_ref() {
            let src_repo_dir = format!("/var/lib/lapdev/projects/{}/.", project.id);
            let temp_repo_dir = temp_repo_dir
                .to_str()
                .ok_or_else(|| anyhow!("can't get repo dir"))?;
            tokio::process::Command::new("sh")
                .arg("-c")
                .arg(format!("cp -a {src_repo_dir} {temp_repo_dir}"))
                .status()
                .await?;
        } else {
            let temp_repo_dir = temp_repo_dir.to_path_buf();
            let repo_url = repo.url.clone();
            clone_repo(repo_url, temp_repo_dir, repo.auth.clone()).await?;
        };

        if repo.head != repo.branch {
            let temp_repo_dir = temp_repo_dir.to_path_buf();
            let branch_name = repo.branch.clone();
            tokio::task::spawn_blocking(move || {
                let repo = git2::Repository::open(temp_repo_dir)?;
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

        Ok(())
    }

    // copy the prebuild repo content from the prebuild folder to workspace folder
    async fn copy_prebuild_content(
        &self,
        prebuild: &entities::prebuild::Model,
        info: RepoBuildInfo,
        ws_client: &WorkspaceServiceClient,
    ) -> Result<(), ApiError> {
        ws_client
            .copy_prebuild_content(
                long_running_context(),
                PrebuildInfo {
                    id: prebuild.id,
                    osuser: prebuild.osuser.clone(),
                },
                info,
            )
            .await??;
        Ok(())
    }

    async fn copy_prebuild_image(
        &self,
        prebuild: &entities::prebuild::Model,
        output: &RepoBuildOutput,
        ws: &entities::workspace::Model,
        ws_client: &WorkspaceServiceClient,
    ) -> Result<(), ApiError> {
        if ws.host_id != prebuild.host_id {
            let replica = self
                .db
                .get_prebuild_replica(prebuild.id, ws.host_id)
                .await?;
            let replica = if let Some(replica) = replica {
                if replica.status == PrebuildReplicaStatus::Failed.to_string() {
                    // if the replica transfer was failed, we just try it again
                    let replica = entities::prebuild_replica::ActiveModel {
                        id: ActiveValue::Set(replica.id),
                        status: ActiveValue::Set(PrebuildReplicaStatus::Transferring.to_string()),
                        ..Default::default()
                    }
                    .update(&self.db.conn)
                    .await?;
                    self.transfer_prebuild_replica(replica.id, prebuild, ws.host_id, &ws.repo_name)
                        .await;
                    replica
                } else {
                    replica
                }
            } else {
                let _ = self
                    .create_prebuild_replica(ws.user_id, prebuild, ws.host_id, &ws.repo_name)
                    .await;
                self.db
                    .get_prebuild_replica(prebuild.id, ws.host_id)
                    .await?
                    .ok_or_else(|| anyhow!("prebuild replica should exist after creation"))?
            };
            if replica.status == PrebuildReplicaStatus::Transferring.to_string() {
                let mut rx = self.prebuild_replica_updates(replica.id).await;
                // prebuild replica status should only get updated once
                // after the tranferring attempt, so we just listen to the update
                // without checking the status
                let _ = tokio::time::timeout(Duration::from_secs(3600), rx.next()).await;
                self.cleanup_prebuild_replica_updates(replica.id).await;
            }

            let replica = self
                .db
                .get_prebuild_replica(prebuild.id, ws.host_id)
                .await?
                .ok_or_else(|| anyhow!("prebuild replica should exist"))?;
            if replica.status != PrebuildReplicaStatus::Ready.to_string() {
                return Err(ApiError::InternalError(
                    "prebuild replica failed".to_string(),
                ));
            }
        }

        ws_client
            .copy_prebuild_image(
                long_running_context(),
                PrebuildInfo {
                    id: prebuild.id,
                    osuser: prebuild.osuser.clone(),
                },
                output.to_owned(),
                ws.osuser.clone(),
                BuildTarget::Workspace {
                    id: ws.id,
                    name: ws.name.clone(),
                },
            )
            .await??;

        Ok(())
    }

    async fn create_prebuild_replica(
        &self,
        user_id: Uuid,
        prebuild: &entities::prebuild::Model,
        host_id: Uuid,
        repo_name: &str,
    ) -> Result<(), ApiError> {
        let id = Uuid::new_v4();
        let result = entities::prebuild_replica::ActiveModel {
            id: ActiveValue::Set(id),
            deleted_at: ActiveValue::Set(None),
            created_at: ActiveValue::Set(Utc::now().into()),
            prebuild_id: ActiveValue::Set(prebuild.id),
            user_id: ActiveValue::Set(Some(user_id)),
            host_id: ActiveValue::Set(host_id),
            status: ActiveValue::Set(PrebuildReplicaStatus::Transferring.to_string()),
        }
        .insert(&self.db.conn)
        .await;
        match result {
            Ok(_) => {}
            Err(e) => {
                if let Some(sea_orm::SqlErr::UniqueConstraintViolation(_)) = e.sql_err() {
                    return Err(ApiError::InvalidRequest(
                        "prebuild already exists".to_string(),
                    ));
                }
                return Err(e)?;
            }
        }

        self.transfer_prebuild_replica(id, prebuild, host_id, repo_name)
            .await;

        Ok(())
    }

    async fn transfer_prebuild_replica(
        &self,
        replica_id: Uuid,
        prebuild: &entities::prebuild::Model,
        host_id: Uuid,
        repo_name: &str,
    ) {
        let conductor = self.clone();
        let prebuild = prebuild.to_owned();
        let repo_name = repo_name.to_string();
        tokio::spawn(async move {
            let status = if let Err(e) = conductor
                .do_transfer_prebuild_replica(&prebuild, host_id, &repo_name)
                .await
            {
                let err = if let ApiError::InternalError(e) = e {
                    e
                } else {
                    e.to_string()
                };
                tracing::error!("transfer prebuild replica failed: {err}");
                PrebuildReplicaStatus::Failed
            } else {
                PrebuildReplicaStatus::Ready
            };
            let _ = entities::prebuild_replica::ActiveModel {
                id: ActiveValue::Set(replica_id),
                status: ActiveValue::Set(status.to_string()),
                ..Default::default()
            }
            .update(&conductor.db.conn)
            .await;
        });
    }

    async fn do_transfer_prebuild_replica(
        &self,
        prebuild: &entities::prebuild::Model,
        host_id: Uuid,
        repo_name: &str,
    ) -> Result<(), ApiError> {
        let ws_host = self
            .db
            .get_workspace_host(host_id)
            .await?
            .ok_or_else(|| anyhow!("can't find workspace host {host_id}"))?;
        let prebuild_ws_client = { self.rpcs.lock().await.get(&prebuild.host_id).cloned() }
            .ok_or_else(|| anyhow!("can't find the workspace host rpc client"))?;
        let ws_client = { self.rpcs.lock().await.get(&host_id).cloned() }
            .ok_or_else(|| anyhow!("can't find the workspace host rpc client"))?;

        let output = prebuild
            .build_output
            .as_ref()
            .and_then(|output| serde_json::from_str::<RepoBuildOutput>(output).ok())
            .ok_or_else(|| anyhow!("prebuild doesn't have valid repo build output"))?;
        prebuild_ws_client
            .transfer_prebuild(
                long_running_context(),
                PrebuildInfo {
                    id: prebuild.id,
                    osuser: prebuild.osuser.clone(),
                },
                output,
                ws_host.host,
                ws_host.inter_port as u16,
            )
            .await??;
        ws_client
            .unarchive_prebuild(
                long_running_context(),
                PrebuildInfo {
                    id: prebuild.id,
                    osuser: prebuild.osuser.clone(),
                },
                repo_name.to_string(),
            )
            .await??;
        Ok(())
    }

    pub async fn get_project_repo_details(
        &self,
        user: &entities::user::Model,
        project: &entities::project::Model,
        branch: Option<&str>,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<RepoDetails, ApiError> {
        let auth = if let Ok(Some(user)) = self.db.get_user(project.created_by).await {
            (user.provider_login, user.access_token)
        } else {
            (user.provider_login.clone(), user.access_token.clone())
        };
        let branches = self
            .project_branches(user.id, project, auth.clone(), ip, user_agent)
            .await?;
        let head = branches
            .first()
            .map(|b| b.name.clone())
            .ok_or_else(|| anyhow!("don't have branches"))?;
        let branch = branch
            .map(|b| b.to_string())
            .unwrap_or_else(|| head.clone());
        let branches: Vec<(String, String)> =
            branches.into_iter().map(|b| (b.name, b.commit)).collect();
        let Some(commit) = branches
            .iter()
            .find(|(b, _)| b == &branch)
            .map(|(_, c)| c.to_string())
        else {
            return Err(ApiError::RepositoryInvalid(format!(
                "repository does't have branch {branch}"
            )));
        };
        Ok(RepoDetails {
            url: project.repo_url.clone(),
            name: project.repo_name.clone(),
            branch,
            auth,
            commit,
            project: Some(project.to_owned()),
            head,
            branches,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn get_repo_details(
        &self,
        org_id: Uuid,
        user: &entities::user::Model,
        source: &RepoSource,
        branch: Option<&str>,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<RepoDetails, ApiError> {
        let project = match source {
            RepoSource::Url(repo) => {
                let repo = self.format_repo_url(repo);
                let project = self.db.get_project_by_repo(org_id, &repo).await?;
                if let Some(project) = project {
                    project
                } else {
                    return self
                        .get_raw_repo_details(
                            &repo,
                            branch,
                            (user.provider_login.clone(), user.access_token.clone()),
                        )
                        .await;
                }
            }
            RepoSource::Project(project_id) => {
                let project =
                    self.db.get_project(*project_id).await.map_err(|_| {
                        ApiError::InvalidRequest("project doesn't exist".to_string())
                    })?;
                if project.organization_id != org_id {
                    return Err(ApiError::Unauthorized);
                }
                project
            }
        };

        self.get_project_repo_details(user, &project, branch, ip, user_agent)
            .await
    }

    async fn prepare_prebuild(
        &self,
        user: &entities::user::Model,
        ws: &entities::workspace::Model,
        project: &entities::project::Model,
        repo: &RepoDetails,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<Option<(entities::prebuild::Model, RepoBuildOutput)>, ApiError> {
        let prebuild = if let Some(prebuild_id) = ws.prebuild_id {
            self.db.get_prebuild(prebuild_id).await?
        } else {
            None
        };

        let prebuild = if let Some(prebuild) = prebuild {
            prebuild
        } else {
            let _ = self
                .create_project_prebuild(user, project, Some(ws), repo, ip, user_agent)
                .await;

            // we read the prebuild from db again instead of using the result from the creation above,
            // because if there's a conflict (e.g. there's another prebuild inserted before we do),
            // we still can start from that prebuild
            let txn = self.db.conn.begin().await?;
            let prebuild = self
                .db
                .get_prebuild_by_branch_and_commit_with_lock(
                    &txn,
                    project.id,
                    &repo.branch,
                    &repo.commit,
                )
                .await?
                .ok_or_else(|| anyhow!("prebuild should exist"))?;
            entities::workspace::ActiveModel {
                id: ActiveValue::Set(ws.id),
                prebuild_id: ActiveValue::Set(Some(prebuild.id)),
                ..Default::default()
            }
            .update(&txn)
            .await?;
            txn.commit().await?;

            prebuild
        };
        if prebuild.status == PrebuildStatus::Building.to_string() {
            // wait for prebuild finish
            let _ = self
                .update_workspace_status(ws, WorkspaceStatus::PrebuildBuilding)
                .await;
            let mut rx = self.prebuild_updates(prebuild.id).await;
            while let Ok(Some(event)) =
                tokio::time::timeout(Duration::from_secs(600), rx.next()).await
            {
                match event {
                    PrebuildUpdateEvent::Status(status) => {
                        if status == PrebuildStatus::Ready {
                            break;
                        } else if status == PrebuildStatus::Failed {
                            self.cleanup_prebuild_updates(prebuild.id).await;
                            return Ok(None);
                        }
                    }
                    PrebuildUpdateEvent::Stdout(s) => {
                        self.add_workspace_update_event(
                            None,
                            ws.id,
                            WorkspaceUpdateEvent::Stdout(s),
                        )
                        .await;
                    }
                    PrebuildUpdateEvent::Stderr(s) => {
                        self.add_workspace_update_event(
                            None,
                            ws.id,
                            WorkspaceUpdateEvent::Stderr(s),
                        )
                        .await;
                    }
                }
            }
            self.cleanup_prebuild_updates(prebuild.id).await;
        }
        let prebuild = self
            .db
            .get_prebuild_by_branch_and_commit(project.id, &repo.branch, &repo.commit)
            .await?
            .ok_or_else(|| anyhow!("prebuild should exist"))?;
        if prebuild.status == PrebuildStatus::Ready.to_string() {
            if let Some(output) = prebuild.build_output.as_ref() {
                if let Ok(output) = serde_json::from_str::<RepoBuildOutput>(output) {
                    return Ok(Some((prebuild, output)));
                }
            }
        }
        Ok(None)
    }

    #[allow(clippy::too_many_arguments)]
    async fn prepare_workspace_image(
        &self,
        user: &entities::user::Model,
        repo: &RepoDetails,
        env: Vec<(String, String)>,
        ws: &entities::workspace::Model,
        ws_client: &WorkspaceServiceClient,
        machine_type: &entities::machine_type::Model,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<(Option<Uuid>, RepoBuildOutput), ApiError> {
        let info = RepoBuildInfo {
            target: BuildTarget::Workspace {
                id: ws.id,
                name: ws.name.clone(),
            },
            env,
            repo_name: ws.repo_name.clone(),
            osuser: ws.osuser.clone(),
            cpus: serde_json::from_str(&ws.cores)?,
            memory: machine_type.memory as usize,
        };

        if let Some(project) = repo.project.as_ref() {
            tracing::info!(
                "prepare workspace {} image from project {}",
                ws.name,
                project.id
            );
            if let Some((prebuild, output)) = self
                .prepare_prebuild(user, ws, project, repo, ip, user_agent)
                .await?
            {
                let _ = self
                    .update_workspace_status(ws, WorkspaceStatus::PrebuildCopying)
                    .await;
                // check if the prebuild is on the workspace host, copy it over if not
                self.copy_prebuild_image(&prebuild, &output, ws, ws_client)
                    .await?;
                // copy the prebuild repo folder to the workspace folder
                self.copy_prebuild_content(&prebuild, info, ws_client)
                    .await?;
                return Ok((Some(prebuild.id), output));
            }
        }

        let temp_repo_dir = tempfile::tempdir()?;
        tracing::debug!("workspace temp repo dir {:?}", temp_repo_dir.path());
        self.prepare_repo(repo, temp_repo_dir.path()).await?;
        self.transfer_repo(ws_client, temp_repo_dir.path(), info.clone())
            .await?;
        let _ = self
            .update_workspace_status(ws, WorkspaceStatus::Building)
            .await;
        tracing::debug!("start to build repo");
        let output = self.build_repo(ws_client, info, temp_repo_dir.path()).await;

        Ok((None, output))
    }

    async fn build_repo(
        &self,
        ws_client: &WorkspaceServiceClient,
        info: RepoBuildInfo,
        repo_path: &Path,
    ) -> RepoBuildOutput {
        tracing::info!("start to build repo {info:?}");
        let result = ws_client
            .build_repo(long_running_context(), info.clone())
            .await;
        match result {
            Ok(Ok(result)) => return result,
            Ok(Err(e)) => {
                tracing::error!("build repo {info:?} error: {e}");
            }
            Err(e) => {
                tracing::error!("build repo {info:?} rpc error: {e}");
            }
        };

        let image_name = self.get_repo_default_image(repo_path).await;
        let image = format!("ghcr.io/lapce/lapdev-devcontainer-{image_name}:latest");
        tracing::debug!("build repo pick default image {image_name}");
        RepoBuildOutput::Image(image)
    }

    async fn get_repo_default_image(&self, repo_path: &Path) -> String {
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

    async fn create_workspace_from_output(
        &self,
        ws: &entities::workspace::Model,
        prebuild_id: Option<Uuid>,
        output: RepoBuildOutput,
        env: Vec<(String, String)>,
        ws_client: &WorkspaceServiceClient,
        machine_type: &entities::machine_type::Model,
    ) -> Result<(), ApiError> {
        let (is_compose, images) = match &output {
            RepoBuildOutput::Compose(services) => (
                true,
                services
                    .iter()
                    .map(|s| (Some(s.name.clone()), s.image.clone(), s.env.clone()))
                    .collect(),
            ),
            RepoBuildOutput::Image(tag) => (false, vec![(None, tag.clone(), Vec::new())]),
        };

        let build_output = serde_json::to_string(&output)?;

        let cores: Vec<usize> = serde_json::from_str(&ws.cores)?;
        for (i, (service, tag, image_env)) in images.into_iter().enumerate() {
            let workspace_name = if let Some(service) = service.clone() {
                if i > 0 {
                    format!("{}-{service}", ws.name)
                } else {
                    ws.name.clone()
                }
            } else {
                ws.name.clone()
            };
            let (id_rsa, ssh_public_key) = if i == 0 {
                (ws.ssh_private_key.clone(), ws.ssh_public_key.clone())
            } else {
                self.generate_key_pair()?
            };
            let mut env = env.clone();
            env.extend_from_slice(&image_env);
            ws_client
                .create_workspace(
                    long_running_context(),
                    CreateWorkspaceRequest {
                        id: ws.id,
                        workspace_name: workspace_name.clone(),
                        volume_name: ws.name.clone(),
                        create_network: i == 0,
                        network_name: ws.name.clone(),
                        service: service.clone(),
                        osuser: ws.osuser.clone(),
                        image: tag,
                        ssh_public_key: ssh_public_key.clone(),
                        repo_name: ws.repo_name.clone(),
                        env: env.clone(),
                        cpus: cores.clone(),
                        memory: machine_type.memory as usize,
                        disk: machine_type.disk as usize,
                    },
                )
                .await??;
            let info = ws_client
                .start_workspace(
                    context::current(),
                    StartWorkspaceRequest {
                        osuser: ws.osuser.clone(),
                        workspace_name: workspace_name.clone(),
                    },
                )
                .await??;
            let ssh_port = info
                .host_config
                .port_bindings
                .get("22/tcp")
                .and_then(|bindings| bindings.first())
                .and_then(|binding| binding.host_port.parse::<u16>().ok());
            let ide_port = info
                .host_config
                .port_bindings
                .get("30000/tcp")
                .and_then(|bindings| bindings.first())
                .and_then(|binding| binding.host_port.parse::<u16>().ok());

            let mut exposed_ports = Vec::new();
            for (port, port_bindings) in &info.host_config.port_bindings {
                if let Some(port) = port.strip_suffix("/tcp") {
                    if let Ok(port) = port.parse::<u16>() {
                        if port != 22 && port != 30000 {
                            for binding in port_bindings {
                                if let Ok(host_port) = binding.host_port.parse::<u16>() {
                                    exposed_ports.push((port, host_port));
                                }
                            }
                        }
                    }
                }
            }
            if i == 0 {
                let txn = self.db.conn.begin().await?;
                let usage = self
                    .enterprise
                    .usage
                    .new_usage(
                        &txn,
                        Utc::now().into(),
                        ws.organization_id,
                        Some(ws.user_id),
                        UsageResourceKind::Workspace,
                        ws.id,
                        ws.name.clone(),
                        machine_type.id,
                        machine_type.cost_per_second,
                    )
                    .await?;
                entities::workspace::ActiveModel {
                    id: ActiveValue::Set(ws.id),
                    ssh_port: ActiveValue::Set(ssh_port.map(|port| port as i32)),
                    ide_port: ActiveValue::Set(ide_port.map(|port| port as i32)),
                    service: ActiveValue::Set(service),
                    status: ActiveValue::Set(WorkspaceStatus::Running.to_string()),
                    prebuild_id: ActiveValue::Set(prebuild_id),
                    build_output: ActiveValue::Set(Some(build_output.clone())),
                    is_compose: ActiveValue::Set(is_compose),
                    env: ActiveValue::Set(serde_json::to_string(&env).ok()),
                    usage_id: ActiveValue::Set(Some(usage.id)),
                    ..Default::default()
                }
                .update(&self.db.conn)
                .await?;
                txn.commit().await?;
            } else {
                entities::workspace::ActiveModel {
                    id: ActiveValue::Set(Uuid::new_v4()),
                    name: ActiveValue::Set(workspace_name),
                    created_at: ActiveValue::Set(Utc::now().into()),
                    deleted_at: ActiveValue::Set(None),
                    status: ActiveValue::Set(WorkspaceStatus::Running.to_string()),
                    repo_url: ActiveValue::Set(ws.repo_url.clone()),
                    repo_name: ActiveValue::Set(ws.repo_name.clone()),
                    branch: ActiveValue::Set(ws.branch.clone()),
                    commit: ActiveValue::Set(ws.commit.clone()),
                    organization_id: ActiveValue::Set(ws.organization_id),
                    user_id: ActiveValue::Set(ws.user_id),
                    project_id: ActiveValue::Set(ws.project_id),
                    prebuild_id: ActiveValue::Set(prebuild_id),
                    host_id: ActiveValue::Set(ws.host_id),
                    osuser: ActiveValue::Set(ws.osuser.clone()),
                    ssh_private_key: ActiveValue::Set(id_rsa),
                    ssh_public_key: ActiveValue::Set(ssh_public_key),
                    ssh_port: ActiveValue::Set(ssh_port.map(|port| port as i32)),
                    ide_port: ActiveValue::Set(ide_port.map(|port| port as i32)),
                    service: ActiveValue::Set(service),
                    cores: ActiveValue::Set(ws.cores.clone()),
                    usage_id: ActiveValue::Set(None),
                    machine_type_id: ActiveValue::Set(ws.machine_type_id),
                    last_inactivity: ActiveValue::Set(None),
                    auto_stop: ActiveValue::Set(ws.auto_stop),
                    auto_start: ActiveValue::Set(ws.auto_start),
                    env: ActiveValue::Set(serde_json::to_string(&env).ok()),
                    build_output: ActiveValue::Set(Some(build_output.clone())),
                    is_compose: ActiveValue::Set(is_compose),
                    compose_parent: ActiveValue::Set(Some(ws.id)),
                }
                .insert(&self.db.conn)
                .await?;
            }

            if !exposed_ports.is_empty() {
                for (port, host_port) in exposed_ports {
                    entities::workspace_port::ActiveModel {
                        id: ActiveValue::Set(Uuid::new_v4()),
                        workspace_id: ActiveValue::Set(ws.id),
                        port: ActiveValue::Set(port as i32),
                        host_port: ActiveValue::Set(host_port as i32),
                        shared: ActiveValue::Set(false),
                    }
                    .insert(&self.db.conn)
                    .await?;
                }
            }
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn do_create_workspace(
        &self,
        user: &entities::user::Model,
        ws: &entities::workspace::Model,
        repo: RepoDetails,
        machine_type: &entities::machine_type::Model,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<(), ApiError> {
        let ws_client = { self.rpcs.lock().await.get(&ws.host_id).cloned() }
            .ok_or_else(|| anyhow!("can't find the workspace host rpc client"))?;
        let env = repo
            .project
            .as_ref()
            .and_then(|project| project.env.as_ref())
            .and_then(|env| serde_json::from_str::<Vec<(String, String)>>(env).ok())
            .unwrap_or_default();
        let (prebuild_id, output) = self
            .prepare_workspace_image(
                user,
                &repo,
                env.clone(),
                ws,
                &ws_client,
                machine_type,
                ip,
                user_agent,
            )
            .await?;
        tracing::debug!(
            "prepare workspace image done, prebuild: {prebuild_id:?}, output: {output:?}"
        );
        self.create_workspace_from_output(ws, prebuild_id, output, env, &ws_client, machine_type)
            .await?;
        self.add_workspace_update_event(
            Some(ws.user_id),
            ws.id,
            WorkspaceUpdateEvent::Status(WorkspaceStatus::Running),
        )
        .await;

        Ok(())
    }

    async fn get_osuser(&self, user: &entities::user::Model) -> String {
        let container_isolated = self.db.is_container_isolated().await.unwrap_or(false);
        if container_isolated {
            user.osuser.clone()
        } else {
            LAPDEV_DEFAULT_OSUSER.to_string()
        }
    }

    pub async fn create_workspace(
        &self,
        user: entities::user::Model,
        org: entities::organization::Model,
        workspace: NewWorkspace,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<NewWorkspaceResponse, ApiError> {
        let repo = self
            .get_repo_details(
                org.id,
                &user,
                &workspace.source,
                workspace.branch.as_deref(),
                ip.clone(),
                user_agent.clone(),
            )
            .await?;

        let machine_type = self
            .db
            .get_machine_type(workspace.machine_type_id)
            .await?
            .ok_or_else(|| ApiError::InvalidRequest("Can't find machine type".to_string()))?;
        let ws = self
            .create_workspace_model(
                &org,
                &user,
                &repo,
                &machine_type,
                ip.clone(),
                user_agent.clone(),
            )
            .await?;

        let resp = NewWorkspaceResponse {
            name: ws.name.clone(),
        };

        let conductor = self.clone();
        tokio::spawn(async move {
            if let Err(e) = conductor
                .do_create_workspace(&user, &ws, repo, &machine_type, ip, user_agent)
                .await
            {
                let err = if let ApiError::InternalError(e) = e {
                    e
                } else {
                    e.to_string()
                };
                tracing::error!("create workspace failed: {err}");
                let _ = conductor
                    .update_workspace_status(&ws, WorkspaceStatus::Failed)
                    .await;
            }
        });

        Ok(resp)
    }

    pub async fn add_workspace_update_event(
        &self,
        user_id: Option<Uuid>,
        workspace_id: Uuid,
        event: WorkspaceUpdateEvent,
    ) {
        {
            let subscribers = {
                let mut ws_updates = self.ws_updates.lock().await;
                let update = ws_updates.entry(workspace_id).or_default();
                if !matches!(event, WorkspaceUpdateEvent::Status(_)) {
                    update.seen.push(event.clone());
                }
                update.subscribers.clone()
            };

            for mut tx in subscribers {
                let _ = tx.send(event.clone()).await;
            }
        }

        if let Some(user_id) = user_id {
            if let WorkspaceUpdateEvent::Status(status) = event {
                let subscribers = {
                    self.all_workspace_updates
                        .lock()
                        .await
                        .get(&user_id)
                        .cloned()
                };

                if let Some(subscribers) = subscribers {
                    for mut tx in subscribers {
                        let _ = tx.send((workspace_id, status)).await;
                    }
                }
            }
        }
    }

    pub async fn update_workspace_status(
        &self,
        ws: &entities::workspace::Model,
        status: WorkspaceStatus,
    ) -> Result<entities::workspace::Model, ApiError> {
        let update_ws = entities::workspace::ActiveModel {
            id: ActiveValue::Set(ws.id),
            status: ActiveValue::Set(status.to_string()),
            ..Default::default()
        };
        let ws = update_ws.update(&self.db.conn).await?;
        Ok(ws)
    }

    pub async fn add_prebuild_update_event(&self, prebuild_id: Uuid, event: PrebuildUpdateEvent) {
        {
            let subscribers = {
                let mut prebuild_updates = self.prebuild_updates.lock().await;
                let update = prebuild_updates.entry(prebuild_id).or_default();
                if !matches!(event, PrebuildUpdateEvent::Status(_)) {
                    update.seen.push(event.clone());
                }
                update.subscribers.clone()
            };

            for mut tx in subscribers {
                let _ = tx.send(event.clone()).await;
            }
        }
    }

    pub async fn add_prebuild_replica_update_event(
        &self,
        prebuild_replica_id: Uuid,
        status: PrebuildReplicaStatus,
    ) {
        {
            let subscribers = {
                let mut prebuild_replica_updates = self.prebuild_replica_updates.lock().await;
                let update = prebuild_replica_updates
                    .entry(prebuild_replica_id)
                    .or_default();
                update.subscribers.clone()
            };

            for mut tx in subscribers {
                let _ = tx.send(status.clone()).await;
            }
        }
    }

    pub async fn prebuild_replica_updates(
        &self,
        prebuild_replica_id: Uuid,
    ) -> UnboundedReceiver<PrebuildReplicaStatus> {
        let (tx, rx) = futures::channel::mpsc::unbounded::<PrebuildReplicaStatus>();
        {
            let mut prebuild_replica_updates = self.prebuild_replica_updates.lock().await;
            let update = prebuild_replica_updates
                .entry(prebuild_replica_id)
                .or_default();
            update.subscribers.push(tx);
        }
        rx
    }

    pub async fn cleanup_prebuild_replica_updates(&self, prebuild_replica_id: Uuid) {
        let mut prebuild_replica_updates = self.prebuild_replica_updates.lock().await;
        if let Some(update) = prebuild_replica_updates.get_mut(&prebuild_replica_id) {
            update.subscribers.retain(|tx| !tx.is_closed());
        }
    }

    pub async fn delete_workspace(
        &self,
        workspace: entities::workspace::Model,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<(), ApiError> {
        if workspace.is_compose && workspace.compose_parent.is_some() {
            return Err(ApiError::InvalidRequest(
                "You can't delete a compose service workspace. You can only delete the main workspace".to_string(),
            ));
        }

        let now = Utc::now();

        let txn = self.db.conn.begin().await?;
        self.enterprise
            .insert_audit_log(
                &txn,
                now.into(),
                workspace.user_id,
                workspace.organization_id,
                AuditResourceKind::Workspace.to_string(),
                workspace.id,
                workspace.name.clone(),
                AuditAction::WorkspaceDelete.to_string(),
                ip.clone(),
                user_agent.clone(),
            )
            .await?;
        let update_ws = entities::workspace::ActiveModel {
            id: ActiveValue::Set(workspace.id),
            status: ActiveValue::Set(WorkspaceStatus::Deleting.to_string()),
            ..Default::default()
        };
        let ws = update_ws.update(&txn).await?;

        txn.commit().await?;

        let compose_services = if ws.is_compose {
            entities::workspace::Entity::find()
                .filter(entities::workspace::Column::DeletedAt.is_null())
                .filter(entities::workspace::Column::ComposeParent.eq(ws.id))
                .all(&self.db.conn)
                .await?
        } else {
            Vec::new()
        };

        {
            let conductor = self.clone();
            tokio::spawn(async move {
                for ws in compose_services {
                    if let Err(e) = conductor
                        .do_delete_workspace(&ws, None, ip.clone(), user_agent.clone())
                        .await
                    {
                        tracing::error!("do delete workspace {} error: {e}", ws.id);
                    }
                }
                if let Err(e) = conductor
                    .do_delete_workspace(&ws, Some(ws.name.clone()), ip, user_agent)
                    .await
                {
                    tracing::error!("do delete workspace {} error: {e}", ws.id);
                }
            });
        }

        Ok(())
    }

    async fn do_delete_workspace(
        &self,
        ws: &entities::workspace::Model,
        network: Option<String>,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<()> {
        let ws_client = { self.rpcs.lock().await.get(&ws.host_id).cloned() };
        let ws_client =
            ws_client.ok_or_else(|| anyhow!("can't connect to the workspace servic client"))?;

        let images = if ws.prebuild_id.is_none() && network.is_some() {
            let output: Option<RepoBuildOutput> = ws
                .build_output
                .as_ref()
                .and_then(|o| serde_json::from_str(o).ok());

            if let Some(output) = output {
                match output {
                    RepoBuildOutput::Compose(services) => {
                        services.into_iter().map(|s| s.image).collect()
                    }
                    RepoBuildOutput::Image(tag) => vec![tag],
                }
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        let result = ws_client
            .delete_workspace(
                long_running_context(),
                DeleteWorkspaceRequest {
                    osuser: ws.osuser.clone(),
                    workspace_name: ws.name.clone(),
                    network,
                    images,
                },
            )
            .await?;

        let now = Utc::now();
        match result {
            Ok(_) => {
                let status = WorkspaceStatus::Deleted;
                let txn = self.db.conn.begin().await?;
                if let Some(usage_id) = ws.usage_id {
                    self.enterprise
                        .usage
                        .end_usage(&txn, usage_id, now.into())
                        .await?;
                }

                let host = self
                    .db
                    .get_workspace_host_with_lock(&txn, ws.host_id)
                    .await?;
                entities::workspace::ActiveModel {
                    id: ActiveValue::Set(ws.id),
                    status: ActiveValue::Set(status.to_string()),
                    usage_id: ActiveValue::Set(None),
                    deleted_at: ActiveValue::Set(Some(now.into())),
                    ..Default::default()
                }
                .update(&txn)
                .await?;

                if let Some(host) = host {
                    scheduler::recalcuate_workspce_host(&txn, &host, self.cpu_overcommit().await)
                        .await?;
                }

                txn.commit().await?;

                if let Some(prebuild_id) = ws.prebuild_id {
                    if let Err(e) = self
                        .check_outdated_prebuild(
                            ws.organization_id,
                            ws.user_id,
                            prebuild_id,
                            ip.clone(),
                            user_agent.clone(),
                        )
                        .await
                    {
                        let e = if let ApiError::InternalError(e) = e {
                            e
                        } else {
                            e.to_string()
                        };
                        tracing::error!("check outdated prebuild {prebuild_id} error: {e}");
                    }
                }
            }
            Err(e) => {
                let e = if let ApiError::InternalError(e) = e {
                    e
                } else {
                    e.to_string()
                };
                tracing::error!("delete workspace {} failed: {e}", ws.id);
                let status = WorkspaceStatus::DeleteFailed;
                entities::workspace::ActiveModel {
                    id: ActiveValue::Set(ws.id),
                    status: ActiveValue::Set(status.to_string()),
                    ..Default::default()
                }
                .update(&self.db.conn)
                .await?;
            }
        };

        Ok(())
    }

    pub async fn start_workspace(
        &self,
        workspace: entities::workspace::Model,
        // waiting for the actual start on workspace host
        waiting: bool,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<(), ApiError> {
        if workspace.is_compose && workspace.compose_parent.is_some() {
            return Err(ApiError::InvalidRequest(
                "You can't start a compose service workspace. You can only start the main workspace".to_string(),
            ));
        }

        let txn = self.db.conn.begin().await?;
        if let Some(quota) = self
            .enterprise
            .check_start_workspace_quota(&txn, workspace.organization_id, workspace.user_id)
            .await?
        {
            return Err(ApiError::QuotaReached(quota));
        }

        let now = Utc::now();
        self.enterprise
            .insert_audit_log(
                &txn,
                now.into(),
                workspace.user_id,
                workspace.organization_id,
                AuditResourceKind::Workspace.to_string(),
                workspace.id,
                workspace.name.clone(),
                AuditAction::WorkspaceStart.to_string(),
                ip,
                user_agent,
            )
            .await?;
        let update_ws = entities::workspace::ActiveModel {
            id: ActiveValue::Set(workspace.id),
            status: ActiveValue::Set(WorkspaceStatus::Starting.to_string()),
            ..Default::default()
        };
        let ws = update_ws.update(&txn).await?;
        txn.commit().await?;

        let ws_client = { self.rpcs.lock().await.get(&workspace.host_id).cloned() };
        let ws_client =
            ws_client.ok_or_else(|| anyhow!("can't connect to the workspace servic client"))?;

        let compose_services = if ws.is_compose {
            entities::workspace::Entity::find()
                .filter(entities::workspace::Column::DeletedAt.is_null())
                .filter(entities::workspace::Column::ComposeParent.eq(ws.id))
                .all(&self.db.conn)
                .await?
        } else {
            Vec::new()
        };

        if waiting {
            for ws in compose_services {
                self.do_start_workspace(&ws_client, &ws).await?;
            }
            self.do_start_workspace(&ws_client, &ws).await?;
        } else {
            let conductor = self.clone();
            tokio::spawn(async move {
                for ws in compose_services {
                    if let Err(e) = conductor.do_start_workspace(&ws_client, &ws).await {
                        tracing::error!("do start workspace {} error: {e}", ws.id);
                    }
                }
                if let Err(e) = conductor.do_start_workspace(&ws_client, &ws).await {
                    tracing::error!("do start workspace {} error: {e}", ws.id);
                }
            });
        }

        Ok(())
    }

    pub async fn stop_workspace(
        &self,
        workspace: entities::workspace::Model,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<(), ApiError> {
        if workspace.is_compose && workspace.compose_parent.is_some() {
            return Err(ApiError::InvalidRequest(
                "You can't stop a compose service workspace. You can only stop the main workspace"
                    .to_string(),
            ));
        }

        let now = Utc::now();

        let txn = self.db.conn.begin().await?;
        self.enterprise
            .insert_audit_log(
                &txn,
                now.into(),
                workspace.user_id,
                workspace.organization_id,
                AuditResourceKind::Workspace.to_string(),
                workspace.id,
                workspace.name.clone(),
                AuditAction::WorkspaceStop.to_string(),
                ip,
                user_agent,
            )
            .await?;
        let update_ws = entities::workspace::ActiveModel {
            id: ActiveValue::Set(workspace.id),
            status: ActiveValue::Set(WorkspaceStatus::Stopping.to_string()),
            ..Default::default()
        };
        let ws = update_ws.update(&txn).await?;
        txn.commit().await?;

        let ws_client = { self.rpcs.lock().await.get(&workspace.host_id).cloned() };
        let ws_client =
            ws_client.ok_or_else(|| anyhow!("can't connect to the workspace servic client"))?;

        let compose_services = if ws.is_compose {
            entities::workspace::Entity::find()
                .filter(entities::workspace::Column::DeletedAt.is_null())
                .filter(entities::workspace::Column::ComposeParent.eq(ws.id))
                .all(&self.db.conn)
                .await?
        } else {
            Vec::new()
        };

        {
            let conductor = self.clone();
            tokio::spawn(async move {
                for ws in compose_services {
                    if let Err(e) = conductor.do_stop_workspace(&ws_client, &ws).await {
                        tracing::error!("do stop workspace {} error: {e}", ws.id);
                    }
                }
                if let Err(e) = conductor.do_stop_workspace(&ws_client, &ws).await {
                    tracing::error!("do stop workspace {} error: {e}", ws.id);
                }
            });
        }

        Ok(())
    }

    async fn do_start_workspace(
        &self,
        ws_client: &WorkspaceServiceClient,
        ws: &entities::workspace::Model,
    ) -> Result<()> {
        let result = ws_client
            .start_workspace(
                long_running_context(),
                StartWorkspaceRequest {
                    osuser: ws.osuser.clone(),
                    workspace_name: ws.name.clone(),
                },
            )
            .await?;
        let now = Utc::now();
        match result {
            Ok(_) => {
                let machine_type = self
                    .db
                    .get_machine_type(ws.machine_type_id)
                    .await?
                    .ok_or_else(|| anyhow!("Can't find machine type".to_string()))?;

                let status = WorkspaceStatus::Running;
                let txn = self.db.conn.begin().await?;
                let usage = if ws.compose_parent.is_none() {
                    let usage = self
                        .enterprise
                        .usage
                        .new_usage(
                            &txn,
                            now.into(),
                            ws.organization_id,
                            Some(ws.user_id),
                            UsageResourceKind::Workspace,
                            ws.id,
                            ws.name.clone(),
                            machine_type.id,
                            machine_type.cost_per_second,
                        )
                        .await?;
                    Some(usage)
                } else {
                    None
                };
                entities::workspace::ActiveModel {
                    id: ActiveValue::Set(ws.id),
                    status: ActiveValue::Set(status.to_string()),
                    usage_id: ActiveValue::Set(usage.map(|u| u.id)),
                    last_inactivity: ActiveValue::Set(None),
                    ..Default::default()
                }
                .update(&txn)
                .await?;
                txn.commit().await?;
                status
            }
            Err(e) => {
                let e = if let ApiError::InternalError(e) = e {
                    e
                } else {
                    e.to_string()
                };
                tracing::error!("start workspace {} failed: {e}", ws.id);
                let status = WorkspaceStatus::Failed;
                entities::workspace::ActiveModel {
                    id: ActiveValue::Set(ws.id),
                    status: ActiveValue::Set(WorkspaceStatus::Failed.to_string()),
                    ..Default::default()
                }
                .update(&self.db.conn)
                .await?;
                status
            }
        };

        Ok(())
    }

    async fn do_stop_workspace(
        &self,
        ws_client: &WorkspaceServiceClient,
        ws: &entities::workspace::Model,
    ) -> Result<()> {
        let result = ws_client
            .stop_workspace(
                long_running_context(),
                StopWorkspaceRequest {
                    osuser: ws.osuser.clone(),
                    workspace_name: ws.name.clone(),
                },
            )
            .await?;
        let now = Utc::now();
        match result {
            Ok(_) => {
                let status = WorkspaceStatus::Stopped;
                let txn = self.db.conn.begin().await?;
                if let Some(usage_id) = ws.usage_id {
                    self.enterprise
                        .usage
                        .end_usage(&txn, usage_id, now.into())
                        .await?;
                }
                entities::workspace::ActiveModel {
                    id: ActiveValue::Set(ws.id),
                    status: ActiveValue::Set(status.to_string()),
                    usage_id: ActiveValue::Set(None),
                    ..Default::default()
                }
                .update(&txn)
                .await?;
                txn.commit().await?;
            }
            Err(e) => {
                let e = if let ApiError::InternalError(e) = e {
                    e
                } else {
                    e.to_string()
                };
                tracing::error!("stop workspace {} failed: {e}", ws.id);
                let status = WorkspaceStatus::StopFailed;
                entities::workspace::ActiveModel {
                    id: ActiveValue::Set(ws.id),
                    status: ActiveValue::Set(status.to_string()),
                    ..Default::default()
                }
                .update(&self.db.conn)
                .await?;
            }
        };

        Ok(())
    }

    pub async fn prebuild_updates(
        &self,
        prebuild_id: Uuid,
    ) -> UnboundedReceiver<PrebuildUpdateEvent> {
        let (mut tx, rx) = futures::channel::mpsc::unbounded::<PrebuildUpdateEvent>();
        {
            let mut prebuild_updates = self.prebuild_updates.lock().await;
            let update = prebuild_updates.entry(prebuild_id).or_default();
            for event in update.seen.clone() {
                let _ = tx.send(event).await;
            }
            update.subscribers.push(tx);
        }
        rx
    }

    pub async fn cleanup_prebuild_updates(&self, prebuild_id: Uuid) {
        let mut prebuild_updates = self.prebuild_updates.lock().await;
        if let Some(update) = prebuild_updates.get_mut(&prebuild_id) {
            update.subscribers.retain(|tx| !tx.is_closed());
        }
    }

    pub async fn workspace_updates(
        &self,
        workspace_id: Uuid,
    ) -> UnboundedReceiver<WorkspaceUpdateEvent> {
        let (mut tx, rx) = futures::channel::mpsc::unbounded::<WorkspaceUpdateEvent>();
        {
            let mut ws_updates = self.ws_updates.lock().await;
            let update = ws_updates.entry(workspace_id).or_default();
            for event in update.seen.clone() {
                let _ = tx.send(event).await;
            }
            update.subscribers.push(tx);
        }
        rx
    }

    pub async fn all_workspace_updates(
        &self,
        user_id: Uuid,
    ) -> UnboundedReceiver<(Uuid, WorkspaceStatus)> {
        let (tx, rx) = futures::channel::mpsc::unbounded();
        {
            let mut status_updates = self.all_workspace_updates.lock().await;
            let update = status_updates.entry(user_id).or_default();
            update.push(tx);
        }
        rx
    }

    pub async fn cleanup_workspace_updates(&self, workspace_id: Uuid) {
        let mut ws_updates = self.ws_updates.lock().await;
        let num = if let Some(update) = ws_updates.get_mut(&workspace_id) {
            update.subscribers.retain(|tx| !tx.is_closed());
            Some(update.subscribers.len())
        } else {
            None
        };
        if let Some(0) = num {
            ws_updates.remove(&workspace_id);
        }
    }

    pub async fn cleanup_all_workspace_updates(&self, user_id: Uuid) {
        let mut all_workspace_updates = self.all_workspace_updates.lock().await;
        let num = if let Some(update) = all_workspace_updates.get_mut(&user_id) {
            update.retain(|tx| !tx.is_closed());
            Some(update.len())
        } else {
            None
        };
        if let Some(0) = num {
            all_workspace_updates.remove(&user_id);
        }
    }

    pub async fn delete_project(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        project: &entities::project::Model,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<(), ApiError> {
        let now = Utc::now();
        let txn = self.db.conn.begin().await?;
        let project = entities::project::ActiveModel {
            id: ActiveValue::Set(project.id),
            deleted_at: ActiveValue::Set(Some(now.into())),
            ..Default::default()
        }
        .update(&txn)
        .await?;
        self.enterprise
            .insert_audit_log(
                &txn,
                now.into(),
                user_id,
                org_id,
                AuditResourceKind::Project.to_string(),
                project.id,
                project.name.clone(),
                AuditAction::ProjectDelete.to_string(),
                ip,
                user_agent,
            )
            .await?;
        txn.commit().await?;

        {
            let conductor = self.clone();
            tokio::spawn(async move {
                if let Err(e) = conductor.do_delete_project(&project).await {
                    let err = if let ApiError::InternalError(e) = e {
                        e.to_string()
                    } else {
                        e.to_string()
                    };
                    tracing::error!("do delete project {} error: {err}", project.id);
                }
            });
        }

        Ok(())
    }

    async fn do_delete_project(&self, project: &entities::project::Model) -> Result<(), ApiError> {
        let prebuilds = entities::prebuild::Entity::find()
            .filter(entities::prebuild::Column::ProjectId.eq(project.id))
            .filter(entities::prebuild::Column::DeletedAt.is_null())
            .all(&self.db.conn)
            .await?;

        for prebuild in prebuilds {
            if let Err(e) = self.do_delete_prebuild(&prebuild).await {
                let err = if let ApiError::InternalError(e) = e {
                    e.to_string()
                } else {
                    e.to_string()
                };
                tracing::error!("do delete prebuild {} error: {err}", prebuild.id);
            }
        }

        let path = PathBuf::from(format!("/var/lib/lapdev/projects/{}", project.id));
        tokio::fs::remove_dir_all(path).await?;

        Ok(())
    }

    pub async fn delete_prebuild(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        project: &entities::project::Model,
        prebuild: &entities::prebuild::Model,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<(), ApiError> {
        if prebuild.status == PrebuildStatus::Building.to_string() {
            return Err(ApiError::InvalidRequest(
                "You can't delete a building prebuild".to_string(),
            ));
        }

        let txn = self.db.conn.begin().await?;
        if entities::workspace::Entity::find()
            .filter(entities::workspace::Column::DeletedAt.is_null())
            .filter(entities::workspace::Column::PrebuildId.eq(prebuild.id))
            .lock_exclusive()
            .one(&txn)
            .await?
            .is_some()
        {
            return Err(ApiError::InvalidRequest(
                "You can't delete a prebuild with depending workspaces".to_string(),
            ));
        }

        entities::prebuild::ActiveModel {
            id: ActiveValue::Set(prebuild.id),
            deleted_at: ActiveValue::Set(Some(Utc::now().into())),
            ..Default::default()
        }
        .update(&txn)
        .await?;
        self.enterprise
            .insert_audit_log(
                &txn,
                Utc::now().into(),
                user_id,
                org_id,
                AuditResourceKind::Project.to_string(),
                prebuild.id,
                format!(
                    "{} {} {}",
                    project.name,
                    prebuild.branch,
                    &prebuild.commit[..7]
                ),
                AuditAction::PrebuildDelete.to_string(),
                ip,
                user_agent,
            )
            .await?;
        txn.commit().await?;

        {
            let conductor = self.clone();
            let prebuild = prebuild.to_owned();
            tokio::spawn(async move {
                if let Err(e) = conductor.do_delete_prebuild(&prebuild).await {
                    let err = if let ApiError::InternalError(e) = e {
                        e.to_string()
                    } else {
                        e.to_string()
                    };
                    tracing::error!("do delete prebuild {} error: {err}", prebuild.id);
                }
            });
        }

        Ok(())
    }

    async fn do_delete_prebuild(
        &self,
        prebuild: &entities::prebuild::Model,
    ) -> Result<(), ApiError> {
        let ws_client = { self.rpcs.lock().await.get(&prebuild.host_id).cloned() }
            .ok_or_else(|| anyhow!("can't find the workspace host rpc client"))?;
        let output: Option<RepoBuildOutput> = prebuild
            .build_output
            .as_ref()
            .and_then(|o| serde_json::from_str(o).ok());
        ws_client
            .delete_prebuild(
                long_running_context(),
                PrebuildInfo {
                    id: prebuild.id,
                    osuser: prebuild.osuser.clone(),
                },
                output.clone(),
            )
            .await??;

        let replicas = entities::prebuild_replica::Entity::find()
            .filter(entities::prebuild_replica::Column::DeletedAt.is_null())
            .filter(entities::prebuild_replica::Column::PrebuildId.eq(prebuild.id))
            .all(&self.db.conn)
            .await?;
        for replica in replicas {
            let ws_client = { self.rpcs.lock().await.get(&replica.host_id).cloned() }
                .ok_or_else(|| anyhow!("can't find the workspace host rpc client"))?;
            ws_client
                .delete_prebuild(
                    long_running_context(),
                    PrebuildInfo {
                        id: prebuild.id,
                        osuser: prebuild.osuser.clone(),
                    },
                    output.clone(),
                )
                .await??;
            entities::prebuild_replica::ActiveModel {
                id: ActiveValue::Set(replica.id),
                deleted_at: ActiveValue::Set(Some(Utc::now().into())),
                ..Default::default()
            }
            .update(&self.db.conn)
            .await?;
        }

        Ok(())
    }

    // check if the prebuild is used by any workspace,
    // if not, and if it's an outdated prebuild,
    // we'll delete it
    async fn check_outdated_prebuild(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        prebuild_id: Uuid,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<(), ApiError> {
        let prebuild = entities::prebuild::Entity::find_by_id(prebuild_id)
            .one(&self.db.conn)
            .await?
            .ok_or_else(|| anyhow!("no prebuild"))?;
        if prebuild.deleted_at.is_some() {
            return Ok(());
        }

        let project = self.db.get_project(prebuild.project_id).await?;
        let latest = entities::prebuild::Entity::find()
            .filter(entities::prebuild::Column::ProjectId.eq(prebuild.project_id))
            .filter(entities::prebuild::Column::DeletedAt.is_null())
            .order_by_desc(entities::prebuild::Column::CreatedAt)
            .one(&self.db.conn)
            .await?
            .ok_or_else(|| anyhow!("project doesn't have prebuilds"))?;

        if prebuild.id == latest.id {
            // if we're latest, check if the branch still exists
            if self
                .project_branches(
                    user_id,
                    &project,
                    ("".to_string(), "".to_string()),
                    ip.clone(),
                    user_agent.clone(),
                )
                .await
                .ok()
                .map(|branches| branches.iter().any(|b| b.name == prebuild.branch))
                .unwrap_or(false)
            {
                return Ok(());
            }
        }

        self.check_unused_prebuild(org_id, user_id, &project, &prebuild, ip, user_agent)
            .await?;

        Ok(())
    }

    async fn check_unused_prebuild(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        project: &entities::project::Model,
        prebuild: &entities::prebuild::Model,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<(), ApiError> {
        if entities::workspace::Entity::find()
            .filter(entities::workspace::Column::DeletedAt.is_null())
            .filter(entities::workspace::Column::PrebuildId.eq(prebuild.id))
            .one(&self.db.conn)
            .await?
            .is_some()
        {
            return Ok(());
        }
        self.delete_prebuild(org_id, user_id, project, prebuild, ip, user_agent)
            .await?;
        Ok(())
    }

    async fn check_deleted_project_branchs(
        &self,
        user_id: Uuid,
        project: &entities::project::Model,
        deleted_branches: Vec<String>,
        ip: Option<String>,
        user_agent: Option<String>,
    ) {
        for branch in deleted_branches {
            let _ = self
                .check_deleted_project_branch(
                    user_id,
                    project,
                    &branch,
                    ip.clone(),
                    user_agent.clone(),
                )
                .await;
        }
    }

    async fn check_deleted_project_branch(
        &self,
        user_id: Uuid,
        project: &entities::project::Model,
        branch: &str,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<(), ApiError> {
        let prebuilds = entities::prebuild::Entity::find()
            .filter(entities::prebuild::Column::ProjectId.eq(project.id))
            .filter(entities::prebuild::Column::Branch.eq(branch))
            .filter(entities::prebuild::Column::DeletedAt.is_null())
            .all(&self.db.conn)
            .await?;
        for prebuild in prebuilds {
            self.check_unused_prebuild(
                project.organization_id,
                user_id,
                project,
                &prebuild,
                ip.clone(),
                user_agent.clone(),
            )
            .await?;
        }
        Ok(())
    }
}

pub fn encode_pkcs8_pem(key: &KeyPair) -> Result<String> {
    let x = pkcs8::encode_pkcs8(key)?;
    Ok(format!(
        "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
        BASE64_MIME.encode(&x)
    ))
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

async fn clone_repo(
    repo_url: String,
    path: PathBuf,
    auth: (String, String),
) -> Result<(), ApiError> {
    tokio::task::spawn_blocking(move || -> Result<()> {
        println!("clone repo {repo_url}");
        let mut opt = git2::FetchOptions::new();
        let mut cbs = RemoteCallbacks::new();
        cbs.credentials(|_, _, _| Cred::userpass_plaintext(&auth.0, &auth.1));
        opt.remote_callbacks(cbs);
        git2::build::RepoBuilder::new()
            .fetch_options(opt)
            .clone(&repo_url, &path)?;
        Ok(())
    })
    .await??;
    Ok(())
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
