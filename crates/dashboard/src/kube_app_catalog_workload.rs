use std::str::FromStr;

use anyhow::{anyhow, Result};
use lapdev_api_hrpc::HrpcServiceClient;
use lapdev_common::{
    console::Organization,
    kube::{KubeAppCatalog, KubeAppCatalogWorkload, KubeWorkloadKind},
};
use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use uuid::Uuid;

use crate::{
    app::AppConfig,
    component::{
        badge::{Badge, BadgeVariant},
        card::Card,
        input::Input,
        table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow},
        typography::{H3, H4, P},
    },
    modal::DatetimeModal,
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

async fn get_app_catalog(
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

#[component]
pub fn WorkloadsList(catalog_id: Uuid) -> impl IntoView {
    let org = get_current_org();
    let is_loading = RwSignal::new(false);
    let search_query = RwSignal::new(String::new());
    let debounced_search = RwSignal::new(String::new());

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

    let workloads_result = LocalResource::new(move || async move {
        let result = get_app_catalog_workloads(org, catalog_id)
            .await
            .unwrap_or_else(|_| vec![]);
        is_loading.set(false);
        result
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

            // Search Input
            <div class="flex items-center gap-4">
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
                            <TableHead>CPU</TableHead>
                            <TableHead>Memory</TableHead>
                        </TableRow>
                    </TableHeader>
                    <TableBody>
                        <For
                            each=move || filtered_workloads.get()
                            key=|workload| format!("{}-{}-{}", workload.name, workload.namespace, workload.kind)
                            children=move |workload| {
                                view! { <WorkloadItem workload=workload.clone() /> }
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
pub fn WorkloadItem(workload: KubeAppCatalogWorkload) -> impl IntoView {
    let kind_variant = match workload.kind {
        KubeWorkloadKind::Deployment => BadgeVariant::Default,
        KubeWorkloadKind::StatefulSet => BadgeVariant::Secondary,
        KubeWorkloadKind::DaemonSet => BadgeVariant::Destructive,
        KubeWorkloadKind::Pod => BadgeVariant::Outline,
        _ => BadgeVariant::Secondary,
    };

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
                <span class="text-sm">
                    {workload.cpu.unwrap_or_else(|| "Not specified".to_string())}
                </span>
            </TableCell>
            <TableCell>
                <span class="text-sm">
                    {workload.memory.unwrap_or_else(|| "Not specified".to_string())}
                </span>
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
