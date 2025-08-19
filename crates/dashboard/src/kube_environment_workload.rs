use std::str::FromStr;

use anyhow::{anyhow, Result};
use lapdev_common::{
    console::Organization,
    kube::{
        KubeContainerImage, KubeContainerInfo, KubeEnvVar, KubeEnvironment, KubeEnvironmentWorkload,
    },
};
use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use uuid::Uuid;

use crate::{
    app::{get_hrpc_client, AppConfig},
    component::{
        badge::{Badge, BadgeVariant},
        button::{Button, ButtonVariant},
        card::Card,
        input::Input,
        typography::{H3, H4, P},
    },
    kube_container::EnvironmentWorkloadContainersCard,
    modal::{DatetimeModal, DeleteModal, ErrorResponse},
    organization::get_current_org,
};

#[component]
pub fn KubeEnvironmentWorkload() -> impl IntoView {
    let params = use_params_map();
    let (environment_id, workload_id) = params.with_untracked(|params| {
        let environment_id = params
            .get("environment_id")
            .and_then(|id| Uuid::from_str(id.as_str()).ok())
            .unwrap_or_default();
        let workload_id = params
            .get("workload_id")
            .and_then(|id| Uuid::from_str(id.as_str()).ok())
            .unwrap_or_default();
        (environment_id, workload_id)
    });

    view! {
        <div class="flex flex-col gap-6">
            <div class="flex flex-col gap-2 items-start">
                <H3>Environment Workload Details</H3>
                <P>
                    View and manage details for this environment workload.
                </P>
                <a href="https://docs.lap.dev/">
                    <Badge variant=BadgeVariant::Secondary>
                        Docs <lucide_leptos::ExternalLink />
                    </Badge>
                </a>
            </div>

            <EnvironmentWorkloadDetail environment_id workload_id />
        </div>
    }
}

async fn get_environment_workload_detail(
    org: Signal<Option<Organization>>,
    environment_id: Uuid,
    workload_id: Uuid,
) -> Result<(KubeEnvironment, KubeEnvironmentWorkload)> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = get_hrpc_client();

    // Get environment info
    let environment = client
        .get_kube_environment(org.id, environment_id)
        .await??;

    // Get all workloads and find the specific one
    let workloads = client
        .get_environment_workloads(org.id, environment_id)
        .await??;
    let workload = workloads
        .into_iter()
        .find(|w| w.id == workload_id)
        .ok_or_else(|| anyhow!("Workload not found"))?;

    Ok((environment, workload))
}

async fn delete_environment_workload(
    org: Signal<Option<Organization>>,
    workload_id: Uuid,
    delete_modal_open: RwSignal<bool>,
) -> Result<(), ErrorResponse> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = get_hrpc_client();

    client
        .delete_environment_workload(org.id, workload_id)
        .await??;

    delete_modal_open.set(false);

    Ok(())
}

async fn update_environment_workload_containers(
    org: Signal<Option<Organization>>,
    workload_id: Uuid,
    containers: Vec<KubeContainerInfo>,
    update_counter: RwSignal<usize>,
) -> Result<(), ErrorResponse> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = get_hrpc_client();

    client
        .update_environment_workload(org.id, workload_id, containers)
        .await??;

    update_counter.update(|c| *c += 1);

    Ok(())
}

#[component]
pub fn EnvironmentWorkloadDetail(environment_id: Uuid, workload_id: Uuid) -> impl IntoView {
    let org = get_current_org();
    let is_loading = RwSignal::new(false);
    let delete_modal_open = RwSignal::new(false);
    let update_counter = RwSignal::new(0usize);

    let config = use_context::<AppConfig>().unwrap();

    let workload_result = LocalResource::new(move || {
        update_counter.track(); // Make reactive to updates
        async move {
            is_loading.set(true);
            let result = get_environment_workload_detail(org, environment_id, workload_id)
                .await
                .ok();
            if let Some((env, workload)) = result.as_ref() {
                config
                    .current_page
                    .set(format!("{} / {}", env.name, workload.name));
            }
            is_loading.set(false);
            result
        }
    });

    let workload_info = Signal::derive(move || workload_result.get().flatten());

    let navigate = leptos_router::hooks::use_navigate();
    let delete_action = Action::new_local(move |_| {
        let nav = navigate.clone();
        async move {
            match delete_environment_workload(org, workload_id, delete_modal_open).await {
                Ok(_) => {
                    nav(
                        &format!("/kubernetes/environments/{}", environment_id),
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

            // Workload Information Card
            <Show when=move || workload_info.get().is_some()>
                <EnvironmentWorkloadInfoCard
                    workload_info
                    delete_modal_open
                    delete_action
                />
            </Show>

            // Containers Card
            <Show when=move || workload_info.get().is_some()>
                <EnvironmentWorkloadContainersCard workload_info update_counter />
            </Show>

            // Delete Modal
            {move || {
                if let Some((_, workload)) = workload_info.get() {
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
pub fn EnvironmentWorkloadInfoCard(
    workload_info: Signal<Option<(KubeEnvironment, KubeEnvironmentWorkload)>>,
    delete_modal_open: RwSignal<bool>,
    delete_action: Action<(), Result<(), ErrorResponse>>,
) -> impl IntoView {
    view! {
        <Show when=move || workload_info.get().is_some() fallback=|| view! { <div></div> }>
           {
                let (environment, workload) = workload_info.get().unwrap();
                let workload_name = workload.name.clone();
                let workload_namespace = workload.namespace.clone();
                let workload_namespace2 = workload.namespace.clone();
                let workload_kind = workload.kind.clone();
                let workload_kind2 = workload.kind.clone();
                let created_at_str = workload.created_at.clone();
                let environment_name = environment.name.clone();
                let environment_id = environment.id;
                let environment_cluster_id = environment.cluster_id;
                let environment_cluster_name = environment.cluster_name.clone();

                view! {
                    <Card class="p-6">
                        <div class="flex flex-col gap-6">
                            // Header with actions
                            <div class="flex items-start justify-between">
                                <div class="flex flex-col gap-2">
                                    <H3>{workload_name}</H3>
                                    <div class="flex items-center gap-2">
                                        <Badge variant=BadgeVariant::Secondary>
                                            {workload_namespace}
                                        </Badge>
                                        <Badge variant=BadgeVariant::Outline>
                                            {workload_kind}
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
                            <div class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-4 text-sm">
                                <div class="flex items-center gap-2 text-muted-foreground font-medium">
                                    <div class="[&>svg]:size-4 [&>svg]:shrink-0">
                                        <lucide_leptos::Layers />
                                    </div>
                                    <span>Environment</span>
                                </div>
                                <div>
                                    <a href=format!("/kubernetes/environments/{}", environment_id) class="text-foreground hover:underline">
                                        {environment_name.clone()}
                                    </a>
                                </div>

                                <div class="flex items-center gap-2 text-muted-foreground font-medium">
                                    <div class="[&>svg]:size-4 [&>svg]:shrink-0">
                                        <lucide_leptos::Server />
                                    </div>
                                    <span>Cluster</span>
                                </div>
                                <div class="flex items-center gap-2">
                                    <a href=format!("/kubernetes/clusters/{}", environment_cluster_id) class="text-foreground hover:underline">
                                        {environment_cluster_name}
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
                                        <lucide_leptos::Package />
                                    </div>
                                    <span>Workload Kind</span>
                                </div>
                                <div>
                                    <Badge variant=BadgeVariant::Outline>
                                        {workload_kind2}
                                    </Badge>
                                </div>

                                <div class="flex items-center gap-2 text-muted-foreground font-medium">
                                    <div class="[&>svg]:size-4 [&>svg]:shrink-0">
                                        <lucide_leptos::Calendar />
                                    </div>
                                    <span>Created</span>
                                </div>
                                <div>
                                    <DatetimeModal time=workload.created_at />
                                </div>
                            </div>
                        </div>
                    </Card>
                }
            }
        </Show>
    }
}


#[component]
pub fn EnvironmentWorkloadContainerItem(
    container: KubeContainerInfo,
    index: usize,
) -> impl IntoView {
    let container_name = container.name.clone();
    let container_image = container.image.clone();
    let container_image2 = container.image.clone();
    let original_image = container.original_image.clone();
    let cpu_request = container.cpu_request.clone();
    let cpu_limit = container.cpu_limit.clone();
    let memory_request = container.memory_request.clone();
    let memory_limit = container.memory_limit.clone();
    let env_vars = container.env_vars.clone();
    let image_display = match &container_image {
        KubeContainerImage::FollowOriginal => original_image.clone(),
        KubeContainerImage::Custom(image) => image.clone(),
    };

    view! {
        <Card class="p-4 border-l-4 border-l-primary">
            <div class="flex flex-col gap-4">
                // Container header
                <div class="flex items-center justify-between">
                    <div class="flex items-center gap-3">
                        <div class="rounded-full bg-primary/10 p-2">
                            <lucide_leptos::Box attr:class="h-4 w-4 text-primary" />
                        </div>
                        <div>
                            <H4 class="text-base">{format!("Container {}: {}", index, container_name)}</H4>
                            <P class="text-sm text-muted-foreground truncate max-w-96">
                                {image_display}
                            </P>
                        </div>
                    </div>
                </div>

                // Container details
                <div class="grid grid-cols-1 md:grid-cols-2 gap-4 text-sm">
                    // Image details
                    <div class="space-y-2">
                        <div class="flex items-center gap-2 text-muted-foreground font-medium">
                            <lucide_leptos::Image attr:class="h-4 w-4" />
                            <span>Image Configuration</span>
                        </div>
                        <div class="pl-6 space-y-1">
                            <div class="flex justify-between">
                                <span class="text-muted-foreground">Strategy:</span>
                                <Badge variant=BadgeVariant::Outline class="text-xs">
                                    {match &container_image {
                                        KubeContainerImage::FollowOriginal => "Follow Original",
                                        KubeContainerImage::Custom(_) => "Custom Image",
                                    }}
                                </Badge>
                            </div>
                            <div class="flex flex-col gap-1">
                                <span class="text-muted-foreground">Original:</span>
                                <code class="text-xs bg-muted p-1 rounded font-mono break-all">
                                    {original_image.clone()}
                                </code>
                            </div>
                            {match &container_image2 {
                                KubeContainerImage::Custom(custom_image) => {
                                    view! {
                                        <div class="flex flex-col gap-1">
                                            <span class="text-muted-foreground">Custom:</span>
                                            <code class="text-xs bg-muted p-1 rounded font-mono break-all">
                                                {custom_image.clone()}
                                            </code>
                                        </div>
                                    }.into_any()
                                }
                                _ => view! { <div></div> }.into_any()
                            }}
                        </div>
                    </div>

                    // Resource limits
                    <div class="space-y-2">
                        <div class="flex items-center gap-2 text-muted-foreground font-medium">
                            <lucide_leptos::Gauge attr:class="h-4 w-4" />
                            <span>Resource Configuration</span>
                        </div>
                        <div class="pl-6 space-y-1">
                            <div class="flex justify-between">
                                <span class="text-muted-foreground">CPU Request:</span>
                                <span class="font-mono text-xs">
                                    {cpu_request.unwrap_or_else(|| "Not set".to_string())}
                                </span>
                            </div>
                            <div class="flex justify-between">
                                <span class="text-muted-foreground">CPU Limit:</span>
                                <span class="font-mono text-xs">
                                    {cpu_limit.unwrap_or_else(|| "Not set".to_string())}
                                </span>
                            </div>
                            <div class="flex justify-between">
                                <span class="text-muted-foreground">Memory Request:</span>
                                <span class="font-mono text-xs">
                                    {memory_request.unwrap_or_else(|| "Not set".to_string())}
                                </span>
                            </div>
                            <div class="flex justify-between">
                                <span class="text-muted-foreground">Memory Limit:</span>
                                <span class="font-mono text-xs">
                                    {memory_limit.unwrap_or_else(|| "Not set".to_string())}
                                </span>
                            </div>
                        </div>
                    </div>
                </div>

                // Environment variables
                {if !env_vars.is_empty() {
                    let env_vars_len = env_vars.len();
                    view! {
                        <div class="space-y-2 border-t pt-4">
                            <div class="flex items-center gap-2 text-muted-foreground font-medium">
                                <lucide_leptos::Settings attr:class="h-4 w-4" />
                                <span>Environment Variables ({env_vars_len})</span>
                            </div>
                            <div class="pl-6">
                                <div class="grid gap-2">
                                    {env_vars.into_iter().map(|env_var| {
                                        view! {
                                            <div class="flex items-center justify-between p-2 bg-muted/30 rounded">
                                                <code class="font-mono text-xs font-semibold">{env_var.name}</code>
                                                <code class="font-mono text-xs text-muted-foreground max-w-48 truncate">
                                                    {env_var.value}
                                                </code>
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            </div>
                        </div>
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }}
            </div>
        </Card>
    }
}

#[component]
pub fn EnvironmentContainerEditor(
    workload_id: Uuid,
    container_index: usize,
    container: KubeContainerInfo,
    all_containers: Vec<KubeContainerInfo>,
    update_counter: RwSignal<usize>,
) -> impl IntoView {
    let org = get_current_org();
    let is_editing = RwSignal::new(false);
    let error_message = RwSignal::new(None::<String>);

    // Clone container fields to avoid move conflicts
    let container_image = container.image.clone();
    let container_env_vars = container.env_vars.clone();
    let container_original_image = container.original_image.clone();
    let name = container.name.clone();

    // Additional clones for specific uses
    let container_image_for_edit = container_image.clone();
    let container_env_vars_for_edit = container_env_vars.clone();
    let container_image_for_display = container_image.clone();
    let container_env_vars_for_display = container_env_vars.clone();

    // Create signals for editable fields (no resource limits for environment workloads)
    let image_signal = RwSignal::new(container_image.clone());
    let custom_image_signal = RwSignal::new(match &container_image {
        KubeContainerImage::Custom(img) => img.clone(),
        KubeContainerImage::FollowOriginal => String::new(),
    });
    let env_vars_signal = RwSignal::new(container_env_vars.clone());

    let update_action = Action::new_local(move |containers: &Vec<KubeContainerInfo>| {
        let error_msg = error_message;
        let containers = containers.clone();
        error_msg.set(None);
        async move {
            match update_environment_workload_containers(
                org,
                workload_id,
                containers.clone(),
                update_counter,
            )
            .await
            {
                Ok(_) => {
                    // Success - clear any errors and close editing mode
                    error_msg.set(None);
                    is_editing.set(false);
                    Ok(())
                }
                Err(err) => {
                    // Error - display the error message and keep editing mode open
                    error_msg.set(Some(err.error.clone()));
                    Err(err)
                }
            }
        }
    });

    let save_changes = {
        let container_name = container.name.clone();
        let all_containers_clone = all_containers.clone();
        let original_image = container.original_image.clone();

        Callback::new(move |_| {
            let updated_container = KubeContainerInfo {
                name: container_name.clone(),
                original_image: original_image.clone(),
                image: image_signal.get(),
                cpu_request: None, // Environment workloads don't support resource customization
                cpu_limit: None,
                memory_request: None,
                memory_limit: None,
                env_vars: env_vars_signal
                    .get()
                    .into_iter()
                    .filter(|var| !var.name.trim().is_empty())
                    .collect(),
            };

            let mut updated_containers = all_containers_clone.clone();
            updated_containers[container_index] = updated_container;
            update_action.dispatch(updated_containers);
        })
    };

    view! {
        <div class="border rounded-lg">
            <div class="p-4 border-b bg-muted/20">
                <div class="flex items-center justify-between">
                    <div class="flex items-center gap-3">
                        <div class="rounded-full bg-primary/10 p-2">
                            <lucide_leptos::Box attr:class="h-4 w-4 text-primary" />
                        </div>
                        <div>
                            <H4 class="text-base">{name.clone()}</H4>
                            {if container.is_customized() {
                                view! {
                                    <Badge variant=BadgeVariant::Secondary class="text-xs mt-1">
                                        <lucide_leptos::Settings attr:class="w-3 h-3 mr-1" />
                                        "Customized"
                                    </Badge>
                                }.into_any()
                            } else {
                                ().into_any()
                            }}
                        </div>
                    </div>
                    <Show when=move || !is_editing.get()>
                        <Button
                            variant=ButtonVariant::Outline
                            on:click={
                                let container_image_clone = container_image_for_edit.clone();
                                let container_env_vars_clone = container_env_vars_for_edit.clone();
                                move |_| {
                                    is_editing.set(true);
                                    // Reset fields when starting to edit
                                    image_signal.set(container_image_clone.clone());
                                    custom_image_signal.set(match &container_image_clone {
                                        KubeContainerImage::Custom(img) => img.clone(),
                                        KubeContainerImage::FollowOriginal => String::new(),
                                    });
                                    env_vars_signal.set(container_env_vars_clone.clone());
                                    error_message.set(None);
                                }
                            }
                        >
                            Edit
                        </Button>
                    </Show>

                    <Show when=move || is_editing.get()>
                        <div class="flex gap-2">
                            <Button
                                variant=ButtonVariant::Outline
                                on:click=move |_| {
                                    is_editing.set(false);
                                    error_message.set(None);
                                }
                                disabled=Signal::derive(move || update_action.pending().get())
                            >
                                Cancel
                            </Button>
                            <Button
                                variant=ButtonVariant::Default
                                on:click=move |_| save_changes.run(())
                                disabled=Signal::derive(move || update_action.pending().get())
                            >
                                {move || if update_action.pending().get() { "Saving..." } else { "Save Changes" }}
                            </Button>
                        </div>
                    </Show>
                </div>
            </div>

            // Container configuration (always visible)
            <div class="p-4 space-y-4">
                <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                    // Image Configuration
                    <div class="space-y-3">
                        <div class="font-medium text-sm flex items-center gap-2">
                            <lucide_leptos::Box attr:class="w-4 h-4 text-primary" />
                            Image Configuration
                        </div>
                        <div class="space-y-2">
                            <div class="text-sm">
                                <span class="text-muted-foreground">{"Original: "}</span>
                                <code class="text-xs bg-muted px-1 py-0.5 rounded break-all">
                                    {container_original_image.clone()}
                                </code>
                            </div>

                            <Show when=move || !is_editing.get()>
                                {
                                    let display_image = container_image_for_display.clone();
                                    move || view! {
                                        <div class="text-sm">
                                            {match &display_image {
                                                KubeContainerImage::FollowOriginal => view! {
                                                    <span class="text-muted-foreground">{"Current: "}</span>
                                                    <span class="text-xs text-muted-foreground">{"Following original"}</span>
                                                }.into_any(),
                                                KubeContainerImage::Custom(custom_img) => view! {
                                                    <span class="text-muted-foreground">{"Current: "}</span>
                                                    <code class="text-xs bg-muted px-1 py-0.5 rounded break-all">
                                                        {custom_img.clone()}
                                                    </code>
                                                    <span class="ml-2 text-xs text-muted-foreground">
                                                        {"(Custom)"}
                                                    </span>
                                                }.into_any(),
                                            }}
                                        </div>
                                    }
                                }
                            </Show>

                            <Show when=move || is_editing.get()>
                                <div class="space-y-2">
                                    <div class="flex items-center space-x-2">
                                        <input
                                            type="radio"
                                            name=format!("image_choice_{}", container_index)
                                            prop:checked=move || matches!(image_signal.get(), KubeContainerImage::FollowOriginal)
                                            on:change=move |_| {
                                                image_signal.set(KubeContainerImage::FollowOriginal);
                                            }
                                        />
                                        <label class="text-sm">Follow Original</label>
                                    </div>
                                    <div class="flex items-center space-x-2">
                                        <input
                                            type="radio"
                                            name=format!("image_choice_{}", container_index)
                                            prop:checked=move || matches!(image_signal.get(), KubeContainerImage::Custom(_))
                                            on:change=move |_| {
                                                let custom_img = custom_image_signal.get();
                                                image_signal.set(KubeContainerImage::Custom(custom_img));
                                            }
                                        />
                                        <label class="text-sm">Use Custom Image</label>
                                    </div>
                                    <Show when=move || matches!(image_signal.get(), KubeContainerImage::Custom(_))>
                                        <Input
                                            prop:value=move || custom_image_signal.get()
                                            on:input=move |ev| {
                                                let new_value = event_target_value(&ev);
                                                custom_image_signal.set(new_value.clone());
                                                image_signal.set(KubeContainerImage::Custom(new_value));
                                            }
                                            attr:placeholder="Enter custom image (e.g., ubuntu:22.04)"
                                        />
                                    </Show>
                                </div>
                            </Show>
                        </div>
                    </div>

                    // Environment Variables
                    <div class="space-y-3">
                        <div class="font-medium text-sm flex items-center gap-2">
                            <lucide_leptos::Settings attr:class="w-4 h-4 text-primary" />
                            Environment Variables
                        </div>
                        <Show when=move || !is_editing.get()>
                            <EnvVarsDisplay env_vars=container_env_vars_for_display.clone() />
                        </Show>
                        <Show when=move || is_editing.get()>
                            <EnvVarsEditor env_vars_signal />
                        </Show>
                    </div>
                </div>

                // Resource Configuration (Read-only display)
                <div class="border-t pt-4">
                    <div class="font-medium text-sm mb-3 flex items-center gap-2">
                        <lucide_leptos::Cpu attr:class="w-4 h-4 text-primary" />
                        Resource Configuration
                    </div>
                    <div class="text-xs text-muted-foreground mb-2">
                        Resource limits and requests are managed at the environment level and cannot be customized per container.
                    </div>
                    <div class="grid grid-cols-2 md:grid-cols-4 gap-4 text-sm">
                        <div>
                            <div class="text-muted-foreground text-xs mb-1 flex items-center gap-1">
                                <lucide_leptos::Gauge attr:class="w-3 h-3" />
                                CPU Request
                            </div>
                            <div class="font-mono text-xs bg-muted px-2 py-1 rounded">
                                {container.cpu_request.clone().unwrap_or_else(|| "Not set".to_string())}
                            </div>
                        </div>
                        <div>
                            <div class="text-muted-foreground text-xs mb-1 flex items-center gap-1">
                                <lucide_leptos::Zap attr:class="w-3 h-3" />
                                CPU Limit
                            </div>
                            <div class="font-mono text-xs bg-muted px-2 py-1 rounded">
                                {container.cpu_limit.clone().unwrap_or_else(|| "Not set".to_string())}
                            </div>
                        </div>
                        <div>
                            <div class="text-muted-foreground text-xs mb-1 flex items-center gap-1">
                                <lucide_leptos::Database attr:class="w-3 h-3" />
                                Memory Request
                            </div>
                            <div class="font-mono text-xs bg-muted px-2 py-1 rounded">
                                {container.memory_request.clone().unwrap_or_else(|| "Not set".to_string())}
                            </div>
                        </div>
                        <div>
                            <div class="text-muted-foreground text-xs mb-1 flex items-center gap-1">
                                <lucide_leptos::HardDrive attr:class="w-3 h-3" />
                                Memory Limit
                            </div>
                            <div class="font-mono text-xs bg-muted px-2 py-1 rounded">
                                {container.memory_limit.clone().unwrap_or_else(|| "Not set".to_string())}
                            </div>
                        </div>
                    </div>
                </div>

                // Error message
                <Show when=move || error_message.get().is_some()>
                    <div class="p-3 bg-destructive/10 border border-destructive/20 rounded-md">
                        <div class="text-sm text-destructive">
                            {move || error_message.get().unwrap_or_default()}
                        </div>
                    </div>
                </Show>

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
                {move || {
                    env_vars_signal.with(|env_vars| {
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
                    })
                }}

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
fn EnvVarsDisplay(env_vars: Vec<KubeEnvVar>) -> impl IntoView {
    view! {
        <div class="space-y-1">
            {
                if env_vars.is_empty() {
                    view! { <div class="text-sm text-muted-foreground">No environment variables</div> }.into_any()
                } else {
                    env_vars.into_iter().map(|env_var| {
                        view! {
                            <div class="flex items-center justify-between p-2 bg-muted/30 rounded text-xs">
                                <code class="font-semibold break-all">{env_var.name}</code>
                                <code class="text-muted-foreground truncate max-w-32">{env_var.value}</code>
                            </div>
                        }
                    }).collect::<Vec<_>>().into_any()
                }
            }
        </div>
    }
}
