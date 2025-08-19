use std::str::FromStr;

use anyhow::{anyhow, Result};
use lapdev_common::{
    console::Organization,
    kube::{
        KubeContainerInfo, KubeEnvironment, KubeEnvironmentWorkload,
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
        typography::{H3, P},
    },
    kube_container::{ContainerEditor, ContainerEditorConfig, ContainersCard},
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
                {move || {
                    if let Some((_, workload)) = workload_info.get() {
                        let workload_id = workload.id;
                        let all_containers = workload.containers.clone();
                        let containers_signal = Signal::derive({
                            let containers = all_containers.clone();
                            move || containers.clone()
                        });
                        let update_action = Action::new_local(move |containers: &Vec<KubeContainerInfo>| {
                            let containers = containers.clone();
                            async move {
                                update_environment_workload_containers(org, workload_id, containers, update_counter).await
                            }
                        });
                        
                        view! {
                            <ContainersCard
                                containers=containers_signal
                                title="Container Configuration"
                                empty_message="This workload doesn't have any container configurations."
                                children=move |index, container| {
                                    let config = ContainerEditorConfig {
                                        enable_resource_limits: false,
                                        show_customization_badge: true,
                                    };
                                    view! {
                                        <ContainerEditor
                                            workload_id
                                            container_index=index
                                            container
                                            all_containers=all_containers.clone()
                                            update_counter
                                            config
                                            update_action
                                        />
                                    }.into_any()
                                }
                            />
                        }.into_any()
                    } else {
                        view! { <div></div> }.into_any()
                    }
                }}
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

