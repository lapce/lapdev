use anyhow::{anyhow, Result};
use lapdev_api_hrpc::HrpcServiceClient;
use lapdev_common::console::Organization;
use lapdev_common::kube::{
    KubeAppCatalog, KubeCluster, KubeClusterStatus, PagePaginationParams, PaginatedInfo,
    PaginatedResult,
};
use leptos::prelude::*;

use crate::component::hover_card::HoverCardPlacement;
use crate::{
    component::{
        badge::{Badge, BadgeVariant},
        button::{Button, ButtonVariant},
        dropdown_menu::{
            DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger,
            DropdownPlacement,
        },
        hover_card::{HoverCard, HoverCardContent, HoverCardTrigger},
        input::Input,
        label::Label,
        pagination::PagePagination,
        select::{Select, SelectContent, SelectItem, SelectTrigger},
        table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow},
        typography::{H3, H4, P},
    },
    modal::{DatetimeModal, DeleteModal, ErrorResponse, Modal},
    organization::get_current_org,
};

#[component]
pub fn KubeAppCatalog() -> impl IntoView {
    let update_counter = RwSignal::new_local(0);

    view! {
        <div class="flex flex-col gap-10">
            <div class="flex flex-col gap-2 items-start">
                <H3>Kubernetes App Catalog</H3>
                <P>
                    {"View and manage your Kubernetes application catalogs. Create collections of workloads that can be deployed as dev environments."}
                </P>
                <a href="https://docs.lap.dev/">
                    <Badge variant=BadgeVariant::Secondary>
                        Docs <lucide_leptos::ExternalLink />
                    </Badge>
                </a>
            </div>

            <AppCatalogList update_counter />
        </div>
    }
}

async fn all_app_catalogs(
    org: Signal<Option<Organization>>,
    search: Option<String>,
    pagination: Option<PagePaginationParams>,
) -> Result<PaginatedResult<KubeAppCatalog>> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());
    Ok(client
        .all_app_catalogs(org.id, search, pagination)
        .await??)
}

async fn delete_app_catalog(
    org: Signal<Option<Organization>>,
    catalog_id: uuid::Uuid,
    delete_modal_open: RwSignal<bool>,
    update_counter: RwSignal<usize, LocalStorage>,
) -> Result<(), ErrorResponse> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());

    client.delete_app_catalog(org.id, catalog_id).await??;

    delete_modal_open.set(false);
    update_counter.update(|c| *c += 1);

    Ok(())
}

#[component]
pub fn AppCatalogList(update_counter: RwSignal<usize, LocalStorage>) -> impl IntoView {
    let org = get_current_org();
    let search_query = RwSignal::new(String::new());
    let debounced_search = RwSignal::new(String::new());
    let current_page = RwSignal::new(1usize);
    let page_size = RwSignal::new(20usize);
    let is_loading = RwSignal::new(false);

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
                current_page.set(1); // Reset to first page on search
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

    let catalogs_result = LocalResource::new(move || {
        let search = debounced_search.get();
        let page = current_page.get();
        let search_param = if search.trim().is_empty() {
            None
        } else {
            Some(search)
        };
        let pagination = Some(PagePaginationParams {
            page,
            page_size: page_size.get(),
        });
        async move {
            is_loading.set(true);
            let result = all_app_catalogs(org, search_param, pagination)
                .await
                .unwrap_or_else(|_| PaginatedResult {
                    data: vec![],
                    pagination_info: PaginatedInfo {
                        total_count: 0,
                        page: 1,
                        page_size: 20,
                        total_pages: 0,
                    },
                });
            is_loading.set(false);
            result
        }
    });

    Effect::new(move |_| {
        update_counter.track();
        catalogs_result.refetch();
    });

    Effect::new(move |_| {
        debounced_search.track();
        catalogs_result.refetch();
    });

    Effect::new(move |_| {
        current_page.track();
        catalogs_result.refetch();
    });

    Effect::new(move |_| {
        page_size.track();
        current_page.set(1); // Reset to first page when page size changes
        catalogs_result.refetch();
    });

    // Auto-adjust current page when data changes (e.g., after delete)
    Effect::new(move |_| {
        if let Some(result) = catalogs_result.get() {
            let current_page_val = current_page.get();
            let total_pages = result.pagination_info.total_pages;

            // If current page is beyond the last page and we're not on page 1, go to the last available page
            if current_page_val > total_pages && total_pages > 0 {
                current_page.set(total_pages);
            }
        }
    });

    let catalog_list =
        Signal::derive(move || catalogs_result.get().map(|r| r.data).unwrap_or_default());
    let pagination_info = Signal::derive(move || {
        catalogs_result
            .get()
            .map(|r| r.pagination_info)
            .unwrap_or_else(|| PaginatedInfo {
                total_count: 0,
                page: 1,
                page_size: page_size.get(),
                total_pages: 0,
            })
    });
    let current_page_signal = Signal::derive(move || current_page.get());
    let total_pages_signal = Signal::derive(move || pagination_info.with(|i| i.total_pages));

    view! {
        <div class="flex flex-col gap-4">
            <div class="flex items-center gap-4">
                <div class="relative flex-1 max-w-sm">
                    <lucide_leptos::Search attr:class="absolute left-3 top-1/2 transform -translate-y-1/2 text-muted-foreground h-4 w-4" />
                    <input
                        type="text"
                        placeholder="Search app catalogs..."
                        class="pl-10 pr-4 py-2 w-full border border-input rounded-md bg-background text-sm focus:outline-none focus:ring-2 focus:ring-ring focus:border-transparent"
                        on:input=move |ev| {
                            search_query.set(event_target_value(&ev));
                        }
                        prop:value=move || search_query.get()
                    />
                </div>
            </div>

            <PagePagination
                pagination_info
                page_size
                current_page=current_page_signal
                total_pages=total_pages_signal
                on_page_change=Callback::new(move |page| {
                    current_page.set(page);
                })
            />

            <div class="rounded-lg border relative">
                // Loading overlay
                <Show when=move || is_loading.get()>
                    <div class="absolute inset-0 bg-white/50 backdrop-blur-sm z-10 flex items-center justify-center">
                        <div class="flex items-center gap-2 text-sm text-muted-foreground">
                            <div class="animate-spin rounded-full h-4 w-4 border-2 border-current border-t-transparent"></div>
                            "Loading..."
                        </div>
                    </div>
                </Show>

                <Table>
                    <TableHeader class="bg-muted">
                        <TableRow>
                            <TableHead>Name</TableHead>
                            <TableHead>Description</TableHead>
                            <TableHead>Cluster</TableHead>
                            <TableHead>Created</TableHead>
                            <TableHead class="pr-4">Actions</TableHead>
                        </TableRow>
                    </TableHeader>
                    <TableBody>
                        <For
                            each=move || catalog_list.get()
                            key=|catalog| catalog.id
                            children=move |catalog| {
                                view! { <AppCatalogItem catalog=catalog.clone() update_counter /> }
                            }
                        />
                    </TableBody>
                </Table>

                {move || {
                    let catalogs = catalog_list.get();
                    let search_term = debounced_search.get();
                    let is_searching = !search_term.trim().is_empty();
                    if catalogs.is_empty() {
                        if is_searching {

                            view! {
                                <div class="flex flex-col items-center justify-center py-12 text-center">
                                    <div class="rounded-full bg-muted p-3 mb-4">
                                        <lucide_leptos::Search />
                                    </div>
                                    <H4 class="mb-2">No Results Found</H4>
                                    <P class="text-muted-foreground mb-4 max-w-sm">
                                        {format!(
                                            "No app catalogs match your search for \"{search_term}\". Try adjusting your search terms.",
                                        )}
                                    </P>
                                </div>
                            }
                                .into_any()
                        } else {
                            view! {
                                <div class="flex flex-col items-center justify-center py-12 text-center">
                                    <div class="rounded-full bg-muted p-3 mb-4">
                                        <lucide_leptos::Package />
                                    </div>
                                    <H4 class="mb-2">No App Catalogs</H4>
                                    <P class="text-muted-foreground mb-4 max-w-sm">
                                        Create your first app catalog by selecting workloads from a Kubernetes cluster.
                                    </P>
                                </div>
                            }
                                .into_any()
                        }
                    } else {
                        view! { <div></div> }.into_any()
                    }
                }}
            </div>

        // Pagination controls
        </div>
    }
}

#[component]
pub fn AppCatalogItem(
    catalog: KubeAppCatalog,
    update_counter: RwSignal<usize, LocalStorage>,
) -> impl IntoView {
    let dropdown_expanded = RwSignal::new(false);
    let delete_modal_open = RwSignal::new(false);
    let create_env_modal_open = RwSignal::new(false);
    let org = get_current_org();

    let catalog_id = catalog.id;
    let catalog_name = catalog.name.clone();
    let catalog_name_clone = catalog_name.clone();
    let catalog_name_for_log = StoredValue::new(catalog_name.clone());
    let catalog_clone = catalog.clone();
    let cluster_id = catalog.cluster_id;
    let cluster_name = catalog.cluster_name.clone();

    let delete_action = Action::new_local(move |_| {
        delete_app_catalog(org, catalog_id, delete_modal_open, update_counter)
    });

    let create_environment = move |_| {
        create_env_modal_open.set(true);
    };

    view! {
        <TableRow>
            <TableCell>
                <span class="font-medium">{catalog_name.clone()}</span>
            </TableCell>
            <TableCell>
                <ExpandableDescription description=catalog.description.clone() />
            </TableCell>
            <TableCell>
                <a href=format!("/kubernetes/clusters/{}", cluster_id)>
                    <Button variant=ButtonVariant::Link class="p-0">
                        <span class="font-medium">{cluster_name.clone()}</span>
                    </Button>
                </a>
            </TableCell>
            <TableCell>
                <DatetimeModal time=catalog.created_at />
            </TableCell>
            <TableCell class="pr-4">
                <div class="flex items-center gap-2">
                    <Button
                        variant=ButtonVariant::Outline
                        size=crate::component::button::ButtonSize::Sm
                        on:click=create_environment
                    >
                        <lucide_leptos::Plus />
                        Create Environment
                    </Button>
                    <DropdownMenu open=dropdown_expanded>
                        <DropdownMenuTrigger
                            open=dropdown_expanded
                            placement=DropdownPlacement::BottomRight
                        >
                            <Button variant=ButtonVariant::Ghost class="px-2">
                                <lucide_leptos::EllipsisVertical />
                            </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent
                            open=dropdown_expanded.read_only()
                            class="min-w-56 -translate-x-2"
                        >
                            <DropdownMenuItem
                                on:click=move |_| {
                                    dropdown_expanded.set(false);
                                    // TODO: Navigate to catalog details or show modal
                                    leptos::logging::log!("View details for catalog: {}", catalog_name_for_log.get_value());
                                }
                                class="cursor-pointer"
                            >
                                <lucide_leptos::Eye />
                                View Details
                            </DropdownMenuItem>
                            <DropdownMenuItem
                                on:click=move |_| {
                                    dropdown_expanded.set(false);
                                    delete_modal_open.set(true);
                                }
                                class="cursor-pointer text-destructive focus:text-destructive"
                            >
                                <lucide_leptos::Trash2 />
                                Delete
                            </DropdownMenuItem>
                        </DropdownMenuContent>
                    </DropdownMenu>
                </div>
                <CreateEnvironmentModal
                    modal_open=create_env_modal_open
                    app_catalog=catalog_clone
                    update_counter
                />
                <DeleteModal
                    resource=catalog_name_clone
                    open=delete_modal_open
                    delete_action
                />
            </TableCell>
        </TableRow>
    }
}

#[component]
pub fn ExpandableDescription(description: Option<String>) -> impl IntoView {
    let description_text =
        StoredValue::new(description.unwrap_or_else(|| "No description".to_string()));
    let open = RwSignal::new(false);

    view! {
        <HoverCard open=open>
            <HoverCardTrigger placement=HoverCardPlacement::BottomRight>
                <div class="max-w-xs text-muted-foreground text-sm text-ellipsis overflow-hidden whitespace-nowrap cursor-help">
                    {move || description_text.get_value()}
                </div>
            </HoverCardTrigger>
            <HoverCardContent class="w-80 max-w-sm -translate-y-6">
                <P class="text-sm whitespace-normal break-words">
                    {move || description_text.get_value()}
                </P>
            </HoverCardContent>
        </HoverCard>
    }
}

#[component]
pub fn CreateEnvironmentModal(
    modal_open: RwSignal<bool>,
    app_catalog: KubeAppCatalog,
    update_counter: RwSignal<usize, LocalStorage>,
) -> impl IntoView {
    let environment_name = RwSignal::new_local("".to_string());
    let namespace = RwSignal::new_local("".to_string());
    let selected_cluster = RwSignal::new(app_catalog.cluster_id);
    let org = get_current_org();
    let client = HrpcServiceClient::new("/api/rpc".to_string());

    // Load available clusters
    let clusters_resource = LocalResource::new(move || async move {
        let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
        let client = HrpcServiceClient::new("/api/rpc".to_string());
        Ok::<Vec<KubeCluster>, anyhow::Error>(client.all_kube_clusters(org.id).await??)
    });

    let clusters = Signal::derive(move || {
        clusters_resource.with(|result| {
            result
                .as_ref()
                .map(|res| res.as_ref().unwrap_or(&vec![]).clone())
                .unwrap_or_default()
        })
    });

    let create_action = Action::new_local(move |_| {
        let client = client.clone();
        async move {
            client
                .create_kube_environment(
                    org.get_untracked()
                        .ok_or_else(|| anyhow!("can't get org"))?
                        .id,
                    app_catalog.id,
                    selected_cluster.get_untracked(),
                    environment_name.get_untracked(),
                    namespace.get_untracked(),
                )
                .await??;

            update_counter.update(|c| {
                *c += 1;
            });
            modal_open.set(false);
            environment_name.set("".to_string());
            namespace.set("".to_string());
            Ok(())
        }
    });

    view! {
        <Modal
            title=format!("Create Environment from {}", app_catalog.name)
            open=modal_open
            action=create_action
        >
            <div class="flex flex-col gap-4">
                <div class="flex flex-col gap-2">
                    <Label>Environment Name</Label>
                    <Input
                        prop:value=move || environment_name.get()
                        on:input=move |ev| {
                            environment_name.set(event_target_value(&ev));
                        }
                        attr:placeholder="Enter environment name"
                        attr:required=true
                    />
                </div>
                <div class="flex flex-col gap-2">
                    <Label>Cluster</Label>
                    {
                        let dropdown_open = RwSignal::new(false);
                        view! {
                            <Select value=selected_cluster open=dropdown_open class="w-full">
                                <SelectTrigger class="w-full">
                                    {move || {
                                        let cluster_id = selected_cluster.get();
                                        if let Some(cluster) = clusters.get()
                                            .into_iter()
                                            .find(|c| c.id == cluster_id && c.can_deploy) {
                                            let status_variant = match cluster.info.status {
                                                KubeClusterStatus::Ready => BadgeVariant::Secondary,
                                                KubeClusterStatus::Provisioning => BadgeVariant::Secondary,
                                                KubeClusterStatus::NotReady | KubeClusterStatus::Error => BadgeVariant::Destructive,
                                            };
                                            view! {
                                                <div class="flex items-center justify-between w-full">
                                                    <span class="max-w-60 text-ellipsis overflow-hidden whitespace-nowrap">{cluster.name}</span>
                                                    <Badge variant=status_variant class="ml-2 text-xs">
                                                        {cluster.info.status.to_string()}
                                                    </Badge>
                                                </div>
                                            }.into_any()
                                        } else {
                                            view! { "Select deployable cluster" }.into_any()
                                        }
                                    }}
                                </SelectTrigger>
                                <SelectContent class="w-full">
                                    {move || {
                                        clusters.get()
                                            .into_iter()
                                            .filter(|cluster| cluster.can_deploy)
                                            .map(|cluster| {
                                                let status_variant = match cluster.info.status {
                                                    KubeClusterStatus::Ready => BadgeVariant::Secondary,
                                                    KubeClusterStatus::Provisioning => BadgeVariant::Secondary,
                                                    KubeClusterStatus::NotReady | KubeClusterStatus::Error => BadgeVariant::Destructive,
                                                };
                                                view! {
                                                    <SelectItem
                                                        value=cluster.id
                                                        display=cluster.name.clone()
                                                    >
                                                        <div class="flex items-center justify-between w-full">
                                                            <span class="max-w-60 text-ellipsis overflow-hidden whitespace-nowrap">{cluster.name}</span>
                                                            <Badge variant=status_variant class="ml-2 text-xs">
                                                                {cluster.info.status.to_string()}
                                                            </Badge>
                                                        </div>
                                                    </SelectItem>
                                                }
                                            })
                                            .collect::<Vec<_>>()
                                    }}
                                </SelectContent>
                            </Select>
                        }
                    }
                </div>
                <div class="flex flex-col gap-2">
                    <Label>Namespace</Label>
                    <Input
                        prop:value=move || namespace.get()
                        on:input=move |ev| {
                            namespace.set(event_target_value(&ev));
                        }
                        attr:placeholder="Enter Kubernetes namespace"
                        attr:required=true
                    />
                </div>
                <div class="flex flex-col gap-2 p-4 bg-muted rounded-lg text-wrap">
                    <P class="font-medium text-sm">App Catalog Details:</P>
                    <ul class="text-sm space-y-1">
                        <li>{format!("• Name: {}", app_catalog.name)}</li>
                        <li>{format!("• Cluster: {}", app_catalog.cluster_name)}</li>
                        {app_catalog.description.as_ref().map(|desc| {
                            view! { <li>{format!("• Description: {}", desc)}</li> }
                        })}
                    </ul>
                </div>
            </div>
        </Modal>
    }
}
