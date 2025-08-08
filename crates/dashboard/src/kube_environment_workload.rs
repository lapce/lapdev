use std::str::FromStr;

use anyhow::{anyhow, Result};
use lapdev_api_hrpc::HrpcServiceClient;
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
    let client = HrpcServiceClient::new("/api/rpc".to_string());

    // Get environment info
    let environment = client.get_kube_environment(org.id, environment_id).await??;

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
    let client = HrpcServiceClient::new("/api/rpc".to_string());

    client
        .delete_environment_workload(org.id, workload_id)
        .await??;

    delete_modal_open.set(false);

    Ok(())
}

#[component]
pub fn EnvironmentWorkloadDetail(environment_id: Uuid, workload_id: Uuid) -> impl IntoView {
    let org = get_current_org();
    let is_loading = RwSignal::new(false);
    let delete_modal_open = RwSignal::new(false);

    let config = use_context::<AppConfig>().unwrap();

    let workload_result = LocalResource::new(move || async move {
        is_loading.set(true);
        let result = get_environment_workload_detail(org, environment_id, workload_id).await.ok();
        if let Some((env, workload)) = result.as_ref() {
            config.current_page.set(format!("{} / {}", env.name, workload.name));
        }
        is_loading.set(false);
        result
    });

    let workload_info = Signal::derive(move || workload_result.get().flatten());

    let navigate = leptos_router::hooks::use_navigate();
    let delete_action = Action::new_local(move |_| {
        let nav = navigate.clone();
        async move {
            match delete_environment_workload(org, workload_id, delete_modal_open).await {
                Ok(_) => {
                    nav(&format!("/kubernetes/environments/{}", environment_id), Default::default());
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
                <EnvironmentWorkloadContainersCard workload_info />
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
                                    <span class="text-sm text-muted-foreground">{created_at_str}</span>
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
pub fn EnvironmentWorkloadContainersCard(
    workload_info: Signal<Option<(KubeEnvironment, KubeEnvironmentWorkload)>>,
) -> impl IntoView {
    view! {
        <Show when=move || workload_info.get().is_some() fallback=|| view! { <div></div> }>
            {move || {
                let (_, workload) = workload_info.get().unwrap();
                let containers = workload.containers.clone();

                view! {
                    <Card class="p-6">
                        <div class="flex flex-col gap-6">
                            <div class="flex items-center justify-between">
                                <H4>Container Configuration</H4>
                                <Badge variant=BadgeVariant::Secondary>
                                    {format!("{} container{}", containers.len(), if containers.len() != 1 { "s" } else { "" })}
                                </Badge>
                            </div>

                            {if containers.is_empty() {
                                view! {
                                    <div class="flex flex-col items-center justify-center py-12 text-center">
                                        <div class="rounded-full bg-muted p-3 mb-4">
                                            <lucide_leptos::Package />
                                        </div>
                                        <H4 class="mb-2">No Containers</H4>
                                        <P class="text-muted-foreground mb-4 max-w-sm">
                                            "This workload doesn't have any container configurations."
                                        </P>
                                    </div>
                                }.into_any()
                            } else {
                                view! {
                                    <div class="grid gap-4">
                                        {containers.into_iter().enumerate().map(|(index, container)| {
                                            view! {
                                                <EnvironmentWorkloadContainerItem
                                                    container=container
                                                    index=index + 1
                                                />
                                            }
                                        }).collect::<Vec<_>>()}
                                    </div>
                                }.into_any()
                            }}
                        </div>
                    </Card>
                }
            }}
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