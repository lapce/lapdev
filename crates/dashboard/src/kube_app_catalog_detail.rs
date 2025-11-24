use std::str::FromStr;

use anyhow::{anyhow, Result};
use lapdev_api_hrpc::HrpcServiceClient;
use lapdev_common::{
    console::Organization,
    kube::{KubeAppCatalog, KubeAppCatalogWorkload, KubeContainerInfo},
};
use leptos::{prelude::*, task::spawn_local_scoped_with_cancellation};
use leptos_router::hooks::use_params_map;
use uuid::Uuid;

use crate::{
    app::AppConfig,
    component::{
        badge::{Badge, BadgeVariant},
        button::{Button, ButtonVariant},
        card::Card,
        input::Input,
        table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow},
        typography::{H3, H4, P},
    },
    docs_url,
    kube_app_catalog::CreateEnvironmentModal,
    modal::{DatetimeModal, DeleteModal, ErrorResponse},
    organization::get_current_org,
    sse::run_sse_with_retry,
    DOCS_APP_CATALOG_PATH,
};

#[component]
pub fn KubeAppCatalogDetail() -> impl IntoView {
    let params = use_params_map();
    let catalog_id = params.with_untracked(|params| {
        params
            .get("catalog_id")
            .and_then(|id| Uuid::from_str(id.as_str()).ok())
            .unwrap_or_default()
    });

    view! {
        <div class="flex flex-col gap-6">
            <div class="flex flex-col gap-2 items-start">
                <H3>App Catalog Workloads</H3>
                <P>
                    View workloads included in this Kubernetes application catalog.
                </P>
                <a href=docs_url(DOCS_APP_CATALOG_PATH) target="_blank" rel="noopener noreferrer">
                    <Badge variant=BadgeVariant::Secondary>
                        Docs <lucide_leptos::ExternalLink />
                    </Badge>
                </a>
            </div>

            <WorkloadsList catalog_id />
        </div>
    }
}

pub async fn get_app_catalog(
    org: Signal<Option<Organization>>,
    catalog_id: Uuid,
) -> Result<KubeAppCatalog> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());
    Ok(client.get_app_catalog(org.id, catalog_id).await??)
}

async fn get_app_catalog_workloads(
    org: Signal<Option<Organization>>,
    catalog_id: Uuid,
) -> Result<Vec<KubeAppCatalogWorkload>> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());
    Ok(client
        .get_app_catalog_workloads(org.id, catalog_id)
        .await??)
}

async fn delete_workload(
    org: Signal<Option<Organization>>,
    workload_id: Uuid,
    delete_modal_open: RwSignal<bool>,
    update_counter: RwSignal<usize>,
) -> Result<(), ErrorResponse> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());

    client
        .delete_app_catalog_workload(org.id, workload_id)
        .await??;

    delete_modal_open.set(false);
    update_counter.update(|c| *c += 1);

    Ok(())
}

async fn delete_catalog(
    org: Signal<Option<Organization>>,
    catalog_id: Uuid,
    delete_modal_open: RwSignal<bool>,
) -> Result<(), ErrorResponse> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());

    client.delete_app_catalog(org.id, catalog_id).await??;

    delete_modal_open.set(false);

    Ok(())
}

#[component]
pub fn WorkloadsList(catalog_id: Uuid) -> impl IntoView {
    let org = get_current_org();
    let is_loading = RwSignal::new(false);
    let search_query = RwSignal::new(String::new());
    let debounced_search = RwSignal::new(String::new());
    let update_counter = RwSignal::new(0usize);
    let create_env_modal_open = RwSignal::new(false);
    let delete_catalog_modal_open = RwSignal::new(false);

    // Debounce search input (300ms delay)
    let search_timeout_handle: StoredValue<Option<leptos::leptos_dom::helpers::TimeoutHandle>> =
        StoredValue::new(None);

    Effect::new(move |_| {
        let query = search_query.get();

        // Clear any existing timeout
        if let Some(h) = search_timeout_handle.get_value() {
            h.clear();
        }

        // Set new timeout
        let h = leptos::leptos_dom::helpers::set_timeout_with_handle(
            move || {
                debounced_search.set(query.clone());
            },
            std::time::Duration::from_millis(300),
        )
        .expect("set timeout for search debounce");
        search_timeout_handle.set_value(Some(h));
    });

    on_cleanup(move || {
        if let Some(Some(h)) = search_timeout_handle.try_get_value() {
            h.clear();
        }
    });

    let config = use_context::<AppConfig>().unwrap();
    let catalog_result = LocalResource::new(move || async move {
        is_loading.set(true);
        let result = get_app_catalog(org, catalog_id).await.ok();
        if let Some(app_catalog) = result.as_ref() {
            config.current_page.set(app_catalog.name.clone());
        }
        result
    });

    let workloads_result = LocalResource::new(move || {
        update_counter.track();
        async move {
            let result = get_app_catalog_workloads(org, catalog_id)
                .await
                .unwrap_or_else(|_| vec![]);
            is_loading.set(false);
            result
        }
    });

    let sse_started = RwSignal::new_local(false);
    let catalog_result_for_sse = catalog_result.clone();
    let update_counter_for_sse = update_counter.clone();
    let org_for_sse = org;
    Effect::new(move |_| {
        if sse_started.get_untracked() {
            return;
        }

        if let Some(org) = org_for_sse.get() {
            sse_started.set(true);
            let org_id = org.id;
            let catalog_result_for_sse = catalog_result_for_sse.clone();
            let update_counter = update_counter_for_sse.clone();
            spawn_local_scoped_with_cancellation({
                async move {
                    let url = format!(
                        "/api/v1/organizations/{}/kube/catalogs/{}/events",
                        org_id, catalog_id
                    );
                    let listener_name = format!("catalog {} detail", catalog_id);
                    run_sse_with_retry(url, "catalog", listener_name, move |_message| {
                        catalog_result_for_sse.refetch();
                        update_counter.update(|c| *c += 1);
                    })
                    .await;
                }
            });
        }
    });

    let catalog_info = Signal::derive(move || catalog_result.get().flatten());
    let all_workloads = Signal::derive(move || workloads_result.get().unwrap_or_default());

    let navigate = leptos_router::hooks::use_navigate();
    let delete_catalog_action = Action::new_local(move |_| {
        let nav = navigate.clone();
        async move {
            match delete_catalog(org, catalog_id, delete_catalog_modal_open).await {
                Ok(_) => {
                    nav("/kubernetes/catalogs", Default::default());
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
    });

    // Filter workloads based on search query
    let filtered_workloads = Signal::derive(move || {
        let workloads = all_workloads.get();
        let search_term = debounced_search.get().to_lowercase();

        if search_term.trim().is_empty() {
            workloads
        } else {
            workloads
                .into_iter()
                .filter(|workload| workload.name.to_lowercase().contains(&search_term))
                .collect()
        }
    });

    view! {
        <div class="flex flex-col gap-4">
            // App Catalog Information Section
            <Show when=move || catalog_info.get().is_some()>
                <AppCatalogInfo catalog=catalog_info delete_modal_open=delete_catalog_modal_open delete_action=delete_catalog_action />
            </Show>

            // Search Input and Action Buttons
            <div class="flex flex-col items-start gap-4">
                <div class="flex items-center gap-2">
                    <AddWorkloadsButton catalog_id catalog_info />
                    <Button
                        variant=ButtonVariant::Outline
                        on:click=move |_| create_env_modal_open.set(true)
                    >
                        <lucide_leptos::Plus />
                        Create Environment
                    </Button>
                </div>
                <div class="relative flex-1 max-w-sm">
                    <lucide_leptos::Search attr:class="absolute left-3 top-1/2 transform -translate-y-1/2 text-muted-foreground h-4 w-4" />
                    <Input
                        attr:placeholder="Search workloads..."
                        class="pl-10"
                        prop:value=move || search_query.get()
                        on:input=move |ev| {
                            search_query.set(event_target_value(&ev));
                        }
                    />
                </div>
            </div>

            <div class="rounded-lg border relative">
                // Loading overlay
                <Show when=move || is_loading.get()>
                    <div class="absolute inset-0 bg-white/50 backdrop-blur-sm z-10 flex items-center justify-center">
                        <div class="flex items-center gap-2 text-sm text-muted-foreground">
                            <div class="animate-spin rounded-full h-4 w-4 border-2 border-current border-t-transparent"></div>
                            "Loading workloads..."
                        </div>
                    </div>
                </Show>

                <Table>
                    <TableHeader class="bg-muted">
                        <TableRow>
                            <TableHead>Name</TableHead>
                            <TableHead>Namespace</TableHead>
                            <TableHead>Kind</TableHead>
                            <TableHead>Containers</TableHead>
                            <TableHead>Actions</TableHead>
                        </TableRow>
                    </TableHeader>
                    <TableBody>
                        <For
                            each=move || filtered_workloads.get()
                            key=|workload| format!("{}-{}-{}", workload.name, workload.namespace, workload.kind)
                            children=move |workload| {
                                view! { <WorkloadItem catalog_id workload=workload.clone() update_counter /> }
                            }
                        />
                    </TableBody>
                </Table>

                {move || {
                    let filtered = filtered_workloads.get();
                    let all_workloads = all_workloads.get();
                    let search_term = debounced_search.get();
                    let is_searching = !search_term.trim().is_empty();

                    if filtered.is_empty() {
                        if is_searching {
                            view! {
                                <div class="flex flex-col items-center justify-center py-12 text-center">
                                    <div class="rounded-full bg-muted p-3 mb-4">
                                        <lucide_leptos::Search />
                                    </div>
                                    <H4 class="mb-2">No Results Found</H4>
                                    <P class="text-muted-foreground mb-4 max-w-sm">
                                        {format!(
                                            "No workloads match your search for \"{}\". Try adjusting your search terms.",
                                            search_term
                                        )}
                                    </P>
                                </div>
                            }
                            .into_any()
                        } else if all_workloads.is_empty() {
                            view! {
                                <div class="flex flex-col items-center justify-center py-12 text-center">
                                    <div class="rounded-full bg-muted p-3 mb-4">
                                        <lucide_leptos::Package />
                                    </div>
                                    <H4 class="mb-2">No Workloads Found</H4>
                                    <P class="text-muted-foreground mb-4 max-w-sm">
                                        "This app catalog doesn't contain any workloads yet."
                                    </P>
                                </div>
                            }
                            .into_any()
                        } else {
                            view! { <div></div> }.into_any()
                        }
                    } else {
                        view! { <div></div> }.into_any()
                    }
                }}
            </div>

            <Show when=move || catalog_info.get().is_some()>
                {move || {
                    if let Some(catalog) = catalog_info.get() {
                        view! {
                            <CreateEnvironmentModal
                                modal_open=create_env_modal_open
                                app_catalog=catalog
                                update_counter
                            />
                        }.into_any()
                    } else {
                        ().into_any()
                    }
                }}
            </Show>

            // Delete Catalog Modal
            {move || {
                if let Some(catalog) = catalog_info.get() {
                    view! {
                        <DeleteModal
                            resource=catalog.name
                            open=delete_catalog_modal_open
                            delete_action=delete_catalog_action
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
pub fn WorkloadItem(
    catalog_id: Uuid,
    workload: KubeAppCatalogWorkload,
    update_counter: RwSignal<usize>,
) -> impl IntoView {
    let org = get_current_org();
    let delete_modal_open = RwSignal::new(false);
    let workload_name = workload.name.clone();
    let workload_id = workload.id;

    let delete_action = Action::new_local(move |_| {
        delete_workload(org, workload_id, delete_modal_open, update_counter)
    });

    let kind_variant = BadgeVariant::Outline;

    view! {
        <TableRow>
            <TableCell>
                <a href=format!("/kubernetes/catalogs/{}/workloads/{}", catalog_id, workload.id)>
                    <Button variant=ButtonVariant::Link class="p-0">
                        <span class="font-medium">{workload.name}</span>
                    </Button>
                </a>
            </TableCell>
            <TableCell>
                <Badge variant=BadgeVariant::Secondary>
                    {workload.namespace}
                </Badge>
            </TableCell>
            <TableCell>
                <Badge variant=BadgeVariant::Outline>
                    {workload.kind.to_string()}
                </Badge>
            </TableCell>
            <TableCell>
                <div class="text-sm flex flex-col gap-2">
                    {
                        let containers = workload.containers.clone();
                        containers.iter().map(|container| {
                            view! {
                                <ContainerDisplay
                                    container=container.clone()
                                />
                            }
                        }).collect::<Vec<_>>()
                    }
                    {if workload.containers.clone().is_empty() {
                        view! { <span class="text-muted-foreground">"No containers"</span> }.into_any()
                    } else {
                        ().into_any()
                    }}
                </div>
            </TableCell>
            <TableCell>
                <Button
                    variant=ButtonVariant::Ghost
                    class="px-2"
                    on:click=move |_| {
                        delete_modal_open.set(true);
                    }
                >
                    <lucide_leptos::Trash />
                </Button>

                <DeleteModal
                    resource=workload_name.clone()
                    open=delete_modal_open
                    delete_action
                />
            </TableCell>
        </TableRow>
    }
}

#[component]
pub fn AppCatalogInfo(
    catalog: Signal<Option<KubeAppCatalog>>,
    delete_modal_open: RwSignal<bool>,
    delete_action: Action<(), Result<(), ErrorResponse>>,
) -> impl IntoView {
    view! {
        {move || {
            if let Some(catalog) = catalog.get() {
                view! {
                    <Card class="p-6">
                        <div class="flex flex-col gap-4">
                            <div class="flex items-start justify-between">
                                <div class="flex flex-col gap-2">
                                    <H3>{catalog.name.clone()}</H3>
                                    {catalog.description.clone().map(|desc| {
                                        view! {
                                            <P class="text-muted-foreground">{desc}</P>
                                        }
                                    })}
                                </div>
                                <Button
                                    variant=ButtonVariant::Destructive
                                    on:click=move |_| delete_modal_open.set(true)
                                >
                                    <lucide_leptos::Trash2 />
                                    Delete Catalog
                                </Button>
                            </div>
                            <div class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-3 text-sm">
                                <div class="flex items-center gap-2 text-muted-foreground font-medium">
                                    <div class="[&>svg]:size-4 [&>svg]:shrink-0">
                                        <lucide_leptos::Server />
                                    </div>
                                    <span>Cluster</span>
                                </div>
                                <div>
                                    <a href=format!("/kubernetes/clusters/{}", catalog.cluster_id) class="text-foreground hover:underline">
                                        {catalog.cluster_name.clone()}
                                    </a>
                                </div>

                                <div class="flex items-center gap-2 text-muted-foreground font-medium">
                                    <div class="[&>svg]:size-4 [&>svg]:shrink-0">
                                        <lucide_leptos::Calendar />
                                    </div>
                                    <span>Created</span>
                                </div>
                                <div>
                                    <DatetimeModal time=catalog.created_at />
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
pub fn AddWorkloadsButton(
    catalog_id: Uuid,
    catalog_info: Signal<Option<KubeAppCatalog>>,
) -> impl IntoView {
    let navigate = leptos_router::hooks::use_navigate();

    let on_click = move |_| {
        if let Some(catalog) = catalog_info.get_untracked() {
            let url = format!(
                "/kubernetes/clusters/{}?catalog_id={}",
                catalog.cluster_id, catalog_id
            );
            navigate(&url, Default::default());
        }
    };

    view! {
        <Button
            variant=ButtonVariant::Default
            on:click=on_click
        >
            <lucide_leptos::Plus />
            "Add Workloads"
        </Button>
    }
}

#[component]
pub fn ContainerDisplay(container: KubeContainerInfo) -> impl IntoView {
    view! {
        <div class="flex flex-col p-2 bg-muted/30 rounded border">
            <div class="flex justify-between items-center">
                <span class="font-medium text-foreground">{container.name.clone()}</span>
            </div>

            <div class="text-sm text-foreground mt-1">
                <div class="flex items-center gap-1">
                    <lucide_leptos::Box attr:class="w-3 h-3" />
                    <span class="truncate w-80 text-muted-foreground">{
                        match &container.image {
                            lapdev_common::kube::KubeContainerImage::FollowOriginal => container.original_image.clone(),
                            lapdev_common::kube::KubeContainerImage::Custom(image) => image.clone(),
                        }
                    }</span>
                </div>
            </div>
        </div>
    }
}
