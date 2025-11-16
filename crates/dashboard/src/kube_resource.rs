use std::{collections::HashSet, str::FromStr};

use anyhow::{anyhow, Result};
use lapdev_common::{
    console::Organization,
    kube::{
        KubeAppCatalogWorkloadCreate, KubeCluster, KubeClusterInfo, KubeClusterStatus,
        KubeNamespace, KubeNamespaceInfo, KubeWorkload, KubeWorkloadKind, KubeWorkloadList,
        KubeWorkloadStatus, PaginationCursor, PaginationParams,
    },
};
use leptos::{prelude::*, task::spawn_local_scoped_with_cancellation};
use leptos_router::hooks::{use_location, use_params_map};
use uuid::Uuid;

use crate::{
    app::{get_hrpc_client, AppConfig},
    component::{
        badge::{Badge, BadgeVariant},
        button::{Button, ButtonSize, ButtonVariant},
        card::Card,
        checkbox::Checkbox,
        input::Input,
        label::Label,
        select::{
            Select, SelectContent, SelectItem, SelectSearchInput, SelectSeparator, SelectTrigger,
        },
        table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow},
        textarea::Textarea,
        typography::{H3, H4, P},
    },
    docs_url,
    kube_app_catalog_detail::get_app_catalog,
    modal::{ErrorResponse, Modal},
    organization::get_current_org,
    sse::run_sse_with_retry,
    DOCS_CLUSTER_PATH,
};

#[component]
pub fn KubeResource() -> impl IntoView {
    let update_counter = RwSignal::new_local(0);
    let params = use_params_map();
    let cluster_id = params.with_untracked(|params| params.get("cluster_id").clone().unwrap());
    let cluster_id = Uuid::from_str(&cluster_id).unwrap_or_default();

    view! {
        <div class="flex flex-col gap-6">
            <div class="flex flex-col gap-2 items-start">
                <H3>Kubernetes Resources</H3>
                <P>
                    View and manage workloads, services, and configurations in your Kubernetes clusters.
                </P>
                <a href=docs_url(DOCS_CLUSTER_PATH) target="_blank" rel="noopener noreferrer">
                    <Badge variant=BadgeVariant::Secondary>Docs <lucide_leptos::ExternalLink /></Badge>
                </a>
            </div>

            <ClusterInfo cluster_id update_counter />

            <KubeResourceList update_counter cluster_id />
        </div>
    }
}

async fn get_workloads_from_api(
    org: Signal<Option<Organization>>,
    cluster_id: uuid::Uuid,
    namespace_filter: Option<String>,
    workload_kind_filter: Option<KubeWorkloadKind>,
    include_system_workloads: bool,
    pagination: PaginationParams,
) -> Result<KubeWorkloadList> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = get_hrpc_client();

    Ok(client
        .get_workloads(
            org.id,
            cluster_id,
            namespace_filter,
            workload_kind_filter,
            include_system_workloads,
            Some(pagination),
        )
        .await??)
}

async fn get_namespaces_from_api(
    org: Signal<Option<Organization>>,
    cluster_id: uuid::Uuid,
) -> Result<Vec<KubeNamespaceInfo>> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = get_hrpc_client();

    Ok(client.get_cluster_namespaces(org.id, cluster_id).await??)
}

async fn get_cluster_info_from_api(
    org: Signal<Option<Organization>>,
    cluster_id: uuid::Uuid,
) -> Result<KubeCluster> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = get_hrpc_client();

    Ok(client.get_cluster_info(org.id, cluster_id).await??)
}

#[component]
pub fn ClusterInfo(
    cluster_id: Uuid,
    update_counter: RwSignal<usize, LocalStorage>,
) -> impl IntoView {
    let org = get_current_org();

    let config = use_context::<AppConfig>().unwrap();
    let cluster_info_resource =
        LocalResource::new(
            move || async move { get_cluster_info_from_api(org, cluster_id).await.ok() },
        );

    let cluster_info_signal = cluster_info_resource.clone();
    let cluster = Signal::derive(move || {
        let cluster_data = cluster_info_signal.get();
        if let Some(Some(cluster)) = cluster_data.as_ref() {
            config.current_page.set(cluster.name.clone());
        }
        cluster_data.flatten().unwrap_or_else(|| KubeCluster {
            id: cluster_id,
            name: "Unknown".to_string(),
            can_deploy_personal: false,
            can_deploy_shared: false,
            info: KubeClusterInfo {
                cluster_version: "Unknown".to_string(),
                node_count: 0,
                available_cpu: "N/A".to_string(),
                available_memory: "N/A".to_string(),
                provider: None,
                region: None,
                status: KubeClusterStatus::NotReady,
                manager_namespace: None,
            },
        })
    });

    let sse_started = RwSignal::new_local(false);
    let cluster_info_for_sse = cluster_info_resource.clone();
    let org_for_sse = org;
    Effect::new(move |_| {
        if sse_started.get_untracked() {
            return;
        }

        if let Some(org) = org_for_sse.get() {
            sse_started.set(true);
            let org_id = org.id;
            spawn_local_scoped_with_cancellation({
                let cluster_info_for_sse = cluster_info_for_sse.clone();
                async move {
                    let url = format!(
                        "/api/v1/organizations/{}/kube/clusters/{}/events",
                        org_id, cluster_id
                    );
                    let listener_name = format!("cluster {} detail", cluster_id);
                    run_sse_with_retry(url, "cluster", listener_name, move |_message| {
                        cluster_info_for_sse.refetch();
                        update_counter.update(|c| *c += 1);
                    })
                    .await;
                }
            });
        }
    });

    view! {
        <Card class="p-6 flex flex-col gap-4">
            <H4>Cluster Information</H4>
            {move || {
                let cluster = cluster.get();
                let info = cluster.info.clone();

                let cluster_name = cluster.name.clone();
                let cluster_version = info.cluster_version.clone();
                let status = info.status.clone();
                let status_label = status.to_string();
                let status_variant = match status {
                    KubeClusterStatus::Ready => BadgeVariant::Secondary,
                    KubeClusterStatus::NotReady => BadgeVariant::Destructive,
                    KubeClusterStatus::Error => BadgeVariant::Destructive,
                    KubeClusterStatus::Provisioning => BadgeVariant::Outline,
                };
                let region = info.region.clone().unwrap_or_else(|| "N/A".to_string());
                let provider = info.provider.clone().unwrap_or_else(|| "Unknown".to_string());

                view! {
                    <div class="grid grid-cols-2 md:grid-cols-5 gap-4">
                        <div>
                            <div class="flex flex-col gap-1">
                                <Label>Name</Label>
                                <P class="font-medium">{cluster_name}</P>
                            </div>
                        </div>
                        <div>
                            <div class="flex flex-col gap-1">
                                <Label>Version</Label>
                                <P class="font-medium">{cluster_version}</P>
                            </div>
                        </div>
                        <div>
                            <div class="flex flex-col gap-1">
                                <Label>Status</Label>
                                <Badge variant=status_variant>{status_label}</Badge>
                            </div>
                        </div>
                        <div>
                            <div class="flex flex-col gap-1">
                                <Label>Provider</Label>
                                <P class="font-medium">{provider}</P>
                            </div>
                        </div>
                        <div>
                            <div class="flex flex-col gap-1">
                                <Label>Region</Label>
                                <P class="font-medium">{region}</P>
                            </div>
                        </div>
                    </div>
                }
            }}
        </Card>
    }
}

#[component]
pub fn KubeResourceList(
    update_counter: RwSignal<usize, LocalStorage>,
    cluster_id: Uuid,
) -> impl IntoView {
    let org = get_current_org();
    let location = use_location();

    // Initialize from URL search parameters
    let initial_namespace = move || {
        let search = location.search.get_untracked();
        let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
        params.get("namespace")
    };

    let catalog_id_from_url = {
        let search = location.search.get_untracked();
        let params = web_sys::UrlSearchParams::new_with_str(&search).ok();
        params.and_then(|p| p.get("catalog_id").and_then(|id| Uuid::from_str(&id).ok()))
    };

    let namespace_filter = RwSignal::new_local(initial_namespace());
    let kind_filter = RwSignal::new_local(None::<KubeWorkloadKind>);
    let limit = RwSignal::new_local(20usize);
    let is_loading = RwSignal::new(false);
    let workload_cache = RwSignal::new_local(WorkloadCache::new(kind_filter.get_untracked()));
    let cache_offset = RwSignal::new_local(0usize);

    let namespace_select_value = RwSignal::new(initial_namespace());
    let workload_kind_select_value = RwSignal::new(None::<KubeWorkloadKind>);
    let limit_select_value = RwSignal::new(20usize);
    let include_system_workloads = RwSignal::new(false);

    let cluster_resource =
        LocalResource::new(
            move || async move { get_cluster_info_from_api(org, cluster_id).await.ok() },
        );
    let cluster_status = Signal::derive(move || {
        cluster_resource
            .get()
            .flatten()
            .map(|cluster| cluster.info.status)
            .unwrap_or(KubeClusterStatus::NotReady)
    });

    let workloads = LocalResource::new(move || async move {
        is_loading.set(true);
        let offset = cache_offset.get_untracked();
        let limit = limit.get_untracked();
        let needs_more_retrieve =
            workload_cache.with_untracked(|c| c.needs_more_retrieve(offset, limit));

        if needs_more_retrieve {
            let pagination_params = PaginationParams {
                cursor: workload_cache.with_untracked(|c| c.cursor.clone()),
                limit,
            };
            let result = get_workloads_from_api(
                org,
                cluster_id,
                namespace_filter.get_untracked(),
                kind_filter.get_untracked(),
                include_system_workloads.get_untracked(),
                pagination_params,
            )
            .await
            .unwrap_or_else(|_| KubeWorkloadList {
                workloads: vec![],
                next_cursor: None,
            });

            workload_cache.update(|c| {
                c.update_cache(result);
            });
        }

        is_loading.set(false);
        workload_cache.with_untracked(|c| c.retrieve_list(offset, limit))
    });

    Effect::new(move |_| {
        update_counter.track();
        namespace_filter.track();
        kind_filter.track();
        include_system_workloads.track();
        limit.track();

        // Clear cache when filters change or limit changes
        workload_cache.set(WorkloadCache::new(kind_filter.get_untracked()));
        cache_offset.set(0);
        workloads.refetch();
    });

    Effect::new(move |_| {
        cache_offset.track();
        workloads.refetch();
    });

    let workload_list = Signal::derive(move || workloads.get().unwrap_or_default());

    let filtered_workloads = Signal::derive(move || workload_list.get());

    let detail_modal_open = RwSignal::new(false);
    let selected_workload = RwSignal::new_local(None::<KubeWorkload>);

    // Sync namespace select with namespace filter and update URL
    Effect::new(move |_| {
        let selected_namespace = namespace_select_value.get();
        let current_filter = namespace_filter.get_untracked();
        if selected_namespace != current_filter {
            namespace_filter.set(selected_namespace.clone());

            // Update URL with new namespace parameter (without navigation)
            let current_path = location.pathname.get_untracked();
            let search = location.search.get_untracked();
            let params = web_sys::UrlSearchParams::new_with_str(&search)
                .unwrap_or_else(|_| web_sys::UrlSearchParams::new().unwrap());

            if let Some(ns) = &selected_namespace {
                params.set("namespace", ns);
            } else {
                params.delete("namespace");
            }

            let new_search = params.to_string().as_string().unwrap_or_default();
            let new_url = if new_search.is_empty() {
                current_path
            } else {
                format!("{}?{}", current_path, new_search)
            };

            // Use replaceState to update URL without navigation
            if let Some(history) = web_sys::window().and_then(|w| w.history().ok()) {
                let _ = history.replace_state_with_url(
                    &wasm_bindgen::JsValue::NULL,
                    "",
                    Some(&new_url),
                );
            }
        }
    });

    // Sync workload kind select with kind filter
    Effect::new(move |_| {
        let selected_kind = workload_kind_select_value.get();
        let current_filter = kind_filter.get_untracked();
        if selected_kind != current_filter {
            kind_filter.set(selected_kind);
        }
    });

    // Sync limit select with limit signal
    Effect::new(move |_| {
        let selected_limit = limit_select_value.get();
        if selected_limit != limit.get_untracked() {
            limit.set(selected_limit);
        }
    });

    let selected_workloads = RwSignal::new(std::collections::HashSet::<WorkloadKey>::new());

    view! {
        <div class="flex flex-col gap-4">
            <AppCatalogActions
                selected_workloads
                filtered_workloads
                cluster_id
                catalog_id_from_url
            />
            <div class="flex flex-wrap gap-4 items-center">
               <NamespaceFilters namespace_select_value cluster_id />
               <WorkloadFilters workload_kind_select_value />
               <SystemWorkloadsCheckbox include_system_workloads />
            </div>
            <WorkloadPagination
                workload_list
                limit_select_value
                workload_cache
                cache_offset
                is_loading=is_loading.read_only().into()
            />

            <WorkloadTable
                selected_workloads
                filtered_workloads
                is_loading=is_loading.read_only().into()
                cluster_status
                on_view_details=Callback::new(move |w| {
                    selected_workload.set(Some(w));
                    detail_modal_open.set(true);
                })
            />

            <WorkloadDetailModal modal_open=detail_modal_open workload=selected_workload />
        </div>
    }
}

#[component]
pub fn KubeResourceItem(
    workload: KubeWorkload,
    selected_workloads: RwSignal<std::collections::HashSet<WorkloadKey>>,
    on_view_details: impl Fn(KubeWorkload) + 'static,
) -> impl IntoView {
    let status_variant = match workload.status {
        KubeWorkloadStatus::Running => BadgeVariant::Secondary,
        KubeWorkloadStatus::Succeeded => BadgeVariant::Secondary,
        KubeWorkloadStatus::Pending => BadgeVariant::Secondary,
        KubeWorkloadStatus::Failed | KubeWorkloadStatus::Unknown => BadgeVariant::Destructive,
    };

    let status_text = workload.status.to_string();
    let replica_text = match (workload.ready_replicas, workload.replicas) {
        (Some(ready), Some(total)) => format!("{}/{}", ready, total),
        (Some(ready), None) => ready.to_string(),
        (None, Some(total)) => format!("?/{}", total),
        (None, None) => "-".to_string(),
    };

    let age_text = workload
        .created_at
        .as_ref()
        .map(|_| "TODO: calculate age".to_string())
        .unwrap_or_else(|| "-".to_string());

    let workload_key = WorkloadKey::from_workload(&workload);
    let workload_key_clone = workload_key.clone();
    let is_selected =
        Signal::derive(move || selected_workloads.with(|s| s.contains(&workload_key)));

    let toggle_selection = move |_| {
        selected_workloads.update(|selected| {
            if selected.contains(&workload_key_clone) {
                selected.remove(&workload_key_clone);
            } else {
                selected.insert(workload_key_clone.clone());
            }
        });
    };

    view! {
        <TableRow>
            <TableCell class="w-10">
                <div class="flex items-center justify-center">
                    <Checkbox
                        prop:checked=move || is_selected.get()
                        on:change=toggle_selection
                    />
                </div>
            </TableCell>
            <TableCell>
                <span class="font-medium">{workload.name.clone()}</span>
            </TableCell>
            <TableCell>
                <Badge variant=BadgeVariant::Outline>{workload.namespace.clone()}</Badge>
            </TableCell>
            <TableCell>{format!("{:?}", workload.kind)}</TableCell>
            <TableCell>
                <Badge variant=status_variant>{status_text}</Badge>
            </TableCell>
            <TableCell>{replica_text}</TableCell>
            <TableCell>{age_text}</TableCell>
            <TableCell class="pr-4">
                <Button variant=ButtonVariant::Outline size=ButtonSize::Sm>
                    // on:click=view_details
                    <lucide_leptos::Eye />
                    Details
                </Button>
            </TableCell>
        </TableRow>
    }
}

#[component]
pub fn WorkloadDetailModal(
    modal_open: RwSignal<bool>,
    workload: RwSignal<Option<KubeWorkload>, LocalStorage>,
) -> impl IntoView {
    view! {
        <Modal
            title="Workload Details"
            open=modal_open
            action=Action::new_local(move |_| async move { Ok(()) })
            hide_action=true
        >
            {move || {
                if let Some(w) = workload.get() {
                    view! {
                        <div class="flex flex-col gap-4">
                            <div class="grid grid-cols-2 gap-4">
                                <div>
                                    <Label>Name</Label>
                                    <P class="font-mono text-sm">{w.name.clone()}</P>
                                </div>
                                <div>
                                    <Label>Namespace</Label>
                                    <P class="font-mono text-sm">{w.namespace.clone()}</P>
                                </div>
                                <div>
                                    <Label>Kind</Label>
                                    <P class="font-mono text-sm">{format!("{:?}", w.kind)}</P>
                                </div>
                                <div>
                                    <Label>Status</Label>
                                    <P class="font-mono text-sm">{w.status.to_string()}</P>
                                </div>
                                {if let (Some(ready), Some(total)) = (
                                    w.ready_replicas,
                                    w.replicas,
                                ) {
                                    view! {
                                        <div>
                                            <Label>Replicas</Label>
                                            <P class="font-mono text-sm">
                                                {format!("{}/{}", ready, total)}
                                            </P>
                                        </div>
                                    }
                                        .into_any()
                                } else {
                                    view! { <div></div> }.into_any()
                                }}
                                {if let Some(created) = &w.created_at {
                                    let created_str = created.clone();
                                    view! {
                                        <div>
                                            <Label>Created</Label>
                                            <P class="font-mono text-sm">{created_str}</P>
                                        </div>
                                    }
                                        .into_any()
                                } else {
                                    view! { <div></div> }.into_any()
                                }}
                            </div>

                            {if !w.labels.is_empty() {
                                let labels_vec: Vec<_> = w
                                    .labels
                                    .iter()
                                    .map(|(k, v)| (k.clone(), v.clone()))
                                    .collect();
                                view! {
                                    <div>
                                        <Label>Labels</Label>
                                        <div class="mt-2 space-y-1">
                                            <For
                                                each=move || labels_vec.clone()
                                                key=|(k, _)| k.clone()
                                                children=move |(key, value)| {
                                                    view! {
                                                        <div class="flex items-center gap-2">
                                                            <Badge
                                                                variant=BadgeVariant::Outline
                                                                class="font-mono text-xs"
                                                            >
                                                                {format!("{}={}", key, value)}
                                                            </Badge>
                                                        </div>
                                                    }
                                                }
                                            />
                                        </div>
                                    </div>
                                }
                                    .into_any()
                            } else {
                                view! { <div></div> }.into_any()
                            }}
                        </div>
                    }
                        .into_any()
                } else {
                    view! { <P>No workload selected</P> }.into_any()
                }
            }}
        </Modal>
    }
}

#[component]
pub fn SystemWorkloadsCheckbox(include_system_workloads: RwSignal<bool>) -> impl IntoView {
    view! {
        <div class="flex items-center gap-2">
            <Checkbox
                attr:id="system-workloads"
                prop:checked=move || include_system_workloads.get()
                on:change=move |ev| {
                    let checked = event_target_checked(&ev);
                    include_system_workloads.set(checked);
                }
            />
            <label for="system-workloads" class="text-sm text-gray-700">
                "Show System Workloads"
            </label>
        </div>
    }
}

#[component]
pub fn WorkloadFilters(
    workload_kind_select_value: RwSignal<Option<KubeWorkloadKind>>,
) -> impl IntoView {
    let workload_kind_select_open = RwSignal::new(false);

    view! {
        <Select open=workload_kind_select_open value=workload_kind_select_value>
            <SelectTrigger
                class="w-48"
            >
                {move || match workload_kind_select_value.get() {
                    Some(k) => k.to_string(),
                    None => "All Workload Types".to_string(),
                }}
            </SelectTrigger>
            <SelectContent
                class="w-48"
            >
                <SelectItem value={None::<KubeWorkloadKind>}>All Types</SelectItem>
                <SelectItem value=Some(KubeWorkloadKind::Deployment)>Deployment</SelectItem>
                <SelectItem value=Some(KubeWorkloadKind::StatefulSet)>StatefulSet</SelectItem>
                <SelectItem value=Some(KubeWorkloadKind::DaemonSet)>DaemonSet</SelectItem>
                <SelectItem value=Some(KubeWorkloadKind::Pod)>Pod</SelectItem>
                <SelectItem value=Some(KubeWorkloadKind::Job)>Job</SelectItem>
                <SelectItem value=Some(KubeWorkloadKind::CronJob)>CronJob</SelectItem>
            </SelectContent>
        </Select>
    }
}

#[component]
pub fn NamespaceFilters(
    namespace_select_value: RwSignal<Option<String>>,
    cluster_id: Uuid,
) -> impl IntoView {
    let namespace_select_open = RwSignal::new(false);
    let org = get_current_org();

    let namespaces = LocalResource::new(move || async move {
        get_namespaces_from_api(org, cluster_id)
            .await
            .unwrap_or_default()
    });

    view! {
        <Select
            open=namespace_select_open
            value=namespace_select_value
        >
            <SelectTrigger
                class="w-64"
            >
                {move || match namespace_select_value.get() {
                    Some(ns) => ns,
                    None => "All Namespaces".to_string(),
                }}
            </SelectTrigger>
            <SelectContent
                class="w-64"
            >
                <SelectSearchInput />
                <SelectSeparator />
                <SelectItem value={None::<String>}>All Namespaces</SelectItem>
                <For
                    each=move || namespaces.get().unwrap_or_default()
                    key=|ns| ns.name.clone()
                    children=move |namespace| {
                        let name = namespace.name.clone();
                        view! {
                            <SelectItem value=Some(name.clone()) display=name.clone()>{name}</SelectItem>
                        }
                    }
                />
            </SelectContent>
        </Select>
    }
}

async fn create_app_catalog_api(
    org: Signal<Option<Organization>>,
    cluster_id: Uuid,
    name: String,
    description: String,
    selected_workloads: std::collections::HashSet<WorkloadKey>,
    filtered_workloads: Vec<KubeWorkload>,
) -> Result<Uuid, ErrorResponse> {
    let org = org
        .get_untracked()
        .ok_or_else(|| anyhow!("can't get org"))?;

    let selected_workloads_data: Vec<KubeWorkload> = filtered_workloads
        .into_iter()
        .filter(|w| selected_workloads.contains(&WorkloadKey::from_workload(w)))
        .collect();

    if name.trim().is_empty() {
        return Err(anyhow!("Catalog name is required"))?;
    }

    if selected_workloads_data.is_empty() {
        return Err(anyhow!(
            "At least one workload must be selected to create a catalog"
        ))?;
    }

    // Convert KubeWorkload to KubeAppCatalogWorkloadCreate
    let workloads: Vec<KubeAppCatalogWorkloadCreate> = selected_workloads_data
        .into_iter()
        .map(|w| KubeAppCatalogWorkloadCreate {
            name: w.name,
            namespace: w.namespace,
            kind: w.kind,
        })
        .collect();

    let client = get_hrpc_client();
    let catalog_id = client
        .create_app_catalog(
            org.id,
            cluster_id,
            name.clone(),
            if description.trim().is_empty() {
                None
            } else {
                Some(description)
            },
            workloads,
        )
        .await??;

    Ok(catalog_id)
}

async fn add_workloads_to_catalog_api(
    org: Signal<Option<Organization>>,
    catalog_id: Uuid,
    selected_workloads: std::collections::HashSet<WorkloadKey>,
    filtered_workloads: Vec<KubeWorkload>,
) -> Result<(), ErrorResponse> {
    let org = org
        .get_untracked()
        .ok_or_else(|| anyhow!("can't get org"))?;

    let selected_workloads_data: Vec<KubeWorkload> = filtered_workloads
        .into_iter()
        .filter(|w| selected_workloads.contains(&WorkloadKey::from_workload(w)))
        .collect();

    if selected_workloads_data.is_empty() {
        return Err(anyhow!(
            "At least one workload must be selected to add to the catalog"
        ))?;
    }

    // Convert KubeWorkload to KubeAppCatalogWorkloadCreate
    let workloads: Vec<KubeAppCatalogWorkloadCreate> = selected_workloads_data
        .into_iter()
        .map(|w| KubeAppCatalogWorkloadCreate {
            name: w.name,
            namespace: w.namespace,
            kind: w.kind,
        })
        .collect();

    let client = get_hrpc_client();
    client
        .add_workloads_to_app_catalog(org.id, catalog_id, workloads)
        .await??;

    Ok(())
}

#[component]
pub fn AppCatalogModal(
    modal_open: RwSignal<bool>,
    selected_workloads: RwSignal<std::collections::HashSet<WorkloadKey>>,
    filtered_workloads: Signal<Vec<KubeWorkload>>,
    cluster_id: Uuid,
    catalog_id_from_url: Option<Uuid>,
) -> impl IntoView {
    let catalog_name = RwSignal::new_local("".to_string());
    let catalog_description = RwSignal::new_local("".to_string());
    let org = get_current_org();

    // Load existing catalog info when adding to existing catalog
    let existing_catalog = LocalResource::new(move || async move {
        if let Some(catalog_id) = catalog_id_from_url {
            get_app_catalog(org, catalog_id).await.ok()
        } else {
            None
        }
    });

    let navigate = leptos_router::hooks::use_navigate();
    let create_action = Action::new_local(move |_| {
        let navigate = navigate.clone();
        async move {
            let selected = selected_workloads.get_untracked();
            let workloads = filtered_workloads.get_untracked();

            if let Some(existing_catalog_id) = catalog_id_from_url {
                // Add to existing catalog mode
                add_workloads_to_catalog_api(org, existing_catalog_id, selected, workloads).await?;

                // Clear form and close modal
                modal_open.set(false);

                // Clear selections after successful addition
                selected_workloads.set(std::collections::HashSet::new());

                // Navigate back to the catalog workloads page
                navigate(
                    &format!("/kubernetes/catalogs/{existing_catalog_id}"),
                    Default::default(),
                );
            } else {
                // Create new catalog mode
                let name = catalog_name.get_untracked();
                let description = catalog_description.get_untracked();

                let new_catalog_id =
                    create_app_catalog_api(org, cluster_id, name, description, selected, workloads)
                        .await?;

                // Clear form and close modal
                catalog_name.set("".to_string());
                catalog_description.set("".to_string());
                modal_open.set(false);

                // Clear selections after successful creation
                selected_workloads.set(std::collections::HashSet::new());

                // Navigate to the created app catalog workloads page
                navigate(
                    &format!("/kubernetes/catalogs/{new_catalog_id}"),
                    Default::default(),
                );
            }

            Ok(())
        }
    });

    view! {
        <Modal
            title={if catalog_id_from_url.is_some() {
                "Add Workloads to Catalog"
            } else {
                "Create App Catalog"
            }}
            open=modal_open
            action=create_action
        >
            <div class="flex flex-col gap-4">
                // Only show name and description fields when creating new catalog
                <Show when=move || catalog_id_from_url.is_none()>
                    <div class="flex flex-col gap-2">
                        <Label>Catalog Name</Label>
                        <Input
                            prop:value=move || catalog_name.get()
                            on:input=move |ev| {
                                catalog_name.set(event_target_value(&ev));
                            }
                            attr:placeholder="Enter catalog name"
                            attr:required=true
                        />
                    </div>

                    <div class="flex flex-col gap-2">
                        <Label>Description</Label>
                        <Textarea
                            prop:value=move || catalog_description.get()
                            on:input=move |ev| {
                                catalog_description.set(event_target_value(&ev));
                            }
                            attr:placeholder="Describe what this app does..."
                            class="min-h-20"
                        />
                    </div>
                </Show>

                // Show existing catalog info when adding to existing catalog
                {move || {
                    if catalog_id_from_url.is_some() {
                        view! {
                            <div class="flex flex-col gap-2 p-4 bg-muted rounded-lg">
                                <P class="font-medium text-sm">Target Catalog:</P>
                                {move || {
                                    if let Some(catalog) = existing_catalog.get().flatten() {
                                        view! {
                                            <div class="space-y-2 text-sm">
                                                <div class="flex items-center gap-2">
                                                    <span class="font-medium">Name:</span>
                                                    <span>{catalog.name}</span>
                                                </div>
                                                {if let Some(desc) = &catalog.description {
                                                    view! {
                                                        <div class="flex flex-col gap-1">
                                                            <span class="font-medium">Description:</span>
                                                            <span class="text-muted-foreground leading-relaxed">{desc.clone()}</span>
                                                        </div>
                                                    }.into_any()
                                                } else {
                                                    view! {
                                                        <div class="text-muted-foreground text-xs">No description</div>
                                                    }.into_any()
                                                }}
                                            </div>
                                        }.into_any()
                                    } else {
                                        view! {
                                            <div class="flex items-center gap-2 text-sm text-muted-foreground">
                                                <div class="animate-spin rounded-full h-4 w-4 border-2 border-current border-t-transparent"></div>
                                                "Loading catalog information..."
                                            </div>
                                        }.into_any()
                                    }
                                }}
                            </div>
                        }.into_any()
                    } else {
                        view! { <div></div> }.into_any()
                    }
                }}

                <div class="flex flex-col gap-2">
                    <Label>
                        {move || {
                            let count = selected_workloads.with(|s| s.len());
                            format!("Selected Workloads ({})", count)
                        }}
                    </Label>
                    <div class="h-64 overflow-y-auto border rounded p-2 bg-muted/30">
                        {move || {
                            let selected = selected_workloads.get();
                            let workloads: Vec<KubeWorkload> = filtered_workloads
                                .get()
                                .into_iter()
                                .filter(|w| selected.contains(&WorkloadKey::from_workload(w)))
                                .collect();

                            if workloads.is_empty() {
                                view! {
                                    <P class="text-sm text-muted-foreground">No workloads selected</P>
                                }.into_any()
                            } else {
                                view! {
                                    <div class="flex flex-col gap-1">
                                        <For
                                            each=move || workloads.clone()
                                            key=|w| WorkloadKey::from_workload(w)
                                            children=move |workload| {
                                                view! {
                                                    <div class="flex items-center gap-2 text-sm">
                                                        <Badge variant=BadgeVariant::Outline class="text-xs">
                                                            {format!("{:?}", workload.kind)}
                                                        </Badge>
                                                        <span class="font-medium">{workload.name}</span>
                                                        <span class="text-muted-foreground">
                                                            "in " {workload.namespace}
                                                        </span>
                                                    </div>
                                                }
                                            }
                                        />
                                    </div>
                                }.into_any()
                            }
                        }}
                    </div>
                </div>
            </div>
        </Modal>
    }
}

#[component]
pub fn AppCatalogActions(
    selected_workloads: RwSignal<HashSet<WorkloadKey>>,
    filtered_workloads: Signal<Vec<KubeWorkload>>,
    cluster_id: Uuid,
    catalog_id_from_url: Option<Uuid>,
) -> impl IntoView {
    let modal_open = RwSignal::new(false);

    let create_app_catalog = move |_| {
        modal_open.set(true);
    };

    let clear_selections = move |_| {
        selected_workloads.set(std::collections::HashSet::new());
    };

    view! {
        <AppCatalogModal
            modal_open
            selected_workloads
            filtered_workloads
            cluster_id
            catalog_id_from_url
        />

        // Action buttons row - always visible
        {move || {
            let selected_count = selected_workloads.with(|s| s.len());
            let has_selections = selected_count > 0;

            view! {
                <div class="flex items-center gap-4">
                    {if let Some(existing_catalog_id) = catalog_id_from_url {
                        view! {
                            <a href=format!("/kubernetes/catalogs/{}", existing_catalog_id)>
                                <Button variant=ButtonVariant::Secondary>
                                    <lucide_leptos::ArrowLeft />
                                    "Back to Catalog"
                                </Button>
                            </a>
                        }.into_any()
                    } else {
                        ().into_any()
                    }}
                    <Button
                        variant=ButtonVariant::Default
                        disabled=!has_selections
                        on:click=create_app_catalog
                    >
                        <lucide_leptos::Plus />
                        {if catalog_id_from_url.is_some() {
                            "Add to Catalog"
                        } else {
                            "Create App Catalog"
                        }}
                    </Button>
                    <Button
                        variant=ButtonVariant::Outline
                        disabled=!has_selections
                        on:click=clear_selections
                    >
                        <lucide_leptos::X />
                        "Clear Selections"
                    </Button>
                    <span class="text-sm text-muted-foreground">
                        {if has_selections {
                            format!("{} selected", selected_count)
                        } else {
                            "No workloads selected".to_string()
                        }}
                    </span>
                </div>
            }.into_any()
        }}
    }
}

#[component]
pub fn WorkloadTable(
    selected_workloads: RwSignal<HashSet<WorkloadKey>>,
    filtered_workloads: Signal<Vec<KubeWorkload>>,
    is_loading: Signal<bool>,
    cluster_status: Signal<KubeClusterStatus>,
    #[prop(into)] on_view_details: Callback<KubeWorkload>,
) -> impl IntoView {
    let all_selected = Signal::derive(move || {
        let workloads = filtered_workloads.get();
        let selected = selected_workloads.get();
        if workloads.is_empty() {
            false
        } else {
            workloads
                .iter()
                .all(|w| selected.contains(&WorkloadKey::from_workload(w)))
        }
    });

    let some_selected = Signal::derive(move || {
        let workloads = filtered_workloads.get();
        let selected = selected_workloads.get();
        workloads
            .iter()
            .any(|w| selected.contains(&WorkloadKey::from_workload(w)))
    });

    let toggle_all = move |_| {
        let workloads = filtered_workloads.get_untracked();
        if all_selected.get_untracked() {
            // Unselect only the workloads currently visible in the table
            let current_workload_keys: std::collections::HashSet<WorkloadKey> = workloads
                .iter()
                .map(|w| WorkloadKey::from_workload(w))
                .collect();

            selected_workloads.update(|selected| {
                for key in &current_workload_keys {
                    selected.remove(key);
                }
            });
        } else {
            // Select all current workloads
            let all_keys: std::collections::HashSet<WorkloadKey> = workloads
                .iter()
                .map(|w| WorkloadKey::from_workload(w))
                .collect();

            selected_workloads.update(|selected| {
                for key in all_keys {
                    selected.insert(key);
                }
            });
        }
    };
    view! {
        <div class="flex flex-col gap-4">

            <div class="relative rounded-lg border">
            <Table class="table-auto">
                <TableHeader class="bg-muted">
                    <TableRow>
                        <TableHead class="w-10">
                            <div class="flex items-center justify-center">
                                <Checkbox
                                    prop:checked=move || all_selected.get()
                                    prop:indeterminate=move || some_selected.get() && !all_selected.get()
                                    on:change=toggle_all
                                />
                            </div>
                        </TableHead>
                        <TableHead>Name</TableHead>
                        <TableHead>Namespace</TableHead>
                        <TableHead>Kind</TableHead>
                        <TableHead>Status</TableHead>
                        <TableHead>Replicas</TableHead>
                        <TableHead>Age</TableHead>
                        <TableHead class="pr-4">Actions</TableHead>
                    </TableRow>
                </TableHeader>
                <TableBody>
                    <For
                        each=move || filtered_workloads.get()
                        key=|w| WorkloadKey::from_workload(w)
                        children=move |workload| {
                            view! {
                                <KubeResourceItem
                                    workload=workload.clone()
                                    selected_workloads
                                    on_view_details=move |w| {
                                        on_view_details.run(w);
                                    }
                                />
                            }
                        }
                    />
                </TableBody>
            </Table>

            {move || {
                let workload_list = filtered_workloads.get();
                if workload_list.is_empty() && !is_loading.get() {
                    let status = cluster_status.get();
                    let (title, message) = match status {
                        KubeClusterStatus::Ready => (
                            "No Workloads Found",
                            "No workloads match your current filters. Try adjusting your namespace or workload type filters."
                        ),
                        KubeClusterStatus::NotReady => (
                            "Cluster Not Ready",
                            "The cluster is not ready yet. Workloads will appear once the cluster is fully connected and operational."
                        ),
                        KubeClusterStatus::Error => (
                            "Cluster Error",
                            "The cluster is experiencing issues. Please check the cluster status and connection before workloads can be displayed."
                        ),
                        KubeClusterStatus::Provisioning => (
                            "Cluster Provisioning",
                            "The cluster is still being provisioned. Workloads will be available once the cluster setup is complete."
                        ),
                    };

                    view! {
                        <div class="flex flex-col items-center justify-center py-12 text-center">
                            <div class="rounded-full bg-muted p-3 mb-4">
                                <lucide_leptos::Layers />
                            </div>
                            <H4 class="mb-2">{title}</H4>
                            <P class="text-muted-foreground mb-4 max-w-sm">
                                {message}
                            </P>
                        </div>
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            }}

            // Loading overlay
            {move || {
                if is_loading.get() {
                    view! {
                        <div class="absolute inset-0 bg-white/80 backdrop-blur-none flex items-center justify-center rounded-lg">
                            <div class="flex items-center gap-3 text-sm text-muted-foreground">
                                <div class="animate-spin rounded-full h-5 w-5 border-b-2 border-gray-900"></div>
                                "Loading workloads..."
                            </div>
                        </div>
                    }
                        .into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            }}
            </div>
        </div>
    }
}

#[component]
pub fn WorkloadPagination(
    workload_list: Signal<Vec<KubeWorkload>>,
    limit_select_value: RwSignal<usize>,
    workload_cache: RwSignal<WorkloadCache, LocalStorage>,
    cache_offset: RwSignal<usize, LocalStorage>,
    is_loading: Signal<bool>,
) -> impl IntoView {
    let limit_select_open = RwSignal::new(false);
    view! {
        {move || {
            let list = workload_list.get();
            let has_previous = cache_offset.get() > 0;
            let has_next = workload_cache
                .with(|c| c.has_next(cache_offset.get(), limit_select_value.get()));

            view! {
                <div class="flex items-center justify-between flex-wrap gap-4">
                    <div class="flex items-center gap-4">
                        <div class="text-sm text-muted-foreground">
                            {
                                let offset = cache_offset.get();
                                let count = list.len();
                                let total_count = workload_cache
                                    .with(|c| {
                                        if c.cursor.is_none() {
                                            Some(c.workloads.len())
                                        } else {
                                            None
                                        }
                                    });
                                if count > 0 {
                                    format!(
                                        "Showing {}-{} of {}workloads",
                                        offset + 1,
                                        offset + count,
                                        if let Some(total_count) = total_count {
                                            format!("{total_count} ")
                                        } else {
                                            "".to_string()
                                        },
                                    )
                                } else {
                                    "No workloads found".to_string()
                                }
                            }
                        </div>
                    </div>
                    <div class="flex items-center gap-2">
                        <div class="flex items-center gap-2">
                            <span class="text-sm text-muted-foreground">"Rows per page:"</span>
                            <div class="w-20">
                                <Select open=limit_select_open value=limit_select_value>
                                    <SelectTrigger class="h-8 text-sm">
                                        {move || { limit_select_value.get().to_string() }}
                                    </SelectTrigger>
                                    <SelectContent>
                                        <SelectItem value=10usize>10</SelectItem>
                                        <SelectItem value=20usize>20</SelectItem>
                                        <SelectItem value=50usize>50</SelectItem>
                                        <SelectItem value=100usize>100</SelectItem>
                                    </SelectContent>
                                </Select>
                            </div>
                        </div>
                        <Button
                            variant=ButtonVariant::Outline
                            size=ButtonSize::Sm
                            disabled=!has_previous || is_loading.get()
                            on:click=move |_| {
                                cache_offset
                                    .set(
                                        cache_offset
                                            .get_untracked()
                                            .saturating_sub(limit_select_value.get_untracked()),
                                    );
                            }
                        >
                            <lucide_leptos::ChevronLeft />
                            Previous
                        </Button>
                        <Button
                            variant=ButtonVariant::Outline
                            size=ButtonSize::Sm
                            disabled=!has_next || is_loading.get()
                            on:click=move |_| {
                                cache_offset.set(cache_offset.get() + limit_select_value.get());
                            }
                        >
                            Next
                            <lucide_leptos::ChevronRight />
                        </Button>
                    </div>
                </div>
            }
                .into_any()
        }}
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkloadKey {
    pub namespace: String,
    pub name: String,
}

impl WorkloadKey {
    pub fn from_workload(workload: &KubeWorkload) -> Self {
        Self {
            namespace: workload.namespace.clone(),
            name: workload.name.clone(),
        }
    }
}

impl std::fmt::Display for WorkloadKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}_{}", self.namespace, self.name)
    }
}

#[derive(Debug, Clone)]
pub struct WorkloadCache {
    workloads: Vec<KubeWorkload>,
    cursor: Option<PaginationCursor>, // The next cursor from API response
}

impl WorkloadCache {
    fn new(kind_filter: Option<KubeWorkloadKind>) -> Self {
        Self {
            workloads: vec![],
            cursor: Some(PaginationCursor {
                workload_kind: kind_filter.unwrap_or(KubeWorkloadKind::Deployment),
                continue_token: None,
            }),
        }
    }

    fn has_next(&self, offset: usize, limit: usize) -> bool {
        offset + limit < self.workloads.len() || self.cursor.is_some()
    }

    fn needs_more_retrieve(&self, offset: usize, limit: usize) -> bool {
        if self.cursor.is_none() {
            // cursor is none, which means there's no more from the api
            return false;
        }

        self.workloads.len() < offset + limit
    }

    fn update_cache(&mut self, list: KubeWorkloadList) {
        self.workloads.extend(list.workloads);
        self.cursor = list.next_cursor;
    }

    fn retrieve_list(&self, offset: usize, limit: usize) -> Vec<KubeWorkload> {
        self.workloads[offset..(offset + limit).min(self.workloads.len())].to_vec()
    }
}
