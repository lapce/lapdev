use anyhow::{anyhow, Result};
use lapdev_api_hrpc::HrpcServiceClient;
use lapdev_common::{
    console::Organization,
    kube::{CreateKubeClusterResponse, KubeCluster, KubeClusterStatus},
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::rc::Rc;

use crate::{
    component::{
        badge::{Badge, BadgeVariant},
        button::{Button, ButtonVariant},
        dropdown_menu::{
            DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger,
            DropdownPlacement,
        },
        input::Input,
        label::Label,
        table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow},
        textarea::Textarea,
        typography::{H3, H4, P},
    },
    modal::{DeleteModal, ErrorResponse, Modal},
    organization::get_current_org,
};

#[component]
pub fn KubeCluster() -> impl IntoView {
    let update_counter = RwSignal::new_local(0);
    view! {
        <div class="flex flex-col gap-6">
            <div class="flex flex-col gap-2 items-start">
                <H3>Kubernetes Clusters</H3>
                <P>View and manage your Kubernetes clusters discovered from your configured providers.</P>
                <a href="https://docs.lap.dev/">
                    <Badge variant=BadgeVariant::Secondary>Docs <lucide_leptos::ExternalLink /></Badge>
                </a>
            </div>

            <KubeClusterList update_counter />
        </div>
    }
}

async fn all_kube_clusters(org: Signal<Option<Organization>>) -> Result<Vec<KubeCluster>> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());
    Ok(client.all_kube_clusters(org.id).await??)
}

async fn delete_kube_cluster(
    org: Signal<Option<Organization>>,
    cluster_id: uuid::Uuid,
    delete_modal_open: RwSignal<bool>,
    update_counter: RwSignal<usize, LocalStorage>,
) -> Result<(), ErrorResponse> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());

    client.delete_kube_cluster(org.id, cluster_id).await??;

    delete_modal_open.set(false);
    update_counter.update(|c| *c += 1);

    Ok(())
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
        <div>
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
                            <TableHead>Provider</TableHead>
                            <TableHead>Region</TableHead>
                            <TableHead>Nodes</TableHead>
                            <TableHead>Can Deploy Personal</TableHead>
                            <TableHead>Can Deploy Shared</TableHead>
                            <TableHead class="w-24">Actions</TableHead>
                        </TableRow>
                    </TableHeader>
                    <TableBody>
                        <For
                            each=move || clusters.get()
                            key=|c| format!("{}_{}_{}", c.id, c.can_deploy_personal, c.can_deploy_shared)
                            children=move |cluster| {
                                view! { <KubeClusterItem cluster update_counter /> }
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
pub fn KubeClusterItem(
    cluster: KubeCluster,
    update_counter: RwSignal<usize, LocalStorage>,
) -> impl IntoView {
    let status_variant = match cluster.info.status {
        KubeClusterStatus::Ready => BadgeVariant::Secondary,
        KubeClusterStatus::Provisioning => BadgeVariant::Secondary,
        KubeClusterStatus::NotReady | KubeClusterStatus::Error => BadgeVariant::Destructive,
    };

    let status_text = cluster.info.status.to_string();
    let dropdown_expanded = RwSignal::new(false);
    let delete_modal_open = RwSignal::new(false);
    let org = get_current_org();

    let cluster_id = cluster.id;
    let cluster_id_for_delete = cluster_id;
    let cluster_id_for_resources = cluster_id;
    let cluster_name = cluster.name.clone();
    let provider = cluster
        .info
        .provider
        .clone()
        .unwrap_or_else(|| "Unknown".to_string());
    let cluster_name_clone = cluster_name.clone();

    let delete_action = Action::new_local(move |_| {
        delete_kube_cluster(
            org,
            cluster_id_for_delete.clone(),
            delete_modal_open,
            update_counter,
        )
    });

    view! {
        <TableRow>
            <TableCell class="pl-4">
                <a href={ format!("/kubernetes/clusters/{}", cluster.id) }>
                    <Button variant=ButtonVariant::Link class="p-0">
                        <span class="font-medium">{cluster_name.clone()}</span>
                    </Button>
                </a>
            </TableCell>
            <TableCell>
                <Badge variant=status_variant>{status_text}</Badge>
            </TableCell>
            <TableCell>{cluster.info.cluster_version}</TableCell>
            <TableCell>{provider}</TableCell>
            <TableCell>{cluster.info.region.unwrap_or("N/A".to_string())}</TableCell>
            <TableCell>{cluster.info.node_count.to_string()}</TableCell>
            <TableCell>
                <Badge variant={
                    if cluster.can_deploy_personal {
                        BadgeVariant::Secondary
                    } else {
                        BadgeVariant::Destructive
                    }
                }>
                    {if cluster.can_deploy_personal { "Yes" } else { "No" }}
                </Badge>
            </TableCell>
            <TableCell>
                <Badge variant={
                    if cluster.can_deploy_shared {
                        BadgeVariant::Secondary
                    } else {
                        BadgeVariant::Destructive
                    }
                }>
                    {if cluster.can_deploy_shared { "Yes" } else { "No" }}
                </Badge>
            </TableCell>
            <TableCell>
                <DropdownMenu open=dropdown_expanded>
                    <div class="flex">
                        <DropdownMenuTrigger
                            open=dropdown_expanded
                            placement=DropdownPlacement::BottomRight
                        >
                            <Button variant=ButtonVariant::Ghost class="px-2">
                                <lucide_leptos::EllipsisVertical />
                            </Button>
                        </DropdownMenuTrigger>
                    </div>
                    <DropdownMenuContent
                        open=dropdown_expanded.read_only()
                        class="min-w-56 -translate-x-2"
                    >
                        <a href=format!("/kubernetes/clusters/{}", cluster_id_for_resources)>
                            <DropdownMenuItem class="cursor-pointer">
                                <lucide_leptos::Box />
                                View Resources
                            </DropdownMenuItem>
                        </a>
                        <DropdownMenuItem
                            on:click=move |_| {
                                if !cluster.can_deploy_personal {
                                    dropdown_expanded.set(false);
                                    // Enable personal deployment
                                    let org = org.get_untracked();
                                    let cluster_id = cluster.id;
                                    spawn_local(async move {
                                        if let Some(org) = org {
                                            let client = HrpcServiceClient::new("/api/rpc".to_string());
                                            let _ = client.set_cluster_personal_deployable(org.id, cluster_id, true).await;
                                            update_counter.update(|c| *c += 1);
                                        }
                                    });
                                }
                            }
                            class=if cluster.can_deploy_personal {
                                "cursor-not-allowed opacity-50"
                            } else {
                                "cursor-pointer"
                            }
                        >
                            <lucide_leptos::Check />
                            "Enable Personal Deployment"
                        </DropdownMenuItem>
                        <DropdownMenuItem
                            on:click=move |_| {
                                if cluster.can_deploy_personal {
                                    dropdown_expanded.set(false);
                                    // Disable personal deployment
                                    let org = org.get_untracked();
                                    let cluster_id = cluster.id;
                                    spawn_local(async move {
                                        if let Some(org) = org {
                                            let client = HrpcServiceClient::new("/api/rpc".to_string());
                                            let _ = client.set_cluster_personal_deployable(org.id, cluster_id, false).await;
                                            update_counter.update(|c| *c += 1);
                                        }
                                    });
                                }
                            }
                            class=if !cluster.can_deploy_personal {
                                "cursor-not-allowed opacity-50"
                            } else {
                                "cursor-pointer"
                            }
                        >
                            <lucide_leptos::X />
                            "Disable Personal Deployment"
                        </DropdownMenuItem>
                        <DropdownMenuItem
                            on:click=move |_| {
                                if !cluster.can_deploy_shared {
                                    dropdown_expanded.set(false);
                                    // Enable shared deployment
                                    let org = org.get_untracked();
                                    let cluster_id = cluster.id;
                                    spawn_local(async move {
                                        if let Some(org) = org {
                                            let client = HrpcServiceClient::new("/api/rpc".to_string());
                                            let _ = client.set_cluster_shared_deployable(org.id, cluster_id, true).await;
                                            update_counter.update(|c| *c += 1);
                                        }
                                    });
                                }
                            }
                            class=if cluster.can_deploy_shared {
                                "cursor-not-allowed opacity-50"
                            } else {
                                "cursor-pointer"
                            }
                        >
                            <lucide_leptos::Check />
                            "Enable Shared Deployment"
                        </DropdownMenuItem>
                        <DropdownMenuItem
                            on:click=move |_| {
                                if cluster.can_deploy_shared {
                                    dropdown_expanded.set(false);
                                    // Disable shared deployment
                                    let org = org.get_untracked();
                                    let cluster_id = cluster.id;
                                    spawn_local(async move {
                                        if let Some(org) = org {
                                            let client = HrpcServiceClient::new("/api/rpc".to_string());
                                            let _ = client.set_cluster_shared_deployable(org.id, cluster_id, false).await;
                                            update_counter.update(|c| *c += 1);
                                        }
                                    });
                                }
                            }
                            class=if !cluster.can_deploy_shared {
                                "cursor-not-allowed opacity-50"
                            } else {
                                "cursor-pointer"
                            }
                        >
                            <lucide_leptos::X />
                            "Disable Shared Deployment"
                        </DropdownMenuItem>
                        <DropdownMenuItem
                            on:click=move |_| {
                                dropdown_expanded.set(false);
                                delete_modal_open.set(true);
                            }
                            class="cursor-pointer text-destructive focus:text-destructive"
                        >
                            <lucide_leptos::Trash2 />
                            Delete
                        </DropdownMenuItem>
                    </DropdownMenuContent>
                </DropdownMenu>
                <DeleteModal
                    resource=cluster_name_clone
                    open=delete_modal_open
                    delete_action
                />
            </TableCell>
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
    let command_copy_success = RwSignal::new_local(false);
    let origin = StoredValue::new(
        window()
            .location()
            .origin()
            .unwrap_or_else(|_| "".to_string())
            .trim_end_matches('/')
            .to_string(),
    );

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

    let manifest_path = Memo::new(move |_| {
        cluster_response
            .get()
            .map(|response| format!("/install/lapdev-kube-manager.yaml?token={}", response.token))
    });

    let install_command = {
        Memo::new(move |_| {
            manifest_path
                .get()
                .map(|path| {
                    let origin = origin.get_value();
                    if origin.is_empty() {
                        format!("kubectl apply -f {}", path)
                    } else {
                        format!("kubectl apply -f {origin}{path}")
                    }
                })
                .unwrap_or_default()
        })
    };

    let copy_install_command = move |_| {
        let command = install_command.get();
        if command.is_empty() {
            return;
        }
        let navigator = window().navigator();
        let clipboard = navigator.clipboard();
        let _ = clipboard.write_text(&command);
        command_copy_success.set(true);
        set_timeout(
            move || command_copy_success.set(false),
            std::time::Duration::from_secs(2),
        );
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

                <div class="flex flex-col gap-2">
                    <Label>Install Command</Label>
                    <div class="relative">
                        <Textarea
                            prop:value=move || install_command.get()
                            prop:readonly=true
                            class="font-mono text-sm min-h-20 pr-16"
                        />
                        <Button
                            on:click=copy_install_command
                            class="absolute top-2 right-2 h-8 w-8 p-0"
                            variant=ButtonVariant::Outline
                        >
                            {move || if command_copy_success.get() {
                                view! { <lucide_leptos::Check /> }.into_any()
                            } else {
                                view! { <lucide_leptos::Copy /> }.into_any()
                            }}
                        </Button>
                    </div>
                    {move || if command_copy_success.get() {
                        view! { <P class="text-sm text-green-600">Install command copied!</P> }
                    } else {
                        view! { <P class="text-sm text-muted-foreground">Run this command in a shell with kubectl access to your cluster</P> }
                    }}
                </div>

                <div class="flex flex-col gap-2 p-4 bg-muted rounded-lg">
                    <P class="font-medium text-sm">Next Steps:</P>
                    <ul class="text-sm space-y-1">
                        <li>{"- Install lapdev-kube-manager in your cluster"}</li>
                        <li>{"- Configure it with this token"}</li>
                        <li>{"- Your cluster will appear in the list once connected"}</li>
                    </ul>
                    {move || manifest_path.get().map(|href| {
                        view! {
                            <a
                                class="text-sm text-primary underline"
                                target="_blank"
                                rel="noopener noreferrer"
                                href=href
                            >
                                "Open manifest in a new tab"
                            </a>
                        }
                        .into_any()
                    }).unwrap_or_else(|| ().into_any() )}
                </div>
            </div>
        </Modal>
    }
}
