use anyhow::{anyhow, Result};
use chrono::DateTime;
use lapdev_api_hrpc::HrpcServiceClient;
use lapdev_common::kube::KubeEnvironment;
use lapdev_common::console::Organization;
use leptos::prelude::*;

use crate::{
    component::{
        badge::{Badge, BadgeVariant},
        button::{Button, ButtonVariant},
        table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow},
        typography::{H3, H4, P},
    },
    modal::DatetimeModal,
    organization::get_current_org,
};

#[component]
pub fn KubeEnvironment() -> impl IntoView {
    let update_counter = RwSignal::new_local(0);
    
    view! {
        <div class="flex flex-col gap-6">
            <div class="flex flex-col gap-2 items-start">
                <H3>Kubernetes Environments</H3>
                <P>View and manage your Kubernetes development environments created from app catalogs.</P>
                <a href="https://docs.lap.dev/">
                    <Badge variant=BadgeVariant::Secondary>Docs <lucide_leptos::ExternalLink /></Badge>
                </a>
            </div>

            <KubeEnvironmentList update_counter />
        </div>
    }
}

async fn all_kube_environments(org: Signal<Option<Organization>>) -> Result<Vec<KubeEnvironment>> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());
    Ok(client.all_kube_environments(org.id).await??)
}

#[component]
pub fn KubeEnvironmentList(update_counter: RwSignal<usize, LocalStorage>) -> impl IntoView {
    let org = get_current_org();
    let environments = LocalResource::new(move || async move { 
        all_kube_environments(org).await.unwrap_or_default() 
    });
    
    Effect::new(move |_| {
        update_counter.track();
        environments.refetch();
    });

    let environment_list = Signal::derive(move || environments.get().unwrap_or_default());

    view! {
        <div class="flex flex-col gap-4">
            <div class="rounded-lg border">
                <Table>
                    <TableHeader class="bg-muted">
                        <TableRow>
                            <TableHead>Name</TableHead>
                            <TableHead>Namespace</TableHead>
                            <TableHead>App Catalog</TableHead>
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
                                view! {
                                    <KubeEnvironmentItem environment=environment.clone() />
                                }
                            }
                        />
                    </TableBody>
                </Table>

                {move || {
                    let environments = environment_list.get();
                    if environments.is_empty() {
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
                        }.into_any()
                    } else {
                        view! { <div></div> }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

#[component]
pub fn KubeEnvironmentItem(environment: KubeEnvironment) -> impl IntoView {
    let env_name = environment.name.clone();
    let env_name_for_delete = environment.name.clone();
    
    let view_details = move |_| {
        // TODO: Navigate to environment details or show modal
        leptos::logging::log!("View details for environment: {}", env_name);
    };

    let delete_environment = move |_| {
        // TODO: Implement delete functionality 
        leptos::logging::log!("Delete environment: {}", env_name_for_delete);
    };

    let status_variant = match environment.status.as_deref() {
        Some("Running") => BadgeVariant::Secondary,
        Some("Pending") => BadgeVariant::Outline,
        Some("Failed") | Some("Error") => BadgeVariant::Destructive,
        _ => BadgeVariant::Outline,
    };

    view! {
        <TableRow>
            <TableCell>
                <span class="font-medium">{environment.name.clone()}</span>
            </TableCell>
            <TableCell>
                <Badge variant=BadgeVariant::Outline>{environment.namespace.clone()}</Badge>
            </TableCell>
            <TableCell>
                <span class="text-sm">{environment.app_catalog_name.clone()}</span>
            </TableCell>
            <TableCell>
                <Badge variant=status_variant>
                    {environment.status.clone().unwrap_or_else(|| "Unknown".to_string())}
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
                <div class="flex items-center gap-2">
                    <Button 
                        variant=ButtonVariant::Outline 
                        size=crate::component::button::ButtonSize::Sm
                        on:click=view_details
                    >
                        <lucide_leptos::Eye />
                        Details
                    </Button>
                    <Button 
                        variant=ButtonVariant::Destructive 
                        size=crate::component::button::ButtonSize::Sm
                        on:click=delete_environment
                    >
                        <lucide_leptos::Trash2 />
                        Delete
                    </Button>
                </div>
            </TableCell>
        </TableRow>
    }
}