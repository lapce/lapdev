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
    hover_card::{HoverCard, HoverCardContent},
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
                <DialogTitle>{move || title.get()}</DialogTitle>
            </DialogHeader>
            <div class="py-4 gap-4 flex flex-col sm:min-w-116 max-w-full">
                {move || {
                    if let Some(error) = error.get() {
                        view! {
                            <Alert variant=AlertVariant::Destructive>
                                <lucide_leptos::CircleAlert />
                                <AlertTitle>Error</AlertTitle>
                                <AlertDescription>{error}</AlertDescription>
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
                <AlertDialogTitle>
                    {format!(
                        "Are you sure you want to delete {}",
                        resource.get().unwrap_or_default(),
                    )}
                </AlertDialogTitle>
            </AlertDialogHeader>
            <div class="pb-4 gap-4 flex flex-col">
                <AlertDialogDescription>
                    This action cannot be undone. It will permanently delete your resource.
                </AlertDialogDescription>
                {move || {
                    if let Some(error) = error.get() {
                        view! {
                            <Alert variant=AlertVariant::Destructive>
                                <lucide_leptos::CircleAlert />
                                <AlertTitle>Error</AlertTitle>
                                <AlertDescription>{error}</AlertDescription>
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
pub fn DeletionModal(
    resource: String,
    modal_hidden: RwSignal<bool, LocalStorage>,
    delete_action: Action<(), Result<(), ErrorResponse>>,
) -> impl IntoView {
    let error = RwSignal::new_local(None);
    let handle_delete = move |_| {
        delete_action.dispatch(());
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
        if modal_hidden.get() {
            error.set(None);
        }
    });

    view! {
        <div
            tabindex="-1"
            class="bg-gray-900/50 flex overflow-y-auto overflow-x-hidden fixed top-0 right-0 left-0 z-50 justify-center items-center w-full h-full"
            class:hidden=move || modal_hidden.get()
            on:click=move |_| modal_hidden.set(true)
        >
            <div
                class="relative p-4 w-full max-w-md max-h-full"
                on:click=move |e| e.stop_propagation()
            >
                <div class="relative bg-white rounded-lg shadow">
                    <button
                        type="button"
                        class="absolute top-3 end-2.5 text-gray-400 bg-transparent hover:bg-gray-200 hover:text-gray-900 rounded-lg text-sm w-8 h-8 ms-auto inline-flex justify-center items-center"
                        on:click=move |_| modal_hidden.set(true)
                    >
                        <svg
                            class="w-3 h-3"
                            aria-hidden="true"
                            xmlns="http://www.w3.org/2000/svg"
                            fill="none"
                            viewBox="0 0 14 14"
                        >
                            <path
                                stroke="currentColor"
                                stroke-linecap="round"
                                stroke-linejoin="round"
                                stroke-width="2"
                                d="m1 1 6 6m0 0 6 6M7 7l6-6M7 7l-6 6"
                            />
                        </svg>
                        <span class="sr-only">Close modal</span>
                    </button>
                    <div class="p-4 md:p-5 text-center">
                        <svg
                            class="mx-auto mb-4 text-gray-400 w-12 h-12"
                            aria-hidden="true"
                            xmlns="http://www.w3.org/2000/svg"
                            fill="none"
                            viewBox="0 0 20 20"
                        >
                            <path
                                stroke="currentColor"
                                stroke-linecap="round"
                                stroke-linejoin="round"
                                stroke-width="2"
                                d="M10 11V6m0 8h.01M19 10a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z"
                            />
                        </svg>
                        <h3 class="mb-5 text-lg font-normal text-gray-500 flex flex-col items-center">
                            <span>Are you sure you want to delete</span>
                            <span class="text-semibold">{resource}</span>
                        </h3>
                        {move || {
                            if let Some(error) = error.get() {
                                view! {
                                    <div class="text-left p-4 mb-4 rounded-lg bg-red-50">
                                        <span class="text-sm font-medium text-red-800">
                                            {error}
                                        </span>
                                    </div>
                                }
                                    .into_any()
                            } else {
                                ().into_any()
                            }
                        }}
                        <button
                            type="button"
                            class="text-white bg-red-600 hover:bg-red-800 focus:ring-4 focus:outline-none focus:ring-red-300 font-medium rounded-lg text-sm inline-flex items-center px-5 py-2.5 text-center me-2"
                            disabled=move || delete_pending.get()
                            on:click=handle_delete
                        >
                            <svg
                                aria-hidden="true"
                                role="status"
                                class="w-3 h-3 me-3 text-white animate-spin"
                                viewBox="0 0 100 101"
                                fill="none"
                                xmlns="http://www.w3.org/2000/svg"
                                class:hidden=move || !delete_pending.get()
                            >
                                <path
                                    d="M100 50.5908C100 78.2051 77.6142 100.591 50 100.591C22.3858 100.591 0 78.2051 0 50.5908C0 22.9766 22.3858 0.59082 50 0.59082C77.6142 0.59082 100 22.9766 100 50.5908ZM9.08144 50.5908C9.08144 73.1895 27.4013 91.5094 50 91.5094C72.5987 91.5094 90.9186 73.1895 90.9186 50.5908C90.9186 27.9921 72.5987 9.67226 50 9.67226C27.4013 9.67226 9.08144 27.9921 9.08144 50.5908Z"
                                    fill="#E5E7EB"
                                />
                                <path
                                    d="M93.9676 39.0409C96.393 38.4038 97.8624 35.9116 97.0079 33.5539C95.2932 28.8227 92.871 24.3692 89.8167 20.348C85.8452 15.1192 80.8826 10.7238 75.2124 7.41289C69.5422 4.10194 63.2754 1.94025 56.7698 1.05124C51.7666 0.367541 46.6976 0.446843 41.7345 1.27873C39.2613 1.69328 37.813 4.19778 38.4501 6.62326C39.0873 9.04874 41.5694 10.4717 44.0505 10.1071C47.8511 9.54855 51.7191 9.52689 55.5402 10.0491C60.8642 10.7766 65.9928 12.5457 70.6331 15.2552C75.2735 17.9648 79.3347 21.5619 82.5849 25.841C84.9175 28.9121 86.7997 32.2913 88.1811 35.8758C89.083 38.2158 91.5421 39.6781 93.9676 39.0409Z"
                                    fill="currentColor"
                                />
                            </svg>
                            {move || {
                                if delete_pending.get() { "Deleting" } else { "Yes, I'm sure" }
                            }}
                        </button>
                        <button
                            type="button"
                            class="text-gray-500 bg-white hover:bg-gray-100 focus:ring-4 focus:outline-none focus:ring-gray-200 rounded-lg border border-gray-200 text-sm font-medium px-5 py-2.5 hover:text-gray-900 focus:z-10"
                            disabled=move || delete_pending.get()
                            on:click=move |_| modal_hidden.set(true)
                        >
                            No, cancel
                        </button>
                    </div>
                </div>
            </div>
        </div>
    }
}

#[component]
pub fn DatetimeModal(time: DateTime<FixedOffset>) -> impl IntoView {
    let offset = web_sys::js_sys::Date::new_0().get_timezone_offset();
    let time = if let Some(offset) = FixedOffset::east_opt((offset * 60.0) as i32) {
        time.with_timezone(&offset)
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
            // <HoverCardTrigger open>
            <span>{formatted}</span>
            // </HoverCardTrigger>
            <HoverCardContent open class="w-auto bottom-7">
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
        <div class="mt-4 flex flex-row items-center">
            <Button attr:disabled=move || save_pending.get() on:click=handle_save>
                <svg
                    aria-hidden="true"
                    role="status"
                    class="w-4 h-4 me-3 text-white animate-spin"
                    viewBox="0 0 100 101"
                    fill="none"
                    xmlns="http://www.w3.org/2000/svg"
                    class:hidden=move || !save_pending.get()
                >
                    <path
                        d="M100 50.5908C100 78.2051 77.6142 100.591 50 100.591C22.3858 100.591 0 78.2051 0 50.5908C0 22.9766 22.3858 0.59082 50 0.59082C77.6142 0.59082 100 22.9766 100 50.5908ZM9.08144 50.5908C9.08144 73.1895 27.4013 91.5094 50 91.5094C72.5987 91.5094 90.9186 73.1895 90.9186 50.5908C90.9186 27.9921 72.5987 9.67226 50 9.67226C27.4013 9.67226 9.08144 27.9921 9.08144 50.5908Z"
                        fill="#E5E7EB"
                    />
                    <path
                        d="M93.9676 39.0409C96.393 38.4038 97.8624 35.9116 97.0079 33.5539C95.2932 28.8227 92.871 24.3692 89.8167 20.348C85.8452 15.1192 80.8826 10.7238 75.2124 7.41289C69.5422 4.10194 63.2754 1.94025 56.7698 1.05124C51.7666 0.367541 46.6976 0.446843 41.7345 1.27873C39.2613 1.69328 37.813 4.19778 38.4501 6.62326C39.0873 9.04874 41.5694 10.4717 44.0505 10.1071C47.8511 9.54855 51.7191 9.52689 55.5402 10.0491C60.8642 10.7766 65.9928 12.5457 70.6331 15.2552C75.2735 17.9648 79.3347 21.5619 82.5849 25.841C84.9175 28.9121 86.7997 32.2913 88.1811 35.8758C89.083 38.2158 91.5421 39.6781 93.9676 39.0409Z"
                        fill="currentColor"
                    />
                </svg>
                {move || if save_pending.get() { "Saving" } else { "Save" }}
            </Button>
            {if let Some(extra) = extra { extra.into_any() } else { ().into_any() }}
            {move || {
                if let Some(error) = error.get() {
                    view! {
                        <div class="ml-4 py-2 px-4 rounded-lg bg-red-50">
                            <span class="text-sm font-medium text-red-800">{error}</span>
                        </div>
                    }
                        .into_any()
                } else {
                    ().into_any()
                }
            }}
            {move || {
                if let Some(success) = success.get() {
                    view! {
                        <div class="ml-4 py-2 px-4 rounded-lg bg-green-50">
                            <span class="text-sm font-medium text-green-800">{success}</span>
                        </div>
                    }
                        .into_any()
                } else {
                    ().into_any()
                }
            }}
        </div>
    }
}
