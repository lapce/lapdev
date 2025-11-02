use anyhow::{anyhow, Result};
use chrono::DateTime;
use futures::StreamExt;
use gloo_net::eventsource::futures::EventSource;
use lapdev_api_hrpc::HrpcServiceClient;
use lapdev_common::console::Organization;
use lapdev_common::kube::{
    KubeEnvironment, KubeEnvironmentStatus, PagePaginationParams, PaginatedInfo, PaginatedResult,
};
use leptos::{prelude::*, task::spawn_local_scoped_with_cancellation};
use leptos_router::hooks::use_location;

use crate::{
    component::{
        badge::{Badge, BadgeVariant},
        button::{Button, ButtonVariant},
        dropdown_menu::{
            DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger,
            DropdownPlacement,
        },
        pagination::PagePagination,
        table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow},
        typography::{H3, H4, P},
    },
    docs_url,
    modal::DatetimeModal,
    organization::get_current_org,
    DOCS_ENVIRONMENT_PATH,
};

#[component]
pub fn KubeEnvironment() -> impl IntoView {
    let update_counter = RwSignal::new_local(0);
    let location = use_location();

    // Derive environment type from the route
    let environment_type = Signal::derive(move || {
        let pathname = location.pathname.get();
        if pathname.ends_with("/shared") {
            EnvironmentType::Shared
        } else if pathname.ends_with("/branch") {
            EnvironmentType::Branch
        } else {
            // Default to Personal for /kubernetes/environments and /kubernetes/environments/personal
            EnvironmentType::Personal
        }
    });

    let title = move || match environment_type.get() {
        EnvironmentType::Personal => "Personal Environments",
        EnvironmentType::Shared => "Shared Environments",
        EnvironmentType::Branch => "Branch Environments",
    };

    let description = move || match environment_type.get() {
        EnvironmentType::Personal => {
            "View and manage your personal Kubernetes development environments."
        }
        EnvironmentType::Shared => {
            "View and manage shared Kubernetes development environments accessible to your team."
        }
        EnvironmentType::Branch => {
            "View and manage branch-based Kubernetes development environments."
        }
    };

    view! {
        <div class="flex flex-col gap-6">
            <div class="flex flex-col gap-2 items-start">
                <H3>{title}</H3>
                <P>{description}</P>
                <a href=docs_url(DOCS_ENVIRONMENT_PATH) target="_blank" rel="noopener noreferrer">
                    <Badge variant=BadgeVariant::Secondary>Docs <lucide_leptos::ExternalLink /></Badge>
                </a>
            </div>

            <KubeEnvironmentList update_counter environment_type />
        </div>
    }
}

async fn all_kube_environments(
    org: Signal<Option<Organization>>,
    search: Option<String>,
    is_shared: bool,
    is_branch: bool,
    pagination: Option<PagePaginationParams>,
) -> Result<PaginatedResult<KubeEnvironment>> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());
    Ok(client
        .all_kube_environments(org.id, search, is_shared, is_branch, pagination)
        .await??)
}

#[derive(Clone, PartialEq)]
pub enum EnvironmentType {
    Personal,
    Shared,
    Branch,
}

#[component]
pub fn KubeEnvironmentList(
    update_counter: RwSignal<usize, LocalStorage>,
    environment_type: Signal<EnvironmentType>,
) -> impl IntoView {
    let org = get_current_org();
    let search_query = RwSignal::new(String::new());
    let debounced_search = RwSignal::new(String::new());
    let current_page = RwSignal::new(1usize);
    let page_size = RwSignal::new(20usize);
    let is_loading = RwSignal::new(false);
    let sse_started = StoredValue::new(false);

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

    let environments_result = LocalResource::new(move || {
        let search = debounced_search.get();
        let page = current_page.get();
        let env_type = environment_type.get();
        let is_shared = env_type == EnvironmentType::Shared;
        let is_branch = env_type == EnvironmentType::Branch;
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
            let result = all_kube_environments(org, search_param, is_shared, is_branch, pagination)
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
        environments_result.refetch();
    });

    Effect::new(move |_| {
        debounced_search.track();
        environments_result.refetch();
    });

    Effect::new(move |_| {
        current_page.track();
        environments_result.refetch();
    });

    Effect::new(move |_| {
        page_size.track();
        current_page.set(1); // Reset to first page when page size changes
        environments_result.refetch();
    });

    Effect::new(move |_| {
        environment_type.track();
        current_page.set(1); // Reset to first page when environment type changes
        environments_result.refetch();
    });

    Effect::new(move |_| {
        if sse_started.get_value() {
            return;
        }

        if let Some(org) = org.get() {
            sse_started.set_value(true);
            let org_id = org.id;
            let update_counter = update_counter;
            spawn_local_scoped_with_cancellation(async move {
                let url = format!("/api/v1/organizations/{}/kube/environments/events", org_id);

                match EventSource::new(&url) {
                    Ok(mut event_source) => {
                        match event_source.subscribe("environment") {
                            Ok(mut stream) => {
                                while let Some(event) = stream.next().await {
                                    if event.is_ok() {
                                        update_counter.update(|c| *c += 1);
                                    }
                                }
                            }
                            Err(err) => {
                                web_sys::console::error_1(
                                    &format!(
                                        "Failed to subscribe to organization environment events: {err}"
                                    )
                                    .into(),
                                );
                            }
                        }
                        event_source.close();
                    }
                    Err(err) => {
                        web_sys::console::error_1(
                            &format!("Failed to connect to organization environment events: {err}")
                                .into(),
                        );
                    }
                }
            });
        }
    });

    let environment_list = Signal::derive(move || {
        environments_result
            .get()
            .map(|r| r.data)
            .unwrap_or_default()
    });
    let pagination_info = Signal::derive(move || {
        environments_result
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
        <EnvironmentContent
            search_query
            debounced_search
            environment_list
            pagination_info
            current_page_signal
            total_pages_signal
            current_page
            is_loading
            page_size
        />
    }
}

#[component]
pub fn EnvironmentContent(
    search_query: RwSignal<String>,
    debounced_search: RwSignal<String>,
    environment_list: Signal<Vec<KubeEnvironment>>,
    pagination_info: Signal<PaginatedInfo>,
    current_page_signal: Signal<usize>,
    total_pages_signal: Signal<usize>,
    current_page: RwSignal<usize>,
    is_loading: RwSignal<bool>,
    page_size: RwSignal<usize>,
) -> impl IntoView {
    view! {
        <div class="flex flex-col gap-4">
            <div class="flex items-center gap-4">
                <div class="relative flex-1 max-w-sm">
                    <lucide_leptos::Search attr:class="absolute left-3 top-1/2 transform -translate-y-1/2 text-muted-foreground h-4 w-4" />
                    <input
                        type="text"
                        placeholder="Search environments..."
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
                            <TableHead>Namespace</TableHead>
                            <TableHead>App Catalog</TableHead>
                            <TableHead>Cluster</TableHead>
                            <TableHead>Type</TableHead>
                            <TableHead>Status</TableHead>
                            <TableHead>Created</TableHead>
                            <TableHead class="pr-4">Actions</TableHead>
                        </TableRow>
                    </TableHeader>
                    <TableBody>
                        <For
                            each=move || environment_list.get()
                            key=|env| env.id
                            children=move |environment| {
                                let env_id = environment.id;
                                let initial_status = environment.status;
                                let status_signal = Signal::derive(move || {
                                    environment_list.with(|list| {
                                        list.iter()
                                            .find(|env| env.id == env_id)
                                            .map(|env| env.status)
                                            .unwrap_or(initial_status)
                                    })
                                });

                                view! {
                                    <KubeEnvironmentItem
                                        environment=environment.clone()
                                        status=status_signal
                                    />
                                }
                            }
                        />
                    </TableBody>
                </Table>

                {move || {
                    let environments = environment_list.get();
                    let search_term = debounced_search.get();
                    let is_searching = !search_term.trim().is_empty();
                    if environments.is_empty() {
                        if is_searching {
                            view! {
                                <div class="flex flex-col items-center justify-center py-12 text-center">
                                    <div class="rounded-full bg-muted p-3 mb-4">
                                        <lucide_leptos::Search />
                                    </div>
                                    <H4 class="mb-2">No Results Found</H4>
                                    <P class="text-muted-foreground mb-4 max-w-sm">
                                        {format!(
                                            "No environments match your search for \"{search_term}\". Try adjusting your search terms.",
                                        )}
                                    </P>
                                </div>
                            }
                                .into_any()
                        } else {
                            view! {
                                <div class="flex flex-col items-center justify-center py-12 text-center">
                                    <div class="rounded-full bg-muted p-3 mb-4">
                                        <lucide_leptos::Container />
                                    </div>
                                    <H4 class="mb-2">No Environments</H4>
                                    <P class="text-muted-foreground mb-4 max-w-sm">
                                        Deploy your first environment from an app catalog to get started.
                                    </P>
                                </div>
                            }
                                .into_any()
                        }
                    } else {
                        ().into_any()
                    }
                }}
            </div>
        </div>
    }
}

#[component]
pub fn KubeEnvironmentItem(
    environment: KubeEnvironment,
    status: Signal<KubeEnvironmentStatus>,
) -> impl IntoView {
    let env_name = environment.name.clone();
    let env_name_for_delete = environment.name.clone();

    let navigate = leptos_router::hooks::use_navigate();
    let environment_id = environment.id;
    let needs_catalog_sync = environment.catalog_update_available;
    // let view_details = move |_| {
    //     let url = format!("/kubernetes/environments/{}", environment_id);
    //     navigate(&url, Default::default());
    // };

    // let delete_environment = move |_| {
    //     // TODO: Implement delete functionality
    //     leptos::logging::log!("Delete environment: {}", env_name_for_delete);
    // };

    let status_signal = status;
    let status_variant = Signal::derive(move || match status_signal.get() {
        KubeEnvironmentStatus::Running => BadgeVariant::Secondary,
        KubeEnvironmentStatus::Creating
        | KubeEnvironmentStatus::Pausing
        | KubeEnvironmentStatus::Resuming
        | KubeEnvironmentStatus::Paused
        | KubeEnvironmentStatus::Deleting
        | KubeEnvironmentStatus::Deleted => BadgeVariant::Outline,
        KubeEnvironmentStatus::Failed
        | KubeEnvironmentStatus::Error
        | KubeEnvironmentStatus::PauseFailed
        | KubeEnvironmentStatus::ResumeFailed => BadgeVariant::Destructive,
    });

    let dropdown_expanded = RwSignal::new(false);
    let env_name_for_delete_clone = env_name_for_delete.clone();
    let details_url = format!("/kubernetes/environments/{}", environment_id);
    let catalog_url = format!("/kubernetes/catalogs/{}", environment.app_catalog_id);
    let cluster_url = format!("/kubernetes/clusters/{}", environment.cluster_id);
    let resources_url = format!(
        "/kubernetes/clusters/{}?namespace={}",
        environment.cluster_id, environment.namespace
    );

    let is_branch = environment.base_environment_id.is_some();
    let is_shared = environment.is_shared;

    view! {
        <TableRow>
            <TableCell>
                <div class="flex items-center gap-2">
                    <a href={format!("/kubernetes/environments/{}", environment.id)}>
                        <Button variant=ButtonVariant::Link class="p-0">
                            <span class="font-medium">{environment.name.clone()}</span>
                        </Button>
                    </a>
                    {if needs_catalog_sync {
                        view! {
                            <Badge variant=BadgeVariant::Destructive class="text-[10px] uppercase tracking-wide">"Sync required"</Badge>
                        }.into_any()
                    } else {
                        view! { <></> }.into_any()
                    }}
                </div>
            </TableCell>
            <TableCell>
                <Badge variant=BadgeVariant::Outline>{environment.namespace.clone()}</Badge>
            </TableCell>
            <TableCell>
                <a href={format!("/kubernetes/catalogs/{}", environment.app_catalog_id)}>
                    <Button variant=ButtonVariant::Link class="p-0">
                        <span class="font-medium">{environment.app_catalog_name.clone()}</span>
                    </Button>
                </a>
            </TableCell>
            <TableCell>
                <a href={format!("/kubernetes/clusters/{}", environment.cluster_id)}>
                    <Button variant=ButtonVariant::Link class="p-0">
                        <span class="font-medium">{environment.cluster_name.clone()}</span>
                    </Button>
                </a>
            </TableCell>
            <TableCell>
                <div class="flex flex-col gap-1">
                    <Badge variant=BadgeVariant::Secondary>
                        <span class="inline-flex items-center gap-2">
                            {if is_branch {
                                view! {
                                    <>
                                        <lucide_leptos::GitBranch attr:class="h-3 w-3" />
                                        <span>"Branch"</span>
                                    </>
                                }.into_any()
                            } else if is_shared {
                                view! {
                                    <>
                                        <lucide_leptos::Users attr:class="h-3 w-3" />
                                        <span>"Shared"</span>
                                    </>
                                }.into_any()
                            } else {
                                view! {
                                    <>
                                        <lucide_leptos::User attr:class="h-3 w-3" />
                                        <span>"Personal"</span>
                                    </>
                                }.into_any()
                            }}
                        </span>
                    </Badge>
                    {if let (Some(base_name), Some(base_env_id)) = (environment.base_environment_name.clone(), environment.base_environment_id) {
                        view! {
                            <span class="text-xs text-muted-foreground">
                                {"from "}
                                <a href={format!("/kubernetes/environments/{}", base_env_id)}>
                                    <Button variant=ButtonVariant::Link class="p-0 h-auto text-xs text-muted-foreground hover:text-foreground">
                                        {base_name}
                                    </Button>
                                </a>
                            </span>
                        }.into_any()
                    } else {
                        view! { <div></div> }.into_any()
                    }}
                </div>
            </TableCell>
            <TableCell>
                <Badge variant=status_variant>
                    {move || status_signal.get().to_string()}
                </Badge>
            </TableCell>
            <TableCell>
                {match DateTime::parse_from_str(&environment.created_at, "%Y-%m-%d %H:%M:%S%.f %z") {
                    Ok(time) => view! { <DatetimeModal time /> }.into_any(),
                    Err(_) => view! {
                        <span class="text-sm text-muted-foreground">{environment.created_at.clone()}</span>
                    }.into_any(),
                }}
            </TableCell>
            <TableCell class="pr-4">
                <DropdownMenu open=dropdown_expanded>
                    <DropdownMenuTrigger
                        open=dropdown_expanded
                        placement=DropdownPlacement::BottomRight
                    >
                        <Button variant=ButtonVariant::Ghost size=crate::component::button::ButtonSize::Sm class="px-2">
                            <lucide_leptos::EllipsisVertical />
                        </Button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent
                        open=dropdown_expanded.read_only()
                        class="min-w-48 -translate-x-2"
                    >
                        {if needs_catalog_sync {
                            view! {
                                <DropdownMenuItem class="p-0">
                                    <button class="flex items-center gap-2 px-2 py-1.5 w-full text-amber-600 dark:text-amber-300 cursor-not-allowed" disabled>
                                        <lucide_leptos::RefreshCcw />
                                        "Sync From Catalog"
                                    </button>
                                </DropdownMenuItem>
                            }.into_any()
                        } else {
                            view! { <></> }.into_any()
                        }}

                        <DropdownMenuItem class="p-0">
                            <a href=format!("/kubernetes/environments/{}", environment_id) class="flex items-center gap-2 px-2 py-1.5 w-full">
                                <lucide_leptos::Eye />
                                View Details
                            </a>
                        </DropdownMenuItem>
                        <DropdownMenuItem class="p-0">
                            <a href=format!("/kubernetes/catalogs/{}", environment.app_catalog_id) class="flex items-center gap-2 px-2 py-1.5 w-full">
                                <lucide_leptos::Package />
                                View App Catalog
                            </a>
                        </DropdownMenuItem>
                        <DropdownMenuItem class="p-0">
                            <a href=format!("/kubernetes/clusters/{}", environment.cluster_id) class="flex items-center gap-2 px-2 py-1.5 w-full">
                                <lucide_leptos::Server />
                                View Cluster
                            </a>
                        </DropdownMenuItem>
                        <DropdownMenuItem
                            on:click=move |_| {
                                dropdown_expanded.set(false);
                                // TODO: Implement delete functionality
                                // leptos::logging::log!("Delete environment: {}", env_name_for_delete_clone);
                            }
                            class="cursor-pointer text-destructive focus:text-destructive"
                        >
                            <lucide_leptos::Trash2 />
                            Delete
                        </DropdownMenuItem>
                    </DropdownMenuContent>
                </DropdownMenu>
            </TableCell>
        </TableRow>
    }
}
