use anyhow::{anyhow, Result};
use lapdev_common::{
    console::Organization,
    kube::{K8sProvider, K8sProviderKind},
};
use lapdev_api_hrpc::HrpcServiceClient;
use leptos::prelude::*;

use crate::{
    component::{
        badge::{Badge, BadgeVariant},
        button::Button,
        dropdown_menu::{DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger},
        icon,
        input::Input,
        label::Label,
        table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow},
        textarea::Textarea,
        typography::{H3, P},
    },
    modal::Modal,
    organization::get_current_org,
};

#[component]
pub fn K8sProvider() -> impl IntoView {
    let update_counter = RwSignal::new_local(0);
    view! {
        <div class="flex flex-col gap-2 items-start">
            <H3>Providers</H3>
            <P>Set up your kubernetes providers to discover and manage your kubernetes clusters.</P>
            <a href="https://docs.lap.dev/">
                <Badge variant=BadgeVariant::Secondary>Docs <lucide_leptos::ExternalLink /></Badge>
            </a>
        </div>

        <NewK8sProvider update_counter />
        <K8sProviderList update_counter />
    }
}

async fn all_k8s_providers(org: Signal<Option<Organization>>) -> Result<Vec<K8sProvider>> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());
    Ok(client.all_k8s_providers(org.id).await??)
}

#[component]
pub fn K8sProviderList(update_counter: RwSignal<usize, LocalStorage>) -> impl IntoView {
    let org = get_current_org();
    let providers =
        LocalResource::new(move || async move { all_k8s_providers(org).await.unwrap_or_default() });
    Effect::new(move |_| {
        update_counter.track();
        providers.refetch();
    });
    let providers = Signal::derive(move || providers.get().unwrap_or_default());

    view! {
        <div class="rounded-lg border mt-4">
            <Table class="table-auto">
                <TableHeader class="bg-muted">
                    <TableRow>
                        <TableHead class="pl-4">Name</TableHead>
                        <TableHead>Provider</TableHead>
                        <TableHead>Clusters</TableHead>
                        <TableHead>Status</TableHead>
                    </TableRow>
                </TableHeader>
                <TableBody>
                    <For
                        each=move || providers.get()
                        key=|p| p.id
                        children=move |provider| {
                            view! { <K8sProviderItem provider /> }
                        }
                    />
                </TableBody>
            </Table>
        </div>
    }
}

#[component]
pub fn K8sProviderItem(provider: K8sProvider) -> impl IntoView {
    view! {
        <TableRow>
            <TableCell class="pl-4">{provider.name}</TableCell>
            <TableCell>
                <p class="flex items-center gap-2">
                    {match provider.provider {
                        K8sProviderKind::GCP => view! { <icon::BrandGoogle /> },
                    }} <span>{provider.provider.to_string()}</span>
                </p>
            </TableCell>
            <TableCell>1</TableCell>
            <TableCell>
                <Badge variant=BadgeVariant::Outline>Active</Badge>
            </TableCell>
        </TableRow>
    }
}

#[component]
pub fn NewK8sProvider(update_counter: RwSignal<usize, LocalStorage>) -> impl IntoView {
    let menu_open = RwSignal::new(false);
    let new_gcp_provider_open = RwSignal::new(false);
    view! {
        <div class="mt-8">
            <DropdownMenu open=menu_open>
                <DropdownMenuTrigger open=menu_open>
                    <Button>Add Provider</Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent
                    open=menu_open.read_only()
                    class="left-0 top-10 min-w-56 rounded-lg"
                >
                    <DropdownMenuItem
                        class="cursor-pointer"
                        on:click=move |_| {
                            menu_open.set(false);
                            new_gcp_provider_open.set(true);
                        }
                    >
                        GCP
                    </DropdownMenuItem>
                </DropdownMenuContent>
            </DropdownMenu>
            <NewGcpProvider new_gcp_provider_open update_counter />
        </div>
    }
}

#[component]
pub fn NewGcpProvider(
    new_gcp_provider_open: RwSignal<bool>,
    update_counter: RwSignal<usize, LocalStorage>,
) -> impl IntoView {
    let new_gcp_provider_name = RwSignal::new_local("".to_string());
    let new_gcp_provider_key = RwSignal::new_local("".to_string());
    let org = get_current_org();
    let client = HrpcServiceClient::new("/api/rpc".to_string());
    let new_gcp_provider_action = Action::new_local(move |_| {
        let client = client.clone();
        async move {
            client
                .create_k8s_gcp_provider(
                    org.get_untracked()
                        .ok_or_else(|| anyhow!("can't get org"))?
                        .id,
                    new_gcp_provider_name.get_untracked(),
                    new_gcp_provider_key.get_untracked(),
                )
                .await??;
            update_counter.update(|c| {
                *c += 1;
            });
            new_gcp_provider_open.set(false);
            Ok(())
        }
    });
    view! {
        <Modal
            title="Create New GCP Provider"
            open=new_gcp_provider_open
            action=new_gcp_provider_action
        >
            <div class="flex flex-col gap-2">
                <Label>Name</Label>
                <Input
                    prop:value=move || new_gcp_provider_name.get()
                    on:input=move |ev| {
                        new_gcp_provider_name.set(event_target_value(&ev));
                    }
                    {..}
                    required=true
                />
            </div>
            <div class="flex flex-col gap-2">
                <Label>Service Account Key</Label>
                <Textarea
                    class="max-h-48"
                    prop:value=move || new_gcp_provider_key.get()
                    on:input=move |ev| {
                        new_gcp_provider_key.set(event_target_value(&ev));
                    }
                    {..}
                    required=true
                />
            </div>
        </Modal>
    }
}
