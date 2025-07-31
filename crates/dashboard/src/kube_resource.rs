use std::str::FromStr;

use anyhow::{anyhow, Result};
use lapdev_api_hrpc::HrpcServiceClient;
use lapdev_common::{
    console::Organization,
    kube::{
        KubeClusterInfo, KubeWorkload, KubeWorkloadKind, KubeWorkloadList, KubeWorkloadStatus,
        PaginationParams,
    },
};
use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use uuid::Uuid;

use crate::{
    component::{
        badge::{Badge, BadgeVariant},
        button::{Button, ButtonSize, ButtonVariant},
        input::Input,
        label::Label,
        select::{Select, SelectContent, SelectItem, SelectTrigger},
        table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow},
        typography::{H3, P},
    },
    modal::Modal,
    organization::get_current_org,
};

#[component]
pub fn KubeResource() -> impl IntoView {
    let update_counter = RwSignal::new_local(0);
    let params = use_params_map();
    let cluster_id = params.with_untracked(|params| params.get("cluster_id").clone().unwrap());
    let cluster_id = Uuid::from_str(&cluster_id).unwrap_or_default();

    view! {
        <div class="flex flex-col gap-2 items-start">
            <H3>Kubernetes Resources</H3>
            <P>View and manage workloads, services, and configurations in your Kubernetes clusters.</P>
            <a href="https://docs.lap.dev/">
                <Badge variant=BadgeVariant::Secondary>Docs <lucide_leptos::ExternalLink /></Badge>
            </a>
        </div>

        <KubeResourceList update_counter cluster_id />
    }
}

async fn get_workloads(
    org: Signal<Option<Organization>>,
    cluster_id: uuid::Uuid,
    namespace_filter: String,
    pagination: PaginationParams,
) -> Result<KubeWorkloadList> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());

    let namespace = if namespace_filter.is_empty() {
        None
    } else {
        Some(namespace_filter)
    };

    Ok(client
        .get_workloads(org.id, cluster_id, namespace, Some(pagination))
        .await??)
}

#[component]
pub fn KubeResourceList(
    update_counter: RwSignal<usize, LocalStorage>,
    cluster_id: Uuid,
) -> impl IntoView {
    let org = get_current_org();
    let namespace_filter = RwSignal::new_local("".to_string());
    let kind_filter = RwSignal::new_local("All".to_string());
    let current_cursor = RwSignal::new_local(None::<String>);
    let limit = RwSignal::new_local(20usize);
    let is_loading = RwSignal::new(false);

    let workloads = LocalResource::new(move || {
        let namespace = if namespace_filter.get().is_empty() {
            None
        } else {
            Some(namespace_filter.get())
        };

        async move {
            is_loading.set(true);
            let result = get_workloads(
                org,
                cluster_id,
                namespace.unwrap_or_default(),
                PaginationParams {
                    cursor: current_cursor.get(),
                    limit: limit.get(),
                },
            )
            .await
            .unwrap_or_else(|_| KubeWorkloadList {
                workloads: vec![],
                next_cursor: None,
                has_next: false,
            });
            is_loading.set(false);
            result
        }
    });

    Effect::new(move |_| {
        update_counter.track();
        namespace_filter.track();
        kind_filter.track();
        current_cursor.track();
        limit.track();
        workloads.refetch();
    });

    let workload_list = Signal::derive(move || {
        workloads.get().unwrap_or_else(|| KubeWorkloadList {
            workloads: vec![],
            next_cursor: None,
            has_next: false,
        })
    });

    let filtered_workloads = Signal::derive(move || {
        let list = workload_list.get();
        let kind_filter_val = kind_filter.get();

        if kind_filter_val == "All" {
            list.workloads
        } else {
            list.workloads
                .into_iter()
                .filter(|w| format!("{:?}", w.kind) == kind_filter_val)
                .collect()
        }
    });

    let detail_modal_open = RwSignal::new(false);
    let selected_workload = RwSignal::new_local(None::<KubeWorkload>);

    let workload_kind_select_open = RwSignal::new(false);
    let workload_kind_select_value = RwSignal::new(None::<KubeWorkloadKind>);

    let limit_select_open = RwSignal::new(false);
    let limit_select_value = RwSignal::new(20usize);

    // Sync limit select with limit signal
    Effect::new(move |_| {
        let selected_limit = limit_select_value.get();
        if selected_limit != limit.get() {
            limit.set(selected_limit);
            current_cursor.set(None); // Reset cursor when changing limit
        }
    });

    view! {
        <div class="mt-8 flex flex-col gap-4">
            <WorkloadFilters
                workload_kind_select_open
                workload_kind_select_value
            />
            <WorkloadPagination
                workload_list
                current_cursor
                limit_select_open
                limit_select_value
            />


            <WorkloadTable
                filtered_workloads
                is_loading=is_loading.read_only().into()
                on_view_details=Callback::new(move |w| {
                    selected_workload.set(Some(w));
                    detail_modal_open.set(true);
                })
            />

            <WorkloadDetailModal
                modal_open=detail_modal_open
                workload=selected_workload
            />
        </div>
    }
}

#[component]
pub fn KubeResourceItem(
    workload: KubeWorkload,
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

    let workload_clone = workload.clone();
    // let view_details = move |_| {
    //     on_view_details(workload_clone.clone());
    // };

    view! {
        <TableRow>
            <TableCell class="pl-4">
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
                <Button
                    variant=ButtonVariant::Outline
                    size=ButtonSize::Sm
                    // on:click=view_details
                >
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
                                {
                                    if let (Some(ready), Some(total)) = (w.ready_replicas, w.replicas) {
                                        view! {
                                            <div>
                                                <Label>Replicas</Label>
                                                <P class="font-mono text-sm">{format!("{}/{}", ready, total)}</P>
                                            </div>
                                        }.into_any()
                                    } else {
                                        view! { <div></div> }.into_any()
                                    }
                                }
                                {
                                    if let Some(created) = &w.created_at {
                                        let created_str = created.clone();
                                        view! {
                                            <div>
                                                <Label>Created</Label>
                                                <P class="font-mono text-sm">{created_str}</P>
                                            </div>
                                        }.into_any()
                                    } else {
                                        view! { <div></div> }.into_any()
                                    }
                                }
                            </div>

                            {
                                if !w.labels.is_empty() {
                                    let labels_vec: Vec<_> = w.labels.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
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
                                                                <Badge variant=BadgeVariant::Outline class="font-mono text-xs">
                                                                    {format!("{}={}", key, value)}
                                                                </Badge>
                                                            </div>
                                                        }
                                                    }
                                                />
                                            </div>
                                        </div>
                                    }.into_any()
                                } else {
                                    view! { <div></div> }.into_any()
                                }
                            }
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <P>No workload selected</P>
                    }.into_any()
                }
            }}
        </Modal>
    }
}

#[component]
pub fn WorkloadFilters(
    workload_kind_select_open: RwSignal<bool>,
    workload_kind_select_value: RwSignal<Option<KubeWorkloadKind>>,
) -> impl IntoView {
    view! {
        <Select
            open=workload_kind_select_open
            value=workload_kind_select_value
        >
            <SelectTrigger>{move || match workload_kind_select_value.get() {
                Some(k) => k.to_string(),
                None => "All Types".to_string(),
            }}</SelectTrigger>
            <SelectContent>
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
pub fn WorkloadTable(
    filtered_workloads: Signal<Vec<KubeWorkload>>,
    is_loading: Signal<bool>,
    #[prop(into)] on_view_details: Callback<KubeWorkload>,
) -> impl IntoView {
    view! {
        <div class="relative rounded-lg border">
            <Table class="table-auto">
                <TableHeader class="bg-muted">
                    <TableRow>
                        <TableHead class="pl-4">Name</TableHead>
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
                        key=|w| format!("{}_{}", w.namespace, w.name)
                        children=move |workload| {
                            view! {
                                <KubeResourceItem
                                    workload=workload.clone()
                                    on_view_details=move |w| {
                                        on_view_details.run(w);
                                    }
                                />
                            }
                        }
                    />
                </TableBody>
            </Table>

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
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            }}
        </div>
    }
}

#[component]
pub fn WorkloadPagination(
    workload_list: Signal<KubeWorkloadList>,
    current_cursor: RwSignal<Option<String>, LocalStorage>,
    limit_select_open: RwSignal<bool>,
    limit_select_value: RwSignal<usize>,
) -> impl IntoView {
    let cursor_history = RwSignal::new_local(Vec::<Option<String>>::new());

    view! {
        {move || {
            let list = workload_list.get();
            let has_previous = !cursor_history.get().is_empty();
            let has_next = list.has_next;

            view! {
                <div class="flex items-center justify-between mt-4">
                    <div class="flex items-center gap-4">
                        <div class="text-sm text-muted-foreground">
                            {format!("Showing {} workloads", list.workloads.len())}
                        </div>
                        <div class="flex items-center gap-2">
                            <span class="text-sm text-muted-foreground">"Rows per page:"</span>
                            <div class="w-20">
                                <Select
                                    open=limit_select_open
                                    value=limit_select_value
                                >
                                    <SelectTrigger class="h-8 text-sm">{move || {
                                        limit_select_value.get().to_string()
                                    }}</SelectTrigger>
                                    <SelectContent>
                                        <SelectItem value=10usize>10</SelectItem>
                                        <SelectItem value=20usize>20</SelectItem>
                                        <SelectItem value=50usize>50</SelectItem>
                                        <SelectItem value=100usize>100</SelectItem>
                                    </SelectContent>
                                </Select>
                            </div>
                        </div>
                    </div>
                    <div class="flex items-center gap-2">
                        <Button
                            variant=ButtonVariant::Outline
                            size=ButtonSize::Sm
                            disabled=!has_previous
                            on:click=move |_| {
                                let mut history = cursor_history.get();
                                if let Some(prev_cursor) = history.pop() {
                                    current_cursor.set(prev_cursor);
                                    cursor_history.set(history);
                                }
                            }
                        >
                            <lucide_leptos::ChevronLeft />
                            Previous
                        </Button>
                        <Button
                            variant=ButtonVariant::Outline
                            size=ButtonSize::Sm
                            disabled=!has_next
                            on:click=move |_| {
                                let mut history = cursor_history.get();
                                history.push(current_cursor.get());
                                cursor_history.set(history);
                                current_cursor.set(list.next_cursor.clone());
                            }
                        >
                            Next
                            <lucide_leptos::ChevronRight />
                        </Button>
                    </div>
                </div>
            }.into_any()
        }}
    }
}
