use std::{collections::HashMap, time::Duration};

use anyhow::{anyhow, Result};
use futures::{SinkExt, StreamExt};
use gloo_net::{
    http::Request,
    websocket::{futures::WebSocket, Message},
};
use lapdev_common::{
    console::Organization, utils, ClusterInfo, GitBranch, NewWorkspace, NewWorkspaceResponse,
    PrebuildStatus, ProjectInfo, ProjectPrebuild, RepoSource, RepobuildError, UpdateWorkspacePort,
    WorkspaceInfo, WorkspacePort, WorkspaceService, WorkspaceStatus, WorkspaceUpdateEvent,
};
use leptos::{
    component, create_action, create_effect, create_local_resource, create_rw_signal, document,
    event_target_checked, event_target_value, expect_context, on_cleanup, set_timeout, spawn_local,
    spawn_local_with_current_owner, use_context, view, window, For, IntoView, RwSignal, Show,
    Signal, SignalGet, SignalGetUntracked, SignalSet, SignalUpdate, SignalWith,
    SignalWithUntracked,
};
use leptos_router::{use_location, use_navigate, use_params_map};
use uuid::Uuid;
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use web_sys::FocusEvent;

use crate::{
    modal::{CreationModal, DatetimeModal, DeletionModal, ErrorResponse},
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

async fn all_workspaces() -> Result<Vec<WorkspaceInfo>> {
    let org =
        use_context::<Signal<Option<Organization>>>().ok_or_else(|| anyhow!("can't get org"))?;
    let org = org
        .get_untracked()
        .ok_or_else(|| anyhow!("can't get org"))?;
    let resp = Request::get(&format!("/api/v1/organizations/{}/workspaces", org.id))
        .send()
        .await?;
    let workspaces: Vec<WorkspaceInfo> = resp.json().await?;
    Ok(workspaces)
}

async fn get_workspace(name: &str) -> Result<WorkspaceInfo> {
    let org =
        use_context::<Signal<Option<Organization>>>().ok_or_else(|| anyhow!("can't get org"))?;
    let org = org
        .get_untracked()
        .ok_or_else(|| anyhow!("can't get org"))?;
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
    name: String,
    delete_modal_hidden: RwSignal<bool>,
    update_counter: Option<RwSignal<i32>>,
) -> Result<(), ErrorResponse> {
    let org =
        use_context::<Signal<Option<Organization>>>().ok_or_else(|| anyhow!("can't get org"))?;
    let org = org
        .get_untracked()
        .ok_or_else(|| anyhow!("can't get org"))?;
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

    delete_modal_hidden.set(true);
    if let Some(update_counter) = update_counter {
        update_counter.update(|c| *c += 1);
    } else {
        use_navigate()("/workspaces", Default::default());
    }

    Ok(())
}

async fn rebuild_workspace(
    name: String,
    rebuild_modal_hidden: RwSignal<bool>,
) -> Result<(), ErrorResponse> {
    let org =
        use_context::<Signal<Option<Organization>>>().ok_or_else(|| anyhow!("can't get org"))?;
    let org = org
        .get_untracked()
        .ok_or_else(|| anyhow!("can't get org"))?;
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

    rebuild_modal_hidden.set(true);

    Ok(())
}

async fn stop_workspace(name: String) -> Result<(), ErrorResponse> {
    let org =
        use_context::<Signal<Option<Organization>>>().ok_or_else(|| anyhow!("can't get org"))?;
    let org = org
        .get_untracked()
        .ok_or_else(|| anyhow!("can't get org"))?;
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

async fn start_workspace(name: String) -> Result<(), ErrorResponse> {
    let org =
        use_context::<Signal<Option<Organization>>>().ok_or_else(|| anyhow!("can't get org"))?;
    let org = org
        .get_untracked()
        .ok_or_else(|| anyhow!("can't get org"))?;
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

async fn pin_workspace(name: String) -> Result<(), ErrorResponse> {
    let org =
        use_context::<Signal<Option<Organization>>>().ok_or_else(|| anyhow!("can't get org"))?;
    let org = org
        .get_untracked()
        .ok_or_else(|| anyhow!("can't get org"))?;
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

async fn unpin_workspace(name: String) -> Result<(), ErrorResponse> {
    let org =
        use_context::<Signal<Option<Organization>>>().ok_or_else(|| anyhow!("can't get org"))?;
    let org = org
        .get_untracked()
        .ok_or_else(|| anyhow!("can't get org"))?;
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
    let dropdown_hidden = create_rw_signal(true);
    let toggle_dropdown = move |_| {
        if dropdown_hidden.get_untracked() {
            dropdown_hidden.set(false);
        } else {
            dropdown_hidden.set(true);
        }
    };

    let on_focusout = move |e: FocusEvent| {
        let node = e
            .current_target()
            .unwrap_throw()
            .unchecked_into::<web_sys::HtmlElement>();

        set_timeout(
            move || {
                let has_focus = if let Some(active) = document().active_element() {
                    let active: web_sys::Node = active.into();
                    node.contains(Some(&active))
                } else {
                    false
                };
                if !has_focus && !dropdown_hidden.get_untracked() {
                    dropdown_hidden.set(true);
                }
            },
            Duration::from_secs(0),
        );
    };

    let open_button_class = if workspace_status == WorkspaceStatus::Running {
        "bg-green-700 hover:bg-green-800"
    } else {
        "bg-green-200"
    };
    let open_button_class = format!("{open_button_class} py-2 text-sm font-medium text-white focus:ring-4 focus:ring-green-300 focus:outline-none");

    let workspace_hostname = workspace_hostname.clone();
    let cluster_info = expect_context::<Signal<Option<ClusterInfo>>>();
    let port =
        cluster_info.with_untracked(|i| i.as_ref().map(|i| i.ssh_proxy_port).unwrap_or(2222));
    view! {
        <div
            class="pr-6 relative"
            on:focusout=on_focusout
        >
            <a
                class={format!("{open_button_class} px-4 rounded-l-lg inline-block")}
                disabled=move || workspace_status != WorkspaceStatus::Running
                target="_blank"
                href={format!("https://{workspace_name}.{workspace_hostname}/")}
            >
            Open
            </a>
            <button
                class={format!("{open_button_class} h-full absolute right-0 px-2 rounded-r-lg border-l-1")}
                disabled=move || workspace_status != WorkspaceStatus::Running
                on:click=toggle_dropdown
            >
                <svg class="w-2.5 h-2.5" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 10 6">
                    <path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="m1 1 4 4 4-4"/>
                </svg>
            </button>
            <div
                class="absolute pt-2 z-10 divide-y divide-gray-100"
                class:hidden=move || dropdown_hidden.get()
                class=("right-0", move || align_right)
            >
                <ul class="py-2 text-sm text-gray-700 bg-white rounded-lg border shadow w-64">
                    <li>
                        <a
                            href={
                                format!(
                                        "vscode://vscode-remote/ssh-remote+{workspace_name}@{}{}/workspaces/{workspace_folder}",
                                        workspace_hostname.split(':').next().unwrap_or(""),
                                        if port == 22 {
                                            "".to_string()
                                        } else {
                                            format!(":{port}")
                                        }
                                    )
                            }
                            class="block px-4 py-2 hover:bg-gray-100"
                            target="_blank"
                        >
                            Open in VSCode Desktop
                        </a>
                    </li>
                </ul>
            </div>
        </div>
    }
}

#[component]
fn WorkspaceControl(
    workspace_name: String,
    workspace_status: WorkspaceStatus,
    workspace_folder: String,
    workspace_hostname: String,
    workspace_pinned: bool,
    rebuild_modal_hidden: RwSignal<bool>,
    delete_modal_hidden: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    align_right: bool,
) -> impl IntoView {
    let dropdown_hidden = create_rw_signal(true);
    let toggle_dropdown = move |_| {
        if dropdown_hidden.get_untracked() {
            dropdown_hidden.set(false);
        } else {
            dropdown_hidden.set(true);
        }
    };

    let on_focusout = move |e: FocusEvent| {
        let node = e
            .current_target()
            .unwrap_throw()
            .unchecked_into::<web_sys::HtmlElement>();

        set_timeout(
            move || {
                let has_focus = if let Some(active) = document().active_element() {
                    let active: web_sys::Node = active.into();
                    node.contains(Some(&active))
                } else {
                    false
                };
                if !has_focus && !dropdown_hidden.get_untracked() {
                    dropdown_hidden.set(true);
                }
            },
            Duration::from_secs(0),
        );
    };

    let delete_workspace = {
        move |_| {
            dropdown_hidden.set(true);
            delete_modal_hidden.set(false);
        }
    };

    let rebuild_workspace = {
        move |_| {
            dropdown_hidden.set(true);
            rebuild_modal_hidden.set(false);
        }
    };

    let stop_action = {
        let workspace_name = workspace_name.clone();
        create_action(move |_| {
            let workspace_name = workspace_name.clone();
            async move {
                if let Err(e) = stop_workspace(workspace_name.clone()).await {
                    error.set(Some(e.error));
                }
            }
        })
    };
    let stop_workspace = move |_| {
        dropdown_hidden.set(true);
        error.set(None);
        stop_action.dispatch(());
    };

    let start_action = {
        let workspace_name = workspace_name.clone();
        create_action(move |_| {
            let workspace_name = workspace_name.clone();
            async move {
                if let Err(e) = start_workspace(workspace_name.clone()).await {
                    error.set(Some(e.error));
                }
            }
        })
    };
    let start_workspace = move |_| {
        dropdown_hidden.set(true);
        error.set(None);
        start_action.dispatch(());
    };

    let pin_action = {
        let workspace_name = workspace_name.clone();
        create_action(move |_| {
            let workspace_name = workspace_name.clone();
            async move {
                if let Err(e) = pin_workspace(workspace_name.clone()).await {
                    error.set(Some(e.error));
                }
            }
        })
    };
    let pin_workspace = move |_| {
        dropdown_hidden.set(true);
        error.set(None);
        pin_action.dispatch(());
    };

    let unpin_action = {
        let workspace_name = workspace_name.clone();
        create_action(move |_| {
            let workspace_name = workspace_name.clone();
            async move {
                if let Err(e) = unpin_workspace(workspace_name.clone()).await {
                    error.set(Some(e.error));
                }
            }
        })
    };
    let unpin_workspace = move |_| {
        dropdown_hidden.set(true);
        error.set(None);
        unpin_action.dispatch(());
    };

    view! {
        <div class="flex flex-row items-center">
            <OpenWorkspaceView workspace_name workspace_status workspace_folder workspace_hostname align_right />
            <div
                class="ml-2 relative"
                on:focusout=on_focusout
            >
                <button
                    class="hover:bg-gray-100 focus:outline-none font-medium rounded-lg text-sm px-2.5 py-2.5 text-center inline-flex items-center"
                    type="button"
                    on:click=toggle_dropdown
                >
                    <svg class="w-4 h-4 text-white" viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg">
                        <path d="M9.5 13a1.5 1.5 0 1 1-3 0 1.5 1.5 0 0 1 3 0zm0-5a1.5 1.5 0 1 1-3 0 1.5 1.5 0 0 1 3 0zm0-5a1.5 1.5 0 1 1-3 0 1.5 1.5 0 0 1 3 0z"/>
                    </svg>
                </button>
                <div
                    class="absolute pt-2 z-10 divide-y divide-gray-100"
                    class:hidden=move || dropdown_hidden.get()
                    class=("right-0", move || align_right)
                >
                    <ul class="py-2 text-sm text-gray-700 bg-white rounded-lg border shadow w-44">
                    <li>
                        <a
                            href="#"
                            class="block px-4 py-2 hover:bg-gray-100"
                            on:click=stop_workspace
                            class:hidden=move || workspace_status == WorkspaceStatus::Stopped
                        >
                            Stop
                        </a>
                    </li>
                    <li>
                        <a
                            href="#"
                            class="block px-4 py-2 hover:bg-gray-100"
                            on:click=start_workspace
                            class:hidden=move || workspace_status != WorkspaceStatus::Stopped
                        >
                            Start
                        </a>
                    </li>
                    <li>
                        <a
                            href="#"
                            class="block px-4 py-2 hover:bg-gray-100"
                            on:click=pin_workspace
                            class:hidden=move || workspace_pinned
                        >
                            Pin
                        </a>
                    </li>
                    <li>
                        <a
                            href="#"
                            class="block px-4 py-2 hover:bg-gray-100"
                            on:click=unpin_workspace
                            class:hidden=move || !workspace_pinned
                        >
                            Unpin
                        </a>
                    </li>
                    <li>
                        <a
                            href="#"
                            class="block px-4 py-2 hover:bg-gray-100"
                            on:click=rebuild_workspace
                        >
                            Rebuild
                        </a>
                    </li>
                    <li>
                        <a
                            href="#"
                            class="block px-4 py-2 hover:bg-gray-100 text-red-700"
                            on:click=delete_workspace
                        >
                            Delete
                        </a>
                    </li>
                    </ul>
                </div>
            </div>
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
    rebuild_modal_hidden: RwSignal<bool>,
) -> impl IntoView {
    let rebuild_action = {
        let workspace_name = workspace_name.clone();
        create_action(move |_| rebuild_workspace(workspace_name.clone(), rebuild_modal_hidden))
    };

    let body = view! {
        <p>Rebuild your workspace image.</p>
        <p>It will pick up your devcontainer config changes.</p>
        <p>
            All files in
            <span class="text-semibold mx-1 px-1 text-gray-800 bg-gray-200 rounded">/workspaces</span>
            folder will persist.
        </p>
    };

    view! {
        <CreationModal
            title="Rebuild Workspace Image".to_string()
            modal_hidden=rebuild_modal_hidden
            action=rebuild_action
            body
            update_text=Some("Rebuild".to_string())
            updating_text=None
            create_button_hidden=Box::new(|| false)
            width_class=None
        />
    }
}

#[component]
pub fn WorkspaceItem(
    workspace: WorkspaceInfo,
    error: RwSignal<Option<String>>,
    update_counter: RwSignal<i32>,
) -> impl IntoView {
    let rebuild_modal_hidden = create_rw_signal(true);
    let delete_modal_hidden = create_rw_signal(true);
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
        create_action(move |_| {
            delete_workspace(
                workspace_name.clone(),
                delete_modal_hidden,
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
                    <a href={ workspace.repo_url.clone() } target="_blank">
                        <img
                            src={ repo_img(&workspace.repo_url) }
                            class="object-contain lg:object-left h-40 rounded-lg justify-self-start"
                        />
                    </a>
                </div>
                <div class="lg:w-1/3 flex flex-col justify-center">
                    <div class="flex flex-col p-4">
                        <a href={ format!("/workspaces/{}", workspace.name) }>
                            <span class="font-semibold" href={ format!("/workspaces/{}", workspace.name) }>{ move || workspace.name.clone() }</span>
                            <span href={ workspace.repo_url.clone() } target="_blank" class="flex flex-row items-center text-sm text-gray-700 mt-2">
                                <svg class="w-4 h-4 mr-1" aria-hidden="true" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
                                    <path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13.2 9.8a3.4 3.4 0 0 0-4.8 0L5 13.2A3.4 3.4 0 0 0 9.8 18l.3-.3m-.3-4.5a3.4 3.4 0 0 0 4.8 0L18 9.8A3.4 3.4 0 0 0 13.2 5l-1 1"/>
                                </svg>
                                <p>{ move || workspace.repo_url.clone() }</p>
                            </span>
                            <span class="flex flex-row truncate items-center text-sm text-gray-700 mt-2">
                                <svg class="w-3 h-3 mr-2" aria-hidden="true" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 14 20">
                                    <path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 5v10M3 5a2 2 0 1 0 0-4 2 2 0 0 0 0 4Zm0 10a2 2 0 1 0 0 4 2 2 0 0 0 0-4Zm6-3.976-2-.01A4.015 4.015 0 0 1 3 7m10 4a2 2 0 1 1-4 0 2 2 0 0 1 4 0Z"/>
                                </svg>
                                <span class="mr-2">{workspace.branch}</span>
                                <span>{workspace.commit[..7].to_string()}</span>
                            </span>
                            <span class="mt-2 text-sm text-gray-800 inline-flex items-center rounded me-2 text-gray-700">
                                <svg class="w-2.5 h-2.5 mr-2.5" aria-hidden="true" xmlns="http://www.w3.org/2000/svg" fill="currentColor" viewBox="0 0 20 20">
                                <path d="M10 0a10 10 0 1 0 10 10A10.011 10.011 0 0 0 10 0Zm3.982 13.982a1 1 0 0 1-1.414 0l-3.274-3.274A1.012 1.012 0 0 1 9 10V6a1 1 0 0 1 2 0v3.586l2.982 2.982a1 1 0 0 1 0 1.414Z"/>
                                </svg>
                                <span class="mr-1">Created</span> <DatetimeModal time=workspace.created_at />
                            </span>
                        </a>
                    </div>
                </div>
                <div class="lg:w-1/3 flex flex-col xl:flex-row items-center justify-between">
                    <div class="flex flex-row items-center">
                        <span
                            class=status_class
                        >{ move || workspace.status.to_string() }</span>
                    </div>
                    <div class="flex flex-row items-center p-4 justify-center">
                        <div class="w-4 h-4 mr-6">
                            <svg
                                class:hidden = move || !workspace.pinned
                                class="w-4 h-4" xmlns="http://www.w3.org/2000/svg" width="24px" height="24px" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="12" x2="12" y1="17" y2="22"></line><path d="M5 17h14v-1.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V6h1a2 2 0 0 0 0-4H8a2 2 0 0 0 0 4h1v4.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24Z"
                            ></path></svg>
                        </div>
                        <WorkspaceControl
                            workspace_name={workspace_name.clone()}
                            workspace_status={workspace.status}
                            workspace_folder=workspace_folder.clone()
                            workspace_hostname=workspace_hostname.clone()
                            workspace_pinned=workspace.pinned
                            align_right=true
                            delete_modal_hidden rebuild_modal_hidden error
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
                            <div class="flex flex-col border-t items-center shadow-sm md:flex-row w-full">
                                <div class="lg:w-1/3 flex flex-row">
                                </div>
                                <div class="md:w-1/3 flex flex-col justify-center">
                                    <div class="flex flex-col p-4">
                                        <span class="font-semibold">{ws_service.name.clone()}</span>
                                        <div class="flex flex-row items-center text-sm text-gray-700 mt-2">
                                            <svg class="w-4 h-4 mr-1" xmlns="http://www.w3.org/2000/svg" fill="currentColor" viewBox="0 0 16 16">
                                                <path fill-rule="evenodd" d="M6 3.5A1.5 1.5 0 0 1 7.5 2h1A1.5 1.5 0 0 1 10 3.5v1A1.5 1.5 0 0 1 8.5 6v1H11a.5.5 0 0 1 .5.5v1a.5.5 0 0 1-1 0V8h-5v.5a.5.5 0 0 1-1 0v-1A.5.5 0 0 1 5 7h2.5V6A1.5 1.5 0 0 1 6 4.5zM8.5 5a.5.5 0 0 0 .5-.5v-1a.5.5 0 0 0-.5-.5h-1a.5.5 0 0 0-.5.5v1a.5.5 0 0 0 .5.5zM3 11.5A1.5 1.5 0 0 1 4.5 10h1A1.5 1.5 0 0 1 7 11.5v1A1.5 1.5 0 0 1 5.5 14h-1A1.5 1.5 0 0 1 3 12.5zm1.5-.5a.5.5 0 0 0-.5.5v1a.5.5 0 0 0 .5.5h1a.5.5 0 0 0 .5-.5v-1a.5.5 0 0 0-.5-.5zm4.5.5a1.5 1.5 0 0 1 1.5-1.5h1a1.5 1.5 0 0 1 1.5 1.5v1a1.5 1.5 0 0 1-1.5 1.5h-1A1.5 1.5 0 0 1 9 12.5zm1.5-.5a.5.5 0 0 0-.5.5v1a.5.5 0 0 0 .5.5h1a.5.5 0 0 0 .5-.5v-1a.5.5 0 0 0-.5-.5z"/>
                                            </svg>
                                            <p>{ws_service.service.clone()}</p>
                                        </div>
                                    </div>
                                </div>
                                <div class="md:w-1/6 flex flex-row items-center p-4 justify-center">
                                </div>
                                <div class="md:w-1/6 flex flex-row items-center p-4 justify-center">
                                    <div class="mr-10">
                                        <OpenWorkspaceView workspace_name=ws_service.name.clone() workspace_status={workspace.status} workspace_folder=workspace_folder.clone() workspace_hostname=workspace_hostname.clone() align_right=true />
                                    </div>
                                </div>
                            </div>
                        }
                    }
                }
            />
        </div>
        <DeletionModal resource=workspace_name.clone() modal_hidden=delete_modal_hidden delete_action />
        <WorkspaceRebuildModal
            workspace_name=workspace_name.clone()
            rebuild_modal_hidden
        />
    }
}

async fn watch_all_workspace_status(
    status_update: RwSignal<(String, WorkspaceStatus)>,
) -> Result<()> {
    let location = window().location();
    let protocol = if location.protocol().map(|p| p == "http:").unwrap_or(false) {
        "ws"
    } else {
        "wss"
    };
    let host = location.host().unwrap();
    let websocket = WebSocket::open(&format!("{protocol}://{host}/all_workspaces_ws"))?;

    let (mut tx, mut rx) = websocket.split();
    on_cleanup(move || {
        spawn_local(async move {
            let _ = tx.close().await;
        });
    });

    while let Some(Ok(msg)) = rx.next().await {
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
    let error = create_rw_signal(None);
    let status_update = create_rw_signal(("".to_string(), WorkspaceStatus::New));

    let _ = spawn_local_with_current_owner(async move {
        let _ = watch_all_workspace_status(status_update).await;
    });

    let new_workspace_modal_hidden = create_rw_signal(true);

    let delete_counter = create_rw_signal(0);
    create_effect(move |_| {
        status_update.track();
        delete_counter.update(|c| {
            *c += 1;
        });
    });

    let workspaces_resource = create_local_resource(
        move || delete_counter.get(),
        |_| async move { all_workspaces().await.unwrap_or_default() },
    );

    let workspace_filter = create_rw_signal(String::new());

    let workspaces = Signal::derive(move || {
        let mut workspaces = workspaces_resource.get().unwrap_or_default();
        let workspace_filter = workspace_filter.get();
        workspaces.retain(|w| w.name.contains(&workspace_filter));
        workspaces
    });

    let new_workspace = move |_| {
        new_workspace_modal_hidden.set(false);
    };

    {
        let hash = use_location().hash.get_untracked();
        if let Some(url) = hash.strip_prefix("#") {
            let url = utils::format_repo_url(url);
            let action_dispatched = create_rw_signal(false);
            let current_org = expect_context::<Signal<Option<Organization>>>();
            let cluster_info = expect_context::<Signal<Option<ClusterInfo>>>();
            let action = create_action(move |url: &String| {
                create_workspace(RepoSource::Url(url.clone()), None, None, true)
            });
            create_effect(move |_| {
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
            create_effect(move |_| {
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
                        <h5 class="mr-3 text-2xl font-semibold">
                            All Workspaces
                        </h5>
                        <p class="text-gray-700">Manage all your existing workspaces or add a new one</p>
                    </div>
                </div>
                <div class="flex flex-row items-center justify-between py-4 space-x-4">
                    <div class="w-1/2">
                        <form class="flex items-center">
                            <label for="simple-search" class="sr-only">Filter Workspaces</label>
                            <div class="relative w-full">
                            <div class="absolute inset-y-0 left-0 flex items-center pl-3 pointer-events-none">
                                <svg aria-hidden="true" class="w-5 h-5 text-gray-500" fill="currentColor" viewbox="0 0 20 20" xmlns="http://www.w3.org/2000/svg">
                                <path fill-rule="evenodd" d="M8 4a4 4 0 100 8 4 4 0 000-8zM2 8a6 6 0 1110.89 3.476l4.817 4.817a1 1 0 01-1.414 1.414l-4.816-4.816A6 6 0 012 8z" clip-rule="evenodd" />
                                </svg>
                            </div>
                            <input
                                prop:value={move || workspace_filter.get()}
                                on:input=move |ev| { workspace_filter.set(event_target_value(&ev)); }
                                type="text"
                                class="bg-white block w-full p-2 pl-10 text-sm text-gray-900 border border-gray-300 rounded-lg bg-gray-50 focus:ring-blue-500 focus:border-blue-500"
                                placeholder="Filter Workspaces"
                                required=""
                            />
                            </div>
                        </form>
                    </div>
                    <button
                        type="button"
                        class="flex items-center justify-center px-4 py-2 text-sm font-medium text-white rounded-lg bg-blue-700 hover:bg-blue-800 focus:ring-4 focus:ring-blue-300 focus:outline-none"
                        on:click=new_workspace
                    >
                        New Workspace
                    </button>
                </div>
            </div>

            { move || if let Some(error) = error.get() {
                view! {
                    <div class="w-full my-4 p-4 rounded-lg bg-red-50">
                        <span class="text-sm font-medium text-red-800">{ error }</span>
                    </div>
                }.into_view()
            } else {
                view!{}.into_view()
            }}

            <div class="relative w-full basis-0 grow">
                <div class="absolute w-full h-full flex flex-col py-4 overflow-y-auto">
                    <For
                        each=move || workspaces.get()
                        key=|w|  (w.name.clone(), w.status, w.pinned)
                        children=move |workspace| {
                            view! {
                                <WorkspaceItem workspace=workspace update_counter=delete_counter error />
                            }
                        }
                    />
                </div>
            </div>

            <NewWorkspaceModal modal_hidden=new_workspace_modal_hidden project_info=create_rw_signal(None) />

        </section>

    }
}

pub async fn create_workspace(
    source: RepoSource,
    branch: Option<String>,
    machine_type: Option<Uuid>,
    from_hash: bool,
) -> Result<(), ErrorResponse> {
    let current_org =
        use_context::<Signal<Option<Organization>>>().ok_or_else(|| anyhow!("can't get org"))?;
    let org = current_org
        .get_untracked()
        .ok_or_else(|| anyhow!("can't get org"))?;

    let machine_type_id = machine_type.unwrap_or_else(|| {
        let cluster_info = expect_context::<Signal<Option<ClusterInfo>>>();
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
    use_navigate()(&format!("/workspaces/{}", resp.name), Default::default());

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
    modal_hidden: RwSignal<bool>,
    project_info: RwSignal<Option<CreateWorkspaceProjectInfo>>,
) -> impl IntoView {
    let repo_url = create_rw_signal("".to_string());
    let current_branch = create_rw_signal(None);
    let current_machine_type = create_rw_signal(None);

    create_effect(move |_| {
        let branch = project_info.with(|info| {
            info.as_ref()
                .and_then(|info| info.branch.as_ref().or(info.branches.first()))
                .cloned()
        });
        current_branch.set(branch);
    });

    let preferred_machine_type = Signal::derive(move || {
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

    let action = create_action(move |_| {
        let source = if let Some(project) = current_project.get_untracked() {
            RepoSource::Project(project)
        } else {
            RepoSource::Url(repo_url.get_untracked())
        };
        create_workspace(
            source,
            current_branch.get_untracked().map(|b| b.name),
            current_machine_type.get_untracked(),
            false,
        )
    });

    let no_project_view = move || {
        view! {
            <div>
                <label class="block mb-2 text-sm font-medium text-gray-900">Your repository url</label>
                <input
                    prop:value={move || repo_url.get()}
                    on:input=move |ev| { repo_url.set(event_target_value(&ev)); }
                    class="bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5"
                    placeholder="https://github.com/owner/repo"
                    required
                />
            </div>
            <MachineTypeView current_machine_type preferred_machine_type />
        }
    };

    let create_info_view = view! {
        <Show
            when=move || { project_info.with(|i| i.is_some()) }
            fallback=no_project_view
        >
            {
                if let Some(project_info) = project_info.get() {
                    let branches = project_info.branches.clone();
                    view! {
                        <div>
                            <label class="block mb-2 text-sm font-medium text-gray-900">
                                Project Name
                            </label>
                            <span class="bg-gray-50 text-gray-900 text-sm rounded-lg block w-full p-2.5">
                                { project_info.project.name }
                            </span>
                        </div>
                        <div>
                            <label class="block mb-2 text-sm font-medium text-gray-900">
                                Project Repository
                            </label>
                            <span class="bg-gray-50 text-gray-900 text-sm rounded-lg block w-full p-2.5">
                                { project_info.project.repo_url }
                            </span>
                        </div>
                        <div>
                            <label class="block mb-2 text-sm font-medium text-gray-900">
                                Branch
                            </label>
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
                                            <option
                                                selected=move || Some(&branch_name) == current_branch.get().as_ref().map(|b| &b.name)
                                            >{branch.name}</option>
                                        }
                                    }
                                />
                            </select>
                        </div>
                        <div>
                            <label class="block mb-2 text-sm font-medium text-gray-900">
                                Prebuild Status
                            </label>
                            <span class="bg-gray-50 text-gray-900 text-sm rounded-lg block w-full p-2.5">
                                {
                                    move || if let Some(branch) = current_branch.get() {
                                        project_info.prebuilds.get(&branch.name).and_then(|prebuild| {
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
                                    }.unwrap_or_else(|| "Not Created".to_string())
                                }
                            </span>
                        </div>
                        <MachineTypeView current_machine_type preferred_machine_type />
                    }.into_view()
                } else {
                    view! {
                    }.into_view()
                }
            }
        </Show>
    };

    view! {
        <CreationModal title="Create New Workspace".to_string() modal_hidden action body=create_info_view update_text=Some("Create".to_string()) updating_text=Some("Creating".to_string()) width_class=None create_button_hidden=Box::new(|| false) />
    }
}

async fn watch_workspace_updates(
    name: &str,
    status: RwSignal<WorkspaceStatus>,
    build_messages: RwSignal<Vec<BuildMessage>>,
) -> Result<()> {
    let location = window().location();
    let host = location.host().map_err(|_| anyhow!("can't get host"))?;
    let protocol = if location.protocol().map(|p| p == "http:").unwrap_or(false) {
        "ws"
    } else {
        "wss"
    };
    let websocket = WebSocket::open(&format!("{protocol}://{host}/ws?name={}", name))?;

    let (mut tx, mut rx) = websocket.split();
    on_cleanup(move || {
        println!("watch workspace updates cleanup");
        spawn_local(async move {
            let _ = tx.close().await;
        });
    });

    while let Some(Ok(msg)) = rx.next().await {
        if let Message::Text(s) = msg {
            if let Ok(event) = serde_json::from_str::<WorkspaceUpdateEvent>(&s) {
                match event {
                    WorkspaceUpdateEvent::Status(s) => {
                        status.set(s);
                        if s == WorkspaceStatus::Deleted {
                            if let Ok(pathname) = window().location().pathname() {
                                if pathname.ends_with(name) {
                                    use_navigate()("/workspaces", Default::default());
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
    let error = create_rw_signal(None);
    let params = use_params_map();
    let name = params.with_untracked(|params| params.get("name").cloned().unwrap());
    let local_name = name.clone();
    let workspace_name = name.clone();
    let rebuild_modal_hidden = create_rw_signal(true);
    let delete_modal_hidden = create_rw_signal(true);
    let delete_action = {
        let workspace_name = workspace_name.clone();
        create_action(move |_| delete_workspace(workspace_name.clone(), delete_modal_hidden, None))
    };
    let cluster_info = expect_context::<Signal<Option<ClusterInfo>>>();
    let machine_types = create_rw_signal(cluster_info.get_untracked().map(|i| i.machine_types));

    let update_counter = create_rw_signal(0);
    let workspace_info = create_local_resource(
        move || update_counter.get(),
        move |_| async move {
            let name = params.with_untracked(|params| params.get("name").cloned().unwrap());
            get_workspace(&name).await
        },
    );

    let build_messages = create_rw_signal(Vec::new());
    let status = create_rw_signal(WorkspaceStatus::New);
    {
        let name = local_name.clone();
        let _ = spawn_local_with_current_owner(async move {
            let _ = watch_workspace_updates(&name, status, build_messages).await;
        });
    }
    create_effect(move |_| {
        if let Some(info) = workspace_info.with(|info| {
            info.as_ref()
                .map(|info| info.as_ref().ok().cloned())
                .clone()
                .flatten()
        }) {
            if status.get_untracked() != info.status {
                status.set(info.status);
            }
        }
    });
    create_effect(move |_| {
        status.track();
        update_counter.update(|c| *c += 1);
    });
    view! {
        <section class="w-full flex flex-col">
            <a href="/workspaces" class="text-sm inline-flex items-center text-gray-700">
                <svg class="w-2 h-2 mr-2" aria-hidden="true" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 14 10">
                <path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 5H1m0 0 4 4M1 5l4-4"/>
                </svg>
                Back to Workspace List
            </a>
            <h5 class="text-semibold text-2xl mt-4">
                { local_name.clone() }
            </h5>
            <Show
                when=move || { workspace_info.with(|info| info.as_ref().map(|info| info.is_ok()).unwrap_or(false)) }
                fallback=move || view! {}
            >
                {
                    if let Some(info) = workspace_info.with(|info|  info.as_ref().map(|info| info.as_ref().ok().cloned()).clone().flatten()) {
                        let workspace_hostname = info.hostname.clone();
                        let workspace_hostname = if workspace_hostname.is_empty() {
                            window().window().location().hostname().unwrap_or_default()
                        } else {
                            workspace_hostname
                        };
                        view! {
                            <div>
                                { move || {
                                    let status = status.get();
                                    let status_class = workspace_status_class(&status);
                                    let status_class = format!("{status_class} text-sm font-medium me-2 px-2.5 py-0.5 rounded");
                                    view! {
                                        <div class="mt-2">
                                        <span class=status_class>{ move || status.to_string() }
                                        </span>
                                        </div>
                                    }
                                }
                                }
                                <a href={ info.repo_url.clone() } target="_blank" class="inline-flex border rounded-lg mt-8">
                                    <img src={ repo_img(&info.repo_url) } class="object-contain object-left h-40" />
                                </a>
                                <span class="flex flex-row items-center text-sm mt-4">
                                    <a href={ info.repo_url.clone() } target="_blank" class="inline-flex items-center">
                                        <svg class="w-4 h-4 mr-1" aria-hidden="true" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
                                            <path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13.2 9.8a3.4 3.4 0 0 0-4.8 0L5 13.2A3.4 3.4 0 0 0 9.8 18l.3-.3m-.3-4.5a3.4 3.4 0 0 0 4.8 0L18 9.8A3.4 3.4 0 0 0 13.2 5l-1 1"/>
                                        </svg>
                                        <span class="mr-1 text-gray-500">{"Repository URL:"}</span>
                                        <p>{ move || info.repo_url.clone() }</p>
                                    </a>
                                </span>
                                <span class="mt-2 text-sm flex flex-row items-center rounded me-2">
                                    <svg class="w-3 h-3 mr-2" aria-hidden="true" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 14 20">
                                        <path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 5v10M3 5a2 2 0 1 0 0-4 2 2 0 0 0 0 4Zm0 10a2 2 0 1 0 0 4 2 2 0 0 0 0-4Zm6-3.976-2-.01A4.015 4.015 0 0 1 3 7m10 4a2 2 0 1 1-4 0 2 2 0 0 1 4 0Z"/>
                                    </svg>
                                    <span class="mr-1 text-gray-500">{"Branch:"}</span>
                                    <span class="mr-2">{ info.branch }</span>
                                    <span>{ info.commit }</span>
                                </span>
                                <span class="mt-2 text-sm flex flex-row items-center rounded me-2">
                                    <svg class="w-2.5 h-2.5 mr-2" xmlns="http://www.w3.org/2000/svg" width="16" height="16" fill="currentColor" viewBox="0 0 16 16">
                                        <path d="M6 9a.5.5 0 0 1 .5-.5h3a.5.5 0 0 1 0 1h-3A.5.5 0 0 1 6 9M3.854 4.146a.5.5 0 1 0-.708.708L4.793 6.5 3.146 8.146a.5.5 0 1 0 .708.708l2-2a.5.5 0 0 0 0-.708z"/>
                                        <path d="M2 1a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V3a2 2 0 0 0-2-2zm12 1a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H2a1 1 0 0 1-1-1V3a1 1 0 0 1 1-1z"/>
                                    </svg>
                                    <span class="mr-1 text-gray-500">{"SSH Connection:"}</span>
                                    {
                                        let workspace_name = workspace_name.clone();
                                        let workspace_hostname = workspace_hostname.clone();
                                        move || {
                                            let ssh_port = cluster_info.with(|i| i.as_ref().map(|i| i.ssh_proxy_port).unwrap_or(2222));
                                            format!("ssh {workspace_name}@{}{}",
                                                workspace_hostname.split(':').next().unwrap_or(""),
                                                if ssh_port == 22 {
                                                    "".to_string()
                                                } else {
                                                    format!(" -p {ssh_port}")
                                                }
                                            )
                                        }
                                    }
                                </span>
                                <span class="mt-2 text-sm flex flex-row items-center rounded me-2">
                                    <svg class="w-2.5 h-2.5 mr-2" xmlns="http://www.w3.org/2000/svg" width="16" height="16" fill="currentColor" viewBox="0 0 16 16">
                                        <path d="M14 10a1 1 0 0 1 1 1v1a1 1 0 0 1-1 1H2a1 1 0 0 1-1-1v-1a1 1 0 0 1 1-1zM2 9a2 2 0 0 0-2 2v1a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-1a2 2 0 0 0-2-2z"/>
                                        <path d="M5 11.5a.5.5 0 1 1-1 0 .5.5 0 0 1 1 0m-2 0a.5.5 0 1 1-1 0 .5.5 0 0 1 1 0M14 3a1 1 0 0 1 1 1v1a1 1 0 0 1-1 1H2a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1zM2 2a2 2 0 0 0-2 2v1a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V4a2 2 0 0 0-2-2z"/>
                                        <path d="M5 4.5a.5.5 0 1 1-1 0 .5.5 0 0 1 1 0m-2 0a.5.5 0 1 1-1 0 .5.5 0 0 1 1 0"/>
                                    </svg>
                                    <span class="mr-1 text-gray-500">{"Machine Type:"}</span>
                                    {
                                        machine_types.get()
                                            .and_then(|m|
                                                m.into_iter()
                                                 .find(|m| m.id == info.machine_type)
                                            )
                                            .map(|machine_type| format!("{} - {} vCPUs, {}GB memory, {}GB disk",machine_type.name, machine_type.cpu, machine_type.memory, machine_type.disk))
                                    }
                                </span>
                                <span class="mt-2 text-sm flex flex-row items-center rounded me-2">
                                    <svg class="w-2.5 h-2.5 mr-2" xmlns="http://www.w3.org/2000/svg" width="24px" height="24px" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="12" x2="12" y1="17" y2="22"></line><path d="M5 17h14v-1.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V6h1a2 2 0 0 0 0-4H8a2 2 0 0 0 0 4h1v4.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24Z"></path></svg>
                                    <span class="mr-1 text-gray-500">{"Pinned:"}</span>
                                    <span>{ info.pinned }</span>
                                </span>
                                <span class="mt-2 text-sm flex flex-row items-center rounded me-2">
                                    <svg class="w-2.5 h-2.5 mr-2" aria-hidden="true" xmlns="http://www.w3.org/2000/svg" fill="currentColor" viewBox="0 0 20 20">
                                    <path d="M10 0a10 10 0 1 0 10 10A10.011 10.011 0 0 0 10 0Zm3.982 13.982a1 1 0 0 1-1.414 0l-3.274-3.274A1.012 1.012 0 0 1 9 10V6a1 1 0 0 1 2 0v3.586l2.982 2.982a1 1 0 0 1 0 1.414Z"/>
                                    </svg>
                                    <span class="mr-1 text-gray-500">{"Created:"}</span>
                                    <DatetimeModal time=info.created_at />
                                </span>
                                <div class="mt-4">
                                    <WorkspaceControl
                                        workspace_name=workspace_name.clone()
                                        workspace_status=status.get()
                                        workspace_folder=info.repo_name.clone()
                                        workspace_hostname=workspace_hostname.clone()
                                        workspace_pinned=info.pinned
                                        align_right=false
                                        delete_modal_hidden
                                        rebuild_modal_hidden
                                        error />
                                </div>
                                { move || if let Some(error) = error.get() {
                                    view! {
                                        <div class="w-full my-4 p-4 rounded-lg bg-red-50">
                                            <span class="text-sm font-medium text-red-800">{ error }</span>
                                        </div>
                                    }.into_view()
                                } else {
                                    view!{}.into_view()
                                }}

                                <Show
                                    when=move || { let status = status.get(); status == WorkspaceStatus::New || status == WorkspaceStatus::Building || status == WorkspaceStatus::PrebuildBuilding || status == WorkspaceStatus::PrebuildCopying || status == WorkspaceStatus::Failed}
                                >
                                    <div id="build-messages-scroll" class="overflow-y-auto p-4 h-48 w-full mt-8 border rounded">
                                        <For
                                            each=move || build_messages.get().into_iter().enumerate()
                                            key=|(i, _)| *i
                                            children=move |(_, msg)| {
                                                let is_error = msg.is_error();
                                                view! {
                                                    <p
                                                        class=("text-red-500", move || is_error)
                                                    >{msg.msg().to_string()}</p>
                                                }
                                            }
                                        />
                                    </div>
                                </Show>

                                <div class="flex flex-col">
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

                                <DeletionModal resource=info.name.clone() modal_hidden=delete_modal_hidden delete_action />
                                <WorkspaceRebuildModal
                                    workspace_name=workspace_name.clone()
                                    rebuild_modal_hidden
                                />
                            </div>
                        }
                    } else {
                        view! {
                            <div>
                            </div>
                        }
                    }
                }
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
    status: RwSignal<WorkspaceStatus>,
    cluster_info: Signal<Option<ClusterInfo>>,
) -> impl IntoView {
    let expanded = create_rw_signal(false);
    let toggle_expand_view = move |_| {
        expanded.update(|e| {
            *e = !*e;
        });
    };
    view! {
        <div class="border rounded-lg p-8 mt-8">
            <div
                class="flex flex-row flex-wrap justify-between items-center"
            >
                <div class="flex flex-row items-center">
                    <button
                        class="p-2"
                        on:click=toggle_expand_view
                    >
                        <svg
                            class:hidden=move || expanded.get()
                            class="text-gray-600 w-3 h-3 mr-3" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 6 10"
                        >
                            <path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="m1 9 4-4-4-4"></path>
                        </svg>
                        <svg
                            class:hidden=move || !expanded.get()
                            class="text-gray-600 w-3 h-3 mr-3" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 10 6"
                        >
                            <path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="m1 1 4 4 4-4"/>
                        </svg>
                    </button>
                    <div class="flex flex-col text-sm">
                        <div class="flex flex-row items-center">
                            <svg class="w-4 h-4 mr-1" xmlns="http://www.w3.org/2000/svg" fill="currentColor" viewBox="0 0 16 16">
                                <path fill-rule="evenodd" d="M6 3.5A1.5 1.5 0 0 1 7.5 2h1A1.5 1.5 0 0 1 10 3.5v1A1.5 1.5 0 0 1 8.5 6v1H11a.5.5 0 0 1 .5.5v1a.5.5 0 0 1-1 0V8h-5v.5a.5.5 0 0 1-1 0v-1A.5.5 0 0 1 5 7h2.5V6A1.5 1.5 0 0 1 6 4.5zM8.5 5a.5.5 0 0 0 .5-.5v-1a.5.5 0 0 0-.5-.5h-1a.5.5 0 0 0-.5.5v1a.5.5 0 0 0 .5.5zM3 11.5A1.5 1.5 0 0 1 4.5 10h1A1.5 1.5 0 0 1 7 11.5v1A1.5 1.5 0 0 1 5.5 14h-1A1.5 1.5 0 0 1 3 12.5zm1.5-.5a.5.5 0 0 0-.5.5v1a.5.5 0 0 0 .5.5h1a.5.5 0 0 0 .5-.5v-1a.5.5 0 0 0-.5-.5zm4.5.5a1.5 1.5 0 0 1 1.5-1.5h1a1.5 1.5 0 0 1 1.5 1.5v1a1.5 1.5 0 0 1-1.5 1.5h-1A1.5 1.5 0 0 1 9 12.5zm1.5-.5a.5.5 0 0 0-.5.5v1a.5.5 0 0 0 .5.5h1a.5.5 0 0 0 .5-.5v-1a.5.5 0 0 0-.5-.5z"/>
                            </svg>
                            <span class="text-gray-500">{"Service"}</span>
                        </div>
                        { ws_service.service.clone() }
                    </div>
                </div>
                <div class="flex flex-col text-sm">
                    <div class="flex flex-row items-center">
                        <svg class="w-2.5 h-2.5 mr-2" xmlns="http://www.w3.org/2000/svg" width="16" height="16" fill="currentColor" viewBox="0 0 16 16">
                            <path d="M6 9a.5.5 0 0 1 .5-.5h3a.5.5 0 0 1 0 1h-3A.5.5 0 0 1 6 9M3.854 4.146a.5.5 0 1 0-.708.708L4.793 6.5 3.146 8.146a.5.5 0 1 0 .708.708l2-2a.5.5 0 0 0 0-.708z"/>
                            <path d="M2 1a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V3a2 2 0 0 0-2-2zm12 1a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H2a1 1 0 0 1-1-1V3a1 1 0 0 1 1-1z"/>
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
                    <OpenWorkspaceView workspace_name=ws_service.name.clone() workspace_status={status.get()} workspace_folder=workspace_folder.clone() workspace_hostname=workspace_hostname.clone() align_right=false />
                </div>
            </div>
            <div
                class="pl-10"
                class:hidden=move || !expanded.get()
            >
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
    cluster_info: Signal<Option<ClusterInfo>>,
) -> impl IntoView {
    view! {
        <span>
        {
            let ws_service_name = name.clone();
            let workspace_hostname = workspace_hostname.clone();
            move || {
                let ssh_proxy_port = cluster_info.with(|i| i.as_ref().map(|i| i.ssh_proxy_port).unwrap_or(2222));
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
    let tab_kind = create_rw_signal(TabKind::Port);
    let active_class =
        "inline-block p-4 text-blue-600 border-b-2 border-blue-600 rounded-t-lg active";
    let inactive_class = "inline-block p-4 border-b-2 border-transparent rounded-t-lg hover:text-gray-600 hover:border-gray-300";
    let change_tab = move |kind: TabKind| {
        tab_kind.set(kind);
    };

    view! {
        <div
            class="mt-8 text-sm font-medium text-center text-gray-500 border-b border-gray-200"
        >
            <ul class="flex flex-wrap -mb-px">
                <li class="me-2">
                    <a
                        href="#"
                        class={ move || if tab_kind.get() == TabKind::Port {active_class} else {inactive_class} }
                        on:click=move |_| change_tab(TabKind::Port)
                    >Ports</a>
                </li>
                <li class="me-2"
                    class:hidden=move || !show_build_error
                >
                    <a
                        href="#"
                        class={ move || if tab_kind.get() == TabKind::BuildOutput {active_class} else {inactive_class} }
                        on:click=move |_| change_tab(TabKind::BuildOutput)
                    >Build Output</a>
                </li>
            </ul>
        </div>
        <div
            class="text-gray-600"
            class=("min-h-32", move || show_build_error)
        >
            <div class:hidden=move || tab_kind.get() != TabKind::Port>
                <WorkspacePortsView name=name workspace_hostname=workspace_hostname.clone() />
            </div>
            <div class:hidden=move || tab_kind.get() != TabKind::BuildOutput>
                {
                    if let Some(e) = build_error {
                        view! {
                            <p class="mt-4 text-sm text-gray-900">{e.msg}</p>
                            <p class="text-sm text-gray-500">So the workspace was created with a default image.</p>
                            <div class="overflow-y-auto p-4 h-48 w-full mt-4 border rounded">
                                <For
                                    each=move || e.stderr.clone()
                                    key=|l| l.clone()
                                    children=move |line| {
                                        view! {
                                            <p class="text-sm text-red-600">{line}</p>
                                        }
                                    }
                                />
                            </div>
                        }.into_view()
                    } else {
                        view! {
                            <p class="my-4 text-sm text-gray-900">Workspace image was built successfully</p>
                        }.into_view()
                    }
                }
            </div>
        </div>
    }
}

async fn workspace_ports(name: &str) -> Result<Vec<WorkspacePort>> {
    let org =
        use_context::<Signal<Option<Organization>>>().ok_or_else(|| anyhow!("can't get org"))?;
    let org = org
        .get_untracked()
        .ok_or_else(|| anyhow!("can't get org"))?;
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
    name: &str,
    port: u16,
    shared: bool,
    public: bool,
) -> Result<(), ErrorResponse> {
    let org =
        use_context::<Signal<Option<Organization>>>().ok_or_else(|| anyhow!("can't get org"))?;
    let org = org
        .get_untracked()
        .ok_or_else(|| anyhow!("can't get org"))?;
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
    let ports = {
        let name = name.clone();
        create_local_resource(
            || (),
            move |_| {
                let name = name.clone();
                async move { workspace_ports(&name.clone()).await }
            },
        )
    };
    let ports = Signal::derive(move || {
        ports
            .with(|p| p.as_ref().map(|p| p.as_ref().ok().cloned()))
            .flatten()
            .unwrap_or_default()
    });

    let success = create_rw_signal(None);
    let error = create_rw_signal(None);

    view! {
        <p class="mt-2 py-2 text-sm text-gray-900">Exposed ports of your workspace</p>
        <div class="text-sm text-gray-500">
            <p>Only you can access the ports by default.</p>
            <p>If you make the port shared, all members in the same organisation can access it.</p>
            <p>If you make the port public, everyone who has the url can access it.</p>
        </div>
        { move || if let Some(success) = success.get() {
            view! {
                <div class="mt-4 p-4 rounded-lg bg-green-50 w-full">
                    <span class="text-sm font-medium text-green-800">{ success }</span>
                </div>
            }.into_view()
        } else {
            view!{}.into_view()
        }}

        { move || if let Some(error) = error.get() {
            view! {
                <div class="mt-4 p-4 rounded-lg bg-red-50 w-full">
                    <span class="text-sm font-medium text-red-800">{ error }</span>
                </div>
            }.into_view()
        } else {
            view!{}.into_view()
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
                    <WorkspacePortView name=name.clone() p=p workspace_hostname=workspace_hostname.clone() success error />
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
    success: RwSignal<Option<String>>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let shared = create_rw_signal(p.shared);
    let public = create_rw_signal(p.public);
    let action = {
        let name = name.clone();
        let port = p.port;
        create_action(move |_| {
            let name = name.clone();
            async move {
                error.set(None);
                success.set(None);
                if let Err(e) = update_workspace_port(
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
            <span class="w-1/3 truncate pr-2">{
                if let Some(label) = p.label.as_ref() {
                    format!("{label} ({})", p.port)
                } else {
                    p.port.to_string()
                }
            }</span>
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
            <div class="w-1/3 flex flex-row items-center">
                <a
                    href={ format!("https://{}-{name}.{workspace_hostname}/", p.port) }
                    target="_blank"
                    class="block px-4 py-2 text-sm font-medium text-white rounded-lg bg-green-700 hover:bg-green-800 focus:ring-4 focus:ring-green-300 focus:outline-none"
                >
                    Open
                </a>
                <button
                    class="ml-4 flex items-center justify-center px-4 py-2 text-sm font-medium text-white rounded-lg bg-blue-700 hover:bg-blue-800 focus:ring-4 focus:ring-blue-300 focus:outline-none"
                    on:click=move |_| action.dispatch(())
                >
                    Update
                </button>
            </div>
        </div>
    }
}
