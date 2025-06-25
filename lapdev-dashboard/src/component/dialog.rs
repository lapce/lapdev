use leptos::prelude::*;
use tailwind_fuse::*;

#[component]
pub fn DialogTrigger(
    #[prop()] open: RwSignal<bool>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <div
            data-slot="dialog-trigger"
            on:click=move |_| {
                open.update(|open| {
                    *open = !*open;
                });
            }
        >
            {children}
        </div>
    }
}

#[component]
pub fn DialogContent(
    #[prop()] open: RwSignal<bool>,
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<ChildrenFn>,
) -> impl IntoView {
    let hide_delay = std::time::Duration::from_millis(100);
    let handle: StoredValue<Option<TimeoutHandle>> = StoredValue::new(None);
    let show = RwSignal::new(open.get_untracked());

    let eff = RenderEffect::new(move |_| {
        if open.get() {
            // clear any possibly active timer
            if let Some(h) = handle.get_value() {
                h.clear();
            }

            show.set(true);
        } else {
            let h = leptos::leptos_dom::helpers::set_timeout_with_handle(
                move || show.set(false),
                hide_delay,
            )
            .expect("set timeout in AnimatedShow");
            handle.set_value(Some(h));
        }
    });

    on_cleanup(move || {
        if let Some(Some(h)) = handle.try_get_value() {
            h.clear();
        }
        drop(eff);
    });

    view! {
        <Show when=move || show.get() fallback=|| ()>
            <div
                class="fixed inset-0 z-50 overflow-y-auto flex items-center justify-center w-full h-full"
            >
                <div
                    class="data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 fixed inset-0 z-50 bg-black/50"
                    data-slot="dialog-overlay"
                    data-state=move || { if open.get() { "open" } else { "closed" } }
                    on:pointerdown=move |_| open.set(false)
                >
                </div>
                <div class="max-h-full p-4">
                    <div
                        class=move || {
                            tw_merge!(
                                "bg-background data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95 z-50 flex flex-col w-full gap-4 rounded-lg border p-6 shadow-lg duration-200 sm:max-w-lg relative",
                                class.get()
                            )
                        }
                        data-state=move || { if open.get() { "open" } else { "closed" } }
                        data-slot="dialog-content"
                    >
                        {children.clone().map(|c| c().into_any()).unwrap_or_else(|| ().into_any())}
                        <button
                            on:click=move |_| open.set(false)
                            class="ring-offset-background focus:ring-ring data-[state=open]:bg-accent data-[state=open]:text-muted-foreground absolute top-4 right-4 rounded-xs opacity-70 transition-opacity hover:opacity-100 focus:ring-2 focus:ring-offset-2 focus:outline-hidden disabled:pointer-events-none [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4"
                        >
                            <lucide_leptos::X />
                            <span class="sr-only">Close</span>
                        </button>
                    </div>
                </div>
            </div>
        </Show>
    }
}

#[component]
pub fn DialogHeader(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <div
            class=move || tw_merge!(
                "flex flex-col gap-2 text-center sm:text-left",
                class.get()
            )
            data-slot="dialog-header"
        >
            {children}
        </div>
    }
}

#[component]
pub fn DialogTitle(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <span
            class=move || tw_merge!(
                "text-lg leading-none font-semibold",
                class.get()
            )
            data-slot="dialog-title"
        >
            {children}
        </span>
    }
}

#[component]
pub fn DialogFooter(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <div
            class=move || tw_merge!(
                "flex flex-col-reverse gap-2 sm:flex-row sm:justify-end",
                class.get()
            )
            data-slot="dialog-footer"
        >
            {children}
        </div>
    }
}
