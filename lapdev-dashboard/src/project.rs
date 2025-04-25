use std::{collections::HashMap, str::FromStr, time::Duration};

use anyhow::{anyhow, Result};
use gloo_net::http::Request;
use lapdev_common::{
    GitBranch, NewProject, NewProjectPrebuild, NewProjectResponse, PrebuildStatus, ProjectInfo,
    ProjectPrebuild,
};
use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_params_map};
use uuid::Uuid;
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use web_sys::FocusEvent;

use crate::{
    cluster::get_cluster_info,
    modal::{
        CreationInput, CreationModal, DatetimeModal, DeletionModal, ErrorResponse, SettingView,
    },
    organization::get_current_org,
    workspace::{repo_img, CreateWorkspaceProjectInfo, NewWorkspaceModal},
};

async fn create_project(repo: String, machine_type: Option<Uuid>) -> Result<(), ErrorResponse> {
    let org = get_current_org()?;

    let machine_type_id = machine_type.unwrap_or_else(|| {
        let cluster_info = get_cluster_info();
        cluster_info
            .get_untracked()
            .and_then(|i| i.machine_types.first().map(|m| m.id))
            .unwrap_or_else(|| Uuid::from_u128(0))
    });
    let resp = Request::post(&format!("/api/v1/organizations/{}/projects", org.id))
        .json(&NewProject {
            repo,
            machine_type_id,
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

    let resp: NewProjectResponse = resp.json().await?;
    use_navigate()(&format!("/projects/{}", resp.id), Default::default());

    Ok(())
}

#[component]
pub fn NewProjectView(new_project_modal_hidden: RwSignal<bool, LocalStorage>) -> impl IntoView {
    let repo = RwSignal::new_local(String::new());
    let current_machine_type = RwSignal::new_local(None);
    let preferred_machine_type = Signal::derive_local(|| None);
    let action = Action::new_local(move |_| {
        create_project(repo.get_untracked(), current_machine_type.get())
    });
    let body = view! {
        <CreationInput label="Your repository url".to_string() value=repo placeholder="https://github.com/owner/repo".to_string() />
        <MachineTypeView current_machine_type preferred_machine_type />
    };
    view! {
        <CreationModal title="Create New Project".to_string() modal_hidden=new_project_modal_hidden action body update_text=Some("Create".to_string()) updating_text=Some("Creating".to_string()) width_class=None create_button_hidden=Box::new(|| false) />
    }
}

#[component]
fn ProjectControl(
    project_info: Option<ProjectInfo>,
    delete_modal_hidden: RwSignal<bool, LocalStorage>,
    project_id: Uuid,
    align_right: bool,
    new_workspace_modal_hidden: RwSignal<bool, LocalStorage>,
    create_workspace_project: RwSignal<Option<CreateWorkspaceProjectInfo>, LocalStorage>,
) -> impl IntoView {
    let hidden = RwSignal::new_local(true);
    let toggle_dropdown = move |_| {
        if hidden.get_untracked() {
            hidden.set(false);
        } else {
            hidden.set(true);
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
                if !has_focus && !hidden.get_untracked() {
                    hidden.set(true);
                }
            },
            Duration::from_secs(0),
        );
    };

    let handle_create_workspace = {
        let project_info = project_info.clone();
        move |_| {
            if let Some(project_info) = project_info.clone() {
                create_workspace_project.update(|project| {
                    *project = Some(CreateWorkspaceProjectInfo {
                        project: project_info,
                        branch: None,
                        branches: Vec::new(),
                        prebuilds: HashMap::new(),
                    });
                });
                project_branches_signal(
                    Signal::derive_local(move || project_id.to_string()),
                    RwSignal::new_local("".to_string()),
                    create_workspace_project,
                );
                project_prebuilds_signal(
                    Signal::derive_local(move || project_id.to_string()),
                    RwSignal::new_local("".to_string()),
                    create_workspace_project,
                );
            } else {
                create_workspace_project.update(|project| {
                    if let Some(project) = project.as_mut() {
                        project.branch = None;
                    }
                });
            }
            new_workspace_modal_hidden.set(false);
        }
    };

    view! {
        <div class="flex flex-row items-center">
            <button
                class="px-4 py-2 text-sm font-medium text-white rounded-lg bg-green-700 hover:bg-green-800 focus:ring-4 focus:ring-green-300 focus:outline-none"
                on:click=handle_create_workspace
            >
                New Workspace
            </button>
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
                    class:hidden=move || hidden.get()
                    class=("right-0", move || align_right)
                >
                    <ul class="py-2 text-sm text-gray-700 bg-white rounded-lg border shadow w-44">
                        <li>
                            <a
                                href="#"
                                class="block px-4 py-2 hover:bg-gray-100 text-red-700"
                                on:click=move |_| delete_modal_hidden.set(false)
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

async fn update_project_machine_type(
    project_id: Uuid,
    machine_type_id: Uuid,
) -> Result<(), ErrorResponse> {
    let org = get_current_org()?;

    let resp = Request::put(&format!(
        "/api/v1/organizations/{}/projects/{project_id}/machine_type/{machine_type_id}",
        org.id,
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

async fn delete_project_prebuild(
    project: String,
    prebuild_id: Uuid,
    prebuilds_counter: RwSignal<i32, LocalStorage>,
    delete_modal_hidden: RwSignal<bool, LocalStorage>,
) -> Result<(), ErrorResponse> {
    let org = get_current_org()?;

    let resp = Request::delete(&format!(
        "/api/v1/organizations/{}/projects/{project}/prebuilds/{prebuild_id}",
        org.id,
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
    prebuilds_counter.update(|c| *c += 1);

    Ok(())
}

async fn delete_project(
    id: Uuid,
    delete_modal_hidden: RwSignal<bool, LocalStorage>,
    update_counter: Option<RwSignal<i32, LocalStorage>>,
) -> Result<(), ErrorResponse> {
    let org = get_current_org()?;
    let resp = Request::delete(&format!("/api/v1/organizations/{}/projects/{id}", org.id))
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
        use_navigate()("/projects", Default::default());
    }
    Ok(())
}

#[component]
pub fn ProjectItem(
    project: ProjectInfo,
    update_counter: RwSignal<i32, LocalStorage>,
    new_workspace_modal_hidden: RwSignal<bool, LocalStorage>,
    create_workspace_project: RwSignal<Option<CreateWorkspaceProjectInfo>, LocalStorage>,
) -> impl IntoView {
    let id = project.id;
    let local_project = project.clone();
    let delete_modal_hidden = RwSignal::new_local(true);
    let delete_action =
        Action::new_local(move |_| delete_project(id, delete_modal_hidden, Some(update_counter)));
    view! {
        <div class="flex flex-col items-center bg-white border border-gray-200 rounded-lg shadow-lg lg:flex-row w-full">
            <div class="lg:w-1/3 flex flex-row">
                <a href={ project.repo_url.clone() } target="_blank">
                    <img
                        src={ repo_img(&project.repo_url) }
                        class="object-contain lg:object-left h-40 rounded-lg justify-self-start"
                    />
                </a>
            </div>
            <div class="lg:w-1/3 flex flex-col justify-center">
                <div class="flex flex-col p-4">
                    <a href={ format!("/projects/{}", project.id) }>
                        <span class="font-semibold">{ project.name.clone() }</span>
                        <span class="flex flex-row items-center text-sm text-gray-700 mt-2">
                            <svg class="w-3 h-3 mr-2" aria-hidden="true" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 14 20">
                                <path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 5v10M3 5a2 2 0 1 0 0-4 2 2 0 0 0 0 4Zm0 10a2 2 0 1 0 0 4 2 2 0 0 0 0-4Zm6-3.976-2-.01A4.015 4.015 0 0 1 3 7m10 4a2 2 0 1 1-4 0 2 2 0 0 1 4 0Z"/>
                            </svg>
                            <p>{ move || project.repo_url.clone() }</p>
                        </span>
                        <span class="mt-2 text-sm text-gray-800 inline-flex items-center rounded me-2 text-gray-700">
                            <svg class="w-2.5 h-2.5 mr-2.5" aria-hidden="true" xmlns="http://www.w3.org/2000/svg" fill="currentColor" viewBox="0 0 20 20">
                            <path d="M10 0a10 10 0 1 0 10 10A10.011 10.011 0 0 0 10 0Zm3.982 13.982a1 1 0 0 1-1.414 0l-3.274-3.274A1.012 1.012 0 0 1 9 10V6a1 1 0 0 1 2 0v3.586l2.982 2.982a1 1 0 0 1 0 1.414Z"/>
                            </svg>
                            <span class="mr-1">Created</span> <DatetimeModal time=project.created_at />
                        </span>
                    </a>
                </div>
            </div>
            <div class="lg:w-1/3 flex flex-row items-center p-4 justify-center">
                <ProjectControl project_info=Some(local_project) delete_modal_hidden project_id=id align_right=true new_workspace_modal_hidden create_workspace_project />
            </div>
        </div>
        <DeletionModal resource=project.name.clone() modal_hidden=delete_modal_hidden delete_action />
    }
}

async fn all_projects() -> Result<Vec<ProjectInfo>> {
    let org = get_current_org()?;
    let resp = Request::get(&format!("/api/v1/organizations/{}/projects", org.id))
        .send()
        .await?;
    let projects: Vec<ProjectInfo> = resp.json().await?;
    Ok(projects)
}

#[component]
pub fn Projects() -> impl IntoView {
    let project_filter = RwSignal::new_local(String::new());
    let new_project_modal_hidden = RwSignal::new_local(true);
    let new_workspace_modal_hidden = RwSignal::new_local(true);
    let create_workspace_project = RwSignal::new_local(None);
    let update_counter = RwSignal::new_local(0);

    let projects = LocalResource::new(|| async move { all_projects().await.unwrap_or_default() });
    Effect::new(move |_| {
        update_counter.track();
        projects.refetch();
    });

    let projects = Signal::derive(move || {
        let mut projects = projects.get().as_deref().cloned().unwrap_or_default();
        let project_filter = project_filter.get();
        projects
            .retain(|p| p.name.contains(&project_filter) || p.repo_url.contains(&project_filter));
        projects
    });

    view! {
        <section class="w-full h-full flex flex-col">
            <div class="flex-row items-center justify-between space-y-3 sm:flex sm:space-y-0 sm:space-x-4">
                <div>
                    <h5 class="mr-3 text-2xl font-semibold">
                        All Projects
                    </h5>
                    <p class="text-gray-700">Manage all your existing projects or add a new one</p>
                </div>
            </div>
            <div class="flex flex-row items-center justify-between py-4 space-x-4">
                <div class="w-1/2">
                    <form class="flex items-center">
                        <label for="simple-search" class="sr-only">Filter Projects</label>
                        <div class="relative w-full">
                        <div class="absolute inset-y-0 left-0 flex items-center pl-3 pointer-events-none">
                            <svg aria-hidden="true" class="w-5 h-5 text-gray-500" fill="currentColor" viewbox="0 0 20 20" xmlns="http://www.w3.org/2000/svg">
                            <path fill-rule="evenodd" d="M8 4a4 4 0 100 8 4 4 0 000-8zM2 8a6 6 0 1110.89 3.476l4.817 4.817a1 1 0 01-1.414 1.414l-4.816-4.816A6 6 0 012 8z" clip-rule="evenodd" />
                            </svg>
                        </div>
                        <input
                            prop:value={move || project_filter.get()}
                            on:input=move |ev| { project_filter.set(event_target_value(&ev)); }
                            type="text"
                            class="bg-white block w-full p-2 pl-10 text-sm text-gray-900 border border-gray-300 rounded-lg bg-gray-50 focus:ring-blue-500 focus:border-blue-500"
                            placeholder="Filter Projects"
                        />
                        </div>
                    </form>
                </div>
                <button
                    type="button"
                    class="flex items-center justify-center px-4 py-2 text-sm font-medium text-white rounded-lg bg-blue-700 hover:bg-blue-800 focus:ring-4 focus:ring-blue-300 focus:outline-none"
                    on:click=move |_| new_project_modal_hidden.set(false)
                >
                    New Project
                </button>
            </div>

            <div class="relative w-full basis-0 grow">
                <div class="w-full h-full flex flex-col py-4 gap-y-8 overflow-y-auto">
                    <For
                        each=move || projects.get()
                        key=|p| p.clone()
                        children=move |project| {
                            view! {
                                <ProjectItem project update_counter new_workspace_modal_hidden create_workspace_project />
                            }
                        }
                    />
                </div>
            </div>

            <NewProjectView new_project_modal_hidden />
            <NewWorkspaceModal modal_hidden=new_workspace_modal_hidden project_info=create_workspace_project />
        </section>
    }
}

async fn project_prebuilds(id: &str) -> Result<Vec<ProjectPrebuild>> {
    let org = get_current_org()?;
    let resp = Request::get(&format!(
        "/api/v1/organizations/{}/projects/{id}/prebuilds",
        org.id
    ))
    .send()
    .await?;
    let prebuilds: Vec<ProjectPrebuild> = resp.json().await?;
    Ok(prebuilds)
}

#[allow(clippy::type_complexity)]
fn project_prebuilds_signal(
    id: Signal<String, LocalStorage>,
    prebuild_filter: RwSignal<String, LocalStorage>,
    create_workspace_project: RwSignal<Option<CreateWorkspaceProjectInfo>, LocalStorage>,
) -> (
    RwSignal<i32, LocalStorage>,
    Signal<Vec<ProjectPrebuild>, LocalStorage>,
    Signal<HashMap<String, ProjectPrebuild>, LocalStorage>,
) {
    let prebuilds_counter = RwSignal::new_local(0);
    let prebuilds = LocalResource::new(move || async move { project_prebuilds(&id.get()).await });
    Effect::new(move |_| {
        prebuilds_counter.track();
        prebuilds.refetch();
    });

    let prebuilds = Signal::derive(move || {
        prebuilds
            .with(|p| p.as_ref().map(|p| p.as_ref().ok().cloned()))
            .flatten()
            .unwrap_or_default()
    });

    let prebuilds_map = Signal::derive_local(move || {
        let mut prebuilds = prebuilds.get();
        prebuilds.reverse();
        prebuilds
            .into_iter()
            .map(|prebuild| (prebuild.branch.clone(), prebuild))
            .collect::<HashMap<String, _>>()
    });

    Effect::new(move |_| {
        prebuilds.track();
        create_workspace_project.update(|project| {
            if let Some(project) = project.as_mut() {
                project.prebuilds = prebuilds_map.get();
            } else {
                *project = Some(CreateWorkspaceProjectInfo {
                    project: ProjectInfo {
                        id: Uuid::new_v4(),
                        name: "".to_string(),
                        repo_url: "".to_string(),
                        repo_name: "".to_string(),
                        machine_type: Uuid::from_u128(0),
                        created_at: Default::default(),
                    },
                    branches: Vec::new(),
                    prebuilds: prebuilds_map.get(),
                    branch: None,
                });
            }
        });
    });

    let prebuilds = Signal::derive_local(move || {
        let mut prebuilds = prebuilds.get();
        let prebuild_filter = prebuild_filter.get();
        prebuilds
            .retain(|p| p.branch.contains(&prebuild_filter) || p.commit.contains(&prebuild_filter));
        prebuilds
    });

    (prebuilds_counter, prebuilds, prebuilds_map)
}

async fn project_branches(id: &str) -> Result<Vec<GitBranch>> {
    let org = get_current_org()?;
    let resp = Request::get(&format!(
        "/api/v1/organizations/{}/projects/{id}/branches",
        org.id
    ))
    .send()
    .await?;
    let branches: Vec<GitBranch> = resp.json().await?;
    Ok(branches)
}

fn project_branches_signal(
    id: Signal<String, LocalStorage>,
    branch_filter: RwSignal<String, LocalStorage>,
    create_workspace_project: RwSignal<Option<CreateWorkspaceProjectInfo>, LocalStorage>,
) -> Signal<Vec<GitBranch>, LocalStorage> {
    let branches = LocalResource::new(move || async move { project_branches(&id.get()).await });

    Effect::new(move |_| {
        branches.with(|branches| {
            if let Some(Ok(branches)) = branches.as_deref() {
                create_workspace_project.update(|project| {
                    if let Some(project) = project.as_mut() {
                        project.branches = branches.to_owned();
                    } else {
                        *project = Some(CreateWorkspaceProjectInfo {
                            project: ProjectInfo {
                                id: Uuid::new_v4(),
                                name: "".to_string(),
                                repo_url: "".to_string(),
                                repo_name: "".to_string(),
                                machine_type: Uuid::from_u128(0),
                                created_at: Default::default(),
                            },
                            branches: branches.to_owned(),
                            prebuilds: HashMap::new(),
                            branch: None,
                        });
                    }
                })
            }
        });
    });

    let branches = Signal::derive_local(move || {
        let mut branches = branches
            .with(|branches| {
                branches
                    .as_ref()
                    .map(|branches| branches.as_ref().ok().cloned())
            })
            .flatten()
            .unwrap_or_default();
        let branch_filter = branch_filter.get();
        branches.retain(|b| {
            b.name.contains(&branch_filter)
                || b.commit.contains(&branch_filter)
                || b.summary.contains(&branch_filter)
        });
        branches
    });

    branches
}

async fn get_project(id: Uuid) -> Result<ProjectInfo, ErrorResponse> {
    let org = get_current_org()?;
    let resp = Request::get(&format!("/api/v1/organizations/{}/projects/{id}", org.id))
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

    let project: ProjectInfo = resp.json().await?;

    Ok(project)
}

async fn create_project_prebuild(
    project: String,
    branch: String,
    prebuilds_counter: RwSignal<i32, LocalStorage>,
) -> Result<()> {
    let org = get_current_org()?;

    let resp = Request::post(&format!(
        "/api/v1/organizations/{}/projects/{project}/prebuilds",
        org.id,
    ))
    .json(&NewProjectPrebuild { branch })?
    .send()
    .await?;
    if resp.status() != 200 {
        return Err(anyhow!("can't create project prebuild"));
    }
    prebuilds_counter.update(|c| *c += 1);

    Ok(())
}

#[component]
pub fn ProjectBranchControl(
    project: String,
    branch: String,
    prebuild: Signal<Option<ProjectPrebuild>, LocalStorage>,
    prebuilds_counter: RwSignal<i32, LocalStorage>,
    new_workspace_modal_hidden: RwSignal<bool, LocalStorage>,
    create_workspace_project: RwSignal<Option<CreateWorkspaceProjectInfo>, LocalStorage>,
) -> impl IntoView {
    let hidden = RwSignal::new_local(true);
    let toggle_dropdown = move |_| {
        if hidden.get_untracked() {
            hidden.set(false);
        } else {
            hidden.set(true);
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
                if !has_focus && !hidden.get_untracked() {
                    hidden.set(true);
                }
            },
            Duration::from_secs(0),
        );
    };

    let create_prebuild = {
        let branch = branch.clone();
        let project = project.clone();
        move |_| {
            hidden.set(true);
            let project = project.clone();
            let branch = branch.clone();
            Action::new_local(move |_| {
                let project = project.clone();
                let branch = branch.clone();
                create_project_prebuild(project, branch, prebuilds_counter)
            })
            .dispatch(());
        }
    };

    let delete_modal_hidden = RwSignal::new_local(true);
    let delete_action = {
        let project = project.clone();
        Action::new_local(move |_| {
            let prebuild = prebuild.get_untracked().unwrap();
            let id = prebuild.id;
            delete_project_prebuild(project.clone(), id, prebuilds_counter, delete_modal_hidden)
        })
    };

    let handle_create_workspace = {
        let branch = branch.clone();
        move |_| {
            create_workspace_project.update(|project| {
                if let Some(project) = project.as_mut() {
                    let branch = project.branches.iter().find(|b| b.name == branch).cloned();
                    project.branch = branch;
                }
            });
            new_workspace_modal_hidden.set(false);
        }
    };

    view! {
        <div class="flex flex-row items-center py-1">
            <button
                class="px-4 py-2 text-sm font-medium text-white rounded-lg bg-green-700 hover:bg-green-800 focus:ring-4 focus:ring-green-300 focus:outline-none"
                on:click=handle_create_workspace
            >
                New Workspace
            </button>
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
                    class="absolute pt-2 z-10 right-0 divide-y divide-gray-100"
                    class:hidden=move || hidden.get()
                >
                    <ul class="py-2 text-sm text-gray-700 bg-white rounded-lg border shadow w-44">
                        <li
                            class:hidden=move || prebuild.with(|p| p.is_some())
                        >
                            <a
                                href="#"
                                class="block px-4 py-2 hover:bg-gray-100"
                                on:click=create_prebuild
                            >Create Prebuild</a>
                        </li>
                        <li
                            class:hidden=move || prebuild.with(|p| p.is_none() || p.as_ref().map(|p| p.status == PrebuildStatus::Building).unwrap_or(false) )
                        >
                            <a
                                href="#"
                                class="block px-4 py-2 hover:bg-gray-100"
                                on:click=move |_| delete_modal_hidden.set(false)
                            >Delete Prebuild</a>
                        </li>
                    </ul>
                </div>
            </div>
        </div>
        <DeletionModal resource=format!("prebuild for {}", branch.clone()) modal_hidden=delete_modal_hidden delete_action />
    }
}

#[component]
pub fn ProjectDetails() -> impl IntoView {
    let params = use_params_map();
    let id =
        params.with_untracked(|params| params.get("id").and_then(|id| Uuid::from_str(&id).ok()));
    let Some(id) = id else {
        return ().into_any();
    };

    let delete_modal_hidden = RwSignal::new_local(true);
    let new_workspace_modal_hidden = RwSignal::new_local(true);
    let create_workspace_project: RwSignal<Option<CreateWorkspaceProjectInfo>, LocalStorage> =
        RwSignal::new_local(None);
    let delete_action = Action::new_local(move |_| delete_project(id, delete_modal_hidden, None));

    let project_update_counter = RwSignal::new_local(0);
    let project_info = LocalResource::new(move || async move { get_project(id).await });
    Effect::new(move |_| {
        project_update_counter.track();
        project_info.refetch();
    });
    let project_info = Signal::derive(move || {
        project_info.with(|info| {
            info.as_ref()
                .map(|info| info.as_ref().ok().cloned())
                .clone()
                .flatten()
        })
    });

    Effect::new(move |_| {
        project_info.with(|info| {
            if let Some(info) = info {
                create_workspace_project.update(|project| {
                    if let Some(project) = project.as_mut() {
                        project.project = info.to_owned();
                    } else {
                        *project = Some(CreateWorkspaceProjectInfo {
                            project: info.to_owned(),
                            branches: Vec::new(),
                            prebuilds: HashMap::new(),
                            branch: None,
                        });
                    }
                })
            }
        });
    });

    view! {
        <a href="/projects" class="text-sm inline-flex items-center text-gray-700">
            <svg class="w-2 h-2 mr-2" aria-hidden="true" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 14 10">
            <path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 5H1m0 0 4 4M1 5l4-4"/>
            </svg>
            Back to Project List
        </a>
        <div>
            {
                move || if let Some(info) = project_info.get() {
                    view! {
                        <ProjectInfoView info delete_modal_hidden new_workspace_modal_hidden create_workspace_project />
                    }.into_any()
                } else {
                    ().into_any()
                }
            }
            <ProjectDetailsTabView project_id=id project_update_counter new_workspace_modal_hidden create_workspace_project />
            {
                move || if let Some(info) = project_info.get() {
                    view! {
                        <DeletionModal resource=info.name modal_hidden=delete_modal_hidden delete_action />
                    }.into_any()
                } else {
                    ().into_any()
                }
            }
            <NewWorkspaceModal modal_hidden=new_workspace_modal_hidden project_info=create_workspace_project />
        </div>
    }.into_any()
}

#[component]
fn ProjectInfoView(
    info: ProjectInfo,
    delete_modal_hidden: RwSignal<bool, LocalStorage>,
    new_workspace_modal_hidden: RwSignal<bool, LocalStorage>,
    create_workspace_project: RwSignal<Option<CreateWorkspaceProjectInfo>, LocalStorage>,
) -> impl IntoView {
    let cluster_info = get_cluster_info();
    let machine_types = RwSignal::new_local(cluster_info.get_untracked().map(|i| i.machine_types));
    view! {
        <h5 class="text-semibold text-2xl mt-4">
            { info.name.clone() }
        </h5>
        <a href={ info.repo_url.clone() } target="_blank" class="inline-flex border rounded-lg mt-8">
            <img src={ repo_img(&info.repo_url) } class="object-contain object-left h-40" />
        </a>
        <span class="flex flex-row items-center text-sm mt-4">
            <a href={ info.repo_url.clone() } target="_blank" class="inline-flex items-center">
                <svg class="w-3 h-3 mr-2" aria-hidden="true" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 14 20">
                    <path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 5v10M3 5a2 2 0 1 0 0-4 2 2 0 0 0 0 4Zm0 10a2 2 0 1 0 0 4 2 2 0 0 0 0-4Zm6-3.976-2-.01A4.015 4.015 0 0 1 3 7m10 4a2 2 0 1 1-4 0 2 2 0 0 1 4 0Z"/>
                </svg>
                <span class="text-gray-500 w-36">{"Repository URL"}</span>
                <span>{ info.repo_url.clone() }</span>
            </a>
        </span>
        <span class="mt-2 text-sm flex flex-row items-center rounded me-2">
            <svg class="w-2.5 h-2.5 mr-2" xmlns="http://www.w3.org/2000/svg" width="16" height="16" fill="currentColor" viewBox="0 0 16 16">
                <path d="M14 10a1 1 0 0 1 1 1v1a1 1 0 0 1-1 1H2a1 1 0 0 1-1-1v-1a1 1 0 0 1 1-1zM2 9a2 2 0 0 0-2 2v1a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-1a2 2 0 0 0-2-2z"/>
                <path d="M5 11.5a.5.5 0 1 1-1 0 .5.5 0 0 1 1 0m-2 0a.5.5 0 1 1-1 0 .5.5 0 0 1 1 0M14 3a1 1 0 0 1 1 1v1a1 1 0 0 1-1 1H2a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1zM2 2a2 2 0 0 0-2 2v1a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V4a2 2 0 0 0-2-2z"/>
                <path d="M5 4.5a.5.5 0 1 1-1 0 .5.5 0 0 1 1 0m-2 0a.5.5 0 1 1-1 0 .5.5 0 0 1 1 0"/>
            </svg>
            <span class="text-gray-500 w-36">{"Machine Type"}</span>
            {
                move || machine_types.get()
                    .and_then(|m|
                        m
                            .into_iter()
                            .find(|m| m.id == info.machine_type)
                    )
                    .map(|machine_type| format!("{} - {} vCPUs, {}GB memory, {}GB disk",machine_type.name, machine_type.cpu, machine_type.memory, machine_type.disk))
            }
        </span>
        <span class="mt-2 text-sm inline-flex items-center rounded me-2">
            <svg class="w-2.5 h-2.5 mr-2" aria-hidden="true" xmlns="http://www.w3.org/2000/svg" fill="currentColor" viewBox="0 0 20 20">
            <path d="M10 0a10 10 0 1 0 10 10A10.011 10.011 0 0 0 10 0Zm3.982 13.982a1 1 0 0 1-1.414 0l-3.274-3.274A1.012 1.012 0 0 1 9 10V6a1 1 0 0 1 2 0v3.586l2.982 2.982a1 1 0 0 1 0 1.414Z"/>
            </svg>
            <span class="text-gray-500 w-36">{"Created"}</span>
            <DatetimeModal time=info.created_at />
        </span>
        <div class="mt-4">
            <ProjectControl project_info=None delete_modal_hidden project_id=info.id align_right=false new_workspace_modal_hidden create_workspace_project />
        </div>
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum TabKind {
    Branch,
    Prebuild,
    Env,
    Setting,
}

#[component]
fn ProjectDetailsTabView(
    project_id: Uuid,
    project_update_counter: RwSignal<i32, LocalStorage>,
    new_workspace_modal_hidden: RwSignal<bool, LocalStorage>,
    create_workspace_project: RwSignal<Option<CreateWorkspaceProjectInfo>, LocalStorage>,
) -> impl IntoView {
    let tab_kind = RwSignal::new_local(TabKind::Branch);
    let active_class =
        "inline-block p-4 text-blue-600 border-b-2 border-blue-600 rounded-t-lg active";
    let inactive_class = "inline-block p-4 border-b-2 border-transparent rounded-t-lg hover:text-gray-600 hover:border-gray-300";
    let change_tab = move |kind: TabKind| {
        tab_kind.set(kind);
    };

    let prebuild_filter = RwSignal::new_local(String::new());
    let (prebuilds_counter, prebuilds, prebuilds_map) = project_prebuilds_signal(
        Signal::derive_local(move || project_id.to_string()),
        prebuild_filter,
        create_workspace_project,
    );

    view! {
        <div class="mt-8 text-sm font-medium text-center text-gray-500 border-b border-gray-200">
            <ul class="flex flex-wrap -mb-px">
                <li class="me-2">
                    <a
                        href="#"
                        class={ move || if tab_kind.get() == TabKind::Branch {active_class} else {inactive_class} }
                        on:click=move |_| change_tab(TabKind::Branch)
                    >Branches</a>
                </li>
                <li class="me-2">
                    <a
                        href="#"
                        class={ move || if tab_kind.get() == TabKind::Prebuild {active_class} else {inactive_class} }
                        on:click=move |_| change_tab(TabKind::Prebuild)
                    >Prebuilds</a>
                </li>
                <li class="me-2">
                    <a
                        href="#"
                        class={ move || if tab_kind.get() == TabKind::Env {active_class} else {inactive_class} }
                        on:click=move |_| change_tab(TabKind::Env)
                    >Environment</a>
                </li>
                <li class="me-2">
                    <a
                        href="#"
                        class={ move || if tab_kind.get() == TabKind::Setting {active_class} else {inactive_class} }
                        on:click=move |_| change_tab(TabKind::Setting)
                    >Settings</a>
                </li>
            </ul>
        </div>
        <ProjectBranchesView tab_kind project_id prebuilds=prebuilds_map prebuilds_counter new_workspace_modal_hidden create_workspace_project />
        <ProjectPrebuildsView tab_kind project_id prebuilds prebuilds_counter prebuild_filter new_workspace_modal_hidden create_workspace_project />
        <ProjectEnvView tab_kind project_id />
        <ProjectSettingsView tab_kind project_id project_update_counter />
    }
}

#[component]
fn ProjectBranchesView(
    tab_kind: RwSignal<TabKind, LocalStorage>,
    project_id: Uuid,
    prebuilds: Signal<HashMap<String, ProjectPrebuild>, LocalStorage>,
    prebuilds_counter: RwSignal<i32, LocalStorage>,
    new_workspace_modal_hidden: RwSignal<bool, LocalStorage>,
    create_workspace_project: RwSignal<Option<CreateWorkspaceProjectInfo>, LocalStorage>,
) -> impl IntoView {
    let branch_filter = RwSignal::new_local(String::new());
    let branches = project_branches_signal(
        Signal::derive_local(move || project_id.to_string()),
        branch_filter,
        create_workspace_project,
    );
    view! {
        <div
            class="mb-4 text-gray-600"
            class:hidden=move || tab_kind.get() != TabKind::Branch
        >
            <div class="w-full md:w-1/2 p-4">
                <label for="simple-search" class="sr-only">Search</label>
                <div class="relative w-full">
                    <div class="absolute inset-y-0 left-0 flex items-center pl-3 pointer-events-none">
                        <svg aria-hidden="true" class="w-5 h-5 text-gray-500" fill="currentColor" viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg">
                            <path fill-rule="evenodd" d="M8 4a4 4 0 100 8 4 4 0 000-8zM2 8a6 6 0 1110.89 3.476l4.817 4.817a1 1 0 01-1.414 1.414l-4.816-4.816A6 6 0 012 8z" clip-rule="evenodd"></path>
                        </svg>
                    </div>
                    <input
                        prop:value={move || branch_filter.get()}
                        on:input=move |ev| { branch_filter.set(event_target_value(&ev)); }
                        type="text"
                        id="simple-search"
                        class="bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-primary-500 focus:border-primary-500 block w-full pl-10 p-2"
                        placeholder="Search" required=""
                    />
                </div>
            </div>
            <div class="flex items-center w-full px-4 py-2 text-gray-900 bg-gray-50">
                <span class="w-1/3 truncate pr-2">Branch</span>
                <span class="w-1/3 truncate">Commit</span>
                <span class="w-1/3 truncate pl-2 flex flex-row items-center">
                    <div class="w-1/3 flex flex-row justify-center">
                        <span>Prebuild</span>
                    </div>
                    <span class="w-2/3">
                    </span>
                </span>
            </div>
            <div class="overflow-y-auto max-h-screen pb-20">
                <For
                    each=move || branches.get().into_iter().enumerate()
                    key=|(_, b)| b.name.clone()
                    children=move |(i,branch)| {
                        let prebuild = {
                            let branch_name = branch.name.clone();
                            Signal::derive_local(move || {
                                prebuilds.with(|prebuilds| {
                                    prebuilds.get(&branch_name).cloned()
                                })
                            })
                        };
                        view! {
                            <div
                                class="flex items-center w-full px-4 py-2"
                                class=("border-t", move || i > 0)
                            >
                                <span class="w-1/3 truncate pr-2 text-gray-900">{branch.name.clone()}</span>
                                <div class="w-1/3 flex flex-col">
                                    <div class="truncate">{branch.summary}</div>
                                    <div class="truncate text-sm mt-1">
                                        <span class="mr-2">{branch.time.to_string()}</span>
                                        <span>{branch.commit[..7].to_string()}</span>
                                    </div>
                                </div>
                                <span class="w-1/3 pl-2 flex flex-row items-center">
                                    <div class="w-1/3 flex flex-row justify-center">
                                        <span>{move ||
                                            prebuild.with(|prebuild| {
                                                prebuild.as_ref().map(|prebuild| {
                                                    if prebuild.commit == branch.commit {
                                                        prebuild.status.to_string()
                                                    } else if prebuild.status == PrebuildStatus::Ready {
                                                        "Outdated".to_string()
                                                    } else {
                                                        "".to_string()
                                                    }
                                                })
                                            }).unwrap_or_default()
                                        }</span>
                                    </div>
                                    <span class="w-2/3">
                                        <ProjectBranchControl project=project_id.to_string() branch=branch.name.clone() prebuild prebuilds_counter new_workspace_modal_hidden create_workspace_project />
                                    </span>
                                </span>
                            </div>
                        }
                    }
                />
            </div>
        </div>
    }
}

#[component]
fn ProjectPrebuildsView(
    tab_kind: RwSignal<TabKind, LocalStorage>,
    project_id: Uuid,
    prebuilds: Signal<Vec<ProjectPrebuild>, LocalStorage>,
    prebuilds_counter: RwSignal<i32, LocalStorage>,
    prebuild_filter: RwSignal<String, LocalStorage>,
    new_workspace_modal_hidden: RwSignal<bool, LocalStorage>,
    create_workspace_project: RwSignal<Option<CreateWorkspaceProjectInfo>, LocalStorage>,
) -> impl IntoView {
    view! {
        <div
            class="mb-4 text-gray-600"
            class:hidden=move || tab_kind.get() != TabKind::Prebuild
        >
            <div class="w-full md:w-1/2 p-4">
                <label for="simple-search" class="sr-only">Search</label>
                <div class="relative w-full">
                    <div class="absolute inset-y-0 left-0 flex items-center pl-3 pointer-events-none">
                        <svg aria-hidden="true" class="w-5 h-5 text-gray-500" fill="currentColor" viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg">
                            <path fill-rule="evenodd" d="M8 4a4 4 0 100 8 4 4 0 000-8zM2 8a6 6 0 1110.89 3.476l4.817 4.817a1 1 0 01-1.414 1.414l-4.816-4.816A6 6 0 012 8z" clip-rule="evenodd"></path>
                        </svg>
                    </div>
                    <input
                        type="text"
                        id="simple-search"
                        class="bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-primary-500 focus:border-primary-500 block w-full pl-10 p-2"
                        placeholder="Search" required=""
                        prop:value={move || prebuild_filter.get()}
                        on:input=move |ev| { prebuild_filter.set(event_target_value(&ev)); }
                    />
                </div>
            </div>
            <div class="flex items-center w-full px-4 py-2 text-gray-900 bg-gray-50">
                <div class="w-2/3 flex flex-row items-center">
                    <span class="w-1/3 truncate pr-2">Branch</span>
                    <span class="w-1/3 truncate">Commit</span>
                    <span class="w-1/3 truncate">Created</span>
                </div>
                <span class="w-1/3 truncate pl-2 flex flex-row items-center">
                    <div class="w-1/3 flex flex-row justify-center">
                        <span>Status</span>
                    </div>
                    <span class="w-2/3">
                    </span>
                </span>
            </div>
            <div class="overflow-y-auto max-h-screen pb-20">
                <For
                    each=move || prebuilds.get().into_iter().enumerate()
                    key=|(_, p)| p.id
                    children=move |(i, prebuild)| {
                        view! {
                            <div
                                class="flex items-center w-full px-4 py-2"
                                class=("border-t", move || i > 0)
                            >
                                <div class="w-2/3 flex flex-row items-center">
                                    <span class="w-1/3 truncate pr-2 text-gray-900">
                                        {prebuild.branch.clone()}
                                    </span>
                                    <span class="w-1/3 flex flex-col">
                                        {prebuild.commit[..7].to_string()}
                                    </span>
                                    <span class="w-1/3 flex flex-col">
                                        <DatetimeModal time=prebuild.created_at />
                                    </span>
                                </div>
                                <span class="w-1/3 pl-2 flex flex-row items-center">
                                    <div class="w-1/3 flex flex-row justify-center">
                                        <span>
                                            {prebuild.status.to_string()}
                                        </span>
                                    </div>
                                    <span class="w-2/3">
                                        <ProjectBranchControl project=project_id.to_string() branch=prebuild.branch.clone() prebuild=Signal::derive_local(move || {Some(prebuild.clone())}) prebuilds_counter new_workspace_modal_hidden create_workspace_project />
                                    </span>
                                </span>
                            </div>
                        }
                    }
                />
            </div>
        </div>
    }
}

#[component]
fn ProjectSettingsView(
    tab_kind: RwSignal<TabKind, LocalStorage>,
    project_id: Uuid,
    project_update_counter: RwSignal<i32, LocalStorage>,
) -> impl IntoView {
    view! {
        <div
            class="p-4"
            class:hidden=move || tab_kind.get() != TabKind::Setting
        >
            <SaveMachineTypeView project_id project_update_counter />
        </div>
    }
}

#[component]
fn SaveMachineTypeView(
    project_id: Uuid,
    project_update_counter: RwSignal<i32, LocalStorage>,
) -> impl IntoView {
    let current_machine_type = RwSignal::new_local(None);
    let preferred_machine_type = Signal::derive_local(|| None);

    let save_action = Action::new_local(move |_| async move {
        let machine_type_id = current_machine_type.get_untracked().unwrap_or_else(|| {
            let cluster_info = get_cluster_info();
            cluster_info
                .get_untracked()
                .and_then(|i| i.machine_types.first().map(|m| m.id))
                .unwrap_or_else(|| Uuid::from_u128(0))
        });
        update_project_machine_type(project_id, machine_type_id).await
    });

    let body = view! {
        <MachineTypeView current_machine_type preferred_machine_type />
    };

    view! {
        <div class="w-96">
            <SettingView title="".to_string() action=save_action body update_counter=project_update_counter extra=None />
        </div>
    }
}

#[component]
pub fn MachineTypeView(
    current_machine_type: RwSignal<Option<Uuid>, LocalStorage>,
    preferred_machine_type: Signal<Option<Uuid>, LocalStorage>,
) -> impl IntoView {
    let cluster_info = get_cluster_info();
    let machine_types = RwSignal::new_local(cluster_info.get_untracked().map(|i| i.machine_types));
    let change_machine_type = move |ev: web_sys::Event| {
        let element = ev
            .target()
            .unwrap_throw()
            .unchecked_into::<web_sys::HtmlSelectElement>();
        let index = element.selected_index().max(0) as usize;
        let machine_types = machine_types.get_untracked().unwrap_or_default();
        if let Some(machine_type) = machine_types.get(index) {
            current_machine_type.set(Some(machine_type.id));
        }
    };

    Effect::new(move |_| {
        if let Some(uuid) = preferred_machine_type.get() {
            current_machine_type.set(Some(uuid));
        }
    });

    view! {
        <div>
            <label class="block mb-2 text-sm font-medium text-gray-900">
                Machine Type
            </label>
            <select
                id="select"
                class="bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5"
                on:change=change_machine_type
            >
                <For
                    each=move || machine_types.get().unwrap_or_default()
                    key=|m| m.id
                    children=move |machine_type| {
                        view! {
                            <option
                                id=move || machine_type.id.to_string()
                                data-uuid=move || machine_type.id.to_string()
                                selected=move || Some(machine_type.id) == preferred_machine_type.get()
                            >
                                {format!("{} - {} vCPUs, {}GB memory, {}GB disk",machine_type.name, machine_type.cpu, machine_type.memory, machine_type.disk)}
                            </option>
                        }
                    }
                />
            </select>
        </div>
    }
}

async fn get_env(project_id: Uuid) -> Result<Vec<(String, String)>, ErrorResponse> {
    let org = get_current_org()?;

    let resp = Request::get(&format!(
        "/api/v1/organizations/{}/projects/{project_id}/env",
        org.id,
    ))
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

    let envs: Vec<(String, String)> = resp.json().await?;
    Ok(envs)
}

async fn update_env(project_id: Uuid, envs: Vec<(String, String)>) -> Result<(), ErrorResponse> {
    let org = get_current_org()?;

    let resp = Request::put(&format!(
        "/api/v1/organizations/{}/projects/{project_id}/env",
        org.id,
    ))
    .json(&envs)?
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
fn ProjectEnvView(project_id: Uuid, tab_kind: RwSignal<TabKind, LocalStorage>) -> impl IntoView {
    let envs = RwSignal::new_local(vec![]);

    let update_counter = RwSignal::new_local(0);
    let current_envs =
        LocalResource::new(move || async move { get_env(project_id).await.unwrap_or_default() });
    Effect::new(move |_| {
        update_counter.track();
        current_envs.refetch();
    });

    Effect::new(move |_| {
        let current_envs = current_envs.with(|envs| envs.as_deref().cloned().unwrap_or_default());
        envs.update(|envs| {
            *envs = current_envs
                .into_iter()
                .map(|(name, value)| (RwSignal::new_local(name), RwSignal::new_local(value)))
                .collect();
        });
    });

    let new_variable = move |_| {
        envs.update(|envs| {
            envs.push((
                RwSignal::new_local(String::new()),
                RwSignal::new_local(String::new()),
            ));
        });
    };
    let delete_var = move |i: usize| {
        envs.update(|envs| {
            envs.remove(i);
        })
    };
    let save_action = Action::new_local(move |_| async move {
        let envs = envs.get_untracked();
        let envs: Vec<(String, String)> = envs
            .into_iter()
            .filter_map(|(name, value)| {
                let name = name.get_untracked();
                let value = value.get_untracked();
                if name.trim().is_empty() || value.trim().is_empty() {
                    return None;
                }
                Some((name, value))
            })
            .collect();
        update_env(project_id, envs).await
    });
    let body = view! {
        <For
            each=move || envs.get().into_iter().enumerate()
            key=|env| env.to_owned()
            children=move |(i, (name, value))| {
                view! {
                    <div
                        class="flex flex-row items-center"
                        class=("mt-2", move || i > 0)
                    >
                            <div class="w-48">
                                <input
                                prop:value={move || name.get()}
                                on:input=move |ev| { name.set(event_target_value(&ev)); }
                                class="bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5"
                            />
                        </div>
                        <div class="ml-2 w-96">
                            <input
                                type="password"
                                prop:value={move || value.get()}
                                on:input=move |ev| { value.set(event_target_value(&ev)); }
                                class="bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5"
                            />
                        </div>
                        <div class="ml-2 hover:bg-gray-200 rounded-lg p-2 cursor-pointer"
                            on:click=move |_| delete_var(i)
                        >
                            <svg class="h-4 text-gray-800" aria-hidden="true" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
                                <path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18 18 6m0 12L6 6"/>
                            </svg>
                        </div>
                    </div>
                }
            }
        />
    };
    let extra = view! {
        <button
            type="button"
            class="ml-2 py-2.5 px-5 text-sm font-medium text-gray-900 focus:outline-none bg-white rounded-lg border border-gray-200 hover:bg-gray-100 hover:text-blue-700 focus:z-10 focus:ring-4 focus:ring-gray-100"
            on:click=new_variable
        >
            New Variable
        </button>
    }.into_any();
    view! {
        <div
            class="p-4"
            class:hidden=move || tab_kind.get() != TabKind::Env
        >
            <SettingView title="".to_string() action=save_action body update_counter extra=Some(extra) />
        </div>
    }
}
