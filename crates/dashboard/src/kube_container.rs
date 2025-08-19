use uuid::Uuid;
use leptos::prelude::*;
use lapdev_common::kube::{KubeEnvironment, KubeEnvironmentWorkload, KubeContainerInfo};

use crate::{
    component::{
        badge::{Badge, BadgeVariant},
        card::Card,
        typography::{H4, P},
    },
    kube_environment_workload::EnvironmentContainerEditor,
};

#[component]
pub fn EnvironmentWorkloadContainersCard(
    workload_info: Signal<Option<(KubeEnvironment, KubeEnvironmentWorkload)>>,
    update_counter: RwSignal<usize>,
) -> impl IntoView {
    view! {
        <Show when=move || workload_info.get().is_some() fallback=|| view! { <div></div> }>
            {move || {
                let (_, workload) = workload_info.get().unwrap();
                let containers = workload.containers.clone();

                view! {
                    <Card class="p-6">
                        <div class="flex flex-col gap-6">
                            <div class="flex items-center justify-between">
                                <H4>Container Configuration</H4>
                                <Badge variant=BadgeVariant::Secondary>
                                    {format!("{} container{}", containers.len(), if containers.len() != 1 { "s" } else { "" })}
                                </Badge>
                            </div>

                            {if containers.is_empty() {
                                view! {
                                    <div class="flex flex-col items-center justify-center py-12 text-center">
                                        <div class="rounded-full bg-muted p-3 mb-4">
                                            <lucide_leptos::Package />
                                        </div>
                                        <H4 class="mb-2">No Containers</H4>
                                        <P class="text-muted-foreground mb-4 max-w-sm">
                                            "This workload doesn't have any container configurations."
                                        </P>
                                    </div>
                                }.into_any()
                            } else {
                                view! {
                                    <div class="space-y-4">
                                        {
                                            containers.iter().enumerate().map(|(index, container)| {
                                                let workload_id = workload.id;
                                                let container_idx = index;
                                                view! {
                                                    <EnvironmentContainerEditor
                                                        workload_id
                                                        container_index=container_idx
                                                        container=container.clone()
                                                        all_containers=workload.containers.clone()
                                                        update_counter
                                                    />
                                                }
                                            }).collect::<Vec<_>>()
                                        }
                                    </div>
                                }.into_any()
                            }}
                        </div>
                    </Card>
                }
            }}
        </Show>
    }
}