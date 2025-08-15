use anyhow::anyhow;
use lapdev_api_hrpc::HrpcServiceClient;
use lapdev_common::{
    console::Organization,
    kube::{
        CreateKubeEnvironmentPreviewUrlRequest, KubeEnvironmentPreviewUrl, KubeEnvironmentService,
        PreviewUrlAccessLevel, UpdateKubeEnvironmentPreviewUrlRequest,
    },
};
use leptos::prelude::*;
use uuid::Uuid;

use crate::{
    component::{
        badge::{Badge, BadgeVariant},
        button::{Button, ButtonSize, ButtonVariant},
        card::{Card, CardContent},
        input::Input,
        label::Label,
        select::{Select, SelectContent, SelectItem, SelectTrigger},
        table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow},
        typography::{H4, P},
    },
    modal::{DeleteModal, ErrorResponse, Modal},
    organization::get_current_org,
};

#[component]
pub fn PreviewUrlsContent(
    environment_id: Uuid,
    services: Signal<Vec<KubeEnvironmentService>>,
    preview_urls: Signal<Vec<KubeEnvironmentPreviewUrl>>,
    update_counter: RwSignal<usize>,
) -> impl IntoView {
    let create_modal_open = RwSignal::new(false);
    let edit_modal_open = RwSignal::new(false);
    let edit_preview_url = RwSignal::new(None::<KubeEnvironmentPreviewUrl>);

    // Use the passed preview URLs signal

    view! {
        <div class="flex flex-col gap-4">
            // Header with create button
            <div class="flex justify-between items-center">
                <div>
                    <h3 class="text-lg font-semibold">"Preview URLs"</h3>
                    <p class="text-sm text-muted-foreground">
                        "Create preview URLs to access your services"
                    </p>
                </div>
            </div>

            // Preview URLs table
            <Card>
                <CardContent class="p-0">
                    <Suspense fallback=move || view! { <div class="p-4">"Loading preview URLs..."</div> }>
                        <Table>
                            <TableHeader class="bg-muted">
                                <TableRow>
                                    <TableHead class="px-4">"URL"</TableHead>
                                    <TableHead>"Service"</TableHead>
                                    <TableHead>"Port"</TableHead>
                                    <TableHead>"Access"</TableHead>
                                    <TableHead>"Actions"</TableHead>
                                </TableRow>
                            </TableHeader>
                            <TableBody>
                                {move || {
                                    let urls = preview_urls.get();
                                    if urls.is_empty() {
                                        view! {
                                            <TableRow>
                                                <TableCell attr:colspan="5" class="py-12 text-center">
                                                    <div class="flex flex-col items-center justify-center">
                                                        <div class="rounded-full bg-muted p-3 mb-4">
                                                            <lucide_leptos::Globe />
                                                        </div>
                                                        <H4 class="mb-2">"No Preview URLs Found"</H4>
                                                        <P class="text-muted-foreground mb-4 max-w-sm">
                                                            "Create a preview URL from services to access them externally"
                                                        </P>
                                                    </div>
                                                </TableCell>
                                            </TableRow>
                                        }.into_any()
                                    } else {
                                        view! {
                                            <For
                                                each=move || urls.clone()
                                                key=|url| url.id
                                                children=move |preview_url| {
                                                    view! {
                                                        <PreviewUrlRow
                                                            preview_url=preview_url.clone()
                                                            services=services
                                                            on_edit=Box::new(move |url| {
                                                                edit_preview_url.set(Some(url));
                                                                edit_modal_open.set(true);
                                                            })
                                                            update_counter=update_counter
                                                        />
                                                    }
                                                }
                                            />
                                        }.into_any()
                                    }
                                }}
                            </TableBody>
                        </Table>
                    </Suspense>
                </CardContent>
            </Card>

            // Create modal - only show if we have at least one service
            {move || {
                let services_list = services.get();
                if !services_list.is_empty() {
                    view! {
                        <CreatePreviewUrlModal
                            open=create_modal_open
                            environment_id=environment_id
                            service=services_list[0].clone()
                            update_counter=update_counter
                        />
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            }}

            // Edit modal
            <EditPreviewUrlModal
                open=edit_modal_open
                preview_url=edit_preview_url.into()
                update_counter=update_counter
            />
        </div>
    }
}

#[component]
fn PreviewUrlRow(
    preview_url: KubeEnvironmentPreviewUrl,
    services: Signal<Vec<KubeEnvironmentService>>,
    on_edit: Box<dyn Fn(KubeEnvironmentPreviewUrl) + Send + 'static>,
    update_counter: RwSignal<usize>,
) -> impl IntoView {
    let org = get_current_org();
    let delete_modal_open = RwSignal::new(false);

    // Find the service name
    let service_name = Signal::derive(move || {
        services
            .get()
            .iter()
            .find(|s| s.id == preview_url.service_id)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| "Unknown".to_string())
    });

    // Delete action
    let delete_action = Action::new_local({
        let preview_url = preview_url.clone();
        move |_| {
            let preview_url = preview_url.clone();
            async move {
                delete_environment_preview_url(org, preview_url.id)
                    .await
                    .map_err(ErrorResponse::from)?;
                update_counter.update(|c| *c += 1);
                Ok::<(), ErrorResponse>(())
            }
        }
    });

    let preview_url_for_view = preview_url.clone();
    let preview_url_for_href = preview_url.clone();
    let preview_url_name = preview_url_for_view.name.clone();
    view! {
        <TableRow>
            <TableCell>
                <a
                    href=preview_url_for_href.url.clone()
                    target="_blank"
                    rel="noopener noreferrer"
                    // class="text-blue-600 hover:text-blue-800 underline font-mono text-sm"
                >
                    <Button variant=ButtonVariant::Link>
                    {preview_url_for_view.url.clone()}
                    <lucide_leptos::ExternalLink />
                    </Button>
                </a>
            </TableCell>
            <TableCell>{service_name}</TableCell>
            <TableCell>
                {preview_url_for_view.port.to_string()}
                {preview_url_for_view.port_name.clone().map(|name| view! {
                    <span class="text-muted-foreground ml-1">{format!("({})", name)}</span>
                })}
            </TableCell>
            <TableCell>
                <div class="flex gap-1">
                    {
                        let variant = match preview_url_for_view.access_level {
                            lapdev_common::kube::PreviewUrlAccessLevel::Public => BadgeVariant::Default,
                            lapdev_common::kube::PreviewUrlAccessLevel::Shared => BadgeVariant::Secondary,
                            lapdev_common::kube::PreviewUrlAccessLevel::Personal => BadgeVariant::Outline,
                        };
                        view! {
                            <Badge variant=variant class="text-xs">
                                {preview_url_for_view.access_level.to_string()}
                            </Badge>
                        }
                    }
                </div>
            </TableCell>
            <TableCell>
                <div class="flex gap-2">
                    <Button
                        variant=ButtonVariant::Ghost
                        size=ButtonSize::Sm
                        on:click={
                            let preview_url = preview_url.clone();
                            move |_| on_edit(preview_url.clone())
                        }
                    >
                        <lucide_leptos::Pen />
                    </Button>
                    <Button
                        variant=ButtonVariant::Ghost
                        size=ButtonSize::Sm
                        on:click=move |_| { delete_modal_open.set(true); }
                        class="text-destructive hover:text-destructive"
                    >
                        <lucide_leptos::Trash  />
                    </Button>
                </div>
            </TableCell>
        </TableRow>

        <DeleteModal
            resource=preview_url_name
            open=delete_modal_open
            delete_action
        />
    }
}

#[component]
pub fn CreatePreviewUrlModal(
    open: RwSignal<bool>,
    environment_id: Uuid,
    service: KubeEnvironmentService,
    update_counter: RwSignal<usize>,
) -> impl IntoView {
    let org = get_current_org();

    // Form state
    let description = RwSignal::new(String::new());
    let selected_port = RwSignal::new(None::<i32>);
    let selected_port_name = RwSignal::new(None::<String>);
    let access_level = RwSignal::new(PreviewUrlAccessLevel::Personal);
    let select_open = RwSignal::new(false);

    // Available ports for the service
    let available_ports = Signal::derive(move || service.ports.clone());

    // Set port name when port is selected
    Effect::new(move |_| {
        if let Some(port) = selected_port.get() {
            let ports = available_ports.get();
            if let Some(port_info) = ports.iter().find(|p| p.port == port) {
                selected_port_name.set(port_info.name.clone());
            }
        }
    });

    // Reset form when modal opens/closes
    Effect::new(move |_| {
        if !open.get() {
            description.set(String::new());
            selected_port.set(None);
            selected_port_name.set(None);
            access_level.set(PreviewUrlAccessLevel::Personal);
        }
    });

    let create_action = Action::new_local(move |_| async move {
        let port = selected_port
            .get()
            .ok_or_else(|| anyhow!("Please select a port"))?;

        let request = CreateKubeEnvironmentPreviewUrlRequest {
            description: if description.get().trim().is_empty() {
                None
            } else {
                Some(description.get())
            },
            service_id: service.id,
            port,
            port_name: selected_port_name.get(),
            protocol: Some("HTTP".to_string()),
            access_level: Some(access_level.get_untracked()),
        };

        create_environment_preview_url(org, environment_id, request).await?;
        open.set(false);
        update_counter.update(|c| *c += 1);
        Ok(())
    });

    view! {
        <Modal
            open=open
            action=create_action
            title="Create Preview URL"
            action_text="Create"
            action_progress_text="Creating..."
        >
            <div class="flex flex-col gap-4">
                <div class="flex flex-col gap-2">
                    <Label>"Description"</Label>
                    <Input
                        prop:value=move || description.get()
                        on:input=move |ev| description.set(event_target_value(&ev))
                        attr:placeholder="Optional description"
                    />
                </div>

                <div class="flex flex-col gap-2">
                    <Label>"Service"</Label>
                    <div class="px-3 py-2 border border-input bg-muted rounded-md text-sm">
                        {service.name.clone()}
                    </div>
                    <p class="text-sm text-muted-foreground">
                        "Preview URL will be created for this service"
                    </p>
                </div>

                <div class="flex flex-col gap-2">
                    <Label>"Port" <span class="text-destructive">"*"</span></Label>
                    <Select
                        value=selected_port
                        open=RwSignal::new(false)
                    >
                        <SelectTrigger class="w-full">
                            {
                                move || {
                                    selected_port.get().map(|p| p.to_string()).unwrap_or_else(|| "Select Port".to_string())
                                }
                            }
                        </SelectTrigger>
                        <SelectContent class="w-full">
                            <For
                                each=move || available_ports.get()
                                key=|port| port.port
                                children=move |port| {
                                    view! {
                                        <SelectItem value=Some(port.port)>
                                            {port.port.to_string()}
                                            {port.name.as_ref().map(|name| view! {
                                                <span class="text-muted-foreground ml-1">
                                                    {format!("({})", name)}
                                                </span>
                                            })}
                                        </SelectItem>
                                    }
                                }
                            />
                        </SelectContent>
                    </Select>
                </div>

                <div class="flex flex-col gap-3">
                    <Label>"Security & Access"</Label>

                    <div class="flex flex-col gap-2">
                        <Label>"Access Level"</Label>
                        <Select
                            value=access_level
                            open=select_open
                        >
                            <SelectTrigger class="w-full">
                                {move || access_level.get().get_detailed_message() }
                            </SelectTrigger>
                            <SelectContent class="w-full">
                                <SelectItem value=PreviewUrlAccessLevel::Personal>{PreviewUrlAccessLevel::Personal.get_detailed_message()}</SelectItem>
                                <SelectItem value=PreviewUrlAccessLevel::Shared>{PreviewUrlAccessLevel::Shared.get_detailed_message()}</SelectItem>
                                <SelectItem value=PreviewUrlAccessLevel::Public>{PreviewUrlAccessLevel::Public.get_detailed_message()}</SelectItem>
                            </SelectContent>
                        </Select>
                    </div>
                </div>
            </div>
        </Modal>
    }
}

#[component]
fn EditPreviewUrlModal(
    open: RwSignal<bool>,
    preview_url: Signal<Option<KubeEnvironmentPreviewUrl>>,
    update_counter: RwSignal<usize>,
) -> impl IntoView {
    let org = get_current_org();

    // Form state
    let description = RwSignal::new(String::new());
    let access_level = RwSignal::new(PreviewUrlAccessLevel::Personal);
    let edit_select_open = RwSignal::new(false);

    // Initialize form when preview URL changes
    Effect::new(move |_| {
        if let Some(url) = preview_url.get() {
            description.set(url.description.unwrap_or_default());
            access_level.set(url.access_level);
        }
    });

    let update_action = Action::new_local(move |_| async move {
        let url = preview_url
            .get()
            .ok_or_else(|| anyhow!("No preview URL to update"))?;

        let request = UpdateKubeEnvironmentPreviewUrlRequest {
            description: if description.get().trim().is_empty() {
                None
            } else {
                Some(description.get())
            },
            access_level: Some(access_level.get_untracked()),
        };

        update_environment_preview_url(org, url.id, request).await?;
        open.set(false);
        update_counter.update(|c| *c += 1);
        Ok(())
    });

    view! {
        <Modal
            open=open
            action=update_action
            title="Edit Preview URL"
            action_text="Save Changes"
            action_progress_text="Saving..."
        >
            <div class="flex flex-col gap-4">
                <div class="flex flex-col gap-2">
                    <Label>"Name"</Label>
                    <div class="px-3 py-2 border border-input bg-muted rounded-md text-sm font-mono">
                        {move || preview_url.get().map(|url| url.name).unwrap_or_default()}
                    </div>
                    <p class="text-sm text-muted-foreground">
                        "Name is auto-generated and cannot be changed"
                    </p>
                </div>

                <div class="flex flex-col gap-2">
                    <Label>"Description"</Label>
                    <Input
                        prop:value=move || description.get()
                        on:input=move |ev| description.set(event_target_value(&ev))
                        attr:placeholder="Optional description"
                    />
                </div>

                <div class="flex flex-col gap-3">
                    <Label>"Settings"</Label>

                    <div class="flex flex-col gap-2">
                        <Label>"Access Level"</Label>
                        <Select
                            value=access_level
                            open=edit_select_open
                        >
                            <SelectTrigger class="w-full">
                                {move || access_level.get().get_detailed_message() }
                            </SelectTrigger>
                            <SelectContent class="w-full">
                                <SelectItem value=PreviewUrlAccessLevel::Personal>{PreviewUrlAccessLevel::Personal.get_detailed_message()}</SelectItem>
                                <SelectItem value=PreviewUrlAccessLevel::Shared>{PreviewUrlAccessLevel::Shared.get_detailed_message()}</SelectItem>
                                <SelectItem value=PreviewUrlAccessLevel::Public>{PreviewUrlAccessLevel::Public.get_detailed_message()}</SelectItem>
                            </SelectContent>
                        </Select>
                    </div>
                </div>
            </div>
        </Modal>
    }
}

// API functions
async fn get_environment_preview_urls(
    org: Signal<Option<Organization>>,
    environment_id: Uuid,
) -> anyhow::Result<Vec<KubeEnvironmentPreviewUrl>> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());
    Ok(client
        .get_environment_preview_urls(org.id, environment_id)
        .await??)
}

async fn create_environment_preview_url(
    org: Signal<Option<Organization>>,
    environment_id: Uuid,
    request: CreateKubeEnvironmentPreviewUrlRequest,
) -> anyhow::Result<KubeEnvironmentPreviewUrl> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());
    Ok(client
        .create_environment_preview_url(org.id, environment_id, request)
        .await??)
}

async fn update_environment_preview_url(
    org: Signal<Option<Organization>>,
    preview_url_id: Uuid,
    request: UpdateKubeEnvironmentPreviewUrlRequest,
) -> anyhow::Result<KubeEnvironmentPreviewUrl> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());
    Ok(client
        .update_environment_preview_url(org.id, preview_url_id, request)
        .await??)
}

async fn delete_environment_preview_url(
    org: Signal<Option<Organization>>,
    preview_url_id: Uuid,
) -> anyhow::Result<()> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let client = HrpcServiceClient::new("/api/rpc".to_string());
    Ok(client
        .delete_environment_preview_url(org.id, preview_url_id)
        .await??)
}
