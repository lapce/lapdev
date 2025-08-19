use lapdev_common::kube::{KubeContainerImage, KubeContainerInfo, KubeEnvVar};
use leptos::prelude::*;
use uuid::Uuid;

use crate::{
    component::{
        badge::{Badge, BadgeVariant},
        button::{Button, ButtonVariant},
        card::Card,
        input::Input,
        typography::{H4, P},
    },
    modal::ErrorResponse,
};

#[component]
pub fn ContainersCard<F>(
    containers: Signal<Vec<KubeContainerInfo>>,
    title: &'static str,
    empty_message: &'static str,
    children: F,
) -> impl IntoView
where
    F: Fn(usize, KubeContainerInfo) -> AnyView + 'static + Send,
{
    view! {
        <Card class="p-6">
            <div class="flex flex-col gap-6">
                <div class="flex items-center justify-between">
                    <H4>{title}</H4>
                    <Badge variant=BadgeVariant::Secondary>
                        {move || {
                            let len = containers.get().len();
                            format!("{} container{}", len, if len != 1 { "s" } else { "" })
                        }}
                    </Badge>
                </div>

                {move || {
                    let containers_vec = containers.get();
                    if containers_vec.is_empty() {
                        view! {
                            <div class="flex flex-col items-center justify-center py-12 text-center">
                                <div class="rounded-full bg-muted p-3 mb-4">
                                    <lucide_leptos::Package />
                                </div>
                                <H4 class="mb-2">No Containers</H4>
                                <P class="text-muted-foreground mb-4 max-w-sm">
                                    {empty_message}
                                </P>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="space-y-4">
                                {
                                    containers_vec.into_iter().enumerate().map(|(index, container)| {
                                        children(index, container)
                                    }).collect::<Vec<_>>()
                                }
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </Card>
    }
}

#[derive(Clone)]
pub struct ContainerEditorConfig {
    pub enable_resource_limits: bool,
    pub show_customization_badge: bool,
}

impl Default for ContainerEditorConfig {
    fn default() -> Self {
        Self {
            enable_resource_limits: true,
            show_customization_badge: true,
        }
    }
}

#[component]
pub fn ContainerEditor(
    workload_id: Uuid,
    container_index: usize,
    container: KubeContainerInfo,
    all_containers: Vec<KubeContainerInfo>,
    update_counter: RwSignal<usize>,
    config: ContainerEditorConfig,
    update_action: Action<Vec<KubeContainerInfo>, Result<(), ErrorResponse>>,
) -> impl IntoView {
    let is_editing = RwSignal::new(false);
    let error_message = RwSignal::new(None::<String>);

    // Clone container fields to avoid move conflicts
    let container_image = container.image.clone();
    let container_env_vars = container.env_vars.clone();
    let container_original_image = container.original_image.clone();
    let name = container.name.clone();

    // Clone resource values for display (since they need to be used in multiple places)
    let cpu_request_display = container.cpu_request.clone();
    let cpu_limit_display = container.cpu_limit.clone();
    let memory_request_display = container.memory_request.clone();
    let memory_limit_display = container.memory_limit.clone();

    // Create signals for editable fields
    let image_signal = RwSignal::new(container_image.clone());
    let custom_image_signal = RwSignal::new(match &container_image {
        KubeContainerImage::Custom(img) => img.clone(),
        KubeContainerImage::FollowOriginal => String::new(),
    });
    let cpu_request_signal = RwSignal::new(container.cpu_request.clone().unwrap_or_default());
    let cpu_limit_signal = RwSignal::new(container.cpu_limit.clone().unwrap_or_default());
    let memory_request_signal = RwSignal::new(container.memory_request.clone().unwrap_or_default());
    let memory_limit_signal = RwSignal::new(container.memory_limit.clone().unwrap_or_default());
    let env_vars_signal = RwSignal::new(container_env_vars.clone());

    // Monitor the update action for error handling
    Effect::new(move || {
        if let Some(result) = update_action.value().get() {
            match result {
                Ok(_) => {
                    error_message.set(None);
                    is_editing.set(false);
                }
                Err(err) => {
                    error_message.set(Some(err.error.clone()));
                }
            }
        }
    });

    let save_changes = {
        let container_name = container.name.clone();
        let all_containers_clone = all_containers.clone();
        let original_image = container.original_image.clone();
        let enable_resources = config.enable_resource_limits;

        Callback::new(move |_| {
            let updated_container = KubeContainerInfo {
                name: container_name.clone(),
                original_image: original_image.clone(),
                image: image_signal.get(),
                cpu_request: if enable_resources && !cpu_request_signal.get().trim().is_empty() {
                    Some(cpu_request_signal.get().trim().to_string())
                } else {
                    None
                },
                cpu_limit: if enable_resources && !cpu_limit_signal.get().trim().is_empty() {
                    Some(cpu_limit_signal.get().trim().to_string())
                } else {
                    None
                },
                memory_request: if enable_resources
                    && !memory_request_signal.get().trim().is_empty()
                {
                    Some(memory_request_signal.get().trim().to_string())
                } else {
                    None
                },
                memory_limit: if enable_resources && !memory_limit_signal.get().trim().is_empty() {
                    Some(memory_limit_signal.get().trim().to_string())
                } else {
                    None
                },
                env_vars: env_vars_signal
                    .get()
                    .into_iter()
                    .filter(|var| !var.name.trim().is_empty())
                    .collect(),
            };

            let mut updated_containers = all_containers_clone.clone();
            updated_containers[container_index] = updated_container;
            update_action.dispatch(updated_containers);
        })
    };

    let cancel_changes = {
        let container_image_reset = container.image.clone();
        let container_env_vars_reset = container.env_vars.clone();
        let cpu_request_reset = container.cpu_request.clone().unwrap_or_default();
        let cpu_limit_reset = container.cpu_limit.clone().unwrap_or_default();
        let memory_request_reset = container.memory_request.clone().unwrap_or_default();
        let memory_limit_reset = container.memory_limit.clone().unwrap_or_default();
        let custom_img_reset = match &container.image {
            KubeContainerImage::Custom(img) => img.clone(),
            KubeContainerImage::FollowOriginal => String::new(),
        };
        Callback::new(move |_| {
            image_signal.set(container_image_reset.clone());
            custom_image_signal.set(custom_img_reset.clone());
            cpu_request_signal.set(cpu_request_reset.clone());
            cpu_limit_signal.set(cpu_limit_reset.clone());
            memory_request_signal.set(memory_request_reset.clone());
            memory_limit_signal.set(memory_limit_reset.clone());
            env_vars_signal.set(container_env_vars_reset.clone());
            error_message.set(None);
            is_editing.set(false);
        })
    };

    view! {
        <div class="border rounded-lg">
            <div class="p-4 border-b bg-muted/20">
                <div class="flex items-center justify-between">
                    <div class="flex items-center gap-3">
                        <div class="rounded-full bg-primary/10 p-2">
                            <lucide_leptos::Box attr:class="h-4 w-4 text-primary" />
                        </div>
                        <div>
                            <H4 class="text-base">{name.clone()}</H4>
                            {if config.show_customization_badge && container.is_customized() {
                                view! {
                                    <Badge variant=BadgeVariant::Secondary class="text-xs mt-1">
                                        <lucide_leptos::Settings attr:class="w-3 h-3 mr-1" />
                                        "Customized"
                                    </Badge>
                                }.into_any()
                            } else {
                                ().into_any()
                            }}
                        </div>
                    </div>
                    <Show when=move || !is_editing.get()>
                        <Button
                            variant=ButtonVariant::Outline
                            on:click=move |_| {
                                is_editing.set(true);
                                error_message.set(None);
                            }
                        >
                            Edit
                        </Button>
                    </Show>

                    <Show when=move || is_editing.get()>
                        <div class="flex gap-2">
                            <Button
                                variant=ButtonVariant::Outline
                                on:click=move |_| cancel_changes.run(())
                                disabled=Signal::derive(move || update_action.pending().get())
                            >
                                Cancel
                            </Button>
                            <Button
                                variant=ButtonVariant::Default
                                on:click=move |_| save_changes.run(())
                                disabled=Signal::derive(move || update_action.pending().get())
                            >
                                {move || if update_action.pending().get() { "Saving..." } else { "Save Changes" }}
                            </Button>
                        </div>
                    </Show>
                </div>
            </div>

            // Container configuration
            <div class="p-4 space-y-4">
                <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                    // Image Configuration
                    <div class="space-y-3">
                        <div class="font-medium text-sm flex items-center gap-2">
                            <lucide_leptos::Box attr:class="w-4 h-4 text-primary" />
                            Image Configuration
                        </div>
                        <div class="space-y-2">
                            <div class="text-sm">
                                <span class="text-muted-foreground">{"Original: "}</span>
                                <code class="text-xs bg-muted px-1 py-0.5 rounded break-all">
                                    {container_original_image.clone()}
                                </code>
                            </div>

                            <Show when=move || !is_editing.get()>
                                {move || {
                                    let display_image = image_signal.get();
                                    view! {
                                        <div class="text-sm">
                                            {match &display_image {
                                                KubeContainerImage::FollowOriginal => view! {
                                                    <span class="text-muted-foreground">{"Current: "}</span>
                                                    <span class="text-xs text-muted-foreground">{"Following original"}</span>
                                                }.into_any(),
                                                KubeContainerImage::Custom(custom_img) => view! {
                                                    <span class="text-muted-foreground">{"Current: "}</span>
                                                    <code class="text-xs bg-muted px-1 py-0.5 rounded break-all">
                                                        {custom_img.clone()}
                                                    </code>
                                                    <span class="ml-2 text-xs text-muted-foreground">
                                                        {"(Custom)"}
                                                    </span>
                                                }.into_any(),
                                            }}
                                        </div>
                                    }
                                }}
                            </Show>

                            <Show when=move || is_editing.get()>
                                <div class="space-y-2">
                                    <div class="flex items-center space-x-2">
                                        <input
                                            type="radio"
                                            name=format!("image_choice_{}", container_index)
                                            prop:checked=move || matches!(image_signal.get(), KubeContainerImage::FollowOriginal)
                                            on:change=move |_| {
                                                image_signal.set(KubeContainerImage::FollowOriginal);
                                            }
                                        />
                                        <label class="text-sm">Follow Original</label>
                                    </div>
                                    <div class="flex items-center space-x-2">
                                        <input
                                            type="radio"
                                            name=format!("image_choice_{}", container_index)
                                            prop:checked=move || matches!(image_signal.get(), KubeContainerImage::Custom(_))
                                            on:change=move |_| {
                                                let custom_img = custom_image_signal.get();
                                                image_signal.set(KubeContainerImage::Custom(custom_img));
                                            }
                                        />
                                        <label class="text-sm">Use Custom Image</label>
                                    </div>
                                    <Show when=move || matches!(image_signal.get(), KubeContainerImage::Custom(_))>
                                        <Input
                                            prop:value=move || custom_image_signal.get()
                                            on:input=move |ev| {
                                                let new_value = event_target_value(&ev);
                                                custom_image_signal.set(new_value.clone());
                                                image_signal.set(KubeContainerImage::Custom(new_value));
                                            }
                                            attr:placeholder="Enter custom image (e.g., ubuntu:22.04)"
                                        />
                                    </Show>
                                </div>
                            </Show>
                        </div>
                    </div>

                    // Environment Variables
                    <div class="space-y-3">
                        <div class="font-medium text-sm flex items-center gap-2">
                            <lucide_leptos::Settings attr:class="w-4 h-4 text-primary" />
                            Environment Variables
                        </div>
                        <Show when=move || !is_editing.get()>
                            <EnvVarsDisplay env_vars=env_vars_signal.get() />
                        </Show>
                        <Show when=move || is_editing.get()>
                            <EnvVarsEditor env_vars_signal />
                        </Show>
                    </div>
                </div>

                // Resource Configuration
                {if config.enable_resource_limits {
                    view! {
                        <div class="border-t pt-4">
                            <div class="font-medium text-sm mb-3 flex items-center gap-2">
                                <lucide_leptos::Cpu attr:class="w-4 h-4 text-primary" />
                                Resource Configuration
                            </div>
                            <div class="grid grid-cols-2 md:grid-cols-4 gap-4 text-sm">
                                <div>
                                    <div class="text-muted-foreground text-xs mb-1 flex items-center gap-1">
                                        <lucide_leptos::Gauge attr:class="w-3 h-3" />
                                        CPU Request
                                    </div>
                                    <Show
                                        when=move || is_editing.get()
                                        fallback=move || view! {
                                            <div class="font-mono text-xs bg-muted px-2 py-1 rounded">
                                                {move || if cpu_request_signal.get().is_empty() { "Not set".to_string() } else { cpu_request_signal.get() }}
                                            </div>
                                        }
                                    >
                                        <Input
                                            prop:value=move || cpu_request_signal.get()
                                            on:input=move |ev| cpu_request_signal.set(event_target_value(&ev))
                                            class="text-xs font-mono"
                                            attr:placeholder="e.g., 100m"
                                        />
                                    </Show>
                                </div>
                                <div>
                                    <div class="text-muted-foreground text-xs mb-1 flex items-center gap-1">
                                        <lucide_leptos::Zap attr:class="w-3 h-3" />
                                        CPU Limit
                                    </div>
                                    <Show
                                        when=move || is_editing.get()
                                        fallback=move || view! {
                                            <div class="font-mono text-xs bg-muted px-2 py-1 rounded">
                                                {move || if cpu_limit_signal.get().is_empty() { "Not set".to_string() } else { cpu_limit_signal.get() }}
                                            </div>
                                        }
                                    >
                                        <Input
                                            prop:value=move || cpu_limit_signal.get()
                                            on:input=move |ev| cpu_limit_signal.set(event_target_value(&ev))
                                            class="text-xs font-mono"
                                            attr:placeholder="e.g., 500m"
                                        />
                                    </Show>
                                </div>
                                <div>
                                    <div class="text-muted-foreground text-xs mb-1 flex items-center gap-1">
                                        <lucide_leptos::Database attr:class="w-3 h-3" />
                                        Memory Request
                                    </div>
                                    <Show
                                        when=move || is_editing.get()
                                        fallback=move || view! {
                                            <div class="font-mono text-xs bg-muted px-2 py-1 rounded">
                                                {move || if memory_request_signal.get().is_empty() { "Not set".to_string() } else { memory_request_signal.get() }}
                                            </div>
                                        }
                                    >
                                        <Input
                                            prop:value=move || memory_request_signal.get()
                                            on:input=move |ev| memory_request_signal.set(event_target_value(&ev))
                                            class="text-xs font-mono"
                                            attr:placeholder="e.g., 128Mi"
                                        />
                                    </Show>
                                </div>
                                <div>
                                    <div class="text-muted-foreground text-xs mb-1 flex items-center gap-1">
                                        <lucide_leptos::HardDrive attr:class="w-3 h-3" />
                                        Memory Limit
                                    </div>
                                    <Show
                                        when=move || is_editing.get()
                                        fallback=move || view! {
                                            <div class="font-mono text-xs bg-muted px-2 py-1 rounded">
                                                {move || if memory_limit_signal.get().is_empty() { "Not set".to_string() } else { memory_limit_signal.get() }}
                                            </div>
                                        }
                                    >
                                        <Input
                                            prop:value=move || memory_limit_signal.get()
                                            on:input=move |ev| memory_limit_signal.set(event_target_value(&ev))
                                            class="text-xs font-mono"
                                            attr:placeholder="e.g., 512Mi"
                                        />
                                    </Show>
                                </div>
                            </div>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="border-t pt-4">
                            <div class="font-medium text-sm mb-3 flex items-center gap-2">
                                <lucide_leptos::Cpu attr:class="w-4 h-4 text-primary" />
                                Resource Configuration
                            </div>
                            <div class="text-xs text-muted-foreground mb-2">
                                Resource limits and requests are managed at the app catalog level and cannot be customized per environment.
                            </div>
                            <div class="grid grid-cols-2 md:grid-cols-4 gap-4 text-sm">
                                <div>
                                    <div class="text-muted-foreground text-xs mb-1 flex items-center gap-1">
                                        <lucide_leptos::Gauge attr:class="w-3 h-3" />
                                        CPU Request
                                    </div>
                                    <div class="font-mono text-xs bg-muted px-2 py-1 rounded">
                                        {cpu_request_display.clone().unwrap_or_else(|| "Not set".to_string())}
                                    </div>
                                </div>
                                <div>
                                    <div class="text-muted-foreground text-xs mb-1 flex items-center gap-1">
                                        <lucide_leptos::Zap attr:class="w-3 h-3" />
                                        CPU Limit
                                    </div>
                                    <div class="font-mono text-xs bg-muted px-2 py-1 rounded">
                                        {cpu_limit_display.clone().unwrap_or_else(|| "Not set".to_string())}
                                    </div>
                                </div>
                                <div>
                                    <div class="text-muted-foreground text-xs mb-1 flex items-center gap-1">
                                        <lucide_leptos::Database attr:class="w-3 h-3" />
                                        Memory Request
                                    </div>
                                    <div class="font-mono text-xs bg-muted px-2 py-1 rounded">
                                        {memory_request_display.clone().unwrap_or_else(|| "Not set".to_string())}
                                    </div>
                                </div>
                                <div>
                                    <div class="text-muted-foreground text-xs mb-1 flex items-center gap-1">
                                        <lucide_leptos::HardDrive attr:class="w-3 h-3" />
                                        Memory Limit
                                    </div>
                                    <div class="font-mono text-xs bg-muted px-2 py-1 rounded">
                                        {memory_limit_display.clone().unwrap_or_else(|| "Not set".to_string())}
                                    </div>
                                </div>
                            </div>
                        </div>
                    }.into_any()
                }}

                // Error message
                <Show when=move || error_message.get().is_some()>
                    <div class="p-3 bg-destructive/10 border border-destructive/20 rounded-md">
                        <div class="text-sm text-destructive">
                            {move || error_message.get().unwrap_or_default()}
                        </div>
                    </div>
                </Show>
            </div>
        </div>
    }
}

#[component]
pub fn EnvVarsEditor(env_vars_signal: RwSignal<Vec<KubeEnvVar>>) -> impl IntoView {
    let add_env_var = Callback::new(move |_| {
        env_vars_signal.update(|vars| {
            vars.push(KubeEnvVar {
                name: String::new(),
                value: String::new(),
            });
        });
    });

    view! {
        <div class="flex flex-col gap-2">
            <div class="flex items-center justify-between">
                <span class="text-xs font-medium text-muted-foreground">Variables</span>
                <Button
                    variant=ButtonVariant::Outline
                    class="px-2 py-1 h-auto text-xs"
                    on:click=move |_| add_env_var.run(())
                >
                    <lucide_leptos::Plus attr:class="w-3 h-3" />
                    Add Variable
                </Button>
            </div>

            <div class="space-y-2">
                {move || {
                    env_vars_signal.with(|env_vars| {
                        env_vars.iter().enumerate().map(|(index, env_var)| {
                            let env_var_name = env_var.name.clone();
                            let env_var_value = env_var.value.clone();

                            view! {
                                <div class="flex gap-2 items-center">
                                    <Input
                                        prop:value=env_var_name.clone()
                                        on:input=move |ev| {
                                            let new_name = event_target_value(&ev);
                                            env_vars_signal.update(|vars| {
                                                if let Some(var) = vars.get_mut(index) {
                                                    var.name = new_name;
                                                }
                                            });
                                        }
                                        attr:placeholder="Variable name"
                                        class="text-xs font-mono flex-1"
                                    />
                                    <Input
                                        prop:value=env_var_value.clone()
                                        on:input=move |ev| {
                                            let new_value = event_target_value(&ev);
                                            env_vars_signal.update(|vars| {
                                                if let Some(var) = vars.get_mut(index) {
                                                    var.value = new_value;
                                                }
                                            });
                                        }
                                        attr:placeholder="Variable value"
                                        class="text-xs font-mono flex-1"
                                    />
                                    <Button
                                        variant=ButtonVariant::Ghost
                                        class="px-2 py-1 h-auto text-red-600 hover:text-red-700"
                                        on:click=move |_| {
                                            env_vars_signal.update(|vars| {
                                                if index < vars.len() {
                                                    vars.remove(index);
                                                }
                                            });
                                        }
                                    >
                                        <lucide_leptos::Trash2 attr:class="w-3 h-3" />
                                    </Button>
                                </div>
                            }
                        }).collect::<Vec<_>>()
                    })
                }}

                {move || {
                    let vars = env_vars_signal.get();
                    if vars.is_empty() {
                        view! {
                            <div class="text-xs text-muted-foreground italic py-2">
                                No environment variables configured
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
pub fn EnvVarsDisplay(env_vars: Vec<KubeEnvVar>) -> impl IntoView {
    view! {
        <div class="space-y-1">
            {
                if env_vars.is_empty() {
                    view! { <div class="text-sm text-muted-foreground">No environment variables</div> }.into_any()
                } else {
                    env_vars.into_iter().map(|env_var| {
                        view! {
                            <div class="flex items-center justify-between p-2 bg-muted/30 rounded text-xs">
                                <code class="font-semibold break-all">{env_var.name}</code>
                                <code class="text-muted-foreground truncate max-w-32">{env_var.value}</code>
                            </div>
                        }
                    }).collect::<Vec<_>>().into_any()
                }
            }
        </div>
    }
}
