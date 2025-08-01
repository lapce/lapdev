use anyhow::{anyhow, Result};
use chrono::DateTime;
use lapdev_api_hrpc::HrpcServiceClient;
use lapdev_common::console::Organization;
use lapdev_common::kube::KubeAppCatalog;
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
pub fn KubeAppCatalog() -> impl IntoView {
    let update_counter = RwSignal::new_local(0);

    view! {
        <div class="flex flex-col gap-6">
            <div class="flex flex-col gap-2 items-start">
                <H3>Kubernetes App Catalog</H3>
                <P>{"View and manage your Kubernetes application catalogs. Create collections of workloads that can be deployed as dev environments."}</P>
                <a href="https://docs.lap.dev/">
                    <Badge variant=BadgeVariant::Secondary>Docs <lucide_leptos::ExternalLink /></Badge>
                </a>
            </div>

            <AppCatalogList update_counter />
        </div>
    }
}

async fn all_app_catalogs(org: Signal<Option<Organization>>) -> Result<Vec<KubeAppCatalog>> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());
    Ok(client.all_app_catalogs(org.id).await??)
}

#[component]
pub fn AppCatalogList(update_counter: RwSignal<usize, LocalStorage>) -> impl IntoView {
    let org = get_current_org();
    let catalogs =
        LocalResource::new(move || async move { all_app_catalogs(org).await.unwrap_or_default() });

    Effect::new(move |_| {
        update_counter.track();
        catalogs.refetch();
    });

    let catalog_list = Signal::derive(move || catalogs.get().unwrap_or_default());

    view! {
        <div class="flex flex-col gap-4">
            <div class="rounded-lg border">
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
                                view! {
                                    <AppCatalogItem catalog=catalog.clone() />
                                }
                            }
                        />
                    </TableBody>
                </Table>

                {move || {
                    let catalogs = catalog_list.get();
                    if catalogs.is_empty() {
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
pub fn AppCatalogItem(catalog: KubeAppCatalog) -> impl IntoView {
    let catalog_name = catalog.name.clone();
    let catalog_name_for_delete = catalog.name.clone();

    let view_details = move |_| {
        // TODO: Navigate to catalog details or show modal
        leptos::logging::log!("View details for catalog: {}", catalog_name);
    };

    let delete_catalog = move |_| {
        // TODO: Implement delete functionality
        leptos::logging::log!("Delete catalog: {}", catalog_name_for_delete);
    };

    view! {
        <TableRow>
            <TableCell>
                <span class="font-medium">{catalog.name.clone()}</span>
            </TableCell>
            <TableCell>
                <ExpandableDescription description={catalog.description.clone()} />
            </TableCell>
            <TableCell>
                <a href={format!("/kubernetes/clusters/{}", catalog.cluster_id)}>
                    <Button variant=ButtonVariant::Link>
                        <span class="font-medium">{catalog.cluster_name.clone()}</span>
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
                        on:click=view_details
                    >
                        <lucide_leptos::Eye />
                        Details
                    </Button>
                    <Button
                        variant=ButtonVariant::Destructive
                        size=crate::component::button::ButtonSize::Sm
                        on:click=delete_catalog
                    >
                        <lucide_leptos::Trash2 />
                        Delete
                    </Button>
                </div>
            </TableCell>
        </TableRow>
    }
}

#[component]
pub fn ExpandableDescription(description: Option<String>) -> impl IntoView {
    let description_text = description.unwrap_or_else(|| "No description".to_string());

    view! {
        <div
            class="max-w-xs text-muted-foreground text-sm cursor-help text-ellipsis overflow-hidden whitespace-nowrap"
            title={description_text.clone()}
        >
            {description_text.clone()}
        </div>
    }
}
