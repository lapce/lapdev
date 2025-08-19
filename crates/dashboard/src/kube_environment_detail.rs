use std::str::FromStr;

use anyhow::{anyhow, Result};
use lapdev_api_hrpc::HrpcServiceClient;
use lapdev_common::{
    console::Organization,
    kube::{
        KubeClusterInfo, KubeClusterStatus, KubeContainerInfo, KubeEnvironment,
        KubeEnvironmentService, KubeEnvironmentWorkload,
    },
};
use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use uuid::Uuid;

use crate::{
    app::AppConfig,
    component::{
        badge::{Badge, BadgeVariant},
        button::{Button, ButtonSize, ButtonVariant},
        card::Card,
        input::Input,
        label::Label,
        table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow},
        tabs::{Tabs, TabsContent, TabsList, TabsTrigger},
        typography::{H3, H4, P},
    },
    modal::{DatetimeModal, DeleteModal, ErrorResponse, Modal},
    organization::get_current_org,
};

#[component]
pub fn KubeEnvironmentDetail() -> impl IntoView {
    let params = use_params_map();
    let environment_id = Signal::derive(move || {
        params.with(|params| {
            params
                .get("environment_id")
                .and_then(|id| Uuid::from_str(id.as_str()).ok())
                .unwrap_or_default()
        })
    });

    view! {
        <div class="flex flex-col gap-6">
            <div class="flex flex-col gap-2 items-start">
                <H3>Environment Details</H3>
                <P>
                    View and manage details for this Kubernetes development environment.
                </P>
                <a href="https://docs.lap.dev/">
                    <Badge variant=BadgeVariant::Secondary>
                        Docs <lucide_leptos::ExternalLink />
                    </Badge>
                </a>
            </div>

            {
                move || view!{
                    <EnvironmentDetailView environment_id=environment_id.get() />
                }
            }
        </div>
    }
}

async fn get_environment_detail(
    org: Signal<Option<Organization>>,
    environment_id: Uuid,
) -> Result<KubeEnvironment> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());
    Ok(client
        .get_kube_environment(org.id, environment_id)
        .await??)
}

async fn get_environment_cluster_info(
    org: Signal<Option<Organization>>,
    cluster_id: Uuid,
) -> Result<KubeClusterInfo> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());
    Ok(client.get_cluster_info(org.id, cluster_id).await??)
}

async fn get_environment_workloads(
    org: Signal<Option<Organization>>,
    environment_id: Uuid,
) -> Result<Vec<KubeEnvironmentWorkload>> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());
    Ok(client
        .get_environment_workloads(org.id, environment_id)
        .await??)
}

async fn get_environment_services(
    org: Signal<Option<Organization>>,
    environment_id: Uuid,
) -> Result<Vec<lapdev_common::kube::KubeEnvironmentService>> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());
    Ok(client
        .get_environment_services(org.id, environment_id)
        .await??)
}

async fn get_environment_preview_urls(
    org: Signal<Option<Organization>>,
    environment_id: Uuid,
) -> Result<Vec<lapdev_common::kube::KubeEnvironmentPreviewUrl>> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());
    Ok(client
        .get_environment_preview_urls(org.id, environment_id)
        .await??)
}

async fn delete_environment_workload(
    org: Signal<Option<Organization>>,
    workload_id: Uuid,
    delete_modal_open: RwSignal<bool>,
    update_counter: RwSignal<usize>,
) -> Result<(), ErrorResponse> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());

    client
        .delete_environment_workload(org.id, workload_id)
        .await??;

    delete_modal_open.set(false);
    update_counter.update(|c| *c += 1);

    Ok(())
}

async fn delete_environment(
    org: Signal<Option<Organization>>,
    environment_id: Uuid,
    delete_modal_open: RwSignal<bool>,
) -> Result<(), ErrorResponse> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());

    client
        .delete_kube_environment(org.id, environment_id)
        .await??;

    delete_modal_open.set(false);

    Ok(())
}

async fn create_branch_environment(
    org: Signal<Option<Organization>>,
    base_environment_id: Uuid,
    name: String,
    create_branch_modal_open: RwSignal<bool>,
) -> Result<(), ErrorResponse> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());

    client
        .create_branch_environment(org.id, base_environment_id, name)
        .await??;

    create_branch_modal_open.set(false);

    Ok(())
}

#[component]
pub fn EnvironmentDetailView(environment_id: Uuid) -> impl IntoView {
    let org = get_current_org();
    let is_loading = RwSignal::new(false);
    let delete_modal_open = RwSignal::new(false);
    let create_branch_modal_open = RwSignal::new(false);
    let branch_name = RwSignal::new(String::new());
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

    let environment_result = LocalResource::new(move || async move {
        is_loading.set(true);
        let result = get_environment_detail(org, environment_id).await.ok();
        if let Some(env) = result.as_ref() {
            config.current_page.set(env.name.clone());
        }
        is_loading.set(false);
        result
    });

    let workloads_result = LocalResource::new(move || {
        update_counter.track();
        async move {
            let result = get_environment_workloads(org, environment_id)
                .await
                .unwrap_or_else(|_| vec![]);
            result
        }
    });

    let services_result = LocalResource::new(move || {
        update_counter.track();
        async move {
            let result = get_environment_services(org, environment_id)
                .await
                .unwrap_or_else(|_| vec![]);
            result
        }
    });

    let preview_urls_result = LocalResource::new(move || {
        update_counter.track();
        async move {
            get_environment_preview_urls(org, environment_id)
                .await
                .unwrap_or_else(|_| vec![])
        }
    });

    let environment_info = Signal::derive(move || environment_result.get().flatten());
    let all_workloads = Signal::derive(move || workloads_result.get().unwrap_or_default());
    let all_services = Signal::derive(move || services_result.get().unwrap_or_default());
    let all_preview_urls = Signal::derive(move || preview_urls_result.get().unwrap_or_default());

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

    let navigate = leptos_router::hooks::use_navigate();
    let navigate_clone = navigate.clone();
    let delete_action = Action::new_local(move |_| {
        let nav = navigate.clone();
        async move {
            match delete_environment(org, environment_id, delete_modal_open).await {
                Ok(_) => {
                    nav("/kubernetes/environments", Default::default());
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
    });

    let create_branch_action = Action::new_local(move |_| {
        let nav = navigate_clone.clone();
        async move {
            let name = branch_name.get().trim().to_string();
            if name.is_empty() {
                return Err(ErrorResponse {
                    error: "Branch environment name is required".to_string(),
                });
            }

            match create_branch_environment(org, environment_id, name, create_branch_modal_open)
                .await
            {
                Ok(_) => {
                    nav("/kubernetes/environments", Default::default());
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
                        "Loading environment details..."
                    </div>
                </div>
            </Show>

            // Environment Information Card
            <Show when=move || environment_info.get().is_some()>
                <EnvironmentInfoCard
                    environment_info
                    delete_modal_open
                    delete_action
                    create_branch_modal_open
                    create_branch_action
                    branch_name
                />
            </Show>

            // Environment Resources (Workloads & Services)
            <Show when=move || environment_info.get().is_some()>
                <EnvironmentResourcesTabs
                    environment_id
                    filtered_workloads
                    search_query
                    debounced_search
                    all_workloads
                    all_services
                    all_preview_urls
                    update_counter
                />
            </Show>

            // Delete Modal
            {move || {
                if let Some(env) = environment_info.get() {
                    view! {
                        <DeleteModal
                            resource=env.name
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
pub fn EnvironmentInfoCard(
    environment_info: Signal<Option<KubeEnvironment>>,
    delete_modal_open: RwSignal<bool>,
    delete_action: Action<(), Result<(), ErrorResponse>>,
    create_branch_modal_open: RwSignal<bool>,
    create_branch_action: Action<(), Result<(), ErrorResponse>>,
    branch_name: RwSignal<String>,
) -> impl IntoView {
    view! {
        <Show when=move || environment_info.get().is_some() fallback=|| view! { <div></div> }>
           {
                let environment = environment_info.get().unwrap();
                let env_name = environment.name.clone();
                let env_namespace = environment.namespace.clone();
                let env_namespace2 = environment.namespace.clone();
                let env_status = environment.status.clone().unwrap_or_else(|| "Unknown".to_string());
                let env_status2 = environment.status.clone().unwrap_or_else(|| "Unknown".to_string());
                let created_at_str = environment.created_at.clone();
                let is_shared = environment.is_shared;
                let is_branch = environment.base_environment_id.is_some();
                let app_catalog_id = environment.app_catalog_id;
                let app_catalog_name = environment.app_catalog_name.clone();
                let cluster_id = environment.cluster_id;
                let cluster_name = environment.cluster_name.clone();

                let status_variant = match env_status.as_str() {
                    "Running" => BadgeVariant::Secondary,
                    "Pending" => BadgeVariant::Outline,
                    "Failed" | "Error" => BadgeVariant::Destructive,
                    _ => BadgeVariant::Outline,
                };

                view! {
                    <Card class="p-6">
                        <div class="flex flex-col gap-6">
                            // Header with actions
                            <div class="flex items-start justify-between">
                                <div class="flex flex-col gap-2">
                                    <H3>{env_name}</H3>
                                    <div class="flex items-center gap-2">
                                        <Badge variant=BadgeVariant::Secondary>
                                            {env_namespace}
                                        </Badge>
                                    </div>
                                </div>
                                <div class="flex items-center gap-2">
                                    <Show when=move || is_shared>
                                        <Button
                                            variant=ButtonVariant::Outline
                                            on:click=move |_| create_branch_modal_open.set(true)
                                        >
                                            <lucide_leptos::GitBranch />
                                            Create Branch
                                        </Button>
                                    </Show>
                                    <Button
                                        variant=ButtonVariant::Destructive
                                        on:click=move |_| delete_modal_open.set(true)
                                    >
                                        <lucide_leptos::Trash2 />
                                        Delete Environment
                                    </Button>
                                </div>
                            </div>

                            // Metadata grid
                            <div class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-4 text-sm">
                                <div class="flex items-center gap-2 text-muted-foreground font-medium">
                                    <div class="[&>svg]:size-4 [&>svg]:shrink-0">
                                        <lucide_leptos::Package />
                                    </div>
                                    <span>App Catalog</span>
                                </div>
                                <div>
                                    <a href=format!("/kubernetes/catalogs/{}", app_catalog_id) class="text-foreground hover:underline">
                                        {app_catalog_name.clone()}
                                    </a>
                                </div>

                                <div class="flex items-center gap-2 text-muted-foreground font-medium">
                                    <div class="[&>svg]:size-4 [&>svg]:shrink-0">
                                        <lucide_leptos::Server />
                                    </div>
                                    <span>Cluster</span>
                                </div>
                                <div class="flex items-center gap-2">
                                    <a href=format!("/kubernetes/clusters/{}", cluster_id) class="text-foreground hover:underline">
                                        {cluster_name}
                                    </a>
                                </div>
                                // Show base environment if this is a branch
                                {if let (Some(base_name), Some(base_env_id)) = (environment.base_environment_name.clone(), environment.base_environment_id) {
                                    view! {
                                        <div class="flex items-center gap-2 text-muted-foreground font-medium">
                                            <div class="[&>svg]:size-4 [&>svg]:shrink-0">
                                                <lucide_leptos::GitBranch />
                                            </div>
                                            <span>Base Environment</span>
                                        </div>
                                        <div>
                                            <a href=format!("/kubernetes/environments/{}", base_env_id) class="text-foreground hover:underline">
                                                {base_name}
                                            </a>
                                        </div>
                                    }.into_any()
                                } else {
                                    ().into_any()
                                }}

                                <div class="flex items-center gap-2 text-muted-foreground font-medium">
                                    <div class="[&>svg]:size-4 [&>svg]:shrink-0">
                                        <lucide_leptos::Hash />
                                    </div>
                                    <span>Namespace</span>
                                </div>
                                <div class="font-mono text-sm">
                                    {env_namespace2}
                                </div>

                                <div class="flex items-center gap-2 text-muted-foreground font-medium">
                                    <div class="[&>svg]:size-4 [&>svg]:shrink-0">
                                        <lucide_leptos::Users />
                                    </div>
                                    <span>Type</span>
                                </div>
                                <div>
                                    <Badge variant=BadgeVariant::Secondary>
                                        {if is_branch {
                                            "Branch - Based on another environment"
                                        } else if is_shared {
                                            "Shared - Accessible by all organization members"
                                        } else {
                                            "Personal - Private to you"
                                        }}
                                    </Badge>
                                </div>

                                <div class="flex items-center gap-2 text-muted-foreground font-medium">
                                    <div class="[&>svg]:size-4 [&>svg]:shrink-0">
                                        <lucide_leptos::Calendar />
                                    </div>
                                    <span>Created</span>
                                </div>
                                <div>
                                    {match chrono::DateTime::parse_from_str(&created_at_str, "%Y-%m-%d %H:%M:%S%.f %z") {
                                        Ok(time) => view! { <DatetimeModal time /> }.into_any(),
                                        Err(_) => view! {
                                            <span class="text-sm text-muted-foreground">{created_at_str}</span>
                                        }.into_any(),
                                    }}
                                </div>

                                <div class="flex items-center gap-2 text-muted-foreground font-medium">
                                    <div class="[&>svg]:size-4 [&>svg]:shrink-0">
                                        <lucide_leptos::Activity />
                                    </div>
                                    <span>Status</span>
                                </div>
                                <div>
                                    <Badge variant=status_variant class="text-sm">
                                        {env_status2}
                                    </Badge>
                                </div>
                            </div>
                        </div>
                    </Card>

                    // Create Branch Environment Modal
                    <CreateBranchEnvironmentModal
                        open=create_branch_modal_open
                        create_branch_action
                        branch_name
                    />
                }
            }
        </Show>
    }
}

#[component]
pub fn EnvironmentResourcesCard(
    environment_info: Signal<Option<KubeEnvironment>>,
) -> impl IntoView {
    view! {
        <Show when=move || environment_info.get().is_some() fallback=|| view! { <div></div> }>
            {move || {
                let environment = environment_info.get().unwrap();
                let cluster_id = environment.cluster_id;
                let namespace = environment.namespace.clone();

                view! {
                    <Card class="p-6">
                        <div class="flex flex-col gap-4">
                            <div class="flex items-center justify-between">
                                <H4>Environment Resources</H4>
                            </div>

                            <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
                                // Namespace info
                                <div class="p-4 bg-muted/30 rounded-lg border">
                                    <div class="flex items-center gap-2 mb-2">
                                        <lucide_leptos::Hash attr:class="h-4 w-4 text-muted-foreground" />
                                        <span class="font-medium text-sm">Namespace</span>
                                    </div>
                                    <div class="font-mono text-sm">{namespace.clone()}</div>
                                    <div class="text-xs text-muted-foreground mt-1">
                                        Kubernetes namespace containing all resources for this environment
                                    </div>
                                </div>

                                // Cluster info
                                <div class="p-4 bg-muted/30 rounded-lg border">
                                    <div class="flex items-center gap-2 mb-2">
                                        <lucide_leptos::Server attr:class="h-4 w-4 text-muted-foreground" />
                                        <span class="font-medium text-sm">Target Cluster</span>
                                    </div>
                                    <div class="text-sm">{environment.cluster_name.clone()}</div>
                                    <div class="text-xs text-muted-foreground mt-1">
                                        Kubernetes cluster where this environment is deployed
                                    </div>
                                </div>

                                // App catalog info
                                <div class="p-4 bg-muted/30 rounded-lg border">
                                    <div class="flex items-center gap-2 mb-2">
                                        <lucide_leptos::Package attr:class="h-4 w-4 text-muted-foreground" />
                                        <span class="font-medium text-sm">Source Catalog</span>
                                    </div>
                                    <div class="text-sm">{environment.app_catalog_name.clone()}</div>
                                    <div class="text-xs text-muted-foreground mt-1">
                                        App catalog used to create this environment
                                    </div>
                                </div>
                            </div>

                            <div class="border-t pt-4">
                                <div class="flex items-center gap-2 text-sm text-muted-foreground">
                                    <lucide_leptos::Info attr:class="h-4 w-4" />
                                    <span>
                                        This environment contains all workloads from the source app catalog deployed in the specified namespace.
                                        Use the actions menu to view the actual Kubernetes resources or navigate to related components.
                                    </span>
                                </div>
                            </div>
                        </div>
                    </Card>
                }
            }}
        </Show>
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResourceTab {
    Workloads,
    Services,
    PreviewUrls,
}

impl std::fmt::Display for ResourceTab {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResourceTab::Workloads => write!(f, "workloads"),
            ResourceTab::Services => write!(f, "services"),
            ResourceTab::PreviewUrls => write!(f, "preview-urls"),
        }
    }
}

#[component]
pub fn EnvironmentResourcesTabs(
    environment_id: Uuid,
    filtered_workloads: Signal<Vec<KubeEnvironmentWorkload>>,
    search_query: RwSignal<String>,
    debounced_search: RwSignal<String>,
    all_workloads: Signal<Vec<KubeEnvironmentWorkload>>,
    all_services: Signal<Vec<KubeEnvironmentService>>,
    all_preview_urls: Signal<Vec<lapdev_common::kube::KubeEnvironmentPreviewUrl>>,
    update_counter: RwSignal<usize>,
) -> impl IntoView {
    view! {
        <Tabs default_value=RwSignal::new(ResourceTab::Workloads) class="gap-4">
            <TabsList>
                <TabsTrigger value=ResourceTab::Workloads>
                    "Workloads"
                    <Badge variant=BadgeVariant::Secondary>
                        {move || all_workloads.get().len()}
                    </Badge>
                </TabsTrigger>
                <TabsTrigger value=ResourceTab::Services>
                    "Services"
                    <Badge variant=BadgeVariant::Secondary>
                        {move || all_services.get().len()}
                    </Badge>
                </TabsTrigger>
                <TabsTrigger value=ResourceTab::PreviewUrls>
                    "Preview URLs"
                    <Badge variant=BadgeVariant::Secondary>
                        {move || all_preview_urls.get().len()}
                    </Badge>
                </TabsTrigger>
            </TabsList>

            <TabsContent value=ResourceTab::Workloads>
                <EnvironmentWorkloadsContent
                    environment_id
                    filtered_workloads
                    search_query
                    debounced_search
                    all_workloads
                    update_counter
                />
            </TabsContent>

            <TabsContent value=ResourceTab::Services>
                <EnvironmentServicesContent
                    environment_id
                    all_services
                    update_counter
                />
            </TabsContent>

            <TabsContent value=ResourceTab::PreviewUrls>
                <crate::kube_environment_preview_url::PreviewUrlsContent
                    environment_id
                    services=all_services
                    preview_urls=all_preview_urls
                    update_counter
                />
            </TabsContent>
        </Tabs>
    }
}

#[component]
pub fn EnvironmentWorkloadsContent(
    environment_id: Uuid,
    filtered_workloads: Signal<Vec<KubeEnvironmentWorkload>>,
    search_query: RwSignal<String>,
    debounced_search: RwSignal<String>,
    all_workloads: Signal<Vec<KubeEnvironmentWorkload>>,
    update_counter: RwSignal<usize>,
) -> impl IntoView {
    view! {
            <div class="flex flex-col gap-4">
                <div class="relative max-w-sm">
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

                // Workloads table
                <div class="rounded-lg border relative">
                    <Table>
                        <TableHeader class="bg-muted">
                            <TableRow>
                                <TableHead>Name</TableHead>
                                <TableHead>Namespace</TableHead>
                                <TableHead>Kind</TableHead>
                                <TableHead>Containers</TableHead>
                                <TableHead>Created</TableHead>
                                <TableHead>Actions</TableHead>
                            </TableRow>
                        </TableHeader>
                        <TableBody>
                            <For
                                each=move || filtered_workloads.get()
                                key=|workload| format!("{}-{}-{}", workload.name, workload.namespace, workload.kind)
                                children=move |workload| {
                                    view! { <EnvironmentWorkloadItem environment_id workload=workload.clone() update_counter /> }
                                }
                            />
                        </TableBody>
                    </Table>

                    // Empty states
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
                                            "This environment doesn't contain any workloads yet."
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
pub fn EnvironmentWorkloadItem(
    environment_id: Uuid,
    workload: KubeEnvironmentWorkload,
    update_counter: RwSignal<usize>,
) -> impl IntoView {
    let org = get_current_org();
    let delete_modal_open = RwSignal::new(false);
    let workload_name = workload.name.clone();
    let workload_id = workload.id;

    let delete_action = Action::new_local(move |_| {
        delete_environment_workload(org, workload_id, delete_modal_open, update_counter)
    });

    view! {
        <TableRow>
            <TableCell>
                <a href=format!("/kubernetes/environments/{}/workloads/{}", environment_id, workload.id)>
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
                <DatetimeModal time=workload.created_at />
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
pub fn ContainerDisplay(container: KubeContainerInfo) -> impl IntoView {
    view! {
        <div class="flex flex-col p-2 bg-muted/30 rounded border">
            <div class="flex justify-between items-center">
                <span class="font-medium text-foreground">{container.name.clone()}</span>
                {if container.is_customized() {
                    view! {
                        <Badge variant=BadgeVariant::Secondary class="text-xs">
                            <lucide_leptos::Settings attr:class="w-3 h-3 mr-1" />
                            "Customized"
                        </Badge>
                    }.into_any()
                } else {
                    ().into_any()
                }}
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

#[component]
pub fn EnvironmentServicesContent(
    environment_id: Uuid,
    all_services: Signal<Vec<KubeEnvironmentService>>,
    update_counter: RwSignal<usize>,
) -> impl IntoView {
    view! {
                // Services table
                <div class="rounded-lg border relative">
                    <Table>
                        <TableHeader class="bg-muted">
                            <TableRow>
                                <TableHead>Name</TableHead>
                                <TableHead>Namespace</TableHead>
                                <TableHead>Ports</TableHead>
                                <TableHead>Selectors</TableHead>
                                <TableHead>Created</TableHead>
                                <TableHead>Actions</TableHead>
                            </TableRow>
                        </TableHeader>
                        <TableBody>
                            <For
                                each=move || all_services.get()
                                key=|service| format!("{}-{}", service.name, service.namespace)
                                children=move |service| {
                                    view! {
                                        <EnvironmentServiceItem
                                            environment_id
                                            service=service.clone()
                                            all_services
                                            update_counter
                                        />
                                    }
                                }
                            />
                        </TableBody>
                    </Table>

                    // Empty state
                    {move || {
                        let services = all_services.get();
                        if services.is_empty() {
                            view! {
                                <div class="flex flex-col items-center justify-center py-12 text-center">
                                    <div class="rounded-full bg-muted p-3 mb-4">
                                        <lucide_leptos::Network />
                                    </div>
                                    <H4 class="mb-2">No Services Found</H4>
                                    <P class="text-muted-foreground mb-4 max-w-sm">
                                        "This environment doesn't contain any services yet."
                                    </P>
                                </div>
                            }
                            .into_any()
                        } else {
                            view! { <div></div> }.into_any()
                        }
                    }}
                </div>
    }
}

#[component]
pub fn EnvironmentServiceItem(
    environment_id: Uuid,
    service: KubeEnvironmentService,
    all_services: Signal<Vec<KubeEnvironmentService>>,
    update_counter: RwSignal<usize>,
) -> impl IntoView {
    let create_preview_url_modal_open = RwSignal::new(false);
    let ports = service.ports.clone();
    let selectors = service.selector.clone();
    let service_for_button = service.clone();
    let service_for_modal = service.clone();

    view! {
        <TableRow>
            <TableCell>
                <div class="font-medium">{service.name.clone()}</div>
            </TableCell>
            <TableCell>
                <Badge variant=BadgeVariant::Secondary>
                    {service.namespace.clone()}
                </Badge>
            </TableCell>
            <TableCell>
                <div class="text-sm flex flex-col gap-1">
                    {if ports.is_empty() {
                        view! { <span class="text-muted-foreground">"No ports"</span> }.into_any()
                    } else {
                        ports.into_iter().map(|port| {
                            let target_port = port
                                .target_port
                                .map(|tp| format!(":{tp}"))
                                .unwrap_or_default();
                            let protocol = port.protocol.as_deref().unwrap_or("TCP");
                            let port_text = format!("{}{} ({})", port.port, target_port, protocol);
                            view! {
                                <Badge variant=BadgeVariant::Outline class="text-xs">
                                    {port_text}
                                </Badge>
                            }
                        }).collect::<Vec<_>>().into_any()
                    }}
                </div>
            </TableCell>
            <TableCell>
                <div class="text-sm flex flex-col gap-1">
                    {if selectors.is_empty() {
                        view! { <span class="text-muted-foreground">"No selectors"</span> }.into_any()
                    } else {
                        selectors.into_iter().map(|(k, v)| {
                            let selector_text = format!("{k}={v}");
                            view! {
                                <Badge variant=BadgeVariant::Secondary class="text-xs">
                                    {selector_text}
                                </Badge>
                            }
                        }).collect::<Vec<_>>().into_any()
                    }}
                </div>
            </TableCell>
            <TableCell>
                <DatetimeModal time=service.created_at />
            </TableCell>
            <TableCell>
                <Button
                    variant=ButtonVariant::Outline
                    size=ButtonSize::Sm
                    on:click=move |_| create_preview_url_modal_open.set(true)
                    disabled=Signal::derive(move || service_for_button.ports.is_empty())
                >
                    <lucide_leptos::Globe />
                    "Create Preview URL"
                </Button>

                // Quick Create Preview URL Modal
                <crate::kube_environment_preview_url::CreatePreviewUrlModal
                    open=create_preview_url_modal_open
                    environment_id
                    service=service_for_modal.clone()
                    update_counter
                />
            </TableCell>
        </TableRow>
    }
}

#[component]
pub fn CreateBranchEnvironmentModal(
    open: RwSignal<bool>,
    create_branch_action: Action<(), Result<(), ErrorResponse>>,
    branch_name: RwSignal<String>,
) -> impl IntoView {
    // Reset form when modal opens/closes
    Effect::new(move |_| {
        if !open.get() {
            branch_name.set(String::new());
        }
    });

    view! {
        <Modal
            open=open
            action=create_branch_action
            title="Create Branch Environment"
            action_text="Create Branch"
            action_progress_text="Creating..."
        >
            <div class="flex flex-col gap-4">
                <div class="flex flex-col gap-2">
                    <Label for_="branch-name">Branch Environment Name</Label>
                    <Input
                        attr:id="branch-name"
                        prop:value=move || branch_name.get()
                        on:input=move |ev| {
                            branch_name.set(event_target_value(&ev));
                        }
                        attr:placeholder="Enter a name for the branch environment"
                    />
                    <P class="text-sm text-muted-foreground">
                        "A new environment will be created based on this environment's configuration."
                    </P>
                </div>
            </div>
        </Modal>
    }
}
