use std::{collections::HashMap, time::Duration};

use anyhow::Result;
use gloo_net::http::Request;
use lapdev_common::{
    AuthProvider, ClusterInfo, ClusterUser, ClusterUserResult, CreateMachineType, MachineType,
    NewWorkspaceHost, OauthSettings, UpdateClusterUser, UpdateMachineType, UpdateWorkspaceHost,
    WorkspaceHost,
};
use leptos::prelude::*;
use uuid::Uuid;
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use web_sys::FocusEvent;

use crate::modal::{
    CreationInput, CreationModal, DatetimeModal, DeletionModal, ErrorResponse, SettingView,
};

async fn get_all_workspaces() -> Result<Vec<WorkspaceHost>> {
    let resp = Request::get("/api/v1/admin/workspace_hosts").send().await?;
    let hosts: Vec<WorkspaceHost> = resp.json().await?;
    Ok(hosts)
}

#[component]
pub fn WorkspaceHostView() -> impl IntoView {
    let new_workspace_host_modal_hidden = RwSignal::new(true);
    let update_counter = RwSignal::new(0);
    let hosts = LocalResource::new(move || async move { get_all_workspaces().await });
    Effect::new(move |_| {
        update_counter.track();
        hosts.refetch();
    });
    let host_filter = RwSignal::new(String::new());
    let hosts = Signal::derive(move || {
        let host_filter = host_filter.get();
        let mut hosts = hosts.with(|hosts| {
            hosts
                .as_ref()
                .and_then(|hosts| hosts.as_ref().ok())
                .cloned()
                .unwrap_or_default()
        });
        hosts.retain(|h| h.host.contains(&host_filter));
        hosts
    });
    view! {
        <div class="pb-4">
            <h5 class="mr-3 text-2xl font-semibold">
                Workspace Hosts
            </h5>
            <p class="text-gray-700">{"Manage the workspace hosts for the cluster"}</p>
            <div class="flex flex-col items-center justify-between py-4 gap-y-3 md:flex-row md:space-y-0 md:space-x-4">
                <div class="w-full md:w-1/2">
                    <form class="flex items-center">
                        <label for="simple-search" class="sr-only">Filter Workspace Hosts</label>
                        <div class="relative w-full">
                        <div class="absolute inset-y-0 left-0 flex items-center pl-3 pointer-events-none">
                            <svg aria-hidden="true" class="w-5 h-5 text-gray-500" fill="currentColor" viewbox="0 0 20 20" xmlns="http://www.w3.org/2000/svg">
                            <path fill-rule="evenodd" d="M8 4a4 4 0 100 8 4 4 0 000-8zM2 8a6 6 0 1110.89 3.476l4.817 4.817a1 1 0 01-1.414 1.414l-4.816-4.816A6 6 0 012 8z" clip-rule="evenodd" />
                            </svg>
                        </div>
                        <input
                            prop:value={move || host_filter.get()}
                            on:input=move |ev| { host_filter.set(event_target_value(&ev)); }
                            type="text"
                            class="bg-white block w-full p-2 pl-10 text-sm text-gray-900 border border-gray-300 rounded-lg bg-gray-50 focus:ring-blue-500 focus:border-blue-500"
                            placeholder="Filter Workspace Hosts"
                        />
                        </div>
                    </form>
                </div>
                <button
                    type="button"
                    class="px-4 py-2 text-sm font-medium text-white rounded-lg bg-blue-700 hover:bg-blue-800 focus:ring-4 focus:ring-blue-300"
                    on:click=move |_| new_workspace_host_modal_hidden.set(false)
                >
                    New Workspace Host
                </button>
            </div>
        </div>

        <div class="flex flex-col gap-y-8">
            <For
                each=move || hosts.get()
                key=|h| (h.id, h.region.clone(), h.zone.clone(), h.status.clone(), h.cpu, h.memory, h.disk)
                children=move |host| {
                    view! {
                        <WorkspaceHostItem host update_counter />
                    }
                }
            />
        </div>
        <NewWorkspaceHostView new_workspace_host_modal_hidden update_counter />
    }
}

#[component]
pub fn NewWorkspaceHostView(
    new_workspace_host_modal_hidden: RwSignal<bool>,
    update_counter: RwSignal<i32>,
) -> impl IntoView {
    let cluster_info = expect_context::<Signal<Option<ClusterInfo>>>();
    let host = RwSignal::new(String::new());
    let region = RwSignal::new(String::new());
    let zone = RwSignal::new(String::new());
    let cpu = RwSignal::new(String::new());
    let memory = RwSignal::new(String::new());
    let disk = RwSignal::new(String::new());
    let action = Action::new_local(move |_| {
        create_workspace_host(
            host.get_untracked(),
            region.get_untracked(),
            zone.get_untracked(),
            cpu.get_untracked(),
            memory.get_untracked(),
            disk.get_untracked(),
            update_counter,
            new_workspace_host_modal_hidden,
        )
    });
    let body = view! {
        <CreationInput label="The host".to_string() value=host placeholder="IP or hostname".to_string() />
        <div
            class:hidden=move || !cluster_info.with(|i| i.as_ref().map(|i| i.has_enterprise)).unwrap_or(false)
        >
            <CreationInput label="Region of the workspace host".to_string() value=region placeholder="".to_string() />
        </div>
        <div
            class:hidden=move || !cluster_info.with(|i| i.as_ref().map(|i| i.has_enterprise)).unwrap_or(false)
        >
            <CreationInput label="Zone of the workspace host".to_string() value=zone placeholder="".to_string() />
        </div>
        <CreationInput label="CPU Cores".to_string() value=cpu placeholder="number of CPU cores".to_string() />
        <CreationInput label="Memory (GB)".to_string() value=memory placeholder="memory in GB".to_string() />
        <CreationInput label="Disk (GB)".to_string() value=disk placeholder="disk in GB".to_string() />
    };
    view! {
        <CreationModal title="Create New Workspace Host".to_string() modal_hidden=new_workspace_host_modal_hidden action body update_text=Some("Create".to_string()) updating_text=Some("Creating".to_string()) width_class=None create_button_hidden=Box::new(|| false) />
    }
}

#[component]
pub fn UpdateWorkspaceHostModal(
    host: WorkspaceHost,
    update_workspace_host_modal_hidden: RwSignal<bool>,
    update_counter: RwSignal<i32>,
) -> impl IntoView {
    let cluster_info = expect_context::<Signal<Option<ClusterInfo>>>();
    let region = RwSignal::new(host.region.clone());
    let zone = RwSignal::new(host.zone.clone());
    let cpu = RwSignal::new(host.cpu.to_string());
    let memory = RwSignal::new(host.memory.to_string());
    let disk = RwSignal::new(host.disk.to_string());
    let action = Action::new_local(move |_| {
        update_workspace_host(
            host.id,
            region.get_untracked(),
            zone.get_untracked(),
            cpu.get_untracked(),
            memory.get_untracked(),
            disk.get_untracked(),
            update_counter,
            update_workspace_host_modal_hidden,
        )
    });
    let body = view! {
        <div
            class:hidden=move || !cluster_info.with(|i| i.as_ref().map(|i| i.has_enterprise)).unwrap_or(false)
        >
            <CreationInput label="Region of the workspace host".to_string() value=region placeholder="".to_string() />
        </div>
        <div
            class:hidden=move || !cluster_info.with(|i| i.as_ref().map(|i| i.has_enterprise)).unwrap_or(false)
        >
            <CreationInput label="Zone of the workspace host".to_string() value=zone placeholder="".to_string() />
        </div>
        <CreationInput label="CPU Cores".to_string() value=cpu placeholder="number of CPU cores".to_string() />
        <CreationInput label="Memory (GB)".to_string() value=memory placeholder="memory in GB".to_string() />
        <CreationInput label="Disk (GB)".to_string() value=disk placeholder="disk in GB".to_string() />
    };
    view! {
        <CreationModal title="Update Workspace Host".to_string() modal_hidden=update_workspace_host_modal_hidden action body update_text=None updating_text=None  width_class=None create_button_hidden=Box::new(|| false) />
    }
}

#[allow(clippy::too_many_arguments)]
async fn create_workspace_host(
    host: String,
    region: String,
    zone: String,
    cpu: String,
    memory: String,
    disk: String,
    update_counter: RwSignal<i32>,
    new_workspace_host_modal_hidden: RwSignal<bool>,
) -> Result<(), ErrorResponse> {
    let cpu: usize = match cpu.parse() {
        Ok(n) => n,
        Err(_) => {
            return Err(ErrorResponse {
                error: "cpu is invalid".to_string(),
            })
        }
    };
    let memory: usize = match memory.parse() {
        Ok(n) => n,
        Err(_) => {
            return Err(ErrorResponse {
                error: "memory is invalid".to_string(),
            })
        }
    };
    let disk: usize = match disk.parse() {
        Ok(n) => n,
        Err(_) => {
            return Err(ErrorResponse {
                error: "disk is invalid".to_string(),
            })
        }
    };
    let resp = Request::post("/api/v1/admin/workspace_hosts")
        .json(&NewWorkspaceHost {
            host,
            region: region.trim().to_string(),
            zone: zone.trim().to_string(),
            cpu,
            memory,
            disk,
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
    update_counter.update(|c| *c += 1);
    new_workspace_host_modal_hidden.set(true);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn update_workspace_host(
    id: Uuid,
    region: String,
    zone: String,
    cpu: String,
    memory: String,
    disk: String,
    update_counter: RwSignal<i32>,
    update_workspace_host_modal_hidden: RwSignal<bool>,
) -> Result<(), ErrorResponse> {
    let cpu: usize = match cpu.parse() {
        Ok(n) => n,
        Err(_) => {
            return Err(ErrorResponse {
                error: "cpu is invalid".to_string(),
            })
        }
    };
    let memory: usize = match memory.parse() {
        Ok(n) => n,
        Err(_) => {
            return Err(ErrorResponse {
                error: "memory is invalid".to_string(),
            })
        }
    };
    let disk: usize = match disk.parse() {
        Ok(n) => n,
        Err(_) => {
            return Err(ErrorResponse {
                error: "disk is invalid".to_string(),
            })
        }
    };

    let resp = Request::put(&format!("/api/v1/admin/workspace_hosts/{id}"))
        .json(&UpdateWorkspaceHost {
            region: region.trim().to_string(),
            zone: zone.trim().to_string(),
            cpu,
            memory,
            disk,
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
    update_counter.update(|c| *c += 1);
    update_workspace_host_modal_hidden.set(true);
    Ok(())
}

async fn delete_workspace_host(
    id: Uuid,
    update_counter: RwSignal<i32>,
    delete_modal_hidden: RwSignal<bool>,
) -> Result<(), ErrorResponse> {
    let resp = Request::delete(&format!("/api/v1/admin/workspace_hosts/{id}"))
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
    update_counter.update(|c| *c += 1);
    delete_modal_hidden.set(true);
    Ok(())
}

#[component]
fn WorkspaceHostItem(host: WorkspaceHost, update_counter: RwSignal<i32>) -> impl IntoView {
    let cluster_info = expect_context::<Signal<Option<ClusterInfo>>>();
    let update_workspace_host_modal_hidden = RwSignal::new(true);
    let delete_modal_hidden = RwSignal::new(true);
    let delete_action = {
        let id = host.id;
        Action::new_local(move |_| async move {
            delete_workspace_host(id, update_counter, delete_modal_hidden).await
        })
    };
    view! {
        <div class="border rounded-xl p-4 flex flex-row items-center">
            <div class="w-2/3 flex flex-row items-center">
                <div
                    class="w-1/3 flex flex-col"
                    class:hidden=move || !cluster_info.with(|i| i.as_ref().map(|i| i.has_enterprise)).unwrap_or(false)
                >
                    <p><span class="text-gray-500 mr-1">{"Region:"}</span>{host.region.clone()}</p>
                    <p><span class="text-gray-500 mr-1">{"Zone:"}</span>{host.zone.clone()}</p>
                </div>
                <div class="w-1/3">
                    <p><span class="text-gray-500 mr-1">{"Host:"}</span>{host.host.clone()}</p>
                </div>
                <div class="w-1/3">
                    <p>{host.status.to_string()}</p>
                </div>
            </div>
            <div class="w-1/3 flex flex-row items-center">
                <div class="w-5/6 flex flex-col">
                    <p><span class="text-gray-500 mr-1">{"CPU Cores:"}</span>{format!("{}/{}", host.cpu - host.available_dedicated_cpu, host.cpu)}</p>
                    <p><span class="text-gray-500 mr-1">{"Memory:"}</span>{format!("{}GB/{}GB", host.memory - host.available_memory, host.memory)}</p>
                    <p><span class="text-gray-500 mr-1">{"Disk:"}</span>{format!("{}GB/{}GB", host.disk - host.available_disk, host.disk)}</p>
                </div>
                <div class="w-1/6">
                    <WorkspaceHostControl delete_modal_hidden update_workspace_host_modal_hidden align_right=true />
                </div>
            </div>
        </div>
        <DeletionModal resource=host.host.clone() modal_hidden=delete_modal_hidden delete_action />
        <UpdateWorkspaceHostModal host=host.clone() update_workspace_host_modal_hidden update_counter />
    }
}

#[component]
fn WorkspaceHostControl(
    delete_modal_hidden: RwSignal<bool>,
    update_workspace_host_modal_hidden: RwSignal<bool>,
    align_right: bool,
) -> impl IntoView {
    let dropdown_hidden = RwSignal::new(true);

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

    let delete_workspace_host = {
        move |_| {
            dropdown_hidden.set(true);
            delete_modal_hidden.set(false);
        }
    };

    view! {
        <div
            class="relative"
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
                            on:click=move |_| {
                                dropdown_hidden.set(true);
                                update_workspace_host_modal_hidden.set(false);
                            }
                        >
                            Update
                        </a>
                    </li>
                    <li>
                        <a
                            href="#"
                            class="block px-4 py-2 hover:bg-gray-100 text-red-700"
                            on:click=delete_workspace_host
                        >
                            Delete
                        </a>
                    </li>
                </ul>
            </div>
        </div>
    }
}

#[component]
pub fn ClusterSettings() -> impl IntoView {
    let cluster_info = expect_context::<Signal<Option<ClusterInfo>>>();
    view! {
        <div class="border-b pb-4 mb-8">
            <h5 class="mr-3 text-2xl font-semibold">
                Cluster Settings
            </h5>
            <p class="text-gray-700">{"Manage your cluster settings"}</p>
        </div>
        <div class="mb-8">
            <div class="w-full p-8 border rounded-xl">
                <ClusterHostnameSetting />
            </div>
            <div class="mt-4 w-full p-8 border rounded-xl">
                <ClusterCertsSetting />
            </div>
            <div
                class="mt-4 w-full p-8 border rounded-xl"
                class:hidden=move || !cluster_info.with(|i| i.as_ref().map(|i| i.has_enterprise)).unwrap_or(false)
            >
                <CpuOvercommitSetting />
            </div>
            <div class="mt-4 w-full p-8 border rounded-xl">
                <OauthSettings reload=false />
            </div>
        </div>
    }
}

async fn update_cpu_overcommit(value: String) -> Result<(), ErrorResponse> {
    let value = value.parse::<usize>().map_err(|_| ErrorResponse {
        error: "cpu overcommit value is invalid".to_string(),
    })?;
    let resp = Request::put(&format!("/api/v1/admin/cpu_overcommit/{value}"))
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

async fn get_cpu_overcommit() -> Result<usize, ErrorResponse> {
    let resp = Request::get("/api/v1/admin/cpu_overcommit").send().await?;
    if resp.status() != 200 {
        let error = resp
            .json::<ErrorResponse>()
            .await
            .unwrap_or_else(|_| ErrorResponse {
                error: "Internal Server Error".to_string(),
            });
        return Err(error);
    }
    let value = resp.text().await?;
    let value = value.parse::<usize>().map_err(|_| ErrorResponse {
        error: "can't parse value to usize".to_string(),
    })?;
    Ok(value)
}

#[component]
fn CpuOvercommitSetting() -> impl IntoView {
    let update_counter = RwSignal::new(0);
    let current_value = LocalResource::new(move || async move { get_cpu_overcommit().await });
    Effect::new(move |_| {
        update_counter.get();
        current_value.refetch();
    });
    let value = RwSignal::new(String::new());
    Effect::new(move |_| {
        let v = current_value.with(|v| v.as_ref().and_then(|v| v.as_ref().ok().copied()));
        if let Some(v) = v {
            value.set(v.to_string());
        }
    });
    let body = view! {
        <div class="mt-2">
            <input
                prop:value={move || value.get()}
                on:input=move |ev| {
                    value.set(event_target_value(&ev));
                }
                class="max-w-96 bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5"
            />
        </div>
    };
    let save_action =
        Action::new_local(
            move |_| async move { update_cpu_overcommit(value.get_untracked()).await },
        );
    view! {
        <SettingView title="CPU Overcommit Setting".to_string() action=save_action body update_counter extra=None />
    }
}

async fn update_oauth2(
    update_oauth: OauthSettings,
    update_counter: RwSignal<i32>,
    reload: bool,
) -> Result<(), ErrorResponse> {
    let resp = Request::put("/api/v1/admin/oauth")
        .json(&update_oauth)?
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
    if reload {
        let _ = location().reload();
    } else {
        update_counter.update(|c| *c += 1);
    }
    Ok(())
}

async fn get_oauth2() -> Result<OauthSettings> {
    let resp: OauthSettings = Request::get("/api/v1/admin/oauth")
        .send()
        .await?
        .json()
        .await?;
    Ok(resp)
}

#[component]
pub fn OauthSettings(reload: bool) -> impl IntoView {
    let update_counter = RwSignal::new(0);
    let github_client_id = RwSignal::new(String::new());
    let github_client_secret = RwSignal::new(String::new());
    let gitlab_client_id = RwSignal::new(String::new());
    let gitlab_client_secret = RwSignal::new(String::new());

    let oauth = LocalResource::new(move || async move { get_oauth2().await });
    Effect::new(move |_| {
        update_counter.track();
        oauth.refetch();
    });

    Effect::new(move |_| {
        let oauth = oauth.with(|oauth| oauth.as_ref().and_then(|o| o.as_ref().ok().cloned()));
        if let Some(oauth) = oauth {
            github_client_id.set(oauth.github_client_id);
            github_client_secret.set(oauth.github_client_secret);
            gitlab_client_id.set(oauth.gitlab_client_id);
            gitlab_client_secret.set(oauth.gitlab_client_secret);
        }
    });

    let body = view! {
        <div class="mt-2">
            <div class="flex flex-row items-center mb-2 text-sm font-medium text-gray-900">
                <svg class="w-4 h-4 me-2" aria-hidden="true" xmlns="http://www.w3.org/2000/svg" fill="currentColor" viewBox="0 0 20 20">
                    <path fill-rule="evenodd" d="M10 .333A9.911 9.911 0 0 0 6.866 19.65c.5.092.678-.215.678-.477 0-.237-.01-1.017-.014-1.845-2.757.6-3.338-1.169-3.338-1.169a2.627 2.627 0 0 0-1.1-1.451c-.9-.615.07-.6.07-.6a2.084 2.084 0 0 1 1.518 1.021 2.11 2.11 0 0 0 2.884.823c.044-.503.268-.973.63-1.325-2.2-.25-4.516-1.1-4.516-4.9A3.832 3.832 0 0 1 4.7 7.068a3.56 3.56 0 0 1 .095-2.623s.832-.266 2.726 1.016a9.409 9.409 0 0 1 4.962 0c1.89-1.282 2.717-1.016 2.717-1.016.366.83.402 1.768.1 2.623a3.827 3.827 0 0 1 1.02 2.659c0 3.807-2.319 4.644-4.525 4.889a2.366 2.366 0 0 1 .673 1.834c0 1.326-.012 2.394-.012 2.72 0 .263.18.572.681.475A9.911 9.911 0 0 0 10 .333Z" clip-rule="evenodd"/>
                </svg>
                <span>GitHub Client Id</span>
            </div>
            <input
                prop:value={move || github_client_id.get()}
                on:input=move |ev| { github_client_id.set(event_target_value(&ev)) }
                class="max-w-96 bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5"
            />
        </div>
        <div class="mt-2">
            <div class="flex flex-row items-center mb-2 text-sm font-medium text-gray-900">
                <svg class="w-4 h-4 me-2" aria-hidden="true" xmlns="http://www.w3.org/2000/svg" fill="currentColor" viewBox="0 0 20 20">
                    <path fill-rule="evenodd" d="M10 .333A9.911 9.911 0 0 0 6.866 19.65c.5.092.678-.215.678-.477 0-.237-.01-1.017-.014-1.845-2.757.6-3.338-1.169-3.338-1.169a2.627 2.627 0 0 0-1.1-1.451c-.9-.615.07-.6.07-.6a2.084 2.084 0 0 1 1.518 1.021 2.11 2.11 0 0 0 2.884.823c.044-.503.268-.973.63-1.325-2.2-.25-4.516-1.1-4.516-4.9A3.832 3.832 0 0 1 4.7 7.068a3.56 3.56 0 0 1 .095-2.623s.832-.266 2.726 1.016a9.409 9.409 0 0 1 4.962 0c1.89-1.282 2.717-1.016 2.717-1.016.366.83.402 1.768.1 2.623a3.827 3.827 0 0 1 1.02 2.659c0 3.807-2.319 4.644-4.525 4.889a2.366 2.366 0 0 1 .673 1.834c0 1.326-.012 2.394-.012 2.72 0 .263.18.572.681.475A9.911 9.911 0 0 0 10 .333Z" clip-rule="evenodd"/>
                </svg>
                <span>GitHub Client Secret</span>
            </div>
            <input
                prop:value={move || github_client_secret.get()}
                on:input=move |ev| { github_client_secret.set(event_target_value(&ev)) }
                class="max-w-96 bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5"
            />
        </div>
        <div class="mt-2">
            <div class="flex flex-row items-center mb-2 text-sm font-medium text-gray-900">
                <svg class="w-4 h-4 me-2" viewBox="0 0 25 24" xmlns="http://www.w3.org/2000/svg"><path d="M24.507 9.5l-.034-.09L21.082.562a.896.896 0 00-1.694.091l-2.29 7.01H7.825L5.535.653a.898.898 0 00-1.694-.09L.451 9.411.416 9.5a6.297 6.297 0 002.09 7.278l.012.01.03.022 5.16 3.867 2.56 1.935 1.554 1.176a1.051 1.051 0 001.268 0l1.555-1.176 2.56-1.935 5.197-3.89.014-.01A6.297 6.297 0 0024.507 9.5z" fill="#E24329"/><path d="M24.507 9.5l-.034-.09a11.44 11.44 0 00-4.56 2.051l-7.447 5.632 4.742 3.584 5.197-3.89.014-.01A6.297 6.297 0 0024.507 9.5z" fill="#FC6D26"/><path d="M7.707 20.677l2.56 1.935 1.555 1.176a1.051 1.051 0 001.268 0l1.555-1.176 2.56-1.935-4.743-3.584-4.755 3.584z" fill="#FCA326"/><path d="M5.01 11.461a11.43 11.43 0 00-4.56-2.05L.416 9.5a6.297 6.297 0 002.09 7.278l.012.01.03.022 5.16 3.867 4.745-3.584-7.444-5.632z" fill="#FC6D26"/></svg>
                <span>GitLab Client Id</span>
            </div>
            <input
                prop:value={move || gitlab_client_id.get()}
                on:input=move |ev| { gitlab_client_id.set(event_target_value(&ev)) }
                class="max-w-96 bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5"
            />
        </div>
        <div class="mt-2">
            <div class="flex flex-row items-center mb-2 text-sm font-medium text-gray-900">
                <svg class="w-4 h-4 me-2" viewBox="0 0 25 24" xmlns="http://www.w3.org/2000/svg"><path d="M24.507 9.5l-.034-.09L21.082.562a.896.896 0 00-1.694.091l-2.29 7.01H7.825L5.535.653a.898.898 0 00-1.694-.09L.451 9.411.416 9.5a6.297 6.297 0 002.09 7.278l.012.01.03.022 5.16 3.867 2.56 1.935 1.554 1.176a1.051 1.051 0 001.268 0l1.555-1.176 2.56-1.935 5.197-3.89.014-.01A6.297 6.297 0 0024.507 9.5z" fill="#E24329"/><path d="M24.507 9.5l-.034-.09a11.44 11.44 0 00-4.56 2.051l-7.447 5.632 4.742 3.584 5.197-3.89.014-.01A6.297 6.297 0 0024.507 9.5z" fill="#FC6D26"/><path d="M7.707 20.677l2.56 1.935 1.555 1.176a1.051 1.051 0 001.268 0l1.555-1.176 2.56-1.935-4.743-3.584-4.755 3.584z" fill="#FCA326"/><path d="M5.01 11.461a11.43 11.43 0 00-4.56-2.05L.416 9.5a6.297 6.297 0 002.09 7.278l.012.01.03.022 5.16 3.867 4.745-3.584-7.444-5.632z" fill="#FC6D26"/></svg>
                <span>GitLab Client Secret</span>
            </div>
            <input
                prop:value={move || gitlab_client_secret.get()}
                on:input=move |ev| { gitlab_client_secret.set(event_target_value(&ev)) }
                class="max-w-96 bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5"
            />
        </div>
    };
    let save_action = Action::new_local(move |_| async move {
        update_oauth2(
            OauthSettings {
                github_client_id: github_client_id.get_untracked(),
                github_client_secret: github_client_secret.get_untracked(),
                gitlab_client_id: gitlab_client_id.get_untracked(),
                gitlab_client_secret: gitlab_client_secret.get_untracked(),
            },
            update_counter,
            reload,
        )
        .await
    });
    view! {
        <SettingView title="OAuth Provider Settings".to_string() action=save_action body update_counter extra=None />
    }
}

async fn get_hostnames() -> Result<HashMap<String, String>> {
    let resp = Request::get("/api/v1/hostnames").send().await?;
    let hostnames = resp.json().await?;
    Ok(hostnames)
}

async fn update_hostnames(
    value: Vec<(String, String)>,
    update_counter: RwSignal<i32>,
) -> Result<(), ErrorResponse> {
    let resp = Request::put("/api/v1/admin/hostnames")
        .json(&value)?
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
    update_counter.update(|c| *c += 1);
    Ok(())
}

#[component]
pub fn ClusterHostnameSetting() -> impl IntoView {
    let cluster_info = expect_context::<Signal<Option<ClusterInfo>>>();

    let update_counter = RwSignal::new(0);

    let set_hostnames = RwSignal::new(vec![]);

    let hostnames = LocalResource::new(|| async move { get_hostnames().await.unwrap_or_default() });
    Effect::new(move |_| {
        update_counter.track();
        hostnames.refetch();
    });

    Effect::new(move |_| {
        let hostnames = hostnames
            .with(|h| h.as_deref().cloned())
            .unwrap_or_default();
        let mut hostnames: Vec<(String, RwSignal<String>)> = hostnames
            .into_iter()
            .map(|(key, value)| (key, RwSignal::new(value)))
            .collect();
        hostnames.sort_by_key(|(region, _)| region.to_owned());
        set_hostnames.set(hostnames);
    });

    let body = view! {
        {
            move || {
                if cluster_info.with(|i| i.as_ref().map(|i| i.has_enterprise)).unwrap_or(false) {
                    view! {
                        <For
                            each=move || set_hostnames.get()
                            key=|pair| pair.clone()
                            children=move |(region, hostname)| {
                                view! {
                                    <div class="mt-2">
                                        <div class="flex flex-row items-center mb-2 text-sm font-medium text-gray-900">
                                            <span>{format!("Region: {region}")}</span>
                                        </div>
                                        <input
                                            prop:value={move || hostname.get()}
                                            on:input=move |ev| { hostname.set(event_target_value(&ev)) }
                                            class="max-w-96 bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5"
                                        />
                                    </div>
                                }
                            }
                        />
                    }.into_any()
                } else {
                    view! {
                        <div class="mt-2">
                            <input
                                prop:value={move || set_hostnames.get().iter().find(|(k,_)| k.is_empty()).map(|(_,v)| v.get()).unwrap_or_default()}
                                on:input=move |ev| {
                                    if let Some(h) = set_hostnames.get().iter().find(|(k,_)| k.is_empty()).map(|(_,v)| *v) {
                                        h.set(event_target_value(&ev))
                                    }
                                }
                                class="max-w-96 bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5"
                            />
                        </div>
                    }.into_any()
                }
            }
        }
    };

    let save_action = Action::new_local(move |_| async move {
        let value = set_hostnames
            .get_untracked()
            .into_iter()
            .map(|(region, hostname)| (region, hostname.get_untracked()))
            .collect();
        update_hostnames(value, update_counter).await
    });

    view! {
        <SettingView title="Hostname Settings".to_string() action=save_action body update_counter extra=None />
    }
}

async fn get_certs() -> Result<Vec<(String, String)>, ErrorResponse> {
    let resp = Request::get("/api/v1/admin/certs").send().await?;
    if resp.status() != 200 {
        let error = resp
            .json::<ErrorResponse>()
            .await
            .unwrap_or_else(|_| ErrorResponse {
                error: "Internal Server Error".to_string(),
            });
        return Err(error);
    }

    let envs: Vec<(String, String)> = resp.json().await?;
    Ok(envs)
}

async fn update_certs(certs: Vec<(String, String)>) -> Result<(), ErrorResponse> {
    let resp = Request::put("/api/v1/admin/certs")
        .json(&certs)?
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
pub fn ClusterCertsSetting() -> impl IntoView {
    let certs = RwSignal::new(vec![]);
    let new_cert = move |_| {
        certs.update(|certs| {
            certs.push((RwSignal::new(String::new()), RwSignal::new(String::new())));
        });
    };
    let delete_cert = move |i: usize| {
        certs.update(|certs| {
            certs.remove(i);
        })
    };

    let update_counter = RwSignal::new(0);
    let current_certs =
        LocalResource::new(move || async move { get_certs().await.unwrap_or_default() });
    Effect::new(move |_| {
        update_counter.track();
        current_certs.refetch();
    });

    Effect::new(move |_| {
        let current_certs =
            current_certs.with(|certs| certs.as_deref().cloned().unwrap_or_default());
        certs.update(|certs| {
            *certs = current_certs
                .into_iter()
                .map(|(name, value)| (RwSignal::new(name), RwSignal::new(value)))
                .collect();
        });
    });

    let save_action = Action::new_local(move |_| async move {
        let certs = certs.get_untracked();
        let certs: Vec<(String, String)> = certs
            .into_iter()
            .filter_map(|(cert, key)| {
                let cert = cert.get_untracked();
                let key = key.get_untracked();
                if cert.trim().is_empty() || key.trim().is_empty() {
                    return None;
                }
                Some((cert.trim().to_string(), key.trim().to_string()))
            })
            .collect();
        update_certs(certs).await
    });

    let body = view! {
        <div class="mt-2">
        <For
            each=move || certs.get().into_iter().enumerate()
            key=|cert| cert.to_owned()
            children=move |(i, (cert, key))| {
                view! {
                    <div
                        class="flex flex-row items-center"
                        class=("mt-2", move || i > 0)
                    >
                            <div class="w-96">
                            <textarea
                                rows=8
                                prop:value={move || cert.get()}
                                on:input=move |ev| { cert.set(event_target_value(&ev)); }
                                class="bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5"
                                placeholder="Certificate Chain"
                            />
                        </div>
                        <div class="ml-2 w-96">
                            <textarea
                                rows=8
                                prop:value={move || key.get()}
                                on:input=move |ev| { key.set(event_target_value(&ev)); }
                                class="bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5"
                                placeholder="Private Key"
                            />
                        </div>
                        <div class="ml-2 hover:bg-gray-200 rounded-lg p-2 cursor-pointer"
                            on:click=move |_| delete_cert(i)
                        >
                            <svg class="h-4 text-gray-800" aria-hidden="true" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
                                <path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18 18 6m0 12L6 6"/>
                            </svg>
                        </div>
                    </div>
                }
            }
        />
        </div>
    };

    let extra = view! {
        <button
            type="button"
            class="ml-2 py-2.5 px-5 text-sm font-medium text-gray-900 focus:outline-none bg-white rounded-lg border border-gray-200 hover:bg-gray-100 hover:text-blue-700 focus:z-10 focus:ring-4 focus:ring-gray-100"
            on:click=new_cert
        >
            New Certificate
        </button>
    }.into_any();

    view! {
        <SettingView title="Certificate Settings".to_string() action=save_action body update_counter extra=Some(extra) />
    }
}

async fn all_machine_types() -> Result<Vec<MachineType>> {
    let resp = Request::get("/api/v1/machine_types").send().await?;
    let machine_types: Vec<MachineType> = resp.json().await?;
    Ok(machine_types)
}

#[component]
pub fn MachineTypeView() -> impl IntoView {
    let new_machine_type_modal_hidden = RwSignal::new(true);
    let update_counter = RwSignal::new(0);
    let machine_types =
        LocalResource::new(|| async move { all_machine_types().await.unwrap_or_default() });
    Effect::new(move |_| {
        update_counter.track();
        machine_types.refetch();
    });
    let machine_type_filter = RwSignal::new(String::new());
    let machine_types = Signal::derive(move || {
        let machine_type_filter = machine_type_filter.get();
        let mut machine_types = machine_types
            .with(|machine_types| machine_types.as_deref().cloned().unwrap_or_default());
        machine_types.retain(|h| h.name.contains(&machine_type_filter));
        machine_types
    });
    view! {
        <div class="pb-4">
            <h5 class="mr-3 text-2xl font-semibold">
                Machine Types
            </h5>
            <p class="text-gray-700">{"Manage the machine types for the cluster"}</p>
            <div class="flex flex-col items-center justify-between py-4 gap-y-3 md:flex-row md:space-y-0 md:space-x-4">
                <div class="w-full md:w-1/2">
                    <form class="flex items-center">
                        <label for="simple-search" class="sr-only">Filter Machine Types</label>
                        <div class="relative w-full">
                        <div class="absolute inset-y-0 left-0 flex items-center pl-3 pointer-events-none">
                            <svg aria-hidden="true" class="w-5 h-5 text-gray-500" fill="currentColor" viewbox="0 0 20 20" xmlns="http://www.w3.org/2000/svg">
                            <path fill-rule="evenodd" d="M8 4a4 4 0 100 8 4 4 0 000-8zM2 8a6 6 0 1110.89 3.476l4.817 4.817a1 1 0 01-1.414 1.414l-4.816-4.816A6 6 0 012 8z" clip-rule="evenodd" />
                            </svg>
                        </div>
                        <input
                            prop:value={move || machine_type_filter.get()}
                            on:input=move |ev| { machine_type_filter.set(event_target_value(&ev)); }
                            type="text"
                            class="bg-white block w-full p-2 pl-10 text-sm text-gray-900 border border-gray-300 rounded-lg bg-gray-50 focus:ring-blue-500 focus:border-blue-500"
                            placeholder="Filter Machine Types"
                        />
                        </div>
                    </form>
                </div>
                <button
                    type="button"
                    class="px-4 py-2 text-sm font-medium text-white rounded-lg bg-blue-700 hover:bg-blue-800 focus:ring-4 focus:ring-blue-300 focus:outline-none"
                    on:click=move |_| new_machine_type_modal_hidden.set(false)
                >
                    New Machine Type
                </button>
            </div>
        </div>

        <div class="flex flex-col gap-y-8">
            <For
                each=move || machine_types.get()
                key=|m| m.clone()
                children=move |machine_type| {
                    view! {
                        <MachineTypeItem machine_type update_counter />
                    }
                }
            />
        </div>
        <NewMachineTypeView new_machine_type_modal_hidden update_counter />
    }
}

#[component]
fn MachineTypeItem(machine_type: MachineType, update_counter: RwSignal<i32>) -> impl IntoView {
    let update_machine_type_modal_hidden = RwSignal::new(true);
    let delete_modal_hidden = RwSignal::new(true);
    let delete_action = {
        let id = machine_type.id;
        Action::new_local(move |_| async move {
            delete_machine_type(id, update_counter, delete_modal_hidden).await
        })
    };
    view! {
        <div class="border rounded-xl p-4 flex flex-row items-center">
            <div class="w-1/3 flex flex-row items-center">
                <p>{machine_type.name.clone()}</p>
            </div>
            <div class="w-1/3 flex flex-row items-center">
                <p><span class="text-gray-500 mr-1">{"Cost per Second:"}</span>{machine_type.cost_per_second.to_string()}</p>
            </div>
            <div class="w-1/3 flex flex-row items-center">
                <div class="w-5/6 flex flex-col">
                    <p><span class="text-gray-500 mr-1">{"CPU Cores:"}</span>{format!("{} {}", machine_type.cpu, if machine_type.shared {"shared"} else {"dedicated"})}</p>
                    <p><span class="text-gray-500 mr-1">{"Memory:"}</span>{format!("{}GB", machine_type.memory)}</p>
                    <p><span class="text-gray-500 mr-1">{"Disk:"}</span>{format!("{}GB", machine_type.disk)}</p>
                </div>
                <div class="w-1/6">
                    <MachineTypeControl delete_modal_hidden update_machine_type_modal_hidden align_right=true />
                </div>
            </div>
        </div>
        <DeletionModal resource=machine_type.name.clone() modal_hidden=delete_modal_hidden delete_action />
        <UpdateMachineTypeModal machine_type=machine_type.clone() update_machine_type_modal_hidden update_counter />
    }
}

#[component]
pub fn NewMachineTypeView(
    new_machine_type_modal_hidden: RwSignal<bool>,
    update_counter: RwSignal<i32>,
) -> impl IntoView {
    let name = RwSignal::new(String::new());
    let cpu = RwSignal::new(String::new());
    let memory = RwSignal::new(String::new());
    let disk = RwSignal::new(String::new());
    let cost = RwSignal::new("0".to_string());
    let shared_cpu = RwSignal::new(false);
    let cluster_info = expect_context::<Signal<Option<ClusterInfo>>>();
    let action = Action::new_local(move |_| {
        create_machine_type(
            name.get_untracked(),
            cpu.get_untracked(),
            shared_cpu.get_untracked(),
            memory.get_untracked(),
            disk.get_untracked(),
            cost.get_untracked(),
            update_counter,
            new_machine_type_modal_hidden,
        )
    });
    let body = view! {
        <CreationInput label="Name".to_string() value=name placeholder="name of the machine type".to_string() />
        <div
            class="flex items-start mb-6"
            class:hidden=move || !cluster_info.with(|i| i.as_ref().map(|i| i.has_enterprise)).unwrap_or(false)
        >
            <div class="flex items-center h-5">
            <input
                type="checkbox"
                value=""
                class="w-4 h-4 border border-gray-300 rounded bg-gray-50 focus:ring-3 focus:ring-blue-300"
                prop:checked=move || shared_cpu.get()
                on:change=move |e| shared_cpu.set(event_target_checked(&e))
            />
            </div>
            <label for="remember" class="ml-2 text-sm font-medium text-gray-900">Shared cpu cores</label>
        </div>
        <CreationInput label="CPU Cores".to_string() value=cpu placeholder="number of CPU cores".to_string() />
        <CreationInput label="Memory (GB)".to_string() value=memory placeholder="memory in GB".to_string() />
        <CreationInput label="Disk (GB)".to_string() value=disk placeholder="disk in GB".to_string() />
        <div
            class:hidden=move || !cluster_info.with(|i| i.as_ref().map(|i| i.has_enterprise)).unwrap_or(false)
        >
            <CreationInput label="Cost per second".to_string() value=cost placeholder="cost per second of the machine type".to_string() />
        </div>
    };
    view! {
        <CreationModal title="Create New Machine Type".to_string() modal_hidden=new_machine_type_modal_hidden action body update_text=Some("Create".to_string()) updating_text=Some("Creating".to_string())  width_class=None create_button_hidden=Box::new(|| false) />
    }
}

#[component]
pub fn UpdateMachineTypeModal(
    machine_type: MachineType,
    update_machine_type_modal_hidden: RwSignal<bool>,
    update_counter: RwSignal<i32>,
) -> impl IntoView {
    let name = RwSignal::new(machine_type.name.clone());
    let cost = RwSignal::new(machine_type.cost_per_second.to_string());
    let cluster_info = expect_context::<Signal<Option<ClusterInfo>>>();
    let action = Action::new_local(move |_| {
        update_machine_type(
            machine_type.id,
            name.get_untracked(),
            cost.get_untracked(),
            update_counter,
            update_machine_type_modal_hidden,
        )
    });
    let body = view! {
        <CreationInput label="Name".to_string() value=name placeholder="name of the machine type".to_string() />
        <div
            class:hidden=move || !cluster_info.with(|i| i.as_ref().map(|i| i.has_enterprise)).unwrap_or(false)
        >
            <CreationInput label="Cost per second".to_string() value=cost placeholder="cost per second of the machine type".to_string() />
        </div>
    };
    view! {
        <CreationModal title="Update Workspace Host".to_string() modal_hidden=update_machine_type_modal_hidden action body update_text=None updating_text=None  width_class=None create_button_hidden=Box::new(|| false) />
    }
}

#[component]
fn MachineTypeControl(
    delete_modal_hidden: RwSignal<bool>,
    update_machine_type_modal_hidden: RwSignal<bool>,
    align_right: bool,
) -> impl IntoView {
    let dropdown_hidden = RwSignal::new(true);

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

    let delete_workspace_host = {
        move |_| {
            dropdown_hidden.set(true);
            delete_modal_hidden.set(false);
        }
    };

    view! {
        <div
            class="relative"
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
                            on:click=move |_| {
                                dropdown_hidden.set(true);
                                update_machine_type_modal_hidden.set(false);
                            }
                        >
                            Update
                        </a>
                    </li>
                    <li>
                        <a
                            href="#"
                            class="block px-4 py-2 hover:bg-gray-100 text-red-700"
                            on:click=delete_workspace_host
                        >
                            Delete
                        </a>
                    </li>
                </ul>
            </div>
        </div>
    }
}

#[allow(clippy::too_many_arguments)]
async fn create_machine_type(
    name: String,
    cpu: String,
    shared: bool,
    memory: String,
    disk: String,
    cost: String,
    update_counter: RwSignal<i32>,
    new_workspace_host_modal_hidden: RwSignal<bool>,
) -> Result<(), ErrorResponse> {
    let cpu: usize = match cpu.parse() {
        Ok(n) => n,
        Err(_) => {
            return Err(ErrorResponse {
                error: "cpu is invalid".to_string(),
            })
        }
    };
    let memory: usize = match memory.parse() {
        Ok(n) => n,
        Err(_) => {
            return Err(ErrorResponse {
                error: "memory is invalid".to_string(),
            })
        }
    };
    let disk: usize = match disk.parse() {
        Ok(n) => n,
        Err(_) => {
            return Err(ErrorResponse {
                error: "disk is invalid".to_string(),
            })
        }
    };
    let cost: usize = match cost.parse() {
        Ok(n) => n,
        Err(_) => {
            return Err(ErrorResponse {
                error: "cost is invalid".to_string(),
            })
        }
    };
    let resp = Request::post("/api/v1/admin/machine_types")
        .json(&CreateMachineType {
            name,
            cpu,
            memory,
            disk,
            shared,
            cost_per_second: cost,
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
    update_counter.update(|c| *c += 1);
    new_workspace_host_modal_hidden.set(true);
    Ok(())
}

async fn update_machine_type(
    id: Uuid,
    name: String,
    cost: String,
    update_counter: RwSignal<i32>,
    update_workspace_host_modal_hidden: RwSignal<bool>,
) -> Result<(), ErrorResponse> {
    let cost: usize = match cost.parse() {
        Ok(n) => n,
        Err(_) => {
            return Err(ErrorResponse {
                error: "cost is invalid".to_string(),
            })
        }
    };

    let resp = Request::put(&format!("/api/v1/admin/machine_types/{id}"))
        .json(&UpdateMachineType {
            name,
            cost_per_second: cost,
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
    update_counter.update(|c| *c += 1);
    update_workspace_host_modal_hidden.set(true);
    Ok(())
}

async fn delete_machine_type(
    id: Uuid,
    update_counter: RwSignal<i32>,
    delete_modal_hidden: RwSignal<bool>,
) -> Result<(), ErrorResponse> {
    let resp = Request::delete(&format!("/api/v1/admin/machine_types/{id}"))
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
    update_counter.update(|c| *c += 1);
    delete_modal_hidden.set(true);
    Ok(())
}

async fn get_cluster_users(
    filter: String,
    page_size: String,
    page: u64,
) -> Result<ClusterUserResult, ErrorResponse> {
    let page_size = page_size.parse::<u64>().unwrap_or(10);

    let mut query = vec![
        ("page", page.to_string()),
        ("page_size", page_size.to_string()),
    ];
    let filter = filter.trim().to_string();
    if !filter.is_empty() {
        query.push(("filter", filter));
    }

    let resp = Request::get("/api/v1/admin/users")
        .query(query)
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

    let result: ClusterUserResult = resp.json().await?;

    Ok(result)
}

async fn update_cluster_user(
    id: Uuid,
    cluster_admin: bool,
    search_action: Action<(), Result<ClusterUserResult, ErrorResponse>, LocalStorage>,
    update_modal_hidden: RwSignal<bool>,
) -> Result<(), ErrorResponse> {
    let resp = Request::put(&format!("/api/v1/admin/users/{id}"))
        .json(&UpdateClusterUser { cluster_admin })?
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
    search_action.dispatch(());
    update_modal_hidden.set(true);
    Ok(())
}

#[component]
pub fn ClusterUsersView() -> impl IntoView {
    let page_size = RwSignal::new(String::new());
    let page = RwSignal::new(0);

    let error = RwSignal::new(None);
    let user_filter = RwSignal::new(String::new());
    let search_action = Action::new_local(move |_| async move {
        error.set(None);
        let result = get_cluster_users(
            user_filter.get_untracked(),
            page_size.get_untracked(),
            page.get_untracked(),
        )
        .await;

        if let Err(e) = &result {
            error.set(Some(e.error.clone()));
        }

        result
    });
    let users = Signal::derive(move || {
        let result = search_action.value().get();
        if let Some(Ok(result)) = result {
            return result;
        }
        ClusterUserResult {
            num_pages: 0,
            page: 0,
            page_size: page_size.get_untracked().parse().unwrap_or(10),
            total_items: 0,
            users: vec![],
        }
    });

    let prev_page = move |_| {
        if users.with_untracked(|a| a.page == 0) {
            return;
        }
        page.update(|p| *p = p.saturating_sub(1));
        search_action.dispatch(());
    };

    let next_page = move |_| {
        if users.with_untracked(|a| a.page + 1 >= a.num_pages) {
            return;
        }
        page.update(|p| *p += 1);
        search_action.dispatch(());
    };

    let change_page_size = move |e| {
        page_size.set(event_target_value(&e));
        page.set(0);
        search_action.dispatch(());
    };

    search_action.dispatch(());
    view! {
        <div class="pb-4">
            <h5 class="mr-3 text-2xl font-semibold">
                Cluster Users
            </h5>
            <p class="text-gray-700">{"Manage all users in the cluster"}</p>
            <div class="flex flex-col items-center justify-between py-4 gap-y-3 md:flex-row md:space-y-0 md:space-x-4">
                <div class="w-full md:w-1/2">
                    <form class="flex items-center">
                        <label for="simple-search" class="sr-only">Filter Workspace Hosts</label>
                        <div class="relative w-full">
                        <div class="absolute inset-y-0 left-0 flex items-center pl-3 pointer-events-none">
                            <svg aria-hidden="true" class="w-5 h-5 text-gray-500" fill="currentColor" viewbox="0 0 20 20" xmlns="http://www.w3.org/2000/svg">
                            <path fill-rule="evenodd" d="M8 4a4 4 0 100 8 4 4 0 000-8zM2 8a6 6 0 1110.89 3.476l4.817 4.817a1 1 0 01-1.414 1.414l-4.816-4.816A6 6 0 012 8z" clip-rule="evenodd" />
                            </svg>
                        </div>
                        <input
                            prop:value={move || user_filter.get()}
                            on:input=move |ev| { user_filter.set(event_target_value(&ev)); }
                            type="text"
                            class="bg-white block w-full p-2 pl-10 text-sm text-gray-900 border border-gray-300 rounded-lg bg-gray-50 focus:ring-blue-500 focus:border-blue-500"
                            placeholder="Filter Cluster Users"
                        />
                        </div>
                    </form>
                </div>
                <button
                    type="button"
                    class="px-4 py-2 text-sm font-medium text-white rounded-lg bg-blue-700 hover:bg-blue-800 focus:ring-4 focus:ring-blue-300 focus:outline-none"
                    on:click=move |_| { search_action.dispatch(()); }
                >
                    Search User
                </button>
            </div>
        </div>

        { move || if let Some(error) = error.get() {
            view! {
                <div class="my-4 p-4 rounded-lg bg-red-50">
                    <span class="text-sm font-medium text-red-800">{ error }</span>
                </div>
            }.into_any()
        } else {
            ().into_any()
        }}

        <div class="mt-4 flex flex-row items-center justify-between">
            <span class="text-sm font-normal text-gray-500">
                {"Showing "}
                <span class="font-semibold text-gray-900">
                    {move || format!("{}-{}", users.with(|a| if a.users.is_empty() {0} else {1} + a.page * a.page_size ), users.with(|a| if a.users.is_empty() {0} else {a.users.len() as u64} + a.page * a.page_size  ))}
                </span>
                {" of "}
                <span class="font-semibold text-gray-900">{move || users.with(|a| a.total_items)}</span>
            </span>
            <div class="flex flex-row items-center">
                <p class="mr-2">{"rows per page"}</p>

                <select
                    class="bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-18 p-2.5"
                    on:change=change_page_size
                >
                    <option selected>10</option>
                    <option>20</option>
                    <option>50</option>
                    <option>100</option>
                    <option>200</option>
                </select>

                <button class="ml-2 p-2 rounded"
                    class=("text-gray-300", move || users.with(|a| a.page == 0))
                    class=("cursor-pointer", move || !users.with(|a| a.page == 0))
                    class=("hover:bg-gray-100", move || !users.with(|a| a.page == 0))
                    disabled=move || users.with(|a| a.page == 0)
                    on:click=prev_page
                >
                    <svg class="w-5 h-5" aria-hidden="true" fill="currentColor" viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg">
                        <path fill-rule="evenodd" d="M12.707 5.293a1 1 0 010 1.414L9.414 10l3.293 3.293a1 1 0 01-1.414 1.414l-4-4a1 1 0 010-1.414l4-4a1 1 0 011.414 0z" clip-rule="evenodd"></path>
                    </svg>
                </button>
                <span class="p-2 rounded"
                    class=("text-gray-300", move || users.with(|a| a.page + 1 >= a.num_pages))
                    class=("cursor-pointer", move || !users.with(|a| a.page + 1 >= a.num_pages))
                    class=("hover:bg-gray-100", move || !users.with(|a| a.page + 1 >= a.num_pages))
                    on:click=next_page
                >
                    <svg class="w-5 h-5" aria-hidden="true" fill="currentColor" viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg">
                        <path fill-rule="evenodd" d="M7.293 14.707a1 1 0 010-1.414L10.586 10 7.293 6.707a1 1 0 011.414-1.414l4 4a1 1 0 010 1.414l-4 4a1 1 0 01-1.414 0z" clip-rule="evenodd"></path>
                    </svg>
                </span>
            </div>
        </div>

        <div class="mt-2 overflow-x-auto w-full">
            <table class="w-full text-left text-gray-700 bg-gray-50">
                <thead>
                    <tr>
                        <th scope="col" class="py-2 px-4">Oauth Provider</th>
                        <th scope="col" class="py-2 px-4">User</th>
                        <th scope="col" class="py-2 px-4">Created At</th>
                        <th scope="col" class="py-2 px-4">Cluster Admin</th>
                    </tr>
                </thead>

                <tbody>
                    <For
                        each=move || users.get().users.into_iter().enumerate()
                        key=|(_, u)| (u.id, u.cluster_admin)
                        children=move |(i, user)| {
                            view! {
                                <ClusterUserItemView i user search_action />
                            }
                        }
                    />
                </tbody>
            </table>
        </div>
    }
}

#[component]
fn ClusterUserItemView(
    i: usize,
    user: ClusterUser,
    search_action: Action<(), Result<ClusterUserResult, ErrorResponse>, LocalStorage>,
) -> impl IntoView {
    let user_id = user.id;
    let update_modal_hidden = RwSignal::new(true);
    let is_cluster_admin = RwSignal::new(user.cluster_admin);

    let update_modal_body = view! {
        <div
            class="flex items-start"
        >
            <div class="flex items-center h-5">
            <input
                id={user_id.to_string()}
                type="checkbox"
                value=""
                class="w-4 h-4 border border-gray-300 rounded bg-gray-50 focus:ring-3 focus:ring-blue-300"
                prop:checked=move || is_cluster_admin.get()
                on:change=move |e| is_cluster_admin.set(event_target_checked(&e))
            />
            </div>
            <label
                for={move || user_id.to_string()}
                class="ml-2 text-sm font-medium text-gray-900"
            >
                Is Cluster Admin
            </label>
        </div>
    };

    let update_action = Action::new_local(move |()| async move {
        update_cluster_user(
            user_id,
            is_cluster_admin.get_untracked(),
            search_action,
            update_modal_hidden,
        )
        .await
    });

    let icon = match user.auth_provider {
        AuthProvider::Github => view! {
            <svg class="w-4 h-4 me-2" aria-hidden="true" xmlns="http://www.w3.org/2000/svg" fill="currentColor" viewBox="0 0 20 20">
                <path fill-rule="evenodd" d="M10 .333A9.911 9.911 0 0 0 6.866 19.65c.5.092.678-.215.678-.477 0-.237-.01-1.017-.014-1.845-2.757.6-3.338-1.169-3.338-1.169a2.627 2.627 0 0 0-1.1-1.451c-.9-.615.07-.6.07-.6a2.084 2.084 0 0 1 1.518 1.021 2.11 2.11 0 0 0 2.884.823c.044-.503.268-.973.63-1.325-2.2-.25-4.516-1.1-4.516-4.9A3.832 3.832 0 0 1 4.7 7.068a3.56 3.56 0 0 1 .095-2.623s.832-.266 2.726 1.016a9.409 9.409 0 0 1 4.962 0c1.89-1.282 2.717-1.016 2.717-1.016.366.83.402 1.768.1 2.623a3.827 3.827 0 0 1 1.02 2.659c0 3.807-2.319 4.644-4.525 4.889a2.366 2.366 0 0 1 .673 1.834c0 1.326-.012 2.394-.012 2.72 0 .263.18.572.681.475A9.911 9.911 0 0 0 10 .333Z" clip-rule="evenodd"/>
            </svg>
        }.into_any(),
        AuthProvider::Gitlab => view! {
            <svg class="w-4 h-4 me-2" viewBox="0 0 25 24" xmlns="http://www.w3.org/2000/svg"><path d="M24.507 9.5l-.034-.09L21.082.562a.896.896 0 00-1.694.091l-2.29 7.01H7.825L5.535.653a.898.898 0 00-1.694-.09L.451 9.411.416 9.5a6.297 6.297 0 002.09 7.278l.012.01.03.022 5.16 3.867 2.56 1.935 1.554 1.176a1.051 1.051 0 001.268 0l1.555-1.176 2.56-1.935 5.197-3.89.014-.01A6.297 6.297 0 0024.507 9.5z" fill="#E24329"/><path d="M24.507 9.5l-.034-.09a11.44 11.44 0 00-4.56 2.051l-7.447 5.632 4.742 3.584 5.197-3.89.014-.01A6.297 6.297 0 0024.507 9.5z" fill="#FC6D26"/><path d="M7.707 20.677l2.56 1.935 1.555 1.176a1.051 1.051 0 001.268 0l1.555-1.176 2.56-1.935-4.743-3.584-4.755 3.584z" fill="#FCA326"/><path d="M5.01 11.461a11.43 11.43 0 00-4.56-2.05L.416 9.5a6.297 6.297 0 002.09 7.278l.012.01.03.022 5.16 3.867 4.745-3.584-7.444-5.632z" fill="#FC6D26"/></svg>
        }.into_any(),
    };
    view! {
        <tr
            class="w-full bg-white"
            class=("border-t", move || i > 0)
        >
            <td class="px-4 py-2">
                <div class="flex flex-row items-center">
                    {icon}
                    <p>{user.auth_provider.to_string()}</p>
                </div>
            </td>
            <td class="px-4 py-2">
                <div class="flex flex-row items-center">
                    <img
                        class="w-8 h-8 rounded-full mr-2"
                        src={ user.avatar_url.clone() }
                        alt="user photo"
                    />
                    <div class="flex flex-col">
                        <p>{user.name}</p>
                        <p>{user.email}</p>
                    </div>
                </div>
            </td>
            <td class="px-4 py-2">
                <DatetimeModal time=user.created_at />
            </td>
            <td class="px-4 py-2">
                <p>{user.cluster_admin}</p>
            </td>
            <td class="px-4 py-2">
                <button
                    class="px-4 py-2 text-sm font-medium text-white rounded-lg bg-green-700 hover:bg-green-800 focus:ring-4 focus:ring-green-300 focus:outline-none"
                    on:click=move |_| update_modal_hidden.set(false)
                >
                    Update
                </button>
            </td>
        </tr>
        <CreationModal title=format!("Update Cluster User") modal_hidden=update_modal_hidden action=update_action body=update_modal_body update_text=None updating_text=None  width_class=None create_button_hidden=Box::new(|| false) />
    }
}
