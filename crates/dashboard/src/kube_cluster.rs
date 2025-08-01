use anyhow::{anyhow, Result};
use lapdev_api_hrpc::HrpcServiceClient;
use lapdev_common::{
    console::Organization,
    kube::{CreateKubeClusterResponse, KubeClusterInfo, KubeClusterStatus},
};
use leptos::prelude::*;

use crate::{
    component::{
        badge::{Badge, BadgeVariant},
        button::{Button, ButtonVariant},
        input::Input,
        label::Label,
        table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow},
        textarea::Textarea,
        typography::{H3, H4, P},
    },
    modal::Modal,
    organization::get_current_org,
};

#[component]
pub fn KubeCluster() -> impl IntoView {
    let update_counter = RwSignal::new_local(0);
    view! {
        <div class="flex flex-col gap-2 items-start">
            <H3>Kubernetes Clusters</H3>
            <P>View and manage your Kubernetes clusters discovered from your configured providers.</P>
            <a href="https://docs.lap.dev/">
                <Badge variant=BadgeVariant::Secondary>Docs <lucide_leptos::ExternalLink /></Badge>
            </a>
        </div>

        <KubeClusterList update_counter />
    }
}

async fn all_kube_clusters(org: Signal<Option<Organization>>) -> Result<Vec<KubeClusterInfo>> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());
    Ok(client.all_kube_clusters(org.id).await??)
}

#[component]
pub fn KubeClusterList(update_counter: RwSignal<usize, LocalStorage>) -> impl IntoView {
    let org = get_current_org();
    let clusters =
        LocalResource::new(move || async move { all_kube_clusters(org).await.unwrap_or_default() });
    Effect::new(move |_| {
        update_counter.track();
        clusters.refetch();
    });
    let clusters = Signal::derive(move || clusters.get().unwrap_or_default());

    let create_cluster_modal_open = RwSignal::new(false);
    let token_modal_open = RwSignal::new(false);
    let cluster_response = RwSignal::new_local(None::<CreateKubeClusterResponse>);

    let create_cluster = move |_| {
        create_cluster_modal_open.set(true);
    };

    view! {
        <div class="mt-4">
            <div class="flex justify-between items-center mb-4">
                <Button on:click=create_cluster>
                    <lucide_leptos::Plus  />
                    Create New Cluster
                </Button>
                <CreateClusterModal
                    modal_open=create_cluster_modal_open
                    update_counter
                    token_modal_open
                    cluster_response
                />
                <TokenDisplayModal
                    modal_open=token_modal_open
                    cluster_response
                />
            </div>
            <div class="rounded-lg border">
                <Table class="table-auto">
                    <TableHeader class="bg-muted">
                        <TableRow>
                            <TableHead class="pl-4">Name</TableHead>
                            <TableHead>Status</TableHead>
                            <TableHead>Version</TableHead>
                            <TableHead>Region</TableHead>
                            <TableHead>Nodes</TableHead>
                        </TableRow>
                    </TableHeader>
                    <TableBody>
                        <For
                            each=move || clusters.get()
                            key=|c| c.cluster_id.clone().unwrap_or_default()
                            children=move |cluster| {
                                view! { <KubeClusterItem cluster /> }
                            }
                        />
                    </TableBody>
                </Table>

                {move || {
                    let cluster_list = clusters.get();
                    if cluster_list.is_empty() {
                        view! {
                            <div class="flex flex-col items-center justify-center py-12 text-center">
                                <div class="rounded-full bg-muted p-3 mb-4">
                                    <lucide_leptos::Server />
                                </div>
                                <H4 class="mb-2">No Kubernetes Clusters</H4>
                                <P class="text-muted-foreground mb-4 max-w-sm">
                                    Connect your first Kubernetes cluster to start deploying workspaces and managing resources.
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
pub fn KubeClusterItem(cluster: KubeClusterInfo) -> impl IntoView {
    let status_variant = match cluster.status {
        KubeClusterStatus::Ready => BadgeVariant::Secondary,
        KubeClusterStatus::Provisioning => BadgeVariant::Secondary,
        KubeClusterStatus::NotReady | KubeClusterStatus::Error => BadgeVariant::Destructive,
    };

    let status_text = cluster.status.to_string();

    view! {
        <TableRow>
            <TableCell class="pl-4">
                <a href={ format!("/kubernetes/clusters/{}", cluster.cluster_id.unwrap_or_default()) }>
                    <Button variant=ButtonVariant::Link>
                        <span class="font-medium">{cluster.cluster_name.clone().unwrap_or("Unknown".to_string())}</span>
                    </Button>
                </a>
            </TableCell>
            <TableCell>
                <Badge variant=status_variant>{status_text}</Badge>
            </TableCell>
            <TableCell>{cluster.cluster_version}</TableCell>
            <TableCell>{cluster.region.unwrap_or("N/A".to_string())}</TableCell>
            <TableCell>{cluster.node_count.to_string()}</TableCell>
        </TableRow>
    }
}

#[component]
pub fn CreateClusterModal(
    modal_open: RwSignal<bool>,
    update_counter: RwSignal<usize, LocalStorage>,
    token_modal_open: RwSignal<bool>,
    cluster_response: RwSignal<Option<CreateKubeClusterResponse>, LocalStorage>,
) -> impl IntoView {
    let cluster_name = RwSignal::new_local("".to_string());
    let org = get_current_org();
    let client = HrpcServiceClient::new("/api/rpc".to_string());

    let create_action = Action::new_local(move |_| {
        let client = client.clone();
        async move {
            let response = client
                .create_kube_cluster(
                    org.get_untracked()
                        .ok_or_else(|| anyhow!("can't get org"))?
                        .id,
                    cluster_name.get_untracked(),
                )
                .await??;

            cluster_response.set(Some(response));
            update_counter.update(|c| {
                *c += 1;
            });
            modal_open.set(false);
            token_modal_open.set(true);
            Ok(())
        }
    });

    view! {
        <Modal
            title="Create New Cluster"
            open=modal_open
            action=create_action
        >
            <div class="flex flex-col gap-2">
                <Label>Cluster Name</Label>
                <Input
                    prop:value=move || cluster_name.get()
                    on:input=move |ev| {
                        cluster_name.set(event_target_value(&ev));
                    }
                    attr:placeholder="Enter cluster name"
                    attr:required=true
                />
            </div>
        </Modal>
    }
}

#[component]
pub fn TokenDisplayModal(
    modal_open: RwSignal<bool>,
    cluster_response: RwSignal<Option<CreateKubeClusterResponse>, LocalStorage>,
) -> impl IntoView {
    let copy_success = RwSignal::new_local(false);

    let copy_token = move |_| {
        if let Some(response) = cluster_response.get_untracked() {
            let navigator = window().navigator();
            let clipboard = navigator.clipboard();
            let _ = clipboard.write_text(&response.token);
            copy_success.set(true);

            // Reset copy success after 2 seconds
            set_timeout(
                move || copy_success.set(false),
                std::time::Duration::from_secs(2),
            );
        }
    };

    view! {
        <Modal
            title="Cluster Token"
            open=modal_open
            action=Action::new_local(move |_| async move { Ok(()) })
            hide_action=true
        >
            <div class="flex flex-col gap-4">
                <div class="flex flex-col gap-2">
                    <P class="text-sm text-muted-foreground">
                        Use this token to connect your cluster to Lapdev.
                        Copy and save it securely - it will not be shown again.
                    </P>
                </div>

                <div class="flex flex-col gap-2">
                    <Label>Cluster ID</Label>
                    <Input
                        prop:value=move || cluster_response.get().map(|r| r.cluster_id.to_string()).unwrap_or_default()
                        prop:readonly=true
                        class="font-mono text-sm"
                    />
                </div>

                <div class="flex flex-col gap-2">
                    <Label>Authentication Token</Label>
                    <div class="relative">
                        <Textarea
                            prop:value=move || cluster_response.get().map(|r| r.token).unwrap_or_default()
                            prop:readonly=true
                            class="font-mono text-sm min-h-24 pr-16"
                        />
                        <Button
                            on:click=copy_token
                            class="absolute top-2 right-2 h-8 w-8 p-0"
                            variant=ButtonVariant::Outline
                        >
                            {move || if copy_success.get() {
                                view! { <lucide_leptos::Check /> }.into_any()
                            } else {
                                view! { <lucide_leptos::Copy /> }.into_any()
                            }}
                        </Button>
                    </div>
                    {move || if copy_success.get() {
                        view! { <P class="text-sm text-green-600">Token copied to clipboard!</P> }
                    } else {
                        view! { <P class="text-sm text-muted-foreground">Click the copy button to copy the token</P> }
                    }}
                </div>

                <div class="flex flex-col gap-2 p-4 bg-muted rounded-lg">
                    <P class="font-medium text-sm">Next Steps:</P>
                    <ul class="text-sm space-y-1">
                        <li>{"- Install lapdev-kube-manager in your cluster"}</li>
                        <li>{"- Configure it with this token"}</li>
                        <li>{"- Your cluster will appear in the list once connected"}</li>
                    </ul>
                </div>
            </div>
        </Modal>
    }
}
