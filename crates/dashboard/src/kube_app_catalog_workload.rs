use std::str::FromStr;

use anyhow::{anyhow, Result};
use lapdev_api_hrpc::HrpcServiceClient;
use lapdev_common::{
    console::Organization,
    kube::{KubeAppCatalog, KubeAppCatalogWorkload, KubeContainerInfo, KubeEnvVar},
};
use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use uuid::Uuid;

use crate::{
    app::AppConfig,
    component::{
        badge::{Badge, BadgeVariant},
        button::{Button, ButtonVariant},
        card::Card,
        input::Input,
        typography::{H3, H4, P},
    },
    modal::{DeleteModal, ErrorResponse},
    organization::get_current_org,
};

#[component]
pub fn KubeAppCatalogWorkload() -> impl IntoView {
    let params = use_params_map();
    let (catalog_id, workload_id) = params.with_untracked(|params| {
        let catalog_id = params
            .get("catalog_id")
            .and_then(|id| Uuid::from_str(id.as_str()).ok())
            .unwrap_or_default();
        let workload_id = params
            .get("workload_id")
            .and_then(|id| Uuid::from_str(id.as_str()).ok())
            .unwrap_or_default();
        (catalog_id, workload_id)
    });

    view! {
        <div class="flex flex-col gap-6">
            <div class="flex flex-col gap-2 items-start">
                <H3>Workload Details</H3>
                <P>
                    View and manage details for this Kubernetes workload.
                </P>
                <a href="https://docs.lap.dev/">
                    <Badge variant=BadgeVariant::Secondary>
                        Docs <lucide_leptos::ExternalLink />
                    </Badge>
                </a>
            </div>

            <WorkloadDetail catalog_id workload_id />
        </div>
    }
}

async fn get_workload_detail(
    org: Signal<Option<Organization>>,
    catalog_id: Uuid,
    workload_id: Uuid,
) -> Result<(KubeAppCatalog, KubeAppCatalogWorkload)> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());

    // Get catalog info
    let catalog = client.get_app_catalog(org.id, catalog_id).await??;

    // Get all workloads and find the specific one
    let workloads = client
        .get_app_catalog_workloads(org.id, catalog_id)
        .await??;
    let workload = workloads
        .into_iter()
        .find(|w| w.id == workload_id)
        .ok_or_else(|| anyhow!("Workload not found"))?;

    Ok((catalog, workload))
}

async fn update_workload_containers(
    org: Signal<Option<Organization>>,
    workload_id: Uuid,
    containers: Vec<KubeContainerInfo>,
    update_counter: RwSignal<usize>,
) -> Result<(), ErrorResponse> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());

    client
        .update_app_catalog_workload(org.id, workload_id, containers)
        .await??;

    update_counter.update(|c| *c += 1);

    Ok(())
}

async fn delete_workload(
    org: Signal<Option<Organization>>,
    workload_id: Uuid,
    delete_modal_open: RwSignal<bool>,
) -> Result<(), ErrorResponse> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());

    client
        .delete_app_catalog_workload(org.id, workload_id)
        .await??;

    delete_modal_open.set(false);

    Ok(())
}

#[component]
pub fn WorkloadDetail(catalog_id: Uuid, workload_id: Uuid) -> impl IntoView {
    let org = get_current_org();
    let is_loading = RwSignal::new(false);
    let update_counter = RwSignal::new(0usize);
    let delete_modal_open = RwSignal::new(false);

    let config = use_context::<AppConfig>().unwrap();
    let detail_result = LocalResource::new(move || {
        update_counter.track();
        async move {
            is_loading.set(true);
            let result = get_workload_detail(org, catalog_id, workload_id).await.ok();
            if let Some((catalog, workload)) = result.as_ref() {
                config
                    .current_page
                    .set(format!("{} - {}", catalog.name, workload.name));
            }
            is_loading.set(false);
            result
        }
    });

    let catalog_info =
        Signal::derive(move || detail_result.get().and_then(|opt| opt.map(|(c, _)| c)));
    let workload_info =
        Signal::derive(move || detail_result.get().and_then(|opt| opt.map(|(_, w)| w)));

    let navigate = leptos_router::hooks::use_navigate();
    let delete_action = Action::new_local(move |_| {
        let nav = navigate.clone();
        async move {
            match delete_workload(org, workload_id, delete_modal_open).await {
                Ok(_) => {
                    nav(
                        &format!("/kubernetes/catalogs/{}", catalog_id),
                        Default::default(),
                    );
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
    });

    view! {
        <div class="flex flex-col gap-6">
            // Loading State
            <Show when=move || is_loading.get()>
                <div class="flex items-center justify-center py-12">
                    <div class="flex items-center gap-2 text-sm text-muted-foreground">
                        <div class="animate-spin rounded-full h-4 w-4 border-2 border-current border-t-transparent"></div>
                        "Loading workload details..."
                    </div>
                </div>
            </Show>

            // Navigation Breadcrumb
            <Show when=move || catalog_info.get().is_some()>
                <nav class="flex items-center gap-2 text-sm text-muted-foreground">
                    <a href="/kubernetes/catalogs" class="hover:text-foreground">
                        App Catalogs
                    </a>
                    <lucide_leptos::ChevronRight attr:class="h-4 w-4" />
                    {move || {
                        if let Some(catalog) = catalog_info.get() {
                            view! {
                                <a href=format!("/kubernetes/catalogs/{}", catalog.id) class="hover:text-foreground">
                                    {catalog.name}
                                </a>
                            }.into_any()
                        } else {
                            view! { <span>"..."</span> }.into_any()
                        }
                    }}
                    <lucide_leptos::ChevronRight attr:class="h-4 w-4" />
                    <span class="text-foreground">
                        {move || workload_info.get().map(|w| w.name).unwrap_or_default()}
                    </span>
                </nav>
            </Show>

            // Workload Information Card
            <Show when=move || workload_info.get().is_some()>
                <WorkloadInfoCard workload_info catalog_info delete_modal_open delete_action />
            </Show>

            // Containers Section
            <Show when=move || workload_info.get().is_some()>
                <ContainersSection workload_info update_counter />
            </Show>

            // Delete Modal
            {move || {
                if let Some(workload) = workload_info.get() {
                    view! {
                        <DeleteModal
                            resource=workload.name
                            open=delete_modal_open
                            delete_action
                        />
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            }}
        </div>
    }
}

#[component]
pub fn WorkloadInfoCard(
    workload_info: Signal<Option<KubeAppCatalogWorkload>>,
    catalog_info: Signal<Option<KubeAppCatalog>>,
    delete_modal_open: RwSignal<bool>,
    delete_action: Action<(), Result<(), ErrorResponse>>,
) -> impl IntoView {
    view! {
        {move || {
            if let (Some(workload), Some(catalog)) = (workload_info.get(), catalog_info.get()) {
                let workload_name = workload.name.clone();
                let workload_namespace1 = workload.namespace.clone();  // For badge
                let workload_namespace2 = workload.namespace.clone();  // For metadata
                let workload_kind_str1 = workload.kind.to_string();    // For badge
                let workload_kind_str2 = workload.kind.to_string();    // For metadata
                let workload_containers_len = workload.containers.len();
                let catalog_id = catalog.id;
                let catalog_name = catalog.name.clone();
                let catalog_cluster_id = catalog.cluster_id;
                let catalog_cluster_name = catalog.cluster_name.clone();

                view! {
                    <Card class="p-6">
                        <div class="flex flex-col gap-6">
                            // Header with actions
                            <div class="flex items-start justify-between">
                                <div class="flex flex-col gap-2">
                                    <H3>{workload_name.clone()}</H3>
                                    <div class="flex items-center gap-2">
                                        <Badge variant=BadgeVariant::Secondary>
                                            {workload_namespace1}
                                        </Badge>
                                    </div>
                                </div>
                                <Button
                                    variant=ButtonVariant::Destructive
                                    on:click=move |_| delete_modal_open.set(true)
                                >
                                    <lucide_leptos::Trash2 />
                                    Delete Workload
                                </Button>
                            </div>

                            // Metadata grid
                            <div class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-3 text-sm">
                                <div class="flex items-center gap-2 text-muted-foreground font-medium">
                                    <div class="[&>svg]:size-4 [&>svg]:shrink-0">
                                        <lucide_leptos::Package />
                                    </div>
                                    <span>App Catalog</span>
                                </div>
                                <div>
                                    <a href=format!("/kubernetes/catalogs/{}", catalog_id) class="text-foreground hover:underline">
                                        {catalog_name.clone()}
                                    </a>
                                </div>

                                <div class="flex items-center gap-2 text-muted-foreground font-medium">
                                    <div class="[&>svg]:size-4 [&>svg]:shrink-0">
                                        <lucide_leptos::Server />
                                    </div>
                                    <span>Cluster</span>
                                </div>
                                <div>
                                    <a href=format!("/kubernetes/clusters/{}", catalog_cluster_id) class="text-foreground hover:underline">
                                        {catalog_cluster_name}
                                    </a>
                                </div>

                                <div class="flex items-center gap-2 text-muted-foreground font-medium">
                                    <div class="[&>svg]:size-4 [&>svg]:shrink-0">
                                        <lucide_leptos::Hash />
                                    </div>
                                    <span>Namespace</span>
                                </div>
                                <div class="font-mono text-sm">
                                    {workload_namespace2}
                                </div>

                                <div class="flex items-center gap-2 text-muted-foreground font-medium">
                                    <div class="[&>svg]:size-4 [&>svg]:shrink-0">
                                        <lucide_leptos::Layers />
                                    </div>
                                    <span>Kind</span>
                                </div>
                                <div>
                                    {workload_kind_str2}
                                </div>

                                <div class="flex items-center gap-2 text-muted-foreground font-medium">
                                    <div class="[&>svg]:size-4 [&>svg]:shrink-0">
                                        <lucide_leptos::Box />
                                    </div>
                                    <span>Containers</span>
                                </div>
                                <div>
                                    {format!("{} container{}", workload_containers_len, if workload_containers_len == 1 { "" } else { "s" })}
                                </div>
                            </div>
                        </div>
                    </Card>
                }
                .into_any()
            } else {
                view! { <div></div> }.into_any()
            }
        }}
    }
}

#[component]
pub fn ContainersSection(
    workload_info: Signal<Option<KubeAppCatalogWorkload>>,
    update_counter: RwSignal<usize>,
) -> impl IntoView {
    view! {
        {move || {
            if let Some(workload) = workload_info.get() {
                view! {
                    <Card class="p-6">
                        <div class="flex flex-col gap-4">
                            <H4>Containers</H4>
                            <div class="space-y-4">
                                {
                                    workload.containers.iter().enumerate().map(|(index, container)| {
                                        let workload_id = workload.id;
                                        let container_idx = index;
                                        view! {
                                            <DetailedContainerEditor
                                                workload_id
                                                container_index=container_idx
                                                container=container.clone()
                                                all_containers=workload.containers.clone()
                                                update_counter
                                            />
                                        }
                                    }).collect::<Vec<_>>()
                                }
                                {if workload.containers.is_empty() {
                                    view! {
                                        <div class="flex flex-col items-center justify-center py-8 text-center text-muted-foreground">
                                            <lucide_leptos::Box attr:class="h-12 w-12 mb-2 opacity-50" />
                                            <P>No containers configured for this workload</P>
                                        </div>
                                    }.into_any()
                                } else {
                                    view! { <div></div> }.into_any()
                                }}
                            </div>
                        </div>
                    </Card>
                }
                .into_any()
            } else {
                view! { <div></div> }.into_any()
            }
        }}
    }
}

#[component]
pub fn DetailedContainerEditor(
    workload_id: Uuid,
    container_index: usize,
    container: KubeContainerInfo,
    all_containers: Vec<KubeContainerInfo>,
    update_counter: RwSignal<usize>,
) -> impl IntoView {
    let org = get_current_org();
    let is_editing = RwSignal::new(false);

    // Create signals for all editable fields
    let image_signal = RwSignal::new(container.image.clone());
    let cpu_request_signal = RwSignal::new(container.cpu_request.clone().unwrap_or_default());
    let cpu_limit_signal = RwSignal::new(container.cpu_limit.clone().unwrap_or_default());
    let memory_request_signal = RwSignal::new(container.memory_request.clone().unwrap_or_default());
    let memory_limit_signal = RwSignal::new(container.memory_limit.clone().unwrap_or_default());
    let env_vars_signal = RwSignal::new(container.env_vars.clone());

    let name = container.name.clone();

    let update_action = Action::new_local(move |containers: &Vec<KubeContainerInfo>| {
        update_workload_containers(org, workload_id, containers.clone(), update_counter)
    });

    let save_changes = {
        let container_name = container.name.clone();
        let all_containers_clone = all_containers.clone();
        Callback::new(move |_| {
            let updated_container = KubeContainerInfo {
                name: container_name.clone(),
                image: image_signal.get(),
                cpu_request: if cpu_request_signal.get().trim().is_empty() {
                    None
                } else {
                    Some(cpu_request_signal.get().trim().to_string())
                },
                cpu_limit: if cpu_limit_signal.get().trim().is_empty() {
                    None
                } else {
                    Some(cpu_limit_signal.get().trim().to_string())
                },
                memory_request: if memory_request_signal.get().trim().is_empty() {
                    None
                } else {
                    Some(memory_request_signal.get().trim().to_string())
                },
                memory_limit: if memory_limit_signal.get().trim().is_empty() {
                    None
                } else {
                    Some(memory_limit_signal.get().trim().to_string())
                },
                env_vars: env_vars_signal.get(),
            };

            // Update all containers with the modified one
            let mut updated_containers = all_containers_clone.clone();
            updated_containers[container_index] = updated_container;

            update_action.dispatch(updated_containers);
            is_editing.set(false);
        })
    };

    let cancel_changes = {
        let container_image = container.image.clone();
        Callback::new(move |_| {
            // Reset to original values
            image_signal.set(container_image.clone());
            cpu_request_signal.set(container.cpu_request.clone().unwrap_or_default());
            cpu_limit_signal.set(container.cpu_limit.clone().unwrap_or_default());
            memory_request_signal.set(container.memory_request.clone().unwrap_or_default());
            memory_limit_signal.set(container.memory_limit.clone().unwrap_or_default());
            env_vars_signal.set(container.env_vars.clone());
            is_editing.set(false);
        })
    };

    view! {
        <div class="border rounded-lg p-4 bg-card">
            <div class="flex flex-col gap-4">
                // Container header with name and actions
                <div class="flex justify-between items-center">
                    <div class="flex items-center gap-2">
                        <lucide_leptos::Box attr:class="h-5 w-5 text-muted-foreground" />
                        <H4 class="text-lg">{name}</H4>
                    </div>
                    <div class="flex gap-2">
                        <Show when=move || !is_editing.get()>
                            <Button
                                variant=ButtonVariant::Outline
                                on:click=move |_| is_editing.set(true)
                            >
                                <lucide_leptos::Pen attr:class="w-4 h-4" />
                                Edit
                            </Button>
                        </Show>
                        <Show when=move || is_editing.get()>
                            <Button
                                variant=ButtonVariant::Default
                                on:click=move |_| save_changes.run(())
                            >
                                <lucide_leptos::Check attr:class="w-4 h-4" />
                                Save
                            </Button>
                            <Button
                                variant=ButtonVariant::Outline
                                on:click=move |_| cancel_changes.run(())
                            >
                                <lucide_leptos::X attr:class="w-4 h-4" />
                                Cancel
                            </Button>
                        </Show>
                    </div>
                </div>

                // Container Configuration - reorganized into vertical sections
                <div class="flex flex-col gap-6">
                    // 1. Container Image
                    <div class="flex flex-col gap-3">
                        <div class="flex items-center gap-2 text-sm font-medium text-foreground border-b border-border pb-2">
                            <lucide_leptos::Image attr:class="h-4 w-4" />
                            Container Image
                        </div>
                        <Show
                            when=move || is_editing.get()
                            fallback=move || view! {
                                <div class="p-3 bg-muted rounded-md font-mono text-sm break-all">
                                    {move || image_signal.get()}
                                </div>
                            }
                        >
                            <Input
                                prop:value=move || image_signal.get()
                                on:input=move |ev| image_signal.set(event_target_value(&ev))
                                attr:placeholder="Container image (e.g., nginx:1.21, postgres:13)"
                                class="font-mono"
                            />
                        </Show>
                    </div>

                    // 2. Resource Configuration
                    <div class="flex flex-col gap-4">
                        <div class="flex items-center gap-2 text-sm font-medium text-foreground border-b border-border pb-2">
                            <lucide_leptos::Gauge attr:class="h-4 w-4" />
                            Resource Configuration
                        </div>
                        
                        <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
                            // CPU Configuration
                            <div class="flex flex-col gap-3">
                                <div class="flex items-center gap-2 text-xs font-semibold text-foreground">
                                    <lucide_leptos::Cpu attr:class="h-4 w-4" />
                                    CPU
                                </div>
                                <div class="grid grid-cols-2 gap-3">
                                    <div class="flex flex-col gap-2">
                                        <label class="text-xs font-medium text-muted-foreground">Request</label>
                                        <Show
                                            when=move || is_editing.get()
                                            fallback=move || view! {
                                                <div class="p-2 bg-muted rounded text-xs font-mono min-h-[36px] flex items-center">
                                                    {move || if cpu_request_signal.get().is_empty() { "-".to_string() } else { cpu_request_signal.get() }}
                                                </div>
                                            }
                                        >
                                            <Input
                                                prop:value=move || cpu_request_signal.get()
                                                on:input=move |ev| cpu_request_signal.set(event_target_value(&ev))
                                                attr:placeholder="100m"
                                                class="text-xs font-mono"
                                            />
                                        </Show>
                                    </div>
                                    <div class="flex flex-col gap-2">
                                        <label class="text-xs font-medium text-muted-foreground">Limit</label>
                                        <Show
                                            when=move || is_editing.get()
                                            fallback=move || view! {
                                                <div class="p-2 bg-muted rounded text-xs font-mono min-h-[36px] flex items-center">
                                                    {move || if cpu_limit_signal.get().is_empty() { "-".to_string() } else { cpu_limit_signal.get() }}
                                                </div>
                                            }
                                        >
                                            <Input
                                                prop:value=move || cpu_limit_signal.get()
                                                on:input=move |ev| cpu_limit_signal.set(event_target_value(&ev))
                                                attr:placeholder="500m"
                                                class="text-xs font-mono"
                                            />
                                        </Show>
                                    </div>
                                </div>
                            </div>
                            
                            // Memory Configuration
                            <div class="flex flex-col gap-3">
                                <div class="flex items-center gap-2 text-xs font-semibold text-foreground">
                                    <lucide_leptos::MemoryStick attr:class="h-4 w-4" />
                                    Memory
                                </div>
                                <div class="grid grid-cols-2 gap-3">
                                    <div class="flex flex-col gap-2">
                                        <label class="text-xs font-medium text-muted-foreground">Request</label>
                                        <Show
                                            when=move || is_editing.get()
                                            fallback=move || view! {
                                                <div class="p-2 bg-muted rounded text-xs font-mono min-h-[36px] flex items-center">
                                                    {move || if memory_request_signal.get().is_empty() { "-".to_string() } else { memory_request_signal.get() }}
                                                </div>
                                            }
                                        >
                                            <Input
                                                prop:value=move || memory_request_signal.get()
                                                on:input=move |ev| memory_request_signal.set(event_target_value(&ev))
                                                attr:placeholder="128Mi"
                                                class="text-xs font-mono"
                                            />
                                        </Show>
                                    </div>
                                    <div class="flex flex-col gap-2">
                                        <label class="text-xs font-medium text-muted-foreground">Limit</label>
                                        <Show
                                            when=move || is_editing.get()
                                            fallback=move || view! {
                                                <div class="p-2 bg-muted rounded text-xs font-mono min-h-[36px] flex items-center">
                                                    {move || if memory_limit_signal.get().is_empty() { "-".to_string() } else { memory_limit_signal.get() }}
                                                </div>
                                            }
                                        >
                                            <Input
                                                prop:value=move || memory_limit_signal.get()
                                                on:input=move |ev| memory_limit_signal.set(event_target_value(&ev))
                                                attr:placeholder="512Mi"
                                                class="text-xs font-mono"
                                            />
                                        </Show>
                                    </div>
                                </div>
                            </div>
                        </div>
                    </div>

                    // 3. Environment Variables
                    <div class="flex flex-col gap-4">
                        <div class="flex items-center gap-2 text-sm font-medium text-foreground border-b border-border pb-2">
                            <lucide_leptos::Settings attr:class="h-4 w-4" />
                            Environment Variables
                        </div>
                        
                        <Show when=move || is_editing.get()>
                            <EnvVarsEditor env_vars_signal />
                        </Show>
                        
                        <Show when=move || !is_editing.get()>
                            <EnvVarsDisplay env_vars_signal />
                        </Show>
                    </div>
                </div>
            </div>
        </div>
    }
}

#[component]
fn EnvVarsEditor(env_vars_signal: RwSignal<Vec<KubeEnvVar>>) -> impl IntoView {
    let add_env_var = Callback::new(move |_| {
        env_vars_signal.update(|vars| {
            vars.push(KubeEnvVar {
                name: String::new(),
                value: String::new(),
            });
        });
    });

    view! {
        <div class="flex flex-col gap-2">
            <div class="flex items-center justify-between">
                <span class="text-xs font-medium text-muted-foreground">Variables</span>
                <Button
                    variant=ButtonVariant::Outline
                    class="px-2 py-1 h-auto text-xs"
                    on:click=move |_| add_env_var.run(())
                >
                    <lucide_leptos::Plus attr:class="w-3 h-3" />
                    Add Variable
                </Button>
            </div>
            
            <div class="space-y-2">
                {
                    let env_vars = env_vars_signal.get();
                    env_vars.iter().enumerate().map(|(index, env_var)| {
                        let env_var_name = env_var.name.clone();
                        let env_var_value = env_var.value.clone();
                        
                        view! {
                            <div class="flex gap-2 items-center">
                                <Input
                                    prop:value=env_var_name.clone()
                                    on:input=move |ev| {
                                        let new_name = event_target_value(&ev);
                                        env_vars_signal.update(|vars| {
                                            if let Some(var) = vars.get_mut(index) {
                                                var.name = new_name;
                                            }
                                        });
                                    }
                                    attr:placeholder="Variable name"
                                    class="text-xs font-mono flex-1"
                                />
                                <Input
                                    prop:value=env_var_value.clone()
                                    on:input=move |ev| {
                                        let new_value = event_target_value(&ev);
                                        env_vars_signal.update(|vars| {
                                            if let Some(var) = vars.get_mut(index) {
                                                var.value = new_value;
                                            }
                                        });
                                    }
                                    attr:placeholder="Variable value"
                                    class="text-xs font-mono flex-1"
                                />
                                <Button
                                    variant=ButtonVariant::Ghost
                                    class="px-2 py-1 h-auto text-red-600 hover:text-red-700"
                                    on:click=move |_| {
                                        env_vars_signal.update(|vars| {
                                            if index < vars.len() {
                                                vars.remove(index);
                                            }
                                        });
                                    }
                                >
                                    <lucide_leptos::Trash2 attr:class="w-3 h-3" />
                                </Button>
                            </div>
                        }
                    }).collect::<Vec<_>>()
                }
                
                {move || {
                    let vars = env_vars_signal.get();
                    if vars.is_empty() {
                        view! {
                            <div class="text-xs text-muted-foreground italic py-2">
                                No environment variables configured
                            </div>
                        }.into_any()
                    } else {
                        view! { <div></div> }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

#[component]  
fn EnvVarsDisplay(env_vars_signal: RwSignal<Vec<KubeEnvVar>>) -> impl IntoView {
    view! {
        <div class="space-y-1">
            {
                let env_vars = env_vars_signal.get();
                env_vars.iter().map(|env_var| {
                    let env_var_name = env_var.name.clone();
                    let env_var_value = env_var.value.clone();
                    
                    view! {
                        <div class="flex gap-2 text-xs">
                            <span class="font-mono font-medium min-w-0 flex-shrink-0">{env_var_name}</span>
                            <span class="text-muted-foreground">=</span>
                            <span class="font-mono text-muted-foreground truncate min-w-0 flex-1">{env_var_value}</span>
                        </div>
                    }
                }).collect::<Vec<_>>()
            }
            
            {move || {
                let vars = env_vars_signal.get();
                if vars.is_empty() {
                    view! {
                        <div class="text-xs text-muted-foreground italic py-1">
                            No environment variables configured
                        </div>
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            }}
        </div>
    }
}
