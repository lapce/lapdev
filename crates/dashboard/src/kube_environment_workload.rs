use std::str::FromStr;

use anyhow::{anyhow, Result};
use lapdev_common::{
    console::Organization,
    kube::{KubeContainerInfo, KubeEnvironment, KubeEnvironmentWorkload},
};
use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_params_map};
use uuid::Uuid;

use crate::{
    app::{get_hrpc_client, AppConfig},
    component::{
        badge::{Badge, BadgeVariant},
        button::{Button, ButtonVariant},
        card::Card,
        typography::{H3, P},
    },
    docs_url,
    kube_container::{ContainerEditorConfig, ContainersCard},
    modal::{DatetimeModal, ErrorResponse},
    organization::get_current_org,
    DOCS_ENVIRONMENT_PATH,
};

#[component]
pub fn KubeEnvironmentWorkload() -> impl IntoView {
    let params = use_params_map();
    let environment_id = Memo::new(move |_| {
        params.with(|params| {
            params
                .get("environment_id")
                .and_then(|id| Uuid::from_str(id.as_str()).ok())
                .unwrap_or_default()
        })
    });
    let workload_id = Memo::new(move |_| {
        params.with(|params| {
            params
                .get("workload_id")
                .and_then(|id| Uuid::from_str(id.as_str()).ok())
                .unwrap_or_default()
        })
    });

    view! {
        <div class="flex flex-col gap-6">
            <div class="flex flex-col gap-2 items-start">
                <H3>Environment Workload Details</H3>
                <P>
                    View and manage details for this environment workload.
                </P>
                <a href=docs_url(DOCS_ENVIRONMENT_PATH) target="_blank" rel="noopener noreferrer">
                    <Badge variant=BadgeVariant::Secondary>
                        Docs <lucide_leptos::ExternalLink />
                    </Badge>
                </a>
            </div>

            <EnvironmentWorkloadDetail environment_id workload_id />
        </div>
    }
}

async fn get_environment_workload_detail(
    org: Signal<Option<Organization>>,
    environment_id: Uuid,
    workload_id: Uuid,
) -> Result<(KubeEnvironment, KubeEnvironmentWorkload)> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = get_hrpc_client();

    let detail = client
        .get_environment_workload_detail(org.id, environment_id, workload_id)
        .await??;

    Ok((detail.environment, detail.workload))
}

async fn update_environment_workload_containers(
    org: Signal<Option<Organization>>,
    environment_id: Uuid,
    workload_id: Uuid,
    containers: Vec<KubeContainerInfo>,
    update_counter: RwSignal<usize>,
) -> Result<Uuid, ErrorResponse> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = get_hrpc_client();

    let new_id = client
        .update_environment_workload(org.id, environment_id, workload_id, containers)
        .await??;

    update_counter.update(|c| *c += 1);

    Ok(new_id)
}

#[component]
pub fn EnvironmentWorkloadDetail(
    environment_id: Memo<Uuid>,
    workload_id: Memo<Uuid>,
) -> impl IntoView {
    let org = get_current_org();
    let is_loading = RwSignal::new(false);
    let update_counter = RwSignal::new(0usize);
    let navigate = StoredValue::new_local(use_navigate());

    let config = use_context::<AppConfig>().unwrap();

    let workload_result = LocalResource::new(move || {
        update_counter.track(); // Make reactive to updates
        async move {
            is_loading.set(true);
            let environment_id = environment_id.get_untracked();
            let workload_id = workload_id.get_untracked();
            let result = get_environment_workload_detail(org, environment_id, workload_id)
                .await
                .ok();
            if let Some((env, workload)) = result.as_ref() {
                config.current_page.set(workload.name.clone());
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
                    header_links.push((
                        env.name.clone(),
                        format!("/kubernetes/environments/{environment_id}"),
                    ));
                });
            }
            is_loading.set(false);
            result
        }
    });

    let workload_info = Signal::derive(move || workload_result.get().flatten());

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
                <EnvironmentWorkloadInfoCard
                    workload_info
                />
            </Show>

            // Containers Card
            <Show when=move || workload_info.get().is_some()>
                {move || {
                    if let Some((_, workload)) = workload_info.get() {
                        let workload_id = workload.id;
                        let all_containers = workload.containers.clone();
                        let navigate_for_action = navigate.get_value();
                        let update_action = Action::new_local(move |containers: &Vec<KubeContainerInfo>| {
                            let containers = containers.clone();
                            let current_workload_id = workload_id;
                            let navigate = navigate_for_action.clone();
                            async move {
                                let result = update_environment_workload_containers(
                                    org,
                                    environment_id.get_untracked(),
                                    current_workload_id,
                                    containers,
                                    update_counter,
                                )
                                .await;

                                if let Ok(new_id) = &result {
                                    if *new_id != current_workload_id {
                                        let url = format!(
                                            "/kubernetes/environments/{}/workloads/{}",
                                            environment_id.get_untracked(), new_id
                                        );
                                        let _ = navigate(&url, Default::default());
                                    }
                                }

                                result
                            }
                        });

                        let config = ContainerEditorConfig {
                            enable_resource_limits: false,
                            show_customization_badge: true,
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

        </div>
    }
}

#[component]
pub fn EnvironmentWorkloadInfoCard(
    workload_info: Signal<Option<(KubeEnvironment, KubeEnvironmentWorkload)>>,
) -> impl IntoView {
    view! {
        <Show when=move || workload_info.get().is_some() fallback=|| view! { <div></div> }>
           {
                let (environment, workload) = workload_info.get().unwrap();
                let workload_name = workload.name.clone();
                let workload_namespace = workload.namespace.clone();
                let workload_namespace2 = workload.namespace.clone();
                let workload_kind = workload.kind.clone();
                let workload_kind2 = workload.kind.clone();
                let created_at_str = workload.created_at.clone();
                let environment_name = environment.name.clone();
                let environment_id = environment.id;
                let environment_cluster_id = environment.cluster_id;
                let environment_cluster_name = environment.cluster_name.clone();
                let is_branch = environment.base_environment_id.is_some();
                let is_shared = environment.is_shared;

                view! {
                    <Card class="p-6">
                        <div class="flex flex-col gap-6">
                            // Header
                            <div class="flex flex-col gap-2">
                                <H3>{workload_name}</H3>
                                <div class="flex items-center gap-2">
                                    <Badge variant=BadgeVariant::Secondary>
                                        {workload_namespace}
                                    </Badge>
                                    <Badge variant=BadgeVariant::Outline>
                                        {workload_kind}
                                    </Badge>
                                </div>
                            </div>

                            // Metadata grid
                            <div class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-4 text-sm">
                                <div class="flex items-center gap-2 text-muted-foreground font-medium">
                                    <div class="[&>svg]:size-4 [&>svg]:shrink-0">
                                        <lucide_leptos::Layers />
                                    </div>
                                    <span>Environment</span>
                                </div>
                                <div class="flex items-center gap-2">
                                    {if is_branch {
                                        view! { <lucide_leptos::GitBranch attr:class="h-4 w-4 text-muted-foreground" /> }.into_any()
                                    } else if is_shared {
                                        view! { <lucide_leptos::Users attr:class="h-4 w-4 text-muted-foreground" /> }.into_any()
                                    } else {
                                        view! { <lucide_leptos::User attr:class="h-4 w-4 text-muted-foreground" /> }.into_any()
                                    }}
                                    <a href=format!("/kubernetes/environments/{}", environment_id) class="text-foreground hover:underline">
                                        {environment_name.clone()}
                                    </a>
                                </div>

                                <div class="flex items-center gap-2 text-muted-foreground font-medium">
                                    <div class="[&>svg]:size-4 [&>svg]:shrink-0">
                                        <lucide_leptos::Server />
                                    </div>
                                    <span>Cluster</span>
                                </div>
                                <div class="flex items-center gap-2">
                                    <a href=format!("/kubernetes/clusters/{}", environment_cluster_id) class="text-foreground hover:underline">
                                        {environment_cluster_name}
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
                                        <lucide_leptos::Package />
                                    </div>
                                    <span>Workload Kind</span>
                                </div>
                                <div>
                                    <Badge variant=BadgeVariant::Outline>
                                        {workload_kind2}
                                    </Badge>
                                </div>

                                <div class="flex items-center gap-2 text-muted-foreground font-medium">
                                    <div class="[&>svg]:size-4 [&>svg]:shrink-0">
                                        <lucide_leptos::Calendar />
                                    </div>
                                    <span>Created</span>
                                </div>
                                <div>
                                    <DatetimeModal time=workload.created_at />
                                </div>
                            </div>
                        </div>
                    </Card>
                }
            }
        </Show>
    }
}
