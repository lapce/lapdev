use std::collections::HashMap;

use anyhow::{anyhow, Result};
use futures::StreamExt;
use gloo_net::{
    http::Request,
    websocket::{futures::WebSocket, Message},
};
use lapdev_common::{
    console::Organization, utils, ClusterInfo, GitBranch, NewWorkspace, NewWorkspaceResponse,
    PrebuildStatus, ProjectInfo, ProjectPrebuild, RepoSource, RepobuildError, UpdateWorkspacePort,
    WorkspaceInfo, WorkspacePort, WorkspaceService, WorkspaceStatus, WorkspaceUpdateEvent,
};
use leptos::{prelude::*, task::spawn_local_scoped_with_cancellation};
use leptos_router::{
    hooks::{use_location, use_navigate, use_params_map},
    NavigateOptions,
};
use uuid::Uuid;

use crate::{
    app::AppConfig,
    cluster::get_cluster_info,
    component::{
        alert::{Alert, AlertDescription, AlertTitle, AlertVariant},
        button::{Button, ButtonVariant},
        dropdown_menu::{
            DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator,
            DropdownMenuTrigger,
        },
        input::Input,
        label::Label,
    },
    modal::{DatetimeModal, DeleteModal, ErrorResponse, Modal},
    organization::get_current_org,
    project::MachineTypeView,
};

#[derive(Clone)]
enum BuildMessage {
    Stdout(String),
    Stderr(String),
}

impl BuildMessage {
    fn msg(&self) -> &str {
        match self {
            BuildMessage::Stdout(s) => s,
            BuildMessage::Stderr(s) => s,
        }
    }

    fn is_error(&self) -> bool {
        matches!(self, BuildMessage::Stderr(_))
    }
}

async fn all_workspaces(org: Signal<Option<Organization>>) -> Result<Vec<WorkspaceInfo>> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let resp = Request::get(&format!("/api/v1/organizations/{}/workspaces", org.id))
        .send()
        .await?;
    let workspaces: Vec<WorkspaceInfo> = resp.json().await?;
    Ok(workspaces)
}

async fn get_workspace(org: Signal<Option<Organization>>, name: &str) -> Result<WorkspaceInfo> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let resp = Request::get(&format!(
        "/api/v1/organizations/{}/workspaces/{name}",
        org.id
    ))
    .send()
    .await?;
    let workspace: WorkspaceInfo = resp.json().await?;
    Ok(workspace)
}

async fn delete_workspace(
    navigate: impl Fn(&str, NavigateOptions) + Clone,
    org: Signal<Option<Organization>>,
    name: String,
    delete_modal_open: RwSignal<bool>,
    update_counter: Option<RwSignal<i32, LocalStorage>>,
) -> Result<(), ErrorResponse> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let resp = Request::delete(&format!(
        "/api/v1/organizations/{}/workspaces/{name}",
        org.id
    ))
    .send()
    .await?;
    if resp.status() != 204 {
        let error = resp
            .json::<ErrorResponse>()
            .await
            .unwrap_or_else(|_| ErrorResponse {
                error: "Internal Server Error".to_string(),
            });
        return Err(error);
    }

    delete_modal_open.set(false);
    if let Some(update_counter) = update_counter {
        update_counter.update(|c| *c += 1);
    } else {
        navigate("/workspaces", Default::default());
    }

    Ok(())
}

async fn rebuild_workspace(
    org: Signal<Option<Organization>>,
    name: String,
    rebuild_modal_open: RwSignal<bool>,
) -> Result<(), ErrorResponse> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let resp = Request::post(&format!(
        "/api/v1/organizations/{}/workspaces/{name}/rebuild",
        org.id
    ))
    .send()
    .await?;
    if resp.status() != 204 {
        let error = resp
            .json::<ErrorResponse>()
            .await
            .unwrap_or_else(|_| ErrorResponse {
                error: "Internal Server Error".to_string(),
            });
        return Err(error);
    }

    rebuild_modal_open.set(false);

    Ok(())
}

async fn stop_workspace(
    org: Signal<Option<Organization>>,
    name: String,
) -> Result<(), ErrorResponse> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let resp = Request::post(&format!(
        "/api/v1/organizations/{}/workspaces/{name}/stop",
        org.id
    ))
    .send()
    .await?;
    if resp.status() != 204 {
        let error = resp
            .json::<ErrorResponse>()
            .await
            .unwrap_or_else(|_| ErrorResponse {
                error: "Internal Server Error".to_string(),
            });
        return Err(error);
    }
    Ok(())
}

async fn start_workspace(
    org: Signal<Option<Organization>>,
    name: String,
) -> Result<(), ErrorResponse> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let resp = Request::post(&format!(
        "/api/v1/organizations/{}/workspaces/{name}/start",
        org.id
    ))
    .send()
    .await?;
    if resp.status() != 204 {
        let error = resp
            .json::<ErrorResponse>()
            .await
            .unwrap_or_else(|_| ErrorResponse {
                error: "Internal Server Error".to_string(),
            });
        return Err(error);
    }
    Ok(())
}

async fn pin_workspace(
    org: Signal<Option<Organization>>,
    name: String,
) -> Result<(), ErrorResponse> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let resp = Request::post(&format!(
        "/api/v1/organizations/{}/workspaces/{name}/pin",
        org.id
    ))
    .send()
    .await?;
    if resp.status() != 204 {
        let error = resp
            .json::<ErrorResponse>()
            .await
            .unwrap_or_else(|_| ErrorResponse {
                error: "Internal Server Error".to_string(),
            });
        return Err(error);
    }
    Ok(())
}

async fn unpin_workspace(
    org: Signal<Option<Organization>>,
    name: String,
) -> Result<(), ErrorResponse> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let resp = Request::post(&format!(
        "/api/v1/organizations/{}/workspaces/{name}/unpin",
        org.id
    ))
    .send()
    .await?;
    if resp.status() != 204 {
        let error = resp
            .json::<ErrorResponse>()
            .await
            .unwrap_or_else(|_| ErrorResponse {
                error: "Internal Server Error".to_string(),
            });
        return Err(error);
    }
    Ok(())
}

#[component]
fn OpenWorkspaceView(
    workspace_name: String,
    workspace_status: WorkspaceStatus,
    workspace_folder: String,
    workspace_hostname: String,
    align_right: bool,
) -> impl IntoView {
    let dropdown_expanded = RwSignal::new(false);

    let workspace_hostname = workspace_hostname.clone();
    let cluster_info = get_cluster_info();
    let port =
        cluster_info.with_untracked(|i| i.as_ref().map(|i| i.ssh_proxy_port).unwrap_or(2222));
    view! {
        <DropdownMenu open=dropdown_expanded>
            <div class="flex flex-row items-center">
                <a
                    // class={format!("{open_button_class} px-4 rounded-l-lg inline-block")}
                    class=if workspace_status == WorkspaceStatus::Running {
                        ""
                    } else {
                        "pointer-events-none cursor-default"
                    }
                    target="_blank"
                    href=format!("https://{workspace_name}.{workspace_hostname}/")
                >
                    <Button
                        class="rounded-r-none"
                        attr:disabled=move || workspace_status != WorkspaceStatus::Running
                    >
                        Open
                    </Button>
                </a>
                <DropdownMenuTrigger
                    open=dropdown_expanded
                    disabled=workspace_status != WorkspaceStatus::Running
                >
                    <Button
                        class="px-2 rounded-l-none"
                        attr:disabled=move || workspace_status != WorkspaceStatus::Running
                    >
                        <lucide_leptos::ChevronDown />
                    </Button>
                </DropdownMenuTrigger>
            </div>
            <DropdownMenuContent
                open=dropdown_expanded.read_only()
                class=format!("{} mt-2 min-w-56", if align_right { "right-0" } else { "" })
            >
                {
                    let workspace_hostname = workspace_hostname.clone();
                    let workspace_name = workspace_name.clone();
                    let workspace_folder = workspace_folder.clone();
                    view! {
                        <DropdownMenuItem class="cursor-pointer">
                            <a
                                href=format!(
                                    "vscode://vscode-remote/ssh-remote+{workspace_name}@{}{}/workspaces/{workspace_folder}",
                                    workspace_hostname.split(':').next().unwrap_or(""),
                                    if port == 22 { "".to_string() } else { format!(":{port}") },
                                )
                                target="_blank"
                            >
                                Open in VSCode Desktop
                            </a>
                        </DropdownMenuItem>
                    }
                }
            </DropdownMenuContent>
        </DropdownMenu>
    }
}

#[component]
fn WorkspaceControl(
    workspace_name: String,
    workspace_status: WorkspaceStatus,
    workspace_folder: String,
    workspace_hostname: String,
    workspace_pinned: bool,
    rebuild_modal_open: RwSignal<bool>,
    delete_modal_open: RwSignal<bool>,
    error: RwSignal<Option<String>, LocalStorage>,
    align_right: bool,
) -> impl IntoView {
    let org = get_current_org();
    let dropdown_expanded = RwSignal::new(false);

    let delete_workspace = {
        move |_| {
            dropdown_expanded.set(false);
            delete_modal_open.set(true);
        }
    };

    let rebuild_workspace = {
        move |_| {
            dropdown_expanded.set(false);
            rebuild_modal_open.set(true);
        }
    };

    let stop_action = {
        let workspace_name = workspace_name.clone();
        Action::new_local(move |_| {
            let workspace_name = workspace_name.clone();
            async move {
                if let Err(e) = stop_workspace(org, workspace_name.clone()).await {
                    error.set(Some(e.error));
                }
            }
        })
    };
    let stop_workspace = move |_| {
        dropdown_expanded.set(false);
        error.set(None);
        stop_action.dispatch(());
    };

    let start_action = {
        let workspace_name = workspace_name.clone();
        Action::new_local(move |_| {
            let workspace_name = workspace_name.clone();
            async move {
                if let Err(e) = start_workspace(org, workspace_name.clone()).await {
                    error.set(Some(e.error));
                }
            }
        })
    };
    let start_workspace = move |_| {
        dropdown_expanded.set(false);
        error.set(None);
        start_action.dispatch(());
    };

    let pin_action = {
        let workspace_name = workspace_name.clone();
        Action::new_local(move |_| {
            let workspace_name = workspace_name.clone();
            async move {
                if let Err(e) = pin_workspace(org, workspace_name.clone()).await {
                    error.set(Some(e.error));
                }
            }
        })
    };
    let pin_workspace = move |_| {
        dropdown_expanded.set(false);
        error.set(None);
        pin_action.dispatch(());
    };

    let unpin_action = {
        let workspace_name = workspace_name.clone();
        Action::new_local(move |_| {
            let workspace_name = workspace_name.clone();
            async move {
                if let Err(e) = unpin_workspace(org, workspace_name.clone()).await {
                    error.set(Some(e.error));
                }
            }
        })
    };
    let unpin_workspace = move |_| {
        dropdown_expanded.set(false);
        error.set(None);
        unpin_action.dispatch(());
    };

    view! {
        <div class="flex flex-row items-center">
            <OpenWorkspaceView
                workspace_name
                workspace_status
                workspace_folder
                workspace_hostname
                align_right
            />

            <DropdownMenu class="ml-2" open=dropdown_expanded>
                <DropdownMenuTrigger open=dropdown_expanded>
                    <Button variant=ButtonVariant::Ghost class="px-2">
                        <lucide_leptos::EllipsisVertical />
                    </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent
                    open=dropdown_expanded.read_only()
                    class=format!("{} mt-2 min-w-56", if align_right { "right-0" } else { "" })
                >
                    <DropdownMenuItem
                        on:click=start_workspace
                        disabled=workspace_status != WorkspaceStatus::Stopped
                        class="cursor-pointer"
                    >
                        Start
                    </DropdownMenuItem>
                    <DropdownMenuItem
                        on:click=stop_workspace
                        disabled=workspace_status == WorkspaceStatus::Stopped
                        class="cursor-pointer"
                    >
                        Stop
                    </DropdownMenuItem>
                    <DropdownMenuSeparator />

                    <DropdownMenuItem
                        on:click=pin_workspace
                        disabled=workspace_pinned
                        class="cursor-pointer"
                    >
                        Pin
                    </DropdownMenuItem>
                    <DropdownMenuItem
                        on:click=unpin_workspace
                        disabled=!workspace_pinned
                        class="cursor-pointer"
                    >
                        Unpin
                    </DropdownMenuItem>
                    <DropdownMenuSeparator />
                    <DropdownMenuItem on:click=rebuild_workspace class="cursor-pointer">
                        Rebuild
                    </DropdownMenuItem>
                    <DropdownMenuItem on:click=delete_workspace class="cursor-pointer">
                        Delete
                    </DropdownMenuItem>
                </DropdownMenuContent>
            </DropdownMenu>
        </div>
    }
}

fn workspace_status_class(status: &WorkspaceStatus) -> &str {
    match status {
        WorkspaceStatus::Running => "bg-green-100 text-green-800",
        WorkspaceStatus::Failed
        | WorkspaceStatus::Deleting
        | WorkspaceStatus::DeleteFailed
        | WorkspaceStatus::StopFailed => "bg-red-100 text-red-800",
        WorkspaceStatus::New
        | WorkspaceStatus::Building
        | WorkspaceStatus::PrebuildBuilding
        | WorkspaceStatus::PrebuildCopying
        | WorkspaceStatus::Stopping
        | WorkspaceStatus::Starting => "bg-yellow-100 text-yellow-800",
        WorkspaceStatus::Stopped | WorkspaceStatus::Deleted => "bg-gray-100 text-gray-800",
    }
}

#[component]
fn WorkspaceRebuildModal(
    workspace_name: String,
    rebuild_modal_open: RwSignal<bool>,
) -> impl IntoView {
    let org = get_current_org();
    let rebuild_action = {
        let workspace_name = workspace_name.clone();
        Action::new_local(move |_| {
            rebuild_workspace(org, workspace_name.clone(), rebuild_modal_open)
        })
    };

    view! {
        <Modal
            title="Create New Workspace"
            open=rebuild_modal_open
            action=rebuild_action
            action_text="Rebuild"
            action_progress_text="Rebuilding"
        >
            <p>Rebuild your workspace image.</p>
            <p>It will pick up your devcontainer config changes.</p>
            <p>
                All files in
                <span class="text-semibold mx-1 px-1 text-gray-800 bg-gray-200 rounded">
                    /workspaces
                </span>folder will persist.
            </p>
        </Modal>
    }
}

#[component]
pub fn WorkspaceItem(
    workspace: WorkspaceInfo,
    error: RwSignal<Option<String>, LocalStorage>,
    update_counter: RwSignal<i32, LocalStorage>,
) -> impl IntoView {
    let rebuild_modal_open = RwSignal::new(false);
    let delete_modal_open = RwSignal::new(false);
    let org = get_current_org();
    let workspace_name = workspace.name.clone();
    let workspace_folder = workspace.repo_name.clone();
    let workspace_hostname = workspace.hostname.clone();
    let workspace_hostname = if workspace_hostname.is_empty() {
        window().window().location().hostname().unwrap_or_default()
    } else {
        workspace_hostname
    };
    let delete_action = {
        let workspace_name = workspace_name.clone();
        let navigate = use_navigate();
        Action::new_local(move |_| {
            delete_workspace(
                navigate.clone(),
                org,
                workspace_name.clone(),
                delete_modal_open,
                Some(update_counter),
            )
        })
    };
    let status_class = workspace_status_class(&workspace.status);
    let status_class = format!("{status_class} text-sm font-medium me-2 px-2.5 py-0.5 rounded");
    view! {
        <div class="w-full mb-8 bg-white rounded-lg border border-gray-200 shadow-lg">
            <div class="flex flex-col items-center w-full lg:flex-row">
                <div class="lg:w-1/3 flex flex-row">
                    <a href=workspace.repo_url.clone() target="_blank">
                        <img
                            src=repo_img(&workspace.repo_url)
                            class="object-contain lg:object-left h-40 rounded-lg justify-self-start"
                        />
                    </a>
                </div>
                <div class="lg:w-1/3 flex flex-col justify-center">
                    <div class="flex flex-col p-4">
                        <a href=format!("/workspaces/{}", workspace.name)>
                            <a
                                class="font-semibold"
                                href=format!("/workspaces/{}", workspace.name.clone())
                            >
                                {
                                    let workspace_name = workspace.name.clone();
                                    move || workspace_name.clone()
                                }
                            </a>
                            <a
                                href=workspace.repo_url.clone()
                                target="_blank"
                                class="flex flex-row items-center text-sm text-gray-700 mt-2"
                            >
                                <svg
                                    class="w-4 h-4 mr-1"
                                    aria-hidden="true"
                                    xmlns="http://www.w3.org/2000/svg"
                                    fill="none"
                                    viewBox="0 0 24 24"
                                >
                                    <path
                                        stroke="currentColor"
                                        stroke-linecap="round"
                                        stroke-linejoin="round"
                                        stroke-width="2"
                                        d="M13.2 9.8a3.4 3.4 0 0 0-4.8 0L5 13.2A3.4 3.4 0 0 0 9.8 18l.3-.3m-.3-4.5a3.4 3.4 0 0 0 4.8 0L18 9.8A3.4 3.4 0 0 0 13.2 5l-1 1"
                                    />
                                </svg>
                                <p>
                                    {
                                        let repo_url = workspace.repo_url.clone();
                                        move || repo_url.clone()
                                    }
                                </p>
                            </a>
                            <span class="flex flex-row truncate items-center text-sm text-gray-700 mt-2">
                                <svg
                                    class="w-3 h-3 mr-2"
                                    aria-hidden="true"
                                    xmlns="http://www.w3.org/2000/svg"
                                    fill="none"
                                    viewBox="0 0 14 20"
                                >
                                    <path
                                        stroke="currentColor"
                                        stroke-linecap="round"
                                        stroke-linejoin="round"
                                        stroke-width="2"
                                        d="M3 5v10M3 5a2 2 0 1 0 0-4 2 2 0 0 0 0 4Zm0 10a2 2 0 1 0 0 4 2 2 0 0 0 0-4Zm6-3.976-2-.01A4.015 4.015 0 0 1 3 7m10 4a2 2 0 1 1-4 0 2 2 0 0 1 4 0Z"
                                    />
                                </svg>
                                <span class="mr-2">{workspace.branch}</span>
                                <span>{workspace.commit[..7].to_string()}</span>
                            </span>
                            <span class="mt-2 text-sm text-gray-800 inline-flex items-center rounded me-2 text-gray-700">
                                <svg
                                    class="w-2.5 h-2.5 mr-2.5"
                                    aria-hidden="true"
                                    xmlns="http://www.w3.org/2000/svg"
                                    fill="currentColor"
                                    viewBox="0 0 20 20"
                                >
                                    <path d="M10 0a10 10 0 1 0 10 10A10.011 10.011 0 0 0 10 0Zm3.982 13.982a1 1 0 0 1-1.414 0l-3.274-3.274A1.012 1.012 0 0 1 9 10V6a1 1 0 0 1 2 0v3.586l2.982 2.982a1 1 0 0 1 0 1.414Z" />
                                </svg>
                                <span class="mr-1">Created</span>
                                <DatetimeModal time=workspace.created_at />
                            </span>
                        </a>
                    </div>
                </div>
                <div class="lg:w-1/3 flex flex-col xl:flex-row items-center justify-between">
                    <div class="flex flex-row items-center">
                        <span class=status_class>{move || workspace.status.to_string()}</span>
                    </div>
                    <div class="flex flex-row items-center p-4 justify-center">
                        <div class="w-4 h-4 mr-6">
                            <svg
                                class:hidden=move || !workspace.pinned
                                class="w-4 h-4"
                                xmlns="http://www.w3.org/2000/svg"
                                width="24px"
                                height="24px"
                                viewBox="0 0 24 24"
                                fill="none"
                                stroke="currentColor"
                                stroke-width="2"
                                stroke-linecap="round"
                                stroke-linejoin="round"
                            >
                                <line x1="12" x2="12" y1="17" y2="22"></line>
                                <path d="M5 17h14v-1.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V6h1a2 2 0 0 0 0-4H8a2 2 0 0 0 0 4h1v4.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24Z"></path>
                            </svg>
                        </div>
                        <WorkspaceControl
                            workspace_name=workspace_name.clone()
                            workspace_status=workspace.status
                            workspace_folder=workspace_folder.clone()
                            workspace_hostname=workspace_hostname.clone()
                            workspace_pinned=workspace.pinned
                            align_right=true
                            delete_modal_open
                            rebuild_modal_open
                            error
                        />
                    </div>
                </div>
            </div>
            <For
                each=move || workspace.services.clone()
                key=move |s| s.clone()
                children={
                    let workspace_folder = workspace_folder.clone();
                    let workspace_hostname = workspace_hostname.clone();
                    move |ws_service| {
                        view! {
                            <div class="flex flex-col border-t items-center shadow-sm lg:flex-row w-full">
                                <div class="hidden w-1/3 lg:flex flex-row"></div>
                                <div class="w-full lg:w-1/3 flex flex-col items-center lg:items-start">
                                    <div class="flex flex-col p-4">
                                        <span class="font-semibold">{ws_service.name.clone()}</span>
                                        <div class="flex flex-row items-center text-sm text-gray-700 mt-2">
                                            <svg
                                                class="w-4 h-4 mr-1"
                                                xmlns="http://www.w3.org/2000/svg"
                                                fill="currentColor"
                                                viewBox="0 0 16 16"
                                            >
                                                <path
                                                    fill-rule="evenodd"
                                                    d="M6 3.5A1.5 1.5 0 0 1 7.5 2h1A1.5 1.5 0 0 1 10 3.5v1A1.5 1.5 0 0 1 8.5 6v1H11a.5.5 0 0 1 .5.5v1a.5.5 0 0 1-1 0V8h-5v.5a.5.5 0 0 1-1 0v-1A.5.5 0 0 1 5 7h2.5V6A1.5 1.5 0 0 1 6 4.5zM8.5 5a.5.5 0 0 0 .5-.5v-1a.5.5 0 0 0-.5-.5h-1a.5.5 0 0 0-.5.5v1a.5.5 0 0 0 .5.5zM3 11.5A1.5 1.5 0 0 1 4.5 10h1A1.5 1.5 0 0 1 7 11.5v1A1.5 1.5 0 0 1 5.5 14h-1A1.5 1.5 0 0 1 3 12.5zm1.5-.5a.5.5 0 0 0-.5.5v1a.5.5 0 0 0 .5.5h1a.5.5 0 0 0 .5-.5v-1a.5.5 0 0 0-.5-.5zm4.5.5a1.5 1.5 0 0 1 1.5-1.5h1a1.5 1.5 0 0 1 1.5 1.5v1a1.5 1.5 0 0 1-1.5 1.5h-1A1.5 1.5 0 0 1 9 12.5zm1.5-.5a.5.5 0 0 0-.5.5v1a.5.5 0 0 0 .5.5h1a.5.5 0 0 0 .5-.5v-1a.5.5 0 0 0-.5-.5z"
                                                />
                                            </svg>
                                            <p>{ws_service.service.clone()}</p>
                                        </div>
                                    </div>
                                </div>
                                <div class="w-full lg:w-1/3 flex flex-row items-center p-4 justify-center xl:justify-end">
                                    <div class="mx-11">
                                        <OpenWorkspaceView
                                            workspace_name=ws_service.name.clone()
                                            workspace_status=workspace.status
                                            workspace_folder=workspace_folder.clone()
                                            workspace_hostname=workspace_hostname.clone()
                                            align_right=true
                                        />
                                    </div>
                                </div>
                            </div>
                        }
                    }
                }
            />
        </div>
        <DeleteModal resource=workspace_name.clone() open=delete_modal_open delete_action />
        <WorkspaceRebuildModal workspace_name=workspace_name.clone() rebuild_modal_open />
    }
}

async fn watch_all_workspace_status(
    status_update: RwSignal<(String, WorkspaceStatus), LocalStorage>,
) -> Result<()> {
    let location = window().location();
    let protocol = if location.protocol().map(|p| p == "http:").unwrap_or(false) {
        "ws"
    } else {
        "wss"
    };
    let host = location.host().unwrap();
    let mut websocket = WebSocket::open(&format!("{protocol}://{host}/all_workspaces_ws"))?;

    while let Some(Ok(msg)) = websocket.next().await {
        if let Message::Text(s) = msg {
            if let Ok((ws_name, status)) = serde_json::from_str::<(String, WorkspaceStatus)>(&s) {
                status_update.set((ws_name, status));
            }
        }
    }

    Ok(())
}

#[component]
pub fn Workspaces() -> impl IntoView {
    let error = RwSignal::new_local(None);
    let status_update = RwSignal::new_local(("".to_string(), WorkspaceStatus::New));
    let org = get_current_org();

    spawn_local_scoped_with_cancellation(async move {
        let _ = watch_all_workspace_status(status_update).await;
    });

    let new_workspace_modal_open = RwSignal::new(false);

    let delete_counter = RwSignal::new_local(0);
    Effect::new(move |_| {
        status_update.track();
        delete_counter.update(|c| {
            *c += 1;
        });
    });

    let workspaces_resource =
        LocalResource::new(move || async move { all_workspaces(org).await.unwrap_or_default() });
    Effect::new(move |_| {
        delete_counter.track();
        workspaces_resource.refetch();
    });

    let workspace_filter = RwSignal::new_local(String::new());

    let workspaces = Signal::derive(move || {
        let mut workspaces = workspaces_resource.get().unwrap_or_default();
        let workspace_filter = workspace_filter.get();
        workspaces.retain(|w| w.name.contains(&workspace_filter));
        workspaces
    });

    let new_workspace = move |_| {
        new_workspace_modal_open.set(true);
    };

    {
        let hash = use_location().hash.get_untracked();
        if let Some(url) = hash.strip_prefix("#") {
            let url = utils::format_repo_url(url);
            let action_dispatched = RwSignal::new_local(false);
            let cluster_info = get_cluster_info();
            let current_org = get_current_org();
            let navigate = use_navigate();
            let action = Action::new_local(move |url: &String| {
                let cluster_info = get_cluster_info();
                create_workspace(
                    navigate.clone(),
                    current_org,
                    cluster_info,
                    RepoSource::Url(url.clone()),
                    None,
                    None,
                    true,
                )
            });
            Effect::new(move |_| {
                let org = current_org.get();
                if org.is_none() {
                    return;
                }
                let cluster_info = cluster_info.get();
                if cluster_info.is_none() {
                    return;
                }
                if !action_dispatched.get_untracked() {
                    action.dispatch(url.clone());
                    action_dispatched.set(true);
                }
            });
            Effect::new(move |_| {
                action.value().with(|result| {
                    if let Some(result) = result {
                        match result {
                            Ok(_) => {}
                            Err(e) => {
                                error.set(Some(e.error.clone()));
                            }
                        }
                    }
                })
            });
        }
    }

    view! {
        <section class="w-full h-full flex items-center flex-col">
            <div class="mx-auto w-full">
                <div class="flex-row items-center justify-between space-y-3 sm:flex sm:space-y-0 sm:space-x-4">
                    <div>
                        <h5 class="mr-3 text-2xl font-semibold">All Workspaces</h5>
                        <p class="text-gray-700">
                            Manage all your existing workspaces or add a new one
                        </p>
                    </div>
                </div>
                <div class="flex flex-row items-center justify-between py-4 space-x-4">
                    <div class="w-1/2">
                        <form class="flex items-center">
                            <label for="simple-search" class="sr-only">
                                Filter Workspaces
                            </label>
                            <div class="relative w-full">
                                <div class="absolute inset-y-0 left-0 flex items-center pl-3 pointer-events-none">
                                    <svg
                                        aria-hidden="true"
                                        class="w-5 h-5 text-gray-500"
                                        fill="currentColor"
                                        viewbox="0 0 20 20"
                                        xmlns="http://www.w3.org/2000/svg"
                                    >
                                        <path
                                            fill-rule="evenodd"
                                            d="M8 4a4 4 0 100 8 4 4 0 000-8zM2 8a6 6 0 1110.89 3.476l4.817 4.817a1 1 0 01-1.414 1.414l-4.816-4.816A6 6 0 012 8z"
                                            clip-rule="evenodd"
                                        />
                                    </svg>
                                </div>
                                <Input
                                    prop:value=move || workspace_filter.get()
                                    on:input=move |ev| {
                                        workspace_filter.set(event_target_value(&ev));
                                    }
                                    class="pl-10"
                                    attr:placeholder="Filter Workspaces"
                                />
                            </div>
                        </form>
                    </div>
                    <Button on:click=new_workspace>New Workspace</Button>
                </div>
            </div>

            {move || {
                if let Some(error) = error.get() {
                    view! {
                        <Alert variant=AlertVariant::Destructive>
                            <lucide_leptos::CircleAlert />
                            <AlertTitle>Error</AlertTitle>
                            <AlertDescription>{error}</AlertDescription>
                        </Alert>
                    }
                        .into_any()
                } else {
                    ().into_any()
                }
            }}

            <div class="w-full basis-0 grow">
                <div class="w-full h-full flex flex-col py-4">
                    <For
                        each=move || workspaces.get()
                        key=|w| (w.name.clone(), w.status, w.pinned)
                        children=move |workspace| {
                            view! {
                                <WorkspaceItem
                                    workspace=workspace
                                    update_counter=delete_counter
                                    error
                                />
                            }
                        }
                    />
                </div>
            </div>

            <NewWorkspaceModal
                modal_open=new_workspace_modal_open
                project_info=RwSignal::new_local(None)
            />

        </section>
    }
}

pub async fn create_workspace(
    navigate: impl Fn(&str, NavigateOptions) + Clone,
    org: Signal<Option<Organization>>,
    cluster_info: Signal<Option<ClusterInfo>, LocalStorage>,
    source: RepoSource,
    branch: Option<String>,
    machine_type: Option<Uuid>,
    from_hash: bool,
) -> Result<(), ErrorResponse> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;

    let machine_type_id = machine_type.unwrap_or_else(|| {
        cluster_info
            .get_untracked()
            .and_then(|i| i.machine_types.first().map(|m| m.id))
            .unwrap_or_else(|| Uuid::from_u128(0))
    });
    let resp = Request::post(&format!("/api/v1/organizations/{}/workspaces", org.id))
        .json(&NewWorkspace {
            source,
            branch,
            machine_type_id,
            from_hash,
        })?
        .send()
        .await?;
    if resp.status() != 200 {
        let error = resp
            .json::<ErrorResponse>()
            .await
            .unwrap_or_else(|_| ErrorResponse {
                error: "Internal Server Error".to_string(),
            });
        return Err(error);
    }

    let resp: NewWorkspaceResponse = resp.json().await?;
    navigate(&format!("/workspaces/{}", resp.name), Default::default());

    Ok(())
}

#[derive(Clone)]
pub struct CreateWorkspaceProjectInfo {
    pub project: ProjectInfo,
    pub branch: Option<GitBranch>,
    pub branches: Vec<GitBranch>,
    pub prebuilds: HashMap<String, ProjectPrebuild>,
}

pub fn repo_img(repo_url: &str) -> String {
    if let Some(sufix) = repo_url.strip_prefix("https://github.com/") {
        format!("https://opengraph.githubassets.com/a/{sufix}")
    } else {
        "".to_string()
    }
}

#[component]
pub fn NewWorkspaceModal(
    modal_open: RwSignal<bool>,
    project_info: RwSignal<Option<CreateWorkspaceProjectInfo>, LocalStorage>,
) -> impl IntoView {
    let repo_url = RwSignal::new_local("".to_string());
    let current_branch = RwSignal::new_local(None);
    let current_machine_type = RwSignal::new_local(None);
    let org = get_current_org();

    Effect::new(move |_| {
        let branch = project_info.with(|info| {
            info.as_ref()
                .and_then(|info| info.branch.as_ref().or(info.branches.first()))
                .cloned()
        });
        current_branch.set(branch);
    });

    let preferred_machine_type = Signal::derive_local(move || {
        project_info.with(|info| info.as_ref().map(|info| info.project.machine_type))
    });

    let current_project =
        Signal::derive(move || project_info.with(|info| info.as_ref().map(|info| info.project.id)));

    let select_on_change = move |ev: web_sys::Event| {
        let name = event_target_value(&ev);
        let branch = project_info.with_untracked(|info| {
            info.as_ref()
                .and_then(|info| info.branches.iter().find(|b| b.name == name))
                .cloned()
        });
        current_branch.set(branch);
    };

    let navigate = use_navigate();
    let action = Action::new_unsync_local(move |_| {
        let source = if let Some(project) = current_project.get_untracked() {
            RepoSource::Project(project)
        } else {
            RepoSource::Url(repo_url.get_untracked())
        };
        let cluster_info = get_cluster_info();
        create_workspace(
            navigate.clone(),
            org,
            cluster_info,
            source,
            current_branch.get_untracked().map(|b| b.name),
            current_machine_type.get_untracked(),
            false,
        )
    });

    let no_project_view = move || {
        view! {
            <div class="flex flex-col gap-2">
                <Label>Your repository url</Label>
                <Input
                    prop:value=move || repo_url.get()
                    on:input=move |ev| {
                        repo_url.set(event_target_value(&ev));
                    }
                    {..}
                    placeholder="https://github.com/owner/repo"
                    required=true
                />
            </div>
            <MachineTypeView current_machine_type preferred_machine_type />
        }
    };

    view! {
        <Modal title="Create New Workspace" open=modal_open action>
            <Show when=move || { project_info.with(|i| i.is_some()) } fallback=no_project_view>
                {if let Some(project_info) = project_info.get() {
                    let branches = project_info.branches.clone();
                    view! {
                        <div class="flex flex-col gap-2">
                            <Label>Project Name</Label>
                            <Input
                                class="disabled:opacity-80 disabled:cursor-text disabled:pointer-events-auto shadow-none border-none"
                                attr:value=project_info.project.name
                                attr:disabled=true
                            />
                        </div>
                        <div class="flex flex-col gap-2">
                            <Label>Project Repository</Label>
                            <Input
                                class="disabled:opacity-80 disabled:cursor-text disabled:pointer-events-auto shadow-none border-none"
                                attr:value=project_info.project.repo_url
                                attr:disabled=true
                            />
                        </div>
                        <div class="flex flex-col gap-2">
                            <Label>Branch</Label>
                            <select
                                class="bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5"
                                on:change=select_on_change
                            >
                                <For
                                    each=move || branches.clone()
                                    key=|b| b.name.clone()
                                    children=move |branch| {
                                        let branch_name = branch.name.clone();
                                        view! {
                                            <option selected=move || {
                                                Some(&branch_name)
                                                    == current_branch.get().as_ref().map(|b| &b.name)
                                            }>{branch.name}</option>
                                        }
                                    }
                                />
                            </select>
                        </div>
                        <div class="flex flex-col gap-2">
                            <Label>Prebuild Status</Label>
                            <Input
                                class="disabled:opacity-80 disabled:cursor-text disabled:pointer-events-auto shadow-none border-none"
                                attr:value=move || {
                                    if let Some(branch) = current_branch.get() {
                                        project_info
                                            .prebuilds
                                            .get(&branch.name)
                                            .and_then(|prebuild| {
                                                if prebuild.commit == branch.commit {
                                                    Some(prebuild.status.to_string())
                                                } else if prebuild.status == PrebuildStatus::Ready {
                                                    Some("Outdated".to_string())
                                                } else {
                                                    None
                                                }
                                            })
                                    } else {
                                        None
                                    }
                                        .unwrap_or_else(|| "Not Created".to_string())
                                }
                                attr:disabled=true
                            />
                        </div>
                        <MachineTypeView current_machine_type preferred_machine_type />
                    }
                        .into_any()
                } else {
                    ().into_any()
                }}
            </Show>
        </Modal>
    }
}

async fn watch_workspace_updates(
    navigate: impl Fn(&str, NavigateOptions) + Clone,
    name: &str,
    status: RwSignal<WorkspaceStatus, LocalStorage>,
    build_messages: RwSignal<Vec<BuildMessage>, LocalStorage>,
) -> Result<()> {
    let location = window().location();
    let host = location.host().map_err(|_| anyhow!("can't get host"))?;
    let protocol = if location.protocol().map(|p| p == "http:").unwrap_or(false) {
        "ws"
    } else {
        "wss"
    };
    let mut websocket = WebSocket::open(&format!("{protocol}://{host}/ws?name={}", name))?;
    while let Some(Ok(msg)) = websocket.next().await {
        if let Message::Text(s) = msg {
            if let Ok(event) = serde_json::from_str::<WorkspaceUpdateEvent>(&s) {
                match event {
                    WorkspaceUpdateEvent::Status(s) => {
                        status.set(s);
                        if s == WorkspaceStatus::Deleted {
                            if let Ok(pathname) = window().location().pathname() {
                                if pathname.ends_with(name) {
                                    navigate("/workspaces", Default::default());
                                }
                            }
                        }
                    }
                    WorkspaceUpdateEvent::Stdout(line) => {
                        build_messages.update(|messages| {
                            messages.push(BuildMessage::Stdout(line));
                        });
                        let _ = scroll_build_messages();
                    }
                    WorkspaceUpdateEvent::Stderr(line) => {
                        build_messages.update(|messages| {
                            messages.push(BuildMessage::Stderr(line));
                        });
                        let _ = scroll_build_messages();
                    }
                }
            }
        }
    }
    Ok(())
}

fn scroll_build_messages() -> Result<()> {
    let scroll = window()
        .document()
        .ok_or_else(|| anyhow!("can't find document"))?
        .get_element_by_id("build-messages-scroll")
        .ok_or_else(|| anyhow!("can't find element"))?;
    scroll.set_scroll_top(scroll.scroll_height());
    Ok(())
}

#[component]
pub fn WorkspaceDetails() -> impl IntoView {
    let error = RwSignal::new_local(None);
    let params = use_params_map();
    let name = params.with_untracked(|params| params.get("name").clone().unwrap());
    let local_name = name.clone();
    let workspace_name = name.clone();
    let rebuild_modal_open = RwSignal::new(false);
    let delete_modal_open = RwSignal::new(false);
    let org = get_current_org();
    let delete_action = {
        let workspace_name = workspace_name.clone();
        let navigate = use_navigate();
        Action::new_local(move |_| {
            delete_workspace(
                navigate.clone(),
                org,
                workspace_name.clone(),
                delete_modal_open,
                None,
            )
        })
    };
    let cluster_info = get_cluster_info();
    let machine_types = RwSignal::new_local(cluster_info.get_untracked().map(|i| i.machine_types));

    let update_counter = RwSignal::new_local(0);
    let workspace_info = LocalResource::new(move || async move {
        let name = params.with_untracked(|params| params.get("name").unwrap_or_default());
        get_workspace(org, &name).await
    });
    Effect::new(move |_| {
        update_counter.track();
        workspace_info.refetch();
    });

    let build_messages = RwSignal::new_local(Vec::new());
    let status = RwSignal::new_local(WorkspaceStatus::New);
    {
        let name = local_name.clone();
        let navigate = use_navigate();
        spawn_local_scoped_with_cancellation(async move {
            let _ = watch_workspace_updates(navigate, &name, status, build_messages).await;
        });
    }

    let config = use_context::<AppConfig>().unwrap();
    Effect::new(move |_| {
        if let Some(info) = workspace_info.with(|info| {
            info.as_ref()
                .map(|info| info.as_ref().ok().cloned())
                .clone()
                .flatten()
        }) {
            config.current_page.set(info.name);
            if status.get_untracked() != info.status {
                status.set(info.status);
            }
        }
    });
    Effect::new(move |_| {
        status.track();
        update_counter.update(|c| *c += 1);
    });
    view! {
        <section class="w-full flex flex-col">
            <a href="/workspaces" class="text-sm inline-flex items-center text-gray-700">
                <svg
                    class="w-2 h-2 mr-2"
                    aria-hidden="true"
                    xmlns="http://www.w3.org/2000/svg"
                    fill="none"
                    viewBox="0 0 14 10"
                >
                    <path
                        stroke="currentColor"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        stroke-width="2"
                        d="M13 5H1m0 0 4 4M1 5l4-4"
                    />
                </svg>
                Back to Workspace List
            </a>
            <h5 class="text-semibold text-2xl mt-4">{local_name.clone()}</h5>
            <Show
                when=move || {
                    workspace_info
                        .with(|info| info.as_ref().map(|info| info.is_ok()).unwrap_or(false))
                }
                fallback=move || {}
            >
                {if let Some(info) = workspace_info
                    .with(|info| {
                        info.as_ref().map(|info| info.as_ref().ok().cloned()).clone().flatten()
                    })
                {
                    let workspace_hostname = info.hostname.clone();
                    let workspace_hostname = if workspace_hostname.is_empty() {
                        window().window().location().hostname().unwrap_or_default()
                    } else {
                        workspace_hostname
                    };
                    view! {
                        <div>
                            {move || {
                                let status = status.get();
                                let status_class = workspace_status_class(&status);
                                let status_class = format!(
                                    "{status_class} text-sm font-medium me-2 px-2.5 py-0.5 rounded",
                                );
                                view! {
                                    <div class="mt-2">
                                        <span class=status_class>{move || status.to_string()}</span>
                                    </div>
                                }
                            }}
                            <a
                                href=info.repo_url.clone()
                                target="_blank"
                                class="inline-flex border rounded-lg mt-8"
                            >
                                <img
                                    src=repo_img(&info.repo_url)
                                    class="object-contain object-left h-40"
                                />
                            </a> <span class="flex flex-row items-center text-sm mt-4">
                                <a
                                    href=info.repo_url.clone()
                                    target="_blank"
                                    class="inline-flex items-center"
                                >
                                    <svg
                                        class="w-4 h-4 mr-1"
                                        aria-hidden="true"
                                        xmlns="http://www.w3.org/2000/svg"
                                        fill="none"
                                        viewBox="0 0 24 24"
                                    >
                                        <path
                                            stroke="currentColor"
                                            stroke-linecap="round"
                                            stroke-linejoin="round"
                                            stroke-width="2"
                                            d="M13.2 9.8a3.4 3.4 0 0 0-4.8 0L5 13.2A3.4 3.4 0 0 0 9.8 18l.3-.3m-.3-4.5a3.4 3.4 0 0 0 4.8 0L18 9.8A3.4 3.4 0 0 0 13.2 5l-1 1"
                                        />
                                    </svg>
                                    <span class="text-gray-500 w-36">{"Repository URL"}</span>
                                    <p>
                                        {
                                            let repo_url = info.repo_url.clone();
                                            move || repo_url.clone()
                                        }
                                    </p>
                                </a>
                            </span>
                            <span class="mt-2 text-sm flex flex-row items-center rounded me-2">
                                <svg
                                    class="w-3 h-3 mr-2"
                                    aria-hidden="true"
                                    xmlns="http://www.w3.org/2000/svg"
                                    fill="none"
                                    viewBox="0 0 14 20"
                                >
                                    <path
                                        stroke="currentColor"
                                        stroke-linecap="round"
                                        stroke-linejoin="round"
                                        stroke-width="2"
                                        d="M3 5v10M3 5a2 2 0 1 0 0-4 2 2 0 0 0 0 4Zm0 10a2 2 0 1 0 0 4 2 2 0 0 0 0-4Zm6-3.976-2-.01A4.015 4.015 0 0 1 3 7m10 4a2 2 0 1 1-4 0 2 2 0 0 1 4 0Z"
                                    />
                                </svg>
                                <span class="text-gray-500 w-36">{"Branch"}</span>
                                <span class="mr-2">{info.branch}</span>
                                <span>{info.commit}</span>
                            </span>
                            <span class="mt-2 text-sm flex flex-row items-center rounded me-2">
                                <svg
                                    class="w-2.5 h-2.5 mr-2"
                                    xmlns="http://www.w3.org/2000/svg"
                                    width="16"
                                    height="16"
                                    fill="currentColor"
                                    viewBox="0 0 16 16"
                                >
                                    <path d="M6 9a.5.5 0 0 1 .5-.5h3a.5.5 0 0 1 0 1h-3A.5.5 0 0 1 6 9M3.854 4.146a.5.5 0 1 0-.708.708L4.793 6.5 3.146 8.146a.5.5 0 1 0 .708.708l2-2a.5.5 0 0 0 0-.708z" />
                                    <path d="M2 1a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V3a2 2 0 0 0-2-2zm12 1a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H2a1 1 0 0 1-1-1V3a1 1 0 0 1 1-1z" />
                                </svg>
                                <span class="text-gray-500 w-36">{"SSH Connection"}</span>
                                {
                                    let workspace_name = workspace_name.clone();
                                    let workspace_hostname = workspace_hostname.clone();
                                    move || {
                                        let ssh_port = cluster_info
                                            .with(|i| {
                                                i.as_ref().map(|i| i.ssh_proxy_port).unwrap_or(2222)
                                            });
                                        format!(
                                            "ssh {workspace_name}@{}{}",
                                            workspace_hostname.split(':').next().unwrap_or(""),
                                            if ssh_port == 22 {
                                                "".to_string()
                                            } else {
                                                format!(" -p {ssh_port}")
                                            },
                                        )
                                    }
                                }
                            </span>
                            <span class="mt-2 text-sm flex flex-row items-center rounded me-2">
                                <svg
                                    class="w-2.5 h-2.5 mr-2"
                                    xmlns="http://www.w3.org/2000/svg"
                                    width="16"
                                    height="16"
                                    fill="currentColor"
                                    viewBox="0 0 16 16"
                                >
                                    <path d="M14 10a1 1 0 0 1 1 1v1a1 1 0 0 1-1 1H2a1 1 0 0 1-1-1v-1a1 1 0 0 1 1-1zM2 9a2 2 0 0 0-2 2v1a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-1a2 2 0 0 0-2-2z" />
                                    <path d="M5 11.5a.5.5 0 1 1-1 0 .5.5 0 0 1 1 0m-2 0a.5.5 0 1 1-1 0 .5.5 0 0 1 1 0M14 3a1 1 0 0 1 1 1v1a1 1 0 0 1-1 1H2a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1zM2 2a2 2 0 0 0-2 2v1a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V4a2 2 0 0 0-2-2z" />
                                    <path d="M5 4.5a.5.5 0 1 1-1 0 .5.5 0 0 1 1 0m-2 0a.5.5 0 1 1-1 0 .5.5 0 0 1 1 0" />
                                </svg>
                                <span class="text-gray-500 w-36">{"Machine Type"}</span>
                                {machine_types
                                    .get()
                                    .and_then(|m| m.into_iter().find(|m| m.id == info.machine_type))
                                    .map(|machine_type| {
                                        format!(
                                            "{} - {} vCPUs, {}GB memory, {}GB disk",
                                            machine_type.name,
                                            machine_type.cpu,
                                            machine_type.memory,
                                            machine_type.disk,
                                        )
                                    })}
                            </span>
                            <span class="mt-2 text-sm flex flex-row items-center rounded me-2">
                                <svg
                                    class="w-2.5 h-2.5 mr-2"
                                    xmlns="http://www.w3.org/2000/svg"
                                    width="24px"
                                    height="24px"
                                    viewBox="0 0 24 24"
                                    fill="none"
                                    stroke="currentColor"
                                    stroke-width="2"
                                    stroke-linecap="round"
                                    stroke-linejoin="round"
                                >
                                    <line x1="12" x2="12" y1="17" y2="22"></line>
                                    <path d="M5 17h14v-1.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V6h1a2 2 0 0 0 0-4H8a2 2 0 0 0 0 4h1v4.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24Z"></path>
                                </svg>
                                <span class="text-gray-500 w-36">{"Pinned"}</span>
                                <span>{info.pinned}</span>
                            </span>
                            <span class="mt-2 text-sm flex flex-row items-center rounded me-2">
                                <svg
                                    class="w-2.5 h-2.5 mr-2"
                                    aria-hidden="true"
                                    xmlns="http://www.w3.org/2000/svg"
                                    fill="currentColor"
                                    viewBox="0 0 20 20"
                                >
                                    <path d="M10 0a10 10 0 1 0 10 10A10.011 10.011 0 0 0 10 0Zm3.982 13.982a1 1 0 0 1-1.414 0l-3.274-3.274A1.012 1.012 0 0 1 9 10V6a1 1 0 0 1 2 0v3.586l2.982 2.982a1 1 0 0 1 0 1.414Z" />
                                </svg>
                                <span class="text-gray-500 w-36">{"Created"}</span>
                                <DatetimeModal time=info.created_at />
                            </span> <div class="mt-4">
                                <WorkspaceControl
                                    workspace_name=workspace_name.clone()
                                    workspace_status=status.get()
                                    workspace_folder=info.repo_name.clone()
                                    workspace_hostname=workspace_hostname.clone()
                                    workspace_pinned=info.pinned
                                    align_right=false
                                    delete_modal_open
                                    rebuild_modal_open
                                    error
                                />
                            </div>
                            {move || {
                                if let Some(error) = error.get() {
                                    view! {
                                        <div class="w-full my-4 p-4 rounded-lg bg-red-50">
                                            <span class="text-sm font-medium text-red-800">
                                                {error}
                                            </span>
                                        </div>
                                    }
                                        .into_any()
                                } else {
                                    ().into_any()
                                }
                            }}
                            <Show when=move || {
                                let status = status.get();
                                status == WorkspaceStatus::New
                                    || status == WorkspaceStatus::Building
                                    || status == WorkspaceStatus::PrebuildBuilding
                                    || status == WorkspaceStatus::PrebuildCopying
                                    || status == WorkspaceStatus::Failed
                            }>
                                <div
                                    id="build-messages-scroll"
                                    class="overflow-y-auto p-4 h-48 w-full mt-8 border rounded"
                                >
                                    <For
                                        each=move || build_messages.get().into_iter().enumerate()
                                        key=|(i, _)| *i
                                        children=move |(_, msg)| {
                                            let is_error = msg.is_error();
                                            view! {
                                                <p class=(
                                                    "text-red-500",
                                                    move || is_error,
                                                )>{msg.msg().to_string()}</p>
                                            }
                                        }
                                    />
                                </div>
                            </Show> <div class="flex flex-col">
                                <For
                                    each=move || info.services.clone()
                                    key=move |s| s.clone()
                                    children={
                                        let workspace_folder = info.repo_name.clone();
                                        let workspace_hostname = workspace_hostname.clone();
                                        move |ws_service| {
                                            view! {
                                                <WorkspaceServiceView
                                                    ws_service
                                                    workspace_hostname=workspace_hostname.clone()
                                                    workspace_folder=workspace_folder.clone()
                                                    status
                                                    cluster_info
                                                />
                                            }
                                        }
                                    }
                                />
                            </div>
                            <WorkspaceTabView
                                name=info.name.clone()
                                workspace_hostname=workspace_hostname.clone()
                                show_build_error=true
                                build_error=info.build_error.clone()
                            />
                            <DeleteModal
                                resource=info.name.clone()
                                open=delete_modal_open
                                delete_action
                            />
                            <WorkspaceRebuildModal
                                workspace_name=workspace_name.clone()
                                rebuild_modal_open
                            />
                        </div>
                    }
                        .into_any()
                } else {
                    view! { <div></div> }.into_any()
                }}
            </Show>
        </section>
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum TabKind {
    Port,
    BuildOutput,
}

#[component]
fn WorkspaceServiceView(
    ws_service: WorkspaceService,
    workspace_hostname: String,
    workspace_folder: String,
    status: RwSignal<WorkspaceStatus, LocalStorage>,
    cluster_info: Signal<Option<ClusterInfo>, LocalStorage>,
) -> impl IntoView {
    let expanded = RwSignal::new_local(false);
    let toggle_expand_view = move |_| {
        expanded.update(|e| {
            *e = !*e;
        });
    };
    view! {
        <div class="border rounded-lg p-8 mt-8">
            <div class="flex flex-row flex-wrap justify-between items-center">
                <div class="flex flex-row items-center">
                    <button class="p-2" on:click=toggle_expand_view>
                        <svg
                            class:hidden=move || expanded.get()
                            class="text-gray-600 w-3 h-3 mr-3"
                            xmlns="http://www.w3.org/2000/svg"
                            fill="none"
                            viewBox="0 0 6 10"
                        >
                            <path
                                stroke="currentColor"
                                stroke-linecap="round"
                                stroke-linejoin="round"
                                stroke-width="2"
                                d="m1 9 4-4-4-4"
                            ></path>
                        </svg>
                        <svg
                            class:hidden=move || !expanded.get()
                            class="text-gray-600 w-3 h-3 mr-3"
                            xmlns="http://www.w3.org/2000/svg"
                            fill="none"
                            viewBox="0 0 10 6"
                        >
                            <path
                                stroke="currentColor"
                                stroke-linecap="round"
                                stroke-linejoin="round"
                                stroke-width="2"
                                d="m1 1 4 4 4-4"
                            />
                        </svg>
                    </button>
                    <div class="flex flex-col text-sm">
                        <div class="flex flex-row items-center">
                            <svg
                                class="w-4 h-4 mr-1"
                                xmlns="http://www.w3.org/2000/svg"
                                fill="currentColor"
                                viewBox="0 0 16 16"
                            >
                                <path
                                    fill-rule="evenodd"
                                    d="M6 3.5A1.5 1.5 0 0 1 7.5 2h1A1.5 1.5 0 0 1 10 3.5v1A1.5 1.5 0 0 1 8.5 6v1H11a.5.5 0 0 1 .5.5v1a.5.5 0 0 1-1 0V8h-5v.5a.5.5 0 0 1-1 0v-1A.5.5 0 0 1 5 7h2.5V6A1.5 1.5 0 0 1 6 4.5zM8.5 5a.5.5 0 0 0 .5-.5v-1a.5.5 0 0 0-.5-.5h-1a.5.5 0 0 0-.5.5v1a.5.5 0 0 0 .5.5zM3 11.5A1.5 1.5 0 0 1 4.5 10h1A1.5 1.5 0 0 1 7 11.5v1A1.5 1.5 0 0 1 5.5 14h-1A1.5 1.5 0 0 1 3 12.5zm1.5-.5a.5.5 0 0 0-.5.5v1a.5.5 0 0 0 .5.5h1a.5.5 0 0 0 .5-.5v-1a.5.5 0 0 0-.5-.5zm4.5.5a1.5 1.5 0 0 1 1.5-1.5h1a1.5 1.5 0 0 1 1.5 1.5v1a1.5 1.5 0 0 1-1.5 1.5h-1A1.5 1.5 0 0 1 9 12.5zm1.5-.5a.5.5 0 0 0-.5.5v1a.5.5 0 0 0 .5.5h1a.5.5 0 0 0 .5-.5v-1a.5.5 0 0 0-.5-.5z"
                                />
                            </svg>
                            <span class="text-gray-500">{"Service"}</span>
                        </div>
                        {ws_service.service.clone()}
                    </div>
                </div>
                <div class="flex flex-col text-sm">
                    <div class="flex flex-row items-center">
                        <svg
                            class="w-2.5 h-2.5 mr-2"
                            xmlns="http://www.w3.org/2000/svg"
                            width="16"
                            height="16"
                            fill="currentColor"
                            viewBox="0 0 16 16"
                        >
                            <path d="M6 9a.5.5 0 0 1 .5-.5h3a.5.5 0 0 1 0 1h-3A.5.5 0 0 1 6 9M3.854 4.146a.5.5 0 1 0-.708.708L4.793 6.5 3.146 8.146a.5.5 0 1 0 .708.708l2-2a.5.5 0 0 0 0-.708z" />
                            <path d="M2 1a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V3a2 2 0 0 0-2-2zm12 1a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H2a1 1 0 0 1-1-1V3a1 1 0 0 1 1-1z" />
                        </svg>
                        <span class="text-gray-500">{"SSH Connection"}</span>
                    </div>
                    <WorkspaceSSHConnView
                        name=ws_service.name.clone()
                        workspace_hostname=workspace_hostname.clone()
                        cluster_info
                    />
                </div>
                <div class="flex flex-row">
                    <OpenWorkspaceView
                        workspace_name=ws_service.name.clone()
                        workspace_status=status.get()
                        workspace_folder=workspace_folder.clone()
                        workspace_hostname=workspace_hostname.clone()
                        align_right=false
                    />
                </div>
            </div>
            <div class="pl-10" class:hidden=move || !expanded.get()>
                <WorkspaceTabView
                    name=ws_service.name.clone()
                    workspace_hostname=workspace_hostname.clone()
                    show_build_error=false
                    build_error=None
                />
            </div>
        </div>
    }
}

#[component]
fn WorkspaceSSHConnView(
    name: String,
    workspace_hostname: String,
    cluster_info: Signal<Option<ClusterInfo>, LocalStorage>,
) -> impl IntoView {
    view! {
        <span>
            {
                let ws_service_name = name.clone();
                let workspace_hostname = workspace_hostname.clone();
                move || {
                    let ssh_proxy_port = cluster_info
                        .with(|i| i.as_ref().map(|i| i.ssh_proxy_port).unwrap_or(2222));
                    format!(
                        "ssh {ws_service_name}@{}{}",
                        workspace_hostname.split(':').next().unwrap_or(""),
                        if ssh_proxy_port == 22 {
                            "".to_string()
                        } else {
                            format!(" -p {ssh_proxy_port}")
                        },
                    )
                }
            }
        </span>
    }
}

#[component]
pub fn WorkspaceTabView(
    name: String,
    workspace_hostname: String,
    show_build_error: bool,
    build_error: Option<RepobuildError>,
) -> impl IntoView {
    let tab_kind = RwSignal::new_local(TabKind::Port);
    let active_class =
        "inline-block p-4 text-blue-600 border-b-2 border-blue-600 rounded-t-lg active";
    let inactive_class = "inline-block p-4 border-b-2 border-transparent rounded-t-lg hover:text-gray-600 hover:border-gray-300";
    let change_tab = move |kind: TabKind| {
        tab_kind.set(kind);
    };

    view! {
        <div class="mt-8 text-sm font-medium text-center text-gray-500 border-b border-gray-200">
            <ul class="flex flex-wrap -mb-px">
                <li class="me-2">
                    <a
                        href="#"
                        class=move || {
                            if tab_kind.get() == TabKind::Port {
                                active_class
                            } else {
                                inactive_class
                            }
                        }
                        on:click=move |_| change_tab(TabKind::Port)
                    >
                        Ports
                    </a>
                </li>
                <li class="me-2" class:hidden=move || !show_build_error>
                    <a
                        href="#"
                        class=move || {
                            if tab_kind.get() == TabKind::BuildOutput {
                                active_class
                            } else {
                                inactive_class
                            }
                        }
                        on:click=move |_| change_tab(TabKind::BuildOutput)
                    >
                        Build Output
                    </a>
                </li>
            </ul>
        </div>
        <div class="text-gray-600" class=("min-h-32", move || show_build_error)>
            <div class:hidden=move || tab_kind.get() != TabKind::Port>
                <WorkspacePortsView name=name workspace_hostname=workspace_hostname.clone() />
            </div>
            <div class:hidden=move || {
                tab_kind.get() != TabKind::BuildOutput
            }>
                {if let Some(e) = build_error {
                    view! {
                        <p class="mt-4 text-sm text-gray-900">{e.msg}</p>
                        <p class="text-sm text-gray-500">
                            So the workspace was created with a default image.
                        </p>
                        <div class="overflow-y-auto p-4 h-48 w-full mt-4 border rounded">
                            <For
                                each=move || e.stderr.clone()
                                key=|l| l.clone()
                                children=move |line| {
                                    view! { <p class="text-sm text-red-600">{line}</p> }
                                }
                            />
                        </div>
                    }
                        .into_any()
                } else {
                    view! {
                        <p class="my-4 text-sm text-gray-900">
                            Workspace image was built successfully
                        </p>
                    }
                        .into_any()
                }}
            </div>
        </div>
    }
}

async fn workspace_ports(
    org: Signal<Option<Organization>>,
    name: &str,
) -> Result<Vec<WorkspacePort>> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let resp = Request::get(&format!(
        "/api/v1/organizations/{}/workspaces/{name}/ports",
        org.id
    ))
    .send()
    .await?;
    let ports: Vec<WorkspacePort> = resp.json().await?;
    Ok(ports)
}

async fn update_workspace_port(
    org: Signal<Option<Organization>>,
    name: &str,
    port: u16,
    shared: bool,
    public: bool,
) -> Result<(), ErrorResponse> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let resp = Request::put(&format!(
        "/api/v1/organizations/{}/workspaces/{name}/ports/{port}",
        org.id
    ))
    .json(&UpdateWorkspacePort { shared, public })?
    .send()
    .await?;
    if resp.status() != 204 {
        let error = resp
            .json::<ErrorResponse>()
            .await
            .unwrap_or_else(|_| ErrorResponse {
                error: "Internal Server Error".to_string(),
            });
        return Err(error);
    }
    Ok(())
}

#[component]
pub fn WorkspacePortsView(name: String, workspace_hostname: String) -> impl IntoView {
    let org = get_current_org();
    let ports = {
        let name = name.clone();
        LocalResource::new(move || {
            let name = name.clone();
            async move { workspace_ports(org, &name.clone()).await }
        })
    };
    let ports = Signal::derive(move || {
        ports
            .with(|p| p.as_ref().map(|p| p.as_ref().ok().cloned()))
            .flatten()
            .unwrap_or_default()
    });

    let success = RwSignal::new_local(None);
    let error = RwSignal::new_local(None);

    view! {
        <p class="mt-2 py-2 text-sm text-gray-900">Exposed ports of your workspace</p>
        <div class="text-sm text-gray-500">
            <p>Only you can access the ports by default.</p>
            <p>If you make the port shared, all members in the same organisation can access it.</p>
            <p>If you make the port public, everyone who has the url can access it.</p>
        </div>
        {move || {
            if let Some(success) = success.get() {
                view! {
                    <Alert variant=AlertVariant::Default class="mt-4">
                        <lucide_leptos::CircleCheck />
                        <AlertTitle> {success} </AlertTitle>
                    </Alert>
                }
                    .into_any()
            } else {
                ().into_any()
            }
        }}

        {move || {
            if let Some(error) = error.get() {
                view! {
                    <Alert variant=AlertVariant::Destructive class="mt-4">
                        <lucide_leptos::CircleAlert />
                        <AlertTitle>{error}</AlertTitle>
                    </Alert>
                }
                    .into_any()
            } else {
                ().into_any()
            }
        }}

        <div class="mt-4 flex items-center w-full px-4 py-2 text-gray-900 bg-gray-50">
            <span class="w-1/3 truncate pr-2">Port</span>
            <div class="w-1/3 truncate flex flex-row items-center">
                <span class="w-1/2 truncate text-center">shared</span>
                <span class="w-1/2 truncate text-center">public</span>
            </div>
            <span class="w-1/3 truncate"></span>
        </div>

        <For
            each=move || ports.get()
            key=|p| p.clone()
            children=move |p| {
                view! {
                    <WorkspacePortView
                        name=name.clone()
                        p=p
                        workspace_hostname=workspace_hostname.clone()
                        success
                        error
                    />
                }
            }
        />
    }
}

#[component]
fn WorkspacePortView(
    name: String,
    p: WorkspacePort,
    workspace_hostname: String,
    success: RwSignal<Option<String>, LocalStorage>,
    error: RwSignal<Option<String>, LocalStorage>,
) -> impl IntoView {
    let shared = RwSignal::new_local(p.shared);
    let public = RwSignal::new_local(p.public);
    let org = get_current_org();
    let action = {
        let name = name.clone();
        let port = p.port;
        Action::new_local(move |_| {
            let name = name.clone();
            async move {
                error.set(None);
                success.set(None);
                if let Err(e) = update_workspace_port(
                    org,
                    &name.clone(),
                    port,
                    shared.get_untracked(),
                    public.get_untracked(),
                )
                .await
                {
                    error.set(Some(e.error.clone()));
                } else {
                    success.set(Some(format!("Port {port} updated successfully")));
                }
            }
        })
    };

    view! {
        <div class="flex flex-row items-center w-full px-4 py-2 border-b">
            <span class="w-1/3 truncate pr-2">
                {if let Some(label) = p.label.as_ref() {
                    format!("{label} ({})", p.port)
                } else {
                    p.port.to_string()
                }}
            </span>
            <div class="w-1/3 truncate flex flex-row items-center">
                <div class="w-1/2 flex items-center justify-center">
                    <input
                        type="checkbox"
                        value=""
                        class="w-4 h-4 border border-gray-300 rounded bg-gray-50  focus:ring-0"
                        prop:checked=p.shared
                        on:change=move |e| shared.set(event_target_checked(&e))
                    />
                </div>
                <div class="w-1/2 flex items-center justify-center">
                    <input
                        type="checkbox"
                        value=""
                        class="w-4 h-4 border border-gray-300 rounded bg-gray-50 focus:ring-0"
                        prop:checked=p.public
                        on:change=move |e| public.set(event_target_checked(&e))
                    />
                </div>
            </div>
            <div class="w-1/3 flex flex-row items-center gap-4">
                <a href=format!("https://{}-{name}.{workspace_hostname}/", p.port) target="_blank">
                    <Button>Open</Button>
                </a>
                <Button
                    variant=ButtonVariant::Outline
                    on:click=move |_| {
                        action.dispatch(());
                    }
                >
                    Update
                </Button>
            </div>
        </div>
    }
}
