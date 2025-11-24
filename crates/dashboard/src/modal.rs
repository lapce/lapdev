use anyhow::Result;
use chrono::{DateTime, FixedOffset};
use leptos::prelude::*;
use serde::Deserialize;

use crate::component::{
    alert::{Alert, AlertDescription, AlertTitle, AlertVariant},
    alert_dialog::{
        AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader,
        AlertDialogTitle,
    },
    button::{Button, ButtonVariant},
    dialog::{DialogContent, DialogFooter, DialogHeader, DialogTitle},
    hover_card::{HoverCard, HoverCardContent, HoverCardPlacement, HoverCardTrigger},
    icon,
    input::Input,
    label::Label,
};

#[derive(Debug, Deserialize, Clone)]
pub struct ErrorResponse {
    pub error: String,
}

impl<E> From<E> for ErrorResponse
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        let err: anyhow::Error = err.into();
        Self {
            error: err.to_string(),
        }
    }
}

#[component]
pub fn CreationInput(
    label: String,
    value: RwSignal<String, LocalStorage>,
    placeholder: String,
) -> impl IntoView {
    view! {
        <div class="flex flex-col gap-2">
            <Label>{label}</Label>
            <Input
                prop:value=move || value.get()
                on:input=move |ev| {
                    value.set(event_target_value(&ev));
                }
                attr:placeholder=placeholder
            />
        </div>
    }
}

#[component]
pub fn Modal(
    #[prop()] open: RwSignal<bool>,
    action: Action<(), Result<(), ErrorResponse>>,
    #[prop(into)] title: MaybeProp<String>,
    #[prop(into, optional)] hide_action: MaybeProp<bool>,
    #[prop(into, optional)] action_text: MaybeProp<String>,
    #[prop(into, optional)] action_progress_text: MaybeProp<String>,
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<ChildrenFn>,
) -> impl IntoView {
    let error = RwSignal::new_local(None);

    let owner = RwSignal::new(Owner::new());
    let handle_action = move |_| {
        error.set(None);
        owner.get_untracked().with(move || {
            action.dispatch(());
        });
    };

    let action_pending = action.pending();
    Effect::new(move |_| {
        open.track();
        error.set(None);
    });
    Effect::new(move |_| {
        action.value().with(|result| {
            if let Some(result) = result {
                match result {
                    Ok(_) => {}
                    Err(e) => {
                        error.set(Some(e.error.clone()));
                    }
                }
            }
        })
    });

    view! {
        <DialogContent class open>
            <DialogHeader>
                <DialogTitle class="text-wrap">{move || title.get()}</DialogTitle>
            </DialogHeader>
            <div class="py-4 gap-4 flex flex-col sm:min-w-116 max-w-full">
                {move || {
                    if let Some(error) = error.get() {
                        view! {
                            <Alert variant=AlertVariant::Destructive>
                                <lucide_leptos::CircleAlert />
                                <AlertTitle>Error</AlertTitle>
                                <AlertDescription class="text-wrap">{error}</AlertDescription>
                            </Alert>
                        }
                            .into_any()
                    } else {
                        ().into_any()
                    }
                }} {children.clone().map(|c| c().into_any()).unwrap_or_else(|| ().into_any())}
            </div>
            <DialogFooter>
                <Show when=move || { hide_action.get() != Some(true) } fallback=|| ()>
                    <Button disabled=action_pending on:click=handle_action>
                        {move || {
                            if action_pending.get() {
                                view! { <icon::LoaderCircle class="animate-spin" /> }.into_any()
                            } else {
                                ().into_any()
                            }
                        }}
                        {move || {
                            if action_pending.get() {
                                action_progress_text.get().unwrap_or_else(|| "Creating".to_string())
                            } else {
                                action_text.get().unwrap_or_else(|| "Create".to_string())
                            }
                        }}
                    </Button>
                </Show>
                <Button
                    variant=ButtonVariant::Outline
                    disabled=action_pending
                    on:click=move |_| open.set(false)
                >
                    Cancel
                </Button>
            </DialogFooter>
        </DialogContent>
    }
}

#[component]
pub fn DeleteModal(
    #[prop()] open: RwSignal<bool>,
    delete_action: Action<(), Result<(), ErrorResponse>>,
    #[prop(into)] resource: MaybeProp<String>,
) -> impl IntoView {
    let error = RwSignal::new_local(None);

    let owner = RwSignal::new(Owner::new());
    let handle_delete = move |_| {
        error.set(None);
        owner.get_untracked().with(move || {
            delete_action.dispatch(());
        });
    };

    let delete_pending = delete_action.pending();
    Effect::new(move |_| {
        delete_action.value().with(|result| {
            if let Some(result) = result {
                match result {
                    Ok(_) => {}
                    Err(e) => {
                        error.set(Some(e.error.clone()));
                    }
                }
            }
        })
    });

    Effect::new(move |_| {
        open.track();
        error.set(None);
    });

    view! {
        <AlertDialogContent open>
            <AlertDialogHeader>
                <AlertDialogTitle class="text-wrap">
                    {format!(
                        "Are you sure you want to delete {}",
                        resource.get().unwrap_or_default(),
                    )}
                </AlertDialogTitle>
            </AlertDialogHeader>
            <div class="pb-4 gap-4 flex flex-col">
                <AlertDialogDescription>
                    {"This action cannot be undone. It will permanently delete your resource."}
                </AlertDialogDescription>
                {move || {
                    if let Some(error) = error.get() {
                        view! {
                            <Alert variant=AlertVariant::Destructive>
                                <lucide_leptos::CircleAlert />
                                <AlertTitle>Error</AlertTitle>
                                <AlertDescription class="text-wrap">{error}</AlertDescription>
                            </Alert>
                        }
                            .into_any()
                    } else {
                        ().into_any()
                    }
                }}
            </div>
            <AlertDialogFooter>
                <Button disabled=delete_pending on:click=handle_delete>
                    {move || {
                        if delete_pending.get() {
                            view! { <icon::LoaderCircle class="animate-spin" /> }.into_any()
                        } else {
                            ().into_any()
                        }
                    }}
                    {move || if delete_pending.get() { "Deleting" } else { "Delete" }}
                </Button>
                <Button
                    variant=ButtonVariant::Outline
                    disabled=delete_pending
                    on:click=move |_| open.set(false)
                >
                    Cancel
                </Button>
            </AlertDialogFooter>
        </AlertDialogContent>
    }
}

#[component]
pub fn DatetimeModal(time: DateTime<FixedOffset>) -> impl IntoView {
    let offset = web_sys::js_sys::Date::new_0().get_timezone_offset();
    let time = if let Some(offset) = FixedOffset::west_opt((offset * 60.0) as i32) {
        time.to_utc().with_timezone(&offset)
    } else {
        time
    };

    let open = RwSignal::new(false);
    let duration = chrono::Utc::now() - time.with_timezone(&chrono::Utc);
    let days = duration.num_days();
    let formatted = move || {
        if days > 365 {
            let unit = days / 365;
            format!("{} year{} ago", unit, if unit > 1 { "s" } else { "" })
        } else if days > 30 {
            let unit = days / 30;
            format!("{} month{} ago", unit, if unit > 1 { "s" } else { "" })
        } else if days > 7 {
            let unit = days / 7;
            format!("{} week{} ago", unit, if unit > 1 { "s" } else { "" })
        } else if days > 0 {
            let unit = days;
            format!("{} day{} ago", unit, if unit > 1 { "s" } else { "" })
        } else {
            let hours = duration.num_hours();
            if hours > 0 {
                format!("{hours} hour{} ago", if hours > 1 { "s" } else { "" })
            } else {
                let minutes = duration.num_minutes();
                if minutes > 0 {
                    format!("{minutes} minute{} ago", if minutes > 1 { "s" } else { "" })
                } else {
                    format!("{} seconds ago", duration.num_seconds())
                }
            }
        }
    };
    view! {
        <HoverCard open>
            <HoverCardTrigger placement=HoverCardPlacement::TopLeft>
                <span>{formatted}</span>
            </HoverCardTrigger>
            <HoverCardContent class="w-auto -translate-y-2">
                <span class="text-nowrap">{format!("{}", time.format("%Y-%m-%d %H:%M:%S"))}</span>
            </HoverCardContent>
        </HoverCard>
    }
}

#[component]
pub fn SettingView<T>(
    title: String,
    action: Action<(), Result<(), ErrorResponse>>,
    body: T,
    update_counter: RwSignal<i32, LocalStorage>,
    extra: Option<AnyView>,
) -> impl IntoView
where
    T: IntoView + 'static,
{
    let success = RwSignal::new_local(None);
    let error = RwSignal::new_local(None);
    let handle_save = move |_| {
        success.set(None);
        error.set(None);
        action.dispatch(());
    };
    let save_pending = action.pending();

    Effect::new(move |_| {
        action.value().with(|result| {
            if let Some(result) = result {
                match result {
                    Ok(_) => {
                        update_counter.update(|c| *c += 1);
                        success.set(Some("Saved successfully"));
                    }
                    Err(e) => {
                        error.set(Some(e.error.clone()));
                    }
                }
            }
        })
    });

    view! {
        {if title.is_empty() {
            ().into_any()
        } else {
            view! { <h5 class="text-lg font-semibold">{title}</h5> }.into_any()
        }}
        {body}
        <div class="mt-4 flex flex-col gap-4 items-start">
            <div class="flex flex-row items-center gap-2">
                <Button attr:disabled=move || save_pending.get() on:click=handle_save>
                    {move || {
                        if save_pending.get() {
                            view! { <icon::LoaderCircle class="animate-spin" /> }.into_any()
                        } else {
                            ().into_any()
                        }
                    }}
                    {move || if save_pending.get() { "Saving" } else { "Save" }}
                </Button>
                {if let Some(extra) = extra { extra.into_any() } else { ().into_any() }}
            </div>
            {move || {
                if let Some(error) = error.get() {
                    view! {
                        <Alert variant=AlertVariant::Destructive
                        class="w-auto"
                        >
                            <lucide_leptos::CircleAlert />
                            // <AlertTitle>Error</AlertTitle>
                            <AlertTitle>
                            {error}
                            </AlertTitle>
                        </Alert>
                    }
                        .into_any()
                } else {
                    ().into_any()
                }
            }}
            {move || {
                if let Some(success) = success.get() {
                        view! {
                            <Alert variant=AlertVariant::Default
                            class="w-auto"
                            >
                                <lucide_leptos::CircleCheck />
                                // <AlertTitle>Error</AlertTitle>
                                <AlertTitle>
                                {success}
                                </AlertTitle>
                            </Alert>
                        }
                    // view! {
                    //     <div class="ml-4 py-2 px-4 rounded-lg bg-green-50">
                    //         <span class="text-sm font-medium text-green-800">{success}</span>
                    //     </div>
                    // }
                        .into_any()
                } else {
                    ().into_any()
                }
            }}
        </div>
    }
}
