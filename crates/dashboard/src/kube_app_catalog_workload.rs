use std::str::FromStr;

use anyhow::{anyhow, Result};
use lapdev_api_hrpc::HrpcServiceClient;
use lapdev_common::{
    console::Organization,
    kube::{
        KubeAppCatalog, KubeAppCatalogWorkload, KubeContainerInfo,
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
        typography::{H3, P},
    },
    kube_container::{ContainerEditorConfig, ContainersCard},
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
                config.current_page.set(workload.name.clone());
                config.header_links.update(|header_links| {
                    header_links.push((
                        catalog.name.clone(),
                        format!("/kubernetes/catalogs/{catalog_id}"),
                    ));
                });
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

            // Workload Information Card
            <Show when=move || workload_info.get().is_some()>
                <WorkloadInfoCard workload_info catalog_info delete_modal_open delete_action />
            </Show>

            // Containers Section
            <Show when=move || workload_info.get().is_some()>
                {move || {
                    if let Some(workload) = workload_info.get() {
                        let workload_id = workload.id;
                        let all_containers = workload.containers.clone();
                        let update_action = Action::new_local(move |containers: &Vec<KubeContainerInfo>| {
                            let containers = containers.clone();
                            async move {
                                update_workload_containers(org, workload_id, containers, update_counter).await
                            }
                        });
                        
                        let config = ContainerEditorConfig {
                            enable_resource_limits: true,
                            show_customization_badge: false,
                        };
                        view! {
                            <ContainersCard
                                all_containers
                                title="Container Configuration"
                                empty_message="This workload doesn't have any container configurations."
                                workload_id
                                update_counter
                                config
                                update_action
                            />
                        }.into_any()
                    } else {
                        view! { <div></div> }.into_any()
                    }
                }}
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


