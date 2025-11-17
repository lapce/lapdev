use std::{collections::HashMap, str::FromStr};

use anyhow::{anyhow, Result};
use futures::StreamExt;
use lapdev_api_hrpc::HrpcServiceClient;
use lapdev_common::{
    console::Organization,
    devbox::{
        DevboxPortMapping, DevboxPortMappingOverride, DevboxSessionSummary,
        DevboxWorkloadInterceptSummary,
    },
    kube::{
        EnvironmentWorkloadStatusEvent, KubeCluster, KubeContainerInfo, KubeEnvironment,
        KubeEnvironmentPreviewUrl, KubeEnvironmentService, KubeEnvironmentStatus,
        KubeEnvironmentSyncStatus, KubeEnvironmentWorkload,
    },
};
use leptos::{prelude::*, task::spawn_local_scoped_with_cancellation};
use leptos_router::hooks::use_params_map;
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    app::AppConfig,
    component::{
        alert::{Alert, AlertDescription, AlertTitle},
        badge::{Badge, BadgeVariant},
        button::{Button, ButtonSize, ButtonVariant},
        card::Card,
        input::Input,
        label::Label,
        table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow},
        tabs::{Tabs, TabsContent, TabsList, TabsTrigger},
        typography::{H3, H4, P},
    },
    docs_url,
    modal::{DatetimeModal, DeleteModal, ErrorResponse, Modal},
    organization::get_current_org,
    sse::run_sse_with_retry,
    DOCS_DEVBOX_PATH, DOCS_ENVIRONMENT_PATH,
};

#[derive(Clone, Debug, Deserialize)]
struct EnvironmentLifecycleEvent {
    status: KubeEnvironmentStatus,
}

const DEVBOX_INSTALL_UNIX_CMD: &str = "curl -fsSL https://get.lap.dev/lapdev-cli.sh | bash";
const DEVBOX_INSTALL_WINDOWS_CMD: &str = "irm https://get.lap.dev/lapdev-cli.ps1 | iex";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DevboxInstallTab {
    LinuxMac,
    Windows,
}

fn detect_devbox_install_tab() -> DevboxInstallTab {
    #[cfg(target_arch = "wasm32")]
    {
        if let Ok(user_agent) = window().navigator().user_agent() {
            let ua = user_agent.to_lowercase();
            if ua.contains("windows") {
                return DevboxInstallTab::Windows;
            }
        }
    }

    DevboxInstallTab::LinuxMac
}

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
                <a href=docs_url(DOCS_ENVIRONMENT_PATH) target="_blank" rel="noopener noreferrer">
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
) -> Result<KubeCluster> {
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

async fn get_environment_intercepts(
    environment_id: Uuid,
) -> Result<Vec<DevboxWorkloadInterceptSummary>> {
    let client = HrpcServiceClient::new("/api/rpc".to_string());
    let response = client.devbox_intercept_list(environment_id).await??;
    Ok(response.intercepts)
}

async fn start_workload_intercept(
    workload_id: Uuid,
    port_mappings: Vec<DevboxPortMappingOverride>,
    update_counter: RwSignal<usize>,
) -> Result<(), ErrorResponse> {
    let client = HrpcServiceClient::new("/api/rpc".to_string());

    client
        .devbox_intercept_start(workload_id, port_mappings)
        .await??;

    update_counter.update(|c| *c += 1);

    Ok(())
}

async fn stop_workload_intercept(
    intercept_id: Uuid,
    update_counter: RwSignal<usize>,
) -> Result<(), ErrorResponse> {
    let client = HrpcServiceClient::new("/api/rpc".to_string());

    client.devbox_intercept_stop(intercept_id).await??;

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

async fn pause_environment(
    org: Signal<Option<Organization>>,
    environment_id: Uuid,
) -> Result<(), ErrorResponse> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());

    client
        .pause_kube_environment(org.id, environment_id)
        .await??;

    Ok(())
}

async fn resume_environment(
    org: Signal<Option<Organization>>,
    environment_id: Uuid,
) -> Result<(), ErrorResponse> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());

    client
        .resume_kube_environment(org.id, environment_id)
        .await??;

    Ok(())
}

async fn create_branch_environment(
    org: Signal<Option<Organization>>,
    base_environment_id: Uuid,
    name: String,
    create_branch_modal_open: RwSignal<bool>,
) -> Result<KubeEnvironment, ErrorResponse> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());

    let env = client
        .create_branch_environment(org.id, base_environment_id, name)
        .await??;

    create_branch_modal_open.set(false);

    Ok(env)
}

async fn sync_environment_from_catalog(
    org: Signal<Option<Organization>>,
    environment_id: Uuid,
) -> Result<(), ErrorResponse> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());

    client
        .sync_environment_from_catalog(org.id, environment_id)
        .await??;

    Ok(())
}

async fn get_active_devbox_session() -> Result<Option<DevboxSessionSummary>> {
    let client = HrpcServiceClient::new("/api/rpc".to_string());
    let session = client
        .devbox_session_get_session()
        .await
        .map_err(|err| anyhow!("failed to fetch devbox session: {err:?}"))?
        .map_err(|err| anyhow!("failed to fetch devbox session: {err:?}"))?;

    Ok(session)
}

fn build_port_overrides_from_workload_ports(
    ports: &[lapdev_common::kube::KubeServicePort],
    previous_mappings: Option<&[DevboxPortMapping]>,
) -> Result<Vec<DevboxPortMappingOverride>, ErrorResponse> {
    use std::convert::TryFrom;

    let mut overrides = Vec::new();

    if ports.is_empty() {
        return Err(ErrorResponse {
            error: "No ports available to intercept".to_string(),
        });
    }

    for port in ports {
        // Prefer the original container port when present; otherwise fall back to the current target or service port.
        let target_port = port.original_target_port.unwrap_or(port.port);

        let workload_port = u16::try_from(target_port).map_err(|_| ErrorResponse {
            error: format!("Unsupported workload port: {}", target_port),
        })?;

        let local_port = previous_mappings
            .and_then(|existing| {
                existing
                    .iter()
                    .find(|mapping| mapping.workload_port == workload_port)
            })
            .map(|mapping| mapping.local_port)
            .unwrap_or(workload_port);

        overrides.push(DevboxPortMappingOverride {
            workload_port,
            local_port: Some(local_port),
        });
    }

    Ok(overrides)
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
    let readiness_map: RwSignal<HashMap<Uuid, Option<i32>>> = RwSignal::new(HashMap::new());
    let sse_started = StoredValue::new(false);
    let readiness_sse_started = StoredValue::new(false);
    let environment_status = RwSignal::new(KubeEnvironmentStatus::Creating);

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
            config.header_links.update(|header_links| {
                if let Some((name, link)) = header_links.first_mut() {
                    let (t, l) = if env.base_environment_id.is_some() {
                        ("Branch", "branch")
                    } else if env.is_shared {
                        ("Shared", "shared")
                    } else {
                        ("Personal", "personal")
                    };
                    *name = format!("{t} Environments");
                    *link = format!("/kubernetes/environments/{l}");
                }
            });
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

    let intercepts_result = LocalResource::new(move || {
        update_counter.track();
        async move {
            get_environment_intercepts(environment_id)
                .await
                .unwrap_or_else(|_| vec![])
        }
    });

    // Fetch active devbox session
    let active_session =
        LocalResource::new(move || async move { get_active_devbox_session().await.ok().flatten() });

    Effect::new(move |_| {
        if sse_started.get_value() {
            return;
        }

        if let Some(org) = org.get() {
            sse_started.set_value(true);
            let org_id = org.id;
            let environment_status = environment_status.clone();
            spawn_local_scoped_with_cancellation({
                let environment_status = environment_status.clone();
                async move {
                    let url = format!(
                        "/api/v1/organizations/{}/kube/environments/{}/events",
                        org_id, environment_id
                    );

                    let listener_name = format!("environment {} status", environment_id);
                    run_sse_with_retry(url, "environment", listener_name, move |message| {
                        if let Some(data) = message.data().as_string() {
                            match serde_json::from_str::<EnvironmentLifecycleEvent>(&data) {
                                Ok(payload) => {
                                    environment_status.set(payload.status);
                                    update_counter.update(|c| *c += 1);
                                }
                                Err(err) => {
                                    web_sys::console::error_1(
                                        &format!(
                                            "Failed to parse environment event payload: {err}"
                                        )
                                        .into(),
                                    );
                                }
                            }
                        }
                    })
                    .await;
                }
            });
        }
    });

    Effect::new(move |_| {
        if readiness_sse_started.get_value() {
            return;
        }

        if let Some(org) = org.get() {
            readiness_sse_started.set_value(true);
            let org_id = org.id;
            spawn_local_scoped_with_cancellation({
                let readiness_map = readiness_map.clone();
                async move {
                    let url = format!(
                        "/api/v1/organizations/{}/kube/environments/{}/workloads/events",
                        org_id, environment_id
                    );

                    let listener_name = format!("environment {} workloads", environment_id);
                    run_sse_with_retry(
                        url,
                        "environment_workload",
                        listener_name,
                        move |message| {
                            if let Some(data) = message.data().as_string() {
                                match serde_json::from_str::<EnvironmentWorkloadStatusEvent>(&data)
                                {
                                    Ok(payload) => {
                                        readiness_map.update(|map| {
                                            map.insert(payload.workload_id, payload.ready_replicas);
                                        });
                                    }
                                    Err(err) => {
                                        web_sys::console::error_1(
                                            &format!(
                                                "Failed to parse environment workload event payload: {err}"
                                            )
                                            .into(),
                                        );
                                    }
                                }
                            }
                        },
                    )
                    .await;
                }
            });
        }
    });

    let environment_info = Signal::derive(move || environment_result.get().flatten());
    {
        let environment_status = environment_status.clone();
        Effect::new(move |_| {
            if let Some(env) = environment_info.get() {
                environment_status.set(env.status.clone());
            }
        });
    }
    let environment_catalog_version = Signal::derive(move || {
        environment_info
            .get()
            .map(|env| env.catalog_sync_version)
            .unwrap_or(0)
    });

    let env_catalog_version_for_filter = environment_catalog_version.clone();
    let all_workloads = Signal::derive(move || workloads_result.get().unwrap_or_default());
    {
        let readiness_map = readiness_map.clone();
        Effect::new(move |_| {
            let workloads = all_workloads.get();
            readiness_map.update(|map| {
                map.retain(|workload_id, _| workloads.iter().any(|w| w.id == *workload_id));
                for workload in &workloads {
                    map.insert(workload.id, workload.ready_replicas);
                }
            });
        });
    }
    let all_services = Signal::derive(move || services_result.get().unwrap_or_default());
    let all_preview_urls = Signal::derive(move || preview_urls_result.get().unwrap_or_default());
    let all_intercepts = Signal::derive(move || intercepts_result.get().unwrap_or_default());

    // Filter workloads based on search query
    let filtered_workloads = Signal::derive(move || {
        let env_version = env_catalog_version_for_filter.get();
        let workloads = all_workloads.get();
        let search_term = debounced_search.get().to_lowercase();

        workloads
            .into_iter()
            .filter(|workload| {
                workload.catalog_sync_version >= env_version
                    && (search_term.trim().is_empty()
                        || workload.name.to_lowercase().contains(&search_term))
            })
            .collect()
    });

    let navigate = leptos_router::hooks::use_navigate();
    let navigate_clone = navigate.clone();
    let delete_action = Action::new_local(move |_| {
        let nav = navigate.clone();
        async move {
            match delete_environment(org, environment_id, delete_modal_open).await {
                Ok(_) => {
                    let t = if let Some(info) = environment_info.get_untracked() {
                        if info.base_environment_id.is_some() {
                            "branch"
                        } else if info.is_shared {
                            "shared"
                        } else {
                            "personal"
                        }
                    } else {
                        "personal"
                    };
                    nav(&format!("/kubernetes/environments/{t}"), Default::default());
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
                Ok(env) => {
                    nav(
                        &format!("/kubernetes/environments/{}", env.id),
                        Default::default(),
                    );
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
    });

    let sync_action = Action::new_local(move |_| async move {
        match sync_environment_from_catalog(org, environment_id).await {
            Ok(_) => {
                // Refresh environment data and resources
                environment_result.refetch();
                update_counter.update(|c| *c += 1);
                Ok(())
            }
            Err(e) => Err(e),
        }
    });

    let pause_action = Action::new_local(move |_| async move {
        match pause_environment(org, environment_id).await {
            Ok(_) => {
                environment_result.refetch();
                update_counter.update(|c| *c += 1);
                Ok(())
            }
            Err(e) => Err(e),
        }
    });

    let resume_action = Action::new_local(move |_| async move {
        match resume_environment(org, environment_id).await {
            Ok(_) => {
                environment_result.refetch();
                update_counter.update(|c| *c += 1);
                Ok(())
            }
            Err(e) => Err(e),
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
                    environment_status=environment_status.into()
                    delete_modal_open
                    delete_action
                    create_branch_modal_open
                    create_branch_action
                    branch_name
                    sync_action
                    pause_action
                    resume_action
                />
            </Show>

            // Devbox info
            <Show when=move || environment_info.get().map(|env| env.is_shared).unwrap_or(false)>
                <SharedEnvironmentDevboxCard />
            </Show>
            <Show when=move || environment_info.get().map(|env| !env.is_shared).unwrap_or(false)>
                <DevboxSessionBanner active_session environment_id />
            </Show>

            // Environment Resources (Workloads & Services)
            <Show when=move || environment_info.get().is_some()>
                <EnvironmentResourcesTabs
                    environment_id
                    environment_is_shared=environment_info
                        .get()
                        .map(|env| env.is_shared)
                        .unwrap_or(false)
                    filtered_workloads
                    search_query
                    debounced_search
                    all_workloads
                    all_services
                    all_preview_urls
                    all_intercepts
                    active_session
                    update_counter
                    environment_catalog_version
                    readiness_map=readiness_map.clone()
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
    environment_status: Signal<KubeEnvironmentStatus>,
    delete_modal_open: RwSignal<bool>,
    delete_action: Action<(), Result<(), ErrorResponse>>,
    create_branch_modal_open: RwSignal<bool>,
    create_branch_action: Action<(), Result<(), ErrorResponse>>,
    branch_name: RwSignal<String>,
    sync_action: Action<(), Result<(), ErrorResponse>>,
    pause_action: Action<(), Result<(), ErrorResponse>>,
    resume_action: Action<(), Result<(), ErrorResponse>>,
) -> impl IntoView {
    let pause_pending = pause_action.pending();
    let resume_pending = resume_action.pending();
    let pause_result = pause_action.value();
    let resume_result = resume_action.value();

    let status_text = {
        let environment_status = environment_status.clone();
        Signal::derive(move || environment_status.get().to_string())
    };
    let status_variant = {
        let environment_status = environment_status.clone();
        Signal::derive(move || match environment_status.get() {
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
        })
    };
    let can_pause = {
        let environment_status = environment_status.clone();
        Signal::derive(move || {
            matches!(
                environment_status.get(),
                KubeEnvironmentStatus::Running | KubeEnvironmentStatus::PauseFailed
            )
        })
    };
    let can_resume = {
        let environment_status = environment_status.clone();
        Signal::derive(move || {
            matches!(
                environment_status.get(),
                KubeEnvironmentStatus::Paused
                    | KubeEnvironmentStatus::PauseFailed
                    | KubeEnvironmentStatus::ResumeFailed
            )
        })
    };
    let is_deleting = {
        let environment_status = environment_status.clone();
        Signal::derive(move || {
            matches!(
                environment_status.get(),
                KubeEnvironmentStatus::Deleting | KubeEnvironmentStatus::Deleted
            )
        })
    };
    let delete_button_text = {
        let environment_status = environment_status.clone();
        Signal::derive(move || match environment_status.get() {
            KubeEnvironmentStatus::Deleted => "Deleted".to_string(),
            KubeEnvironmentStatus::Deleting => "Deleting...".to_string(),
            _ => "Delete Environment".to_string(),
        })
    };
    let pause_disabled = {
        let can_pause = can_pause.clone();
        let pause_pending = pause_pending.clone();
        Signal::derive(move || !can_pause.get() || pause_pending.get())
    };
    let resume_disabled = {
        let can_resume = can_resume.clone();
        let resume_pending = resume_pending.clone();
        Signal::derive(move || !can_resume.get() || resume_pending.get())
    };

    view! {
        <Show when=move || environment_info.get().is_some() fallback=|| view! { <div></div> }>
           {
                let environment = environment_info.get().unwrap();
                let env_name = environment.name.clone();
                let env_namespace = environment.namespace.clone();
                let env_namespace2 = environment.namespace.clone();
                let created_at_str = environment.created_at.clone();
                let is_shared = environment.is_shared;
                let is_branch = environment.base_environment_id.is_some();
                let app_catalog_id = environment.app_catalog_id;
                let app_catalog_name = environment.app_catalog_name.clone();
                let cluster_id = environment.cluster_id;
                let cluster_name = environment.cluster_name.clone();
                let catalog_update_available = environment.catalog_update_available;
                let catalog_last_sync_actor_id = environment.catalog_last_sync_actor_id;
                let last_catalog_synced_at = environment.last_catalog_synced_at.clone();
                let sync_status = environment.sync_status;
                let last_sync_message = last_catalog_synced_at
                    .clone()
                    .map(|ts| format!("Last synced at {ts}"))
                    .unwrap_or_else(|| "This environment has not been synced from the catalog yet.".to_string());

                // Determine if this is a cluster auto-update or admin edit
                let is_cluster_update = catalog_last_sync_actor_id.is_none();
                let sync_source = if is_cluster_update { "cluster" } else { "catalog" };
                let sync_button_text = if is_cluster_update { "Sync From Cluster" } else { "Sync From Catalog" };
                let sync_description = if is_cluster_update {
                    "New cluster changes are ready to apply. Sync the environment to pull the latest workloads from the production cluster."
                } else {
                    "New catalog changes are ready to apply. Sync the environment to pull the latest workloads."
                };

                let (type_label, type_description) = if is_branch {
                    ("Branch", "Based on a shared environment")
                } else if is_shared {
                    ("Shared", "Accessible by all organization members")
                } else {
                    ("Personal", "Private to you")
                };

                view! {
                    <div class="flex flex-col gap-4">
                        {if catalog_update_available || sync_status == KubeEnvironmentSyncStatus::Syncing {
                            let sync_pending = sync_action.pending();
                            let sync_result = sync_action.value();
                            let is_syncing = sync_status == KubeEnvironmentSyncStatus::Syncing || sync_pending.get();

                            view! {
                                <Alert class="border-amber-200 bg-amber-50 dark:border-amber-900/60 dark:bg-amber-950/40 text-amber-900 dark:text-amber-100">
                                    <lucide_leptos::TriangleAlert attr:class="h-5 w-5 text-amber-600 dark:text-amber-400" />
                                    <AlertTitle>{
                                        if is_syncing {
                                            "Sync in progress".to_string()
                                        } else {
                                            format!("Update available from {}", sync_source)
                                        }
                                    }</AlertTitle>
                                    <AlertDescription class="gap-3">
                                        <span>{
                                            if is_syncing {
                                                "Syncing environment with latest changes..."
                                            } else {
                                                sync_description
                                            }
                                        }</span>
                                        <Show when=move || !is_syncing>
                                            <span class="text-xs text-amber-800 dark:text-amber-300">{last_sync_message.clone()}</span>
                                        </Show>
                                        <Button
                                            variant=ButtonVariant::Outline
                                            size=ButtonSize::Sm
                                            on:click=move |_| { sync_action.dispatch(()); }
                                            disabled=is_syncing
                                        >
                                            <lucide_leptos::RefreshCcw attr:class=move || if is_syncing { "animate-spin" } else { "" } />
                                            {move || if is_syncing { "Syncing..." } else { sync_button_text }}
                                        </Button>
                                        <Show when=move || matches!(sync_result.get(), Some(Err(_)))>
                                            <span class="text-xs text-red-800 dark:text-red-300">
                                                {move || {
                                                    sync_result
                                                        .get()
                                                        .and_then(|res| res.err())
                                                        .map(|err| err.error)
                                                        .unwrap_or_default()
                                                }}
                                            </span>
                                        </Show>
                                    </AlertDescription>
                                </Alert>
                            }.into_any()
                        } else {
                            view! { <></> }.into_any()
                        }}

                        <Card class="p-6">
                        <div class="flex flex-col gap-6">
                            // Header with actions
                            <div class="flex items-start justify-between">
                                <div class="flex flex-col gap-2">
                                    <div class="flex items-center gap-2">
                                        {if is_branch {
                                            view! { <lucide_leptos::GitBranch attr:class="h-5 w-5 text-muted-foreground" /> }.into_any()
                                        } else if is_shared {
                                            view! { <lucide_leptos::Users attr:class="h-5 w-5 text-muted-foreground" /> }.into_any()
                                        } else {
                                            view! { <lucide_leptos::User attr:class="h-5 w-5 text-muted-foreground" /> }.into_any()
                                        }}
                                        <H3 class="m-0">{env_name}</H3>
                                    </div>
                                    <div class="flex items-center gap-2">
                                        <Badge variant=BadgeVariant::Secondary>
                                            {env_namespace}
                                        </Badge>
                                    </div>
                                </div>
                                <div class="flex flex-col items-end gap-2">
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
                                        <Show when=move || !is_shared>
                                            {let pause_disabled = pause_disabled.clone();
                                            let resume_disabled = resume_disabled.clone();
                                            view! {
                                                <>
                                                    <Button
                                                        variant=ButtonVariant::Outline
                                                        on:click=move |_| { pause_action.dispatch(()); }
                                                        disabled=pause_disabled.clone()
                                                    >
                                                        <lucide_leptos::Pause />
                                                        {move || if pause_pending.get() { "Pausing...".to_string() } else { "Pause".to_string() }}
                                                    </Button>
                                                    <Button
                                                        variant=ButtonVariant::Outline
                                                        on:click=move |_| { resume_action.dispatch(()); }
                                                        disabled=resume_disabled.clone()
                                                    >
                                                        <lucide_leptos::Play />
                                                        {move || if resume_pending.get() { "Resuming...".to_string() } else { "Resume".to_string() }}
                                                    </Button>
                                                </>
                                            }.into_any()}
                                        </Show>
                                        {let delete_button_text = delete_button_text.clone();
                                        let delete_disabled = is_deleting.clone();
                                        view! {
                                            <Button
                                                variant=ButtonVariant::Destructive
                                                on:click=move |_| delete_modal_open.set(true)
                                                disabled=delete_disabled
                                            >
                                                <lucide_leptos::Trash2 />
                                                {move || delete_button_text.get()}
                                            </Button>
                                        }.into_any()}
                                    </div>
                                    <Show when=move || matches!(pause_result.get(), Some(Err(_)))>
                                        <span class="text-xs text-red-600 dark:text-red-400">
                                            {move || {
                                                pause_result
                                                    .get()
                                                    .and_then(|res| res.err())
                                                    .map(|err| err.error)
                                                    .unwrap_or_default()
                                            }}
                                        </span>
                                    </Show>
                                    <Show when=move || matches!(resume_result.get(), Some(Err(_)))>
                                        <span class="text-xs text-red-600 dark:text-red-400">
                                            {move || {
                                                resume_result
                                                    .get()
                                                    .and_then(|res| res.err())
                                                    .map(|err| err.error)
                                                    .unwrap_or_default()
                                            }}
                                        </span>
                                    </Show>
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
                                                <lucide_leptos::Layers />
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
                                        {if is_branch {
                                            view! { <lucide_leptos::GitBranch /> }.into_any()
                                        } else if is_shared {
                                            view! { <lucide_leptos::Users /> }.into_any()
                                        } else {
                                            view! { <lucide_leptos::User /> }.into_any()
                                        }}
                                    </div>
                                    <span>Type</span>
                                </div>
                                <div class="flex gap-1 items-center">
                                    <Badge variant=BadgeVariant::Secondary class="flex items-center gap-2 text-sm">
                                        {if is_branch {
                                            view! { <lucide_leptos::GitBranch attr:class="h-3 w-3" /> }.into_any()
                                        } else if is_shared {
                                            view! { <lucide_leptos::Users attr:class="h-3 w-3" /> }.into_any()
                                        } else {
                                            view! { <lucide_leptos::User attr:class="h-3 w-3" /> }.into_any()
                                        }}
                                        <span>{type_label}</span>
                                    </Badge>
                                    <span class="text-sm text-muted-foreground">
                                        {type_description}
                                    </span>
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
                                    <Badge variant=status_variant.clone() class="text-sm">
                                        {move || status_text.get()}
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
                    </div>
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
    environment_is_shared: bool,
    filtered_workloads: Signal<Vec<KubeEnvironmentWorkload>>,
    search_query: RwSignal<String>,
    debounced_search: RwSignal<String>,
    all_workloads: Signal<Vec<KubeEnvironmentWorkload>>,
    all_services: Signal<Vec<KubeEnvironmentService>>,
    all_preview_urls: Signal<Vec<lapdev_common::kube::KubeEnvironmentPreviewUrl>>,
    all_intercepts: Signal<Vec<DevboxWorkloadInterceptSummary>>,
    active_session: LocalResource<Option<DevboxSessionSummary>>,
    update_counter: RwSignal<usize>,
    environment_catalog_version: Signal<i64>,
    readiness_map: RwSignal<HashMap<Uuid, Option<i32>>>,
) -> impl IntoView {
    let active_tab = RwSignal::new(ResourceTab::Workloads);
    let focus_preview_tab = Callback::new({
        let active_tab = active_tab.clone();
        move |_| active_tab.set(ResourceTab::PreviewUrls)
    });

    view! {
        <Tabs default_value=active_tab class="gap-4">
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
                    environment_is_shared
                    filtered_workloads
                    search_query
                    debounced_search
                    all_workloads
                    all_intercepts
                    active_session
                    update_counter
                    environment_catalog_version=environment_catalog_version.clone()
                    readiness_map=readiness_map.clone()
                />
            </TabsContent>

            <TabsContent value=ResourceTab::Services>
                <EnvironmentServicesContent
                    environment_id
                    all_services
                    update_counter
                    focus_preview_tab=focus_preview_tab.clone()
                    preview_urls=all_preview_urls.clone()
                />
            </TabsContent>

            <TabsContent value=ResourceTab::PreviewUrls>
                <crate::kube_environment_preview_url::PreviewUrlsContent
                    environment_id
                    services=all_services
                    preview_urls=all_preview_urls
                    update_counter
                    focus_preview_tab=focus_preview_tab.clone()
                />
            </TabsContent>
        </Tabs>
    }
}

#[component]
pub fn EnvironmentWorkloadsContent(
    environment_id: Uuid,
    environment_is_shared: bool,
    filtered_workloads: Signal<Vec<KubeEnvironmentWorkload>>,
    search_query: RwSignal<String>,
    debounced_search: RwSignal<String>,
    all_workloads: Signal<Vec<KubeEnvironmentWorkload>>,
    all_intercepts: Signal<Vec<DevboxWorkloadInterceptSummary>>,
    active_session: LocalResource<Option<DevboxSessionSummary>>,
    update_counter: RwSignal<usize>,
    environment_catalog_version: Signal<i64>,
    readiness_map: RwSignal<HashMap<Uuid, Option<i32>>>,
) -> impl IntoView {
    let env_catalog_version_signal = environment_catalog_version.clone();

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
                                <TableHead class="w-0">
                                    <span class="sr-only">Branch</span>
                                </TableHead>
                                <TableHead>Name</TableHead>
                                <TableHead>Kind</TableHead>
                                <TableHead>Ready</TableHead>
                                {if environment_is_shared {
                                    view! { <></> }.into_any()
                                } else {
                                    view! { <TableHead>Intercepts</TableHead> }.into_any()
                                }}
                                <TableHead>Containers</TableHead>
                                <TableHead>Actions</TableHead>
                            </TableRow>
                        </TableHeader>
                        <TableBody>
                            <For
                                each=move || filtered_workloads.get()
                                key=|workload| format!("{}-{}-{}", workload.name, workload.namespace, workload.kind)
                            children=move |workload| {
                                let env_catalog_version = env_catalog_version_signal.clone();
                                let ready_signal = {
                                    let readiness_map = readiness_map.clone();
                                    let workload_id = workload.id;
                                    let fallback_ready = workload.ready_replicas;
                                    Signal::derive(move || {
                                        readiness_map
                                            .get()
                                            .get(&workload_id)
                                            .copied()
                                            .flatten()
                                            .or(fallback_ready)
                                    })
                                };
                                view! {
                                    <EnvironmentWorkloadItem
                                        environment_id
                                        environment_is_shared
                                        workload=workload.clone()
                                        all_intercepts
                                        active_session
                                        update_counter
                                        env_catalog_version=env_catalog_version
                                        ready_signal
                                    />
                                }
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
    environment_is_shared: bool,
    workload: KubeEnvironmentWorkload,
    all_intercepts: Signal<Vec<DevboxWorkloadInterceptSummary>>,
    active_session: LocalResource<Option<DevboxSessionSummary>>,
    update_counter: RwSignal<usize>,
    env_catalog_version: Signal<i64>,
    ready_signal: Signal<Option<i32>>,
) -> impl IntoView {
    let workload_id = workload.id;
    let workload_name = workload.name.clone();
    let workload_containers = workload.containers.clone();
    let workload_ports = workload.ports.clone();
    let workload_catalog_version = workload.catalog_sync_version;
    let is_branch_workload = workload
        .base_workload_id
        .map(|base_id| base_id != workload_id)
        .unwrap_or(false);
    let row_class = Signal::derive(move || {
        if env_catalog_version.get() < workload_catalog_version {
            "bg-amber-50/70 dark:bg-amber-900/20 border-l-4 border-amber-500".to_string()
        } else {
            String::new()
        }
    });

    // Find intercepts for this workload
    let workload_intercepts = Signal::derive(move || {
        all_intercepts
            .get()
            .into_iter()
            .filter(|intercept| {
                intercept.workload_id == workload_id && intercept.restored_at.is_none()
            })
            .collect::<Vec<_>>()
    });
    let start_error = RwSignal::new(None::<String>);

    let start_action = {
        let all_intercepts = all_intercepts;
        let ports_for_start = workload_ports.clone();
        Action::new_local(move |_| {
            let all_intercepts = all_intercepts;
            let ports_for_start = ports_for_start.clone();
            async move {
                start_error.set(None);

                let previous_ports = all_intercepts
                    .get_untracked()
                    .into_iter()
                    .filter(|summary| summary.workload_id == workload_id)
                    .max_by(|a, b| a.created_at.cmp(&b.created_at))
                    .map(|summary| summary.port_mappings);

                let port_overrides = match build_port_overrides_from_workload_ports(
                    &ports_for_start,
                    previous_ports.as_ref().map(|v| v.as_slice()),
                ) {
                    Ok(overrides) => overrides,
                    Err(err) => {
                        start_error.set(Some(err.error.clone()));
                        return Err(err);
                    }
                };

                match start_workload_intercept(workload_id, port_overrides, update_counter).await {
                    Ok(()) => Ok(()),
                    Err(err) => {
                        start_error.set(Some(err.error.clone()));
                        Err(err)
                    }
                }
            }
        })
    };
    let start_pending = start_action.pending();

    view! {
        <>
            <TableRow class=row_class>
                <TableCell class="w-0">
                    {if is_branch_workload {
                        view! { <lucide_leptos::GitBranch attr:class="h-4 w-4 text-muted-foreground" /> }.into_any()
                    } else {
                        view! { <></> }.into_any()
                    }}
                </TableCell>
                <TableCell>
                    <a href=format!("/kubernetes/environments/{}/workloads/{}", environment_id, workload.id)>
                        <Button variant=ButtonVariant::Link class="p-0">
                            <span class="font-medium">{workload.name}</span>
                            {if workload.catalog_sync_version > env_catalog_version.get_untracked() {
                                view! {
                                    <Badge variant=BadgeVariant::Destructive class="ml-2 text-[10px] uppercase tracking-wide">
                                        "Update"
                                    </Badge>
                                }.into_any()
                            } else {
                                view! { <></> }.into_any()
                            }}
                        </Button>
                    </a>
                </TableCell>
                <TableCell>
                    <Badge variant=BadgeVariant::Outline>
                        {workload.kind.to_string()}
                    </Badge>
                </TableCell>
                <TableCell>
                    {move || {
                        let ready = ready_signal.get().unwrap_or(0);
                        format!("{ready}/1")
                    }}
                </TableCell>
                {if !environment_is_shared {
                    view! {
                        <TableCell>
                            {move || {
                                let intercepts = workload_intercepts.get();
                                let (has_active_session, is_active_environment) =
                                    match active_session.get().flatten() {
                                        Some(session) => {
                                            let is_active_env =
                                                session.active_environment_id == Some(environment_id);
                                            (true, is_active_env)
                                        }
                                        None => (false, false),
                                    };

                                if intercepts.is_empty() {
                                    view! {
                                        <div class="flex flex-col gap-2">
                                            <Button
                                                variant=ButtonVariant::Outline
                                                size=ButtonSize::Sm
                                                class="self-start w-auto"
                                                on:click=move |_| { start_action.dispatch(()); }
                                                disabled=Signal::derive(move || start_pending.get())
                                            >
                                                <lucide_leptos::Cable />
                                                {move || if start_pending.get() { "Starting..." } else { "Start Intercept" }}
                                            </Button>
                                            <Show when=move || start_error.get().is_some()>
                                                <P class="text-xs text-destructive">
                                                    {move || start_error.get().unwrap_or_default()}
                                                </P>
                                            </Show>
                                        </div>
                                    }.into_any()
                                } else {
                                    view! {
                                        <div class="flex flex-col gap-2">
                                            {intercepts.into_iter().map(|intercept| {
                                                view! {
                                                    <WorkloadInterceptCard
                                                        intercept=intercept
                                                        has_active_session=has_active_session
                                                        is_active_environment=is_active_environment
                                                        update_counter
                                                        workload_ports=workload_ports.clone()
                                                        workload_id
                                                        workload_name=workload_name.clone()
                                                    />
                                                }
                                            }).collect::<Vec<_>>()}
                                        </div>
                                    }.into_any()
                                }
                            }}
                        </TableCell>
                    }.into_any()
                } else {
                    view! { <></> }.into_any()
                }}
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
                    // Actions removed - workload deletion is no longer available
                </TableCell>
            </TableRow>
        </>
    }
}

#[component]
fn WorkloadInterceptCard(
    intercept: DevboxWorkloadInterceptSummary,
    has_active_session: bool,
    is_active_environment: bool,
    update_counter: RwSignal<usize>,
    workload_ports: Vec<lapdev_common::kube::KubeServicePort>,
    workload_id: Uuid,
    workload_name: String,
) -> impl IntoView {
    let intercept_id = intercept.intercept_id;
    let restored_at = intercept.restored_at;
    let port_mappings = intercept.port_mappings.clone();

    let is_stopped = restored_at.is_some();

    let (bg_class, border_class, badge_class, status_text) = if is_stopped {
        (
            "bg-gray-50 dark:bg-gray-950/20",
            "border-gray-200 dark:border-gray-900",
            "text-xs",
            "Stopped",
        )
    } else if has_active_session && is_active_environment {
        (
            "bg-green-50 dark:bg-green-950/20",
            "border-green-200 dark:border-green-900",
            "text-xs bg-green-100 text-green-800 dark:bg-green-900/30 dark:text-green-300",
            "Active",
        )
    } else if has_active_session {
        (
            "bg-red-50 dark:bg-red-950/20",
            "border-red-200 dark:border-red-900",
            "text-xs bg-red-100 text-red-800 dark:bg-red-900/30 dark:text-red-300",
            "Inactive Environment",
        )
    } else {
        (
            "bg-amber-50 dark:bg-amber-950/20",
            "border-amber-200 dark:border-amber-900",
            "text-xs bg-amber-100 text-amber-800 dark:bg-amber-900/30 dark:text-amber-300",
            "No Session",
        )
    };

    let error_message = RwSignal::new(None::<String>);
    let edit_modal_open = RwSignal::new(false);
    let stop_action = Action::new_local({
        move |_| {
            let error_message = error_message;
            async move {
                let result = stop_workload_intercept(intercept_id, update_counter).await;
                if let Err(err) = &result {
                    error_message.set(Some(err.error.clone()));
                } else {
                    error_message.set(None);
                }
                result
            }
        }
    });
    let stop_pending = stop_action.pending();

    let port_mappings_for_modal = port_mappings.clone();

    view! {
        <div class=format!("flex flex-col gap-3 p-3 rounded border {} {}", bg_class, border_class)>
            <div class="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between sm:gap-4">
                <div class="flex items-center gap-2">
                    <lucide_leptos::Cable attr:class="h-3 w-3" />
                    <Badge variant=BadgeVariant::Secondary class=badge_class>
                        {status_text}
                    </Badge>
                </div>
                {if !is_stopped {
                    view! {
                        <div class="flex items-center gap-2">
                            <Button
                                variant=ButtonVariant::Outline
                                size=ButtonSize::Sm
                                class="h-7 px-2 text-xs gap-1"
                                on:click=move |_| edit_modal_open.set(true)
                            >
                                <lucide_leptos::SlidersHorizontal attr:class="h-3 w-3" />
                                "Edit Ports"
                            </Button>
                            <Button
                                variant=ButtonVariant::Destructive
                                size=ButtonSize::Sm
                                class="h-7 px-2 text-xs gap-1"
                                on:click=move |_| { stop_action.dispatch(()); }
                                disabled=Signal::derive(move || stop_pending.get())
                            >
                                <lucide_leptos::X attr:class="h-3 w-3" />
                                {move || if stop_pending.get() { "Stopping..." } else { "Stop" }}
                            </Button>
                        </div>
                    }.into_any()
                } else {
                    ().into_any()
                }}
            </div>
            <div class="flex flex-col gap-1">
                <span class="text-[10px] uppercase tracking-wide text-muted-foreground">
                    Ports
                </span>
                <div class="text-xs font-mono flex flex-col gap-1">
                    {port_mappings
                        .iter()
                        .map(|pm| {
                            let mapping = format!("{} → {}", pm.workload_port, pm.local_port);
                            view! { <span>{mapping}</span> }
                        })
                        .collect::<Vec<_>>()
                        .into_any()}
                </div>
            </div>
            <Show when=move || error_message.get().is_some()>
                <P class="text-xs text-destructive">
                    {move || error_message.get().unwrap_or_default()}
                </P>
            </Show>
            <EditInterceptModal
                open=edit_modal_open
                intercept_id
                workload_id
                workload_name=workload_name.clone()
                workload_ports=workload_ports.clone()
                existing_mappings=port_mappings_for_modal
                update_counter
            />
        </div>
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
    focus_preview_tab: Callback<()>,
    preview_urls: Signal<Vec<KubeEnvironmentPreviewUrl>>,
) -> impl IntoView {
    view! {
                // Services table
                <div class="rounded-lg border relative">
                    <Table>
                        <TableHeader class="bg-muted">
                            <TableRow>
                                <TableHead>Name</TableHead>
                                <TableHead>Ports</TableHead>
                                <TableHead>Selectors</TableHead>
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
                                            update_counter
                                            focus_preview_tab=focus_preview_tab.clone()
                                            preview_urls=preview_urls.clone()
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
    update_counter: RwSignal<usize>,
    focus_preview_tab: Callback<()>,
    preview_urls: Signal<Vec<KubeEnvironmentPreviewUrl>>,
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
                <div class="text-sm flex flex-col gap-1">
                    {if ports.is_empty() {
                        view! { <span class="text-muted-foreground">"No ports"</span> }.into_any()
                    } else {
                        ports
                            .into_iter()
                            .map(|port| {
                                let target_port = port
                                    .original_target_port
                                    .map(|tp| format!(":{tp}"))
                                    .unwrap_or_default();
                                let protocol = port.protocol.as_deref().unwrap_or("TCP");
                                let port_text = format!("{}{} ({protocol})", port.port, target_port);
                                let port_number = port.port;
                                let service_id = service.id;
                                let preview_urls = preview_urls.clone();
                                let preview_for_port = Signal::derive(move || {
                                    preview_urls
                                        .get()
                                        .into_iter()
                                        .find(|url| url.service_id == service_id && url.port == port_number)
                                });
                                view! {
                                    <div class="flex flex-row items-center gap-2">
                                        <Badge variant=BadgeVariant::Outline class="text-xs w-fit">
                                            {port_text.clone()}
                                        </Badge>
                                        <Show
                                            when=move || preview_for_port.get().is_some()
                                            fallback=|| view! { <></> }
                                        >
                                            {move || {
                                                let preview = preview_for_port.get().unwrap();
                                                let preview_url = preview.url.clone();
                                                view! {
                                                    <a
                                                        href=preview.url.clone()
                                                        target="_blank"
                                                        rel="noopener noreferrer"
                                                        class="text-primary text-xs inline-flex items-center gap-1"
                                                    >
                                                        <Button variant=ButtonVariant::Link>
                                                            <span class="text-xs">{preview_url}</span>
                                                            <lucide_leptos::ExternalLink attr:class="h-3 w-3" />
                                                        </Button>
                                                    </a>
                                                }
                                            }}
                                        </Show>
                                    </div>
                                }
                            })
                            .collect::<Vec<_>>()
                            .into_any()
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
                    on_created=focus_preview_tab.clone()
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

#[derive(Debug, Clone)]
struct AvailablePort {
    container_name: String,
    port: i32,
    port_name: Option<String>,
    protocol: Option<String>,
}

#[derive(Debug, Clone)]
struct PortMapping {
    port: i32,
    default_local_port: String,
    local_port: RwSignal<String>,
}

#[component]
pub fn EditInterceptModal(
    open: RwSignal<bool>,
    intercept_id: Uuid,
    workload_id: Uuid,
    workload_name: String,
    workload_ports: Vec<lapdev_common::kube::KubeServicePort>,
    existing_mappings: Vec<DevboxPortMapping>,
    update_counter: RwSignal<usize>,
) -> impl IntoView {
    // Extract all available ports from workload service ports, preferring the recorded original container port.
    let available_ports: Vec<AvailablePort> = workload_ports
        .iter()
        .map(|port| AvailablePort {
            container_name: String::new(), // Service ports don't have container names
            port: port.original_target_port.unwrap_or(port.port),
            port_name: port.name.clone(),
            protocol: port.protocol.clone(),
        })
        .collect();

    use std::convert::TryFrom;

    let mapping_lookup: std::collections::HashMap<u16, u16> = existing_mappings
        .into_iter()
        .map(|mapping| (mapping.workload_port, mapping.local_port))
        .collect();

    let mut port_mappings_vec = Vec::new();
    for ap in &available_ports {
        let default_local_port = u16::try_from(ap.port)
            .ok()
            .map(|workload_port| {
                mapping_lookup
                    .get(&workload_port)
                    .copied()
                    .unwrap_or(workload_port)
                    .to_string()
            })
            .unwrap_or_default();

        port_mappings_vec.push(PortMapping {
            port: ap.port,
            default_local_port: default_local_port.clone(),
            local_port: RwSignal::new(default_local_port),
        });
    }
    let port_mappings_stored = StoredValue::new(port_mappings_vec);

    // Reset form when modal closes
    Effect::new(move |_| {
        if !open.get() {
            for mapping in &port_mappings_stored.get_value() {
                mapping.local_port.set(mapping.default_local_port.clone());
            }
        }
    });

    let error_message = RwSignal::new(None::<String>);
    let update_action = Action::new_local(move |_| async move {
        error_message.set(None);

        let mut port_overrides = Vec::new();
        for mapping in &port_mappings_stored.get_value() {
            let workload_port = match u16::try_from(mapping.port) {
                Ok(value) => value,
                Err(_) => {
                    let err = ErrorResponse {
                        error: format!("Unsupported workload port: {}", mapping.port),
                    };
                    error_message.set(Some(err.error.clone()));
                    return Err(err);
                }
            };

            let local_port_str = mapping.local_port.get().trim().to_string();
            let local_port = if local_port_str.is_empty() {
                Some(workload_port)
            } else {
                match local_port_str.parse::<u16>() {
                    Ok(value) => Some(value),
                    Err(_) => {
                        let err = ErrorResponse {
                            error: format!("Invalid local port: {}", local_port_str),
                        };
                        error_message.set(Some(err.error.clone()));
                        return Err(err);
                    }
                }
            };

            port_overrides.push(DevboxPortMappingOverride {
                workload_port,
                local_port,
            });
        }

        if port_overrides.is_empty() {
            let err = ErrorResponse {
                error: "No ports available to intercept".to_string(),
            };
            error_message.set(Some(err.error.clone()));
            return Err(err);
        }

        if let Err(err) = stop_workload_intercept(intercept_id, update_counter).await {
            error_message.set(Some(err.error.clone()));
            return Err(err);
        }

        match start_workload_intercept(workload_id, port_overrides, update_counter).await {
            Ok(_) => {
                open.set(false);
                Ok(())
            }
            Err(err) => {
                error_message.set(Some(err.error.clone()));
                Err(err)
            }
        }
    });
    view! {
        <Modal
            open=open
            action=update_action
            title="Edit Intercept Ports"
            action_text="Save Changes"
            action_progress_text="Saving..."
        >
            <div class="flex flex-col gap-4">
                <P class="text-sm text-muted-foreground">
                    {format!(
                        "Adjust local port overrides for workload: {}. Changes will restart the intercept.",
                        workload_name
                    )}
                </P>

                {if available_ports.is_empty() {
                    view! {
                        <div class="bg-amber-50 dark:bg-amber-950/20 border border-amber-200 dark:border-amber-900 rounded p-4">
                            <div class="flex items-start gap-3">
                                <lucide_leptos::TriangleAlert attr:class="h-5 w-5 text-amber-600 dark:text-amber-400 shrink-0 mt-0.5" />
                                <div class="flex flex-col gap-1">
                                    <span class="font-medium text-sm text-amber-900 dark:text-amber-200">
                                        "No exposed ports found"
                                    </span>
                                    <span class="text-xs text-amber-800 dark:text-amber-300">
                                        "This workload does not expose any container ports. Make sure your containers define port specifications in their Kubernetes manifests."
                                    </span>
                                </div>
                            </div>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="flex flex-col gap-3">
                            <div class="flex items-center justify-between">
                                <Label>"Port mappings"</Label>
                                <Badge variant=BadgeVariant::Secondary class="text-xs">
                                    {format!("{} port{}", available_ports.len(), if available_ports.len() == 1 { "" } else { "s" })}
                                </Badge>
                            </div>

                            <div class="flex flex-col gap-2 max-h-64 overflow-y-auto">
                                {available_ports.iter().enumerate().map(|(idx, port)| {
                                    let mapping = port_mappings_stored.get_value()[idx].clone();
                                    let port_display = if let Some(name) = &port.port_name {
                                        format!("{} ({})", port.port, name)
                                    } else {
                                        format!("{}", port.port)
                                    };
                                    let protocol_display = port.protocol.clone().unwrap_or_else(|| "TCP".to_string());

                                    view! {
                                        <div class="flex items-center gap-3 p-3 border rounded bg-muted/30">
                                            <div class="flex-1 flex items-center gap-2">
                                                <lucide_leptos::Network attr:class="h-4 w-4 text-muted-foreground" />
                                                <span class="font-medium">{port_display}</span>
                                                <Badge variant=BadgeVariant::Outline class="text-xs">
                                                    {protocol_display}
                                                </Badge>
                                            </div>
                                            <div class="w-40">
                                                <Input
                                                    attr:id=format!("local-port-{}", idx)
                                                    attr:r#type="number"
                                                    attr:min="1"
                                                    attr:max="65535"
                                                    prop:value=move || mapping.local_port.get()
                                                    on:input=move |ev| {
                                                        mapping.local_port.set(event_target_value(&ev));
                                                    }
                                                    attr:placeholder=format!("{}", port.port)
                                                    class="h-8 text-sm"
                                                />
                                            </div>
                                        </div>
                                    }
                                }).collect::<Vec<_>>()}
                            </div>
                        </div>

                        <div class="text-xs text-muted-foreground bg-muted/30 p-3 rounded">
                            <div class="flex gap-2">
                                <lucide_leptos::Info attr:class="h-4 w-4 shrink-0 mt-0.5" />
                                <div class="flex flex-col gap-1">
                                    <span class="font-medium">"Tips"</span>
                                    <ul class="list-disc list-inside space-y-1">
                                        <li>"Leave a field blank to use the workload port as the local port"</li>
                                        <li>"Updates are applied by restarting the intercept with the new configuration"</li>
                                    </ul>
                                </div>
                            </div>
                        </div>
                    }.into_any()
                }}

                <Show when=move || error_message.get().is_some()>
                    <P class="text-xs text-destructive">
                        {move || error_message.get().unwrap_or_default()}
                    </P>
                </Show>
            </div>
        </Modal>
    }
}

#[component]
pub fn SharedEnvironmentDevboxCard() -> impl IntoView {
    view! {
        <Card class="border-amber-200 bg-amber-50 dark:border-amber-900/60 dark:bg-amber-950/30">
            <div class="p-4 flex flex-col gap-4 md:flex-row md:items-start md:justify-between">
                <div class="flex gap-3">
                    <div class="rounded-full bg-amber-100 dark:bg-amber-900/40 p-2">
                        <lucide_leptos::ShieldAlert attr:class="h-5 w-5 text-amber-700 dark:text-amber-300" />
                    </div>
                    <div class="flex flex-col gap-1">
                        <span class="font-semibold text-amber-900 dark:text-amber-100">
                            "Devbox isn't available on shared environments"
                        </span>
                        <p class="text-sm text-amber-800 dark:text-amber-300">
                            "Branch this shared baseline to unlock Devbox intercepts. Personal and branch environments can be selected as your active Devbox namespace."
                        </p>
                    </div>
                </div>
                <div class="flex items-center gap-2">
                    <a href=docs_url(DOCS_DEVBOX_PATH) target="_blank" rel="noopener noreferrer">
                        <Button
                            variant=ButtonVariant::Outline
                            size=ButtonSize::Sm
                            class="border-amber-300 text-amber-800 dark:border-amber-800 dark:text-amber-100"
                        >
                            <lucide_leptos::BookOpen attr:class="h-4 w-4" />
                            "Devbox Docs"
                            <lucide_leptos::ExternalLink attr:class="h-3 w-3" />
                        </Button>
                    </a>
                </div>
            </div>
        </Card>
    }
}

#[component]
pub fn DevboxSessionBanner(
    active_session: LocalResource<Option<DevboxSessionSummary>>,
    environment_id: Uuid,
) -> impl IntoView {
    let is_expanded = RwSignal::new(false);

    // Action to set this environment as active
    let active_session_clone = active_session.clone();
    let set_active_action = Action::new_local(move |_| {
        let active_session_clone = active_session_clone.clone();
        async move {
            let client = HrpcServiceClient::new("/api/rpc".to_string());
            match client
                .devbox_session_set_active_environment(environment_id)
                .await
            {
                Ok(Ok(())) => {
                    active_session_clone.refetch();
                    Ok(())
                }
                Ok(Err(e)) => Err(ErrorResponse {
                    error: format!("{:?}", e),
                }),
                Err(e) => Err(ErrorResponse {
                    error: e.to_string(),
                }),
            }
        }
    });
    let set_active_pending = set_active_action.pending();
    let set_active_result = set_active_action.value();

    view! {
        <Suspense fallback=move || view! { <div></div> }>
            {move || {
                active_session
                    .get()
                    .map(|session_opt| {
                        if let Some(session) = session_opt {
                            // Active session exists
                            let is_active = session.active_environment_id == Some(environment_id);

                            view! {
                                <div class="rounded-lg border bg-muted/50 p-4">
                                    <div class="flex items-center justify-between">
                                        <div class="flex items-center gap-3">
                                            <div class="rounded-full bg-primary/10 p-2">
                                                <lucide_leptos::Plug attr:class="h-5 w-5 text-primary" />
                                            </div>
                                            <div class="flex flex-col gap-1">
                                                <div class="flex items-center gap-2">
                                                    <span class="font-semibold">Devbox Connected</span>
                                                    <Badge variant=BadgeVariant::Secondary>
                                                        <lucide_leptos::Monitor attr:class="h-3 w-3" />
                                                        {session.device_name.clone()}
                                                    </Badge>
                                                    {if is_active {
                                                        view! {
                                                            <Badge variant=BadgeVariant::Secondary class="flex items-center gap-1 text-sm bg-green-100 text-green-800 dark:bg-green-900/30 dark:text-green-300">
                                                                <lucide_leptos::Check attr:class="h-3 w-3" />
                                                                "Active Environment"
                                                            </Badge>
                                                        }.into_any()
                                                    } else {
                                                        view! {
                                                            <Badge variant=BadgeVariant::Secondary class="flex items-center gap-1 text-sm bg-red-100 text-red-800 dark:bg-red-900/30 dark:text-red-300">
                                                                <lucide_leptos::Ban attr:class="h-3 w-3" />
                                                                "Inactive Environment"
                                                            </Badge>
                                                        }
                                                            .into_any()
                                                    }}
                                                </div>
                                                <div class="text-sm text-muted-foreground flex items-center gap-1">
                                                    <span>"Last active: "</span>
                                                    <DatetimeModal time=session.last_used_at.fixed_offset() />
                                                </div>
                                            </div>
                                        </div>
                                        {if !is_active {
                                            view! {
                                                <Button
                                                    variant=ButtonVariant::Outline
                                                    size=ButtonSize::Sm
                                                    on:click=move |_| { set_active_action.dispatch(()); }
                                                    disabled=Signal::derive(move || set_active_pending.get())
                                                >
                                                    <lucide_leptos::Check attr:class="h-4 w-4" />
                                                    {move || if set_active_pending.get() { "Setting..." } else { "Set as Active" }}
                                                </Button>
                                            }
                                                .into_any()
                                        } else {
                                            ().into_any()
                                        }}
                                        <Show when=move || matches!(set_active_result.get(), Some(Err(_)))>
                                            <P class="text-sm text-destructive mt-2">
                                                {move || {
                                                    set_active_result
                                                        .get()
                                                        .and_then(|res| res.err())
                                                        .map(|err| err.error)
                                                        .unwrap_or_default()
                                                }}
                                            </P>
                                        </Show>
                                    </div>
                                </div>
                            }
                                .into_any()
                        } else {
                            // No active session - expandable panel
                            view! {
                                <div class="rounded-lg border bg-amber-50 dark:bg-amber-950/20 border-amber-200 dark:border-amber-900">
                                    <div class="p-4">
                                        <div class="flex items-start justify-between gap-3">
                                            <div
                                                class="flex items-start gap-3 flex-1 cursor-pointer"
                                                on:click=move |_| {
                                                    is_expanded.update(|v| *v = !*v);
                                                }
                                            >
                                                <div class="rounded-full bg-amber-100 dark:bg-amber-900/30 p-2 mt-0.5">
                                                    <lucide_leptos::Info attr:class="h-5 w-5 text-amber-700 dark:text-amber-500" />
                                                </div>
                                                <div class="flex-1">
                                                    <h4 class="font-semibold text-amber-900 dark:text-amber-200 mb-1">
                                                        "You Can Connect Your Local Development Machine"
                                                    </h4>
                                        <Show when=move || !is_expanded.get() fallback=|| view! {
                                            <p class="text-sm text-amber-800 dark:text-amber-300">
                                                "To intercept workloads and develop locally, you can connect your machine using the Lapdev CLI."
                                            </p>
                                        }>
                                                        <p class="text-sm text-amber-800 dark:text-amber-300 hover:text-amber-900 dark:hover:text-amber-200">
                                                            "Click to view setup instructions"
                                                        </p>
                                                    </Show>
                                                </div>
                                            </div>
                                            <Button
                                                variant=ButtonVariant::Ghost
                                                size=ButtonSize::Sm
                                                on:click=move |_| is_expanded.update(|v| *v = !*v)
                                                class="text-amber-700 dark:text-amber-400 hover:bg-amber-100 dark:hover:bg-amber-900/30 shrink-0"
                                            >
                                                <Show when=move || is_expanded.get() fallback=|| view! {
                                                    <lucide_leptos::ChevronDown attr:class="h-4 w-4" />
                                                }>
                                                    <lucide_leptos::ChevronUp attr:class="h-4 w-4" />
                                                </Show>
                                            </Button>
                                        </div>

                                        <Show when=move || is_expanded.get()>
                                            <DevboxInstallInstructions />
                                        </Show>
                                    </div>
                                </div>
                            }
                                .into_any()
                        }
                    })
            }}
        </Suspense>
    }
}

#[component]
fn DevboxInstallInstructions() -> impl IntoView {
    let default_tab = RwSignal::new(detect_devbox_install_tab());

    view! {
        <div class="mt-3 ml-14 flex flex-col gap-3">
            <div class="flex flex-col gap-2 text-sm">
                <div class="flex items-start gap-2">
                    <span class="font-medium text-amber-900 dark:text-amber-200 min-w-[2rem]">
                        "1."
                    </span>
                    <div class="flex-1 flex flex-col gap-2">
                        <span class="text-amber-800 dark:text-amber-300">
                            "Install the CLI:"
                        </span>
                        <Tabs default_value=default_tab class="gap-2">
                            <TabsList class="w-full gap-2">
                                <TabsTrigger value=DevboxInstallTab::LinuxMac class="flex-1">
                                    "Linux / macOS"
                                </TabsTrigger>
                                <TabsTrigger value=DevboxInstallTab::Windows class="flex-1">
                                    "Windows PowerShell"
                                </TabsTrigger>
                            </TabsList>
                            <TabsContent value=DevboxInstallTab::LinuxMac class="mt-2">
                                <code class="block px-2 py-1 rounded bg-amber-100 dark:bg-amber-900/50 text-amber-900 dark:text-amber-200 font-mono text-xs break-all">
                                    {DEVBOX_INSTALL_UNIX_CMD}
                                </code>
                            </TabsContent>
                            <TabsContent value=DevboxInstallTab::Windows class="mt-2">
                                <code class="block px-2 py-1 rounded bg-amber-100 dark:bg-amber-900/50 text-amber-900 dark:text-amber-200 font-mono text-xs break-all">
                                    {DEVBOX_INSTALL_WINDOWS_CMD}
                                </code>
                            </TabsContent>
                        </Tabs>
                    </div>
                </div>
                <div class="flex items-start gap-2">
                    <span class="font-medium text-amber-900 dark:text-amber-200 min-w-[2rem]">
                        "2."
                    </span>
                    <div class="flex-1">
                        <span class="text-amber-800 dark:text-amber-300">
                            "Connect: "
                        </span>
                        <code class="px-2 py-1 rounded bg-amber-100 dark:bg-amber-900/50 text-amber-900 dark:text-amber-200 font-mono text-xs">
                            "lapdev connect"
                        </code>
                    </div>
                </div>
            </div>
            <div class="flex gap-2">
                <a href=docs_url(DOCS_DEVBOX_PATH) target="_blank" rel="noopener noreferrer">
                    <Button
                        variant=ButtonVariant::Outline
                        size=ButtonSize::Sm
                        class="text-amber-700 dark:text-amber-400 border-amber-300 dark:border-amber-700 hover:bg-amber-100 dark:hover:bg-amber-900/30"
                    >
                        <lucide_leptos::BookOpen attr:class="h-4 w-4" />
                        "View Documentation"
                        <lucide_leptos::ExternalLink attr:class="h-3 w-3" />
                    </Button>
                </a>
            </div>
        </div>
    }
}
