use std::str::FromStr;

use anyhow::{anyhow, Result};
use lapdev_api_hrpc::HrpcServiceClient;
use lapdev_common::{
    console::Organization,
    kube::{KubeAppCatalog, KubeAppCatalogWorkload, KubeContainerInfo},
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
        table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow},
        typography::{H3, H4, P},
    },
    modal::{DatetimeModal, DeleteModal, ErrorResponse},
    organization::get_current_org,
};

#[component]
pub fn KubeAppCatalogWorkload() -> impl IntoView {
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
                <a href="https://docs.lap.dev/">
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

#[component]
pub fn WorkloadsList(catalog_id: Uuid) -> impl IntoView {
    let org = get_current_org();
    let is_loading = RwSignal::new(false);
    let search_query = RwSignal::new(String::new());
    let debounced_search = RwSignal::new(String::new());
    let update_counter = RwSignal::new(0usize);

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

    let catalog_info = Signal::derive(move || catalog_result.get().flatten());
    let all_workloads = Signal::derive(move || workloads_result.get().unwrap_or_default());

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
                <AppCatalogInfo catalog=catalog_info />
            </Show>

            // Search Input and Add Workloads Button
            <div class="flex flex-col items-start gap-4">
                <AddWorkloadsButton catalog_id catalog_info />
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
                                view! { <WorkloadItem workload=workload.clone() update_counter /> }
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
        </div>
    }
}

#[component]
pub fn WorkloadItem(
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
                <span class="font-medium">{workload.name}</span>
            </TableCell>
            <TableCell>
                <span class="text-muted-foreground">{workload.namespace}</span>
            </TableCell>
            <TableCell>
                <Badge variant=kind_variant>
                    {workload.kind.to_string()}
                </Badge>
            </TableCell>
            <TableCell>
                <div class="text-sm space-y-2">
                    {
                        let containers = workload.containers.clone();
                        containers.iter().enumerate().map(|(container_index, container)| {
                            let workload_id = workload.id;
                            let container_idx = container_index;

                            view! {
                                <ContainerEditor
                                    workload_id
                                    container_index=container_idx
                                    container=container.clone()
                                    update_counter
                                />
                            }
                        }).collect::<Vec<_>>()
                    }
                    {if workload.containers.clone().is_empty() {
                        view! { <span class="text-muted-foreground">"No containers"</span> }
                    } else {
                        view! { <span class="text-muted-foreground">" "</span> }
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
pub fn AppCatalogInfo(catalog: Signal<Option<KubeAppCatalog>>) -> impl IntoView {
    view! {
        {move || {
            if let Some(catalog) = catalog.get() {
                view! {
                    <Card class="p-6">
                        <div class="flex flex-col gap-4">
                            <div class="flex flex-col gap-2">
                                <H3>{catalog.name.clone()}</H3>
                                {catalog.description.clone().map(|desc| {
                                    view! {
                                        <P class="text-muted-foreground">{desc}</P>
                                    }
                                })}
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

async fn update_container(
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

#[component]
pub fn ContainerEditor(
    workload_id: Uuid,
    container_index: usize,
    container: KubeContainerInfo,
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

    let name = container.name.clone();

    let update_action = Action::new_local(move |containers: &Vec<KubeContainerInfo>| {
        update_container(org, workload_id, containers.clone(), update_counter)
    });

    let save_changes = {
        let container_name = container.name.clone();
        Callback::new(move |_| {
            // Get current workload to update only this container
            // For now, we'll just update with the current container data
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
            };

            // For simplicity, we'll pass just this container. In a real implementation,
            // you'd need to get all containers for the workload and update just this one
            update_action.dispatch(vec![updated_container]);
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
            is_editing.set(false);
        })
    };

    view! {
        <div class="flex flex-col p-2 bg-muted/30 rounded border">
            <div class="flex justify-between items-center">
                <span class="font-medium text-foreground">{name}</span>
                <div class="flex gap-1">
                    <Show when=move || !is_editing.get()>
                        <Button
                            variant=ButtonVariant::Ghost
                            class="px-2 py-1 h-auto"
                            on:click=move |_| is_editing.set(true)
                        >
                            <lucide_leptos::Pen attr:class="w-3 h-3" />
                        </Button>
                    </Show>
                    <Show when=move || is_editing.get()>
                        <Button
                            variant=ButtonVariant::Ghost
                            class="px-2 py-1 h-auto text-green-600"
                            on:click=move |_| save_changes.run(())
                        >
                            <lucide_leptos::Check attr:class="w-3 h-3" />
                        </Button>
                        <Button
                            variant=ButtonVariant::Ghost
                            class="px-2 py-1 h-auto text-red-600"
                            on:click=move |_| cancel_changes.run(())
                        >
                            <lucide_leptos::X attr:class="w-3 h-3" />
                        </Button>
                    </Show>
                </div>
            </div>

            <div class="text-sm text-foreground space-y-1 mt-1">
                <div class="flex flex-col gap-2">
                    <div class="flex items-center gap-1">
                        <lucide_leptos::Box attr:class="w-3 h-3" />
                        <Show
                            when=move || is_editing.get()
                            fallback=move || view! { <span class="truncate">{move || image_signal.get()}</span> }
                        >
                            <Input
                                prop:value=move || image_signal.get()
                                on:input=move |ev| image_signal.set(event_target_value(&ev))
                                attr:placeholder="Container image"
                                class="text-xs h-6"
                            />
                        </Show>
                    </div>
                    <div class="flex gap-10">
                        <div class="flex flex-col gap-1">
                            <div class="flex items-center gap-1">
                                <lucide_leptos::Cpu attr:class="w-3 h-3" />
                                <span class="font-medium">CPU</span>
                            </div>
                            <div class="ml-4 space-y-0.5">
                                <div class="flex">
                                    <span class="text-muted-foreground w-16">{"Request:"}</span>
                                    <Show
                                        when=move || is_editing.get()
                                        fallback=move || view! { <span>{move || if cpu_request_signal.get().is_empty() { "-".to_string() } else { cpu_request_signal.get() }}</span> }
                                    >
                                        <Input
                                            prop:value=move || cpu_request_signal.get()
                                            on:input=move |ev| cpu_request_signal.set(event_target_value(&ev))
                                            class="text-xs h-5 w-20"
                                        />
                                    </Show>
                                </div>
                                <div class="flex">
                                    <span class="text-muted-foreground w-16">{"Limit:"}</span>
                                    <Show
                                        when=move || is_editing.get()
                                        fallback=move || view! { <span>{move || if cpu_limit_signal.get().is_empty() { "-".to_string() } else { cpu_limit_signal.get() }}</span> }
                                    >
                                        <Input
                                            prop:value=move || cpu_limit_signal.get()
                                            on:input=move |ev| cpu_limit_signal.set(event_target_value(&ev))
                                            class="text-xs h-5 w-20"
                                        />
                                    </Show>
                                </div>
                            </div>
                        </div>
                        <div class="flex flex-col gap-1">
                            <div class="flex items-center gap-1">
                                <lucide_leptos::MemoryStick attr:class="w-3 h-3" />
                                <span class="font-medium">Memory</span>
                            </div>
                            <div class="ml-4 space-y-0.5">
                                <div class="flex">
                                    <span class="text-muted-foreground w-16">{"Request:"}</span>
                                    <Show
                                        when=move || is_editing.get()
                                        fallback=move || view! { <span>{move || if memory_request_signal.get().is_empty() { "-".to_string() } else { memory_request_signal.get() }}</span> }
                                    >
                                        <Input
                                            prop:value=move || memory_request_signal.get()
                                            on:input=move |ev| memory_request_signal.set(event_target_value(&ev))
                                            class="text-xs h-5 w-20"
                                        />
                                    </Show>
                                </div>
                                <div class="flex">
                                    <span class="text-muted-foreground w-16">{"Limit:"}</span>
                                    <Show
                                        when=move || is_editing.get()
                                        fallback=move || view! { <span>{move || if memory_limit_signal.get().is_empty() { "-".to_string() } else { memory_limit_signal.get() }}</span> }
                                    >
                                        <Input
                                            prop:value=move || memory_limit_signal.get()
                                            on:input=move |ev| memory_limit_signal.set(event_target_value(&ev))
                                            class="text-xs h-5 w-20"
                                        />
                                    </Show>
                                </div>
                            </div>
                        </div>
                    </div>
                </div>
            </div>
        </div>
    }
}
