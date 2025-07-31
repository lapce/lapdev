use leptos::prelude::*;
use tailwind_fuse::*;

#[component]
pub fn HoverCard(
    #[prop()] open: RwSignal<bool>,
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    let hide_delay = std::time::Duration::from_millis(500);
    let handle: StoredValue<Option<TimeoutHandle>> = StoredValue::new(None);
    let show = RwSignal::new(open.get_untracked());

    let eff = RenderEffect::new(move |_| {
        if show.get() {
            // clear any possibly active timer
            if let Some(h) = handle.get_value() {
                h.clear();
            }
            open.set(true)
        } else {
            let h = leptos::leptos_dom::helpers::set_timeout_with_handle(
                move || open.set(false),
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
        <div
            class=move || tw_merge!("relative", class.get())
            on:mouseenter=move |_| show.set(true)
            on:mouseleave=move |_| show.set(false)
            data-slot="hover-card"
        >
            {children}
        </div>
    }
}

#[component]
pub fn HoverCardTrigger(
    #[prop()] open: RwSignal<bool>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <div
            data-slot="hover-card-trigger"
            on:mouseenter=move |_| open.set(true)
            on:mouseleave=move |_| open.set(false)
        >
            {children}
        </div>
    }
}

#[component]
pub fn HoverCardContent(
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
                class=move || {
                    tw_merge!(
                        "absolute bg-popover text-popover-foreground data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 z-50 w-64 origin-(--radix-hover-card-content-transform-origin) rounded-md border p-4 shadow-md outline-hidden",                        class.get()
                    )
                }
                data-state=move || { if open.get() { "open" } else { "closed" } }
                data-slot="hover-card-content"
            >
                {children.clone().map(|c| c().into_any()).unwrap_or_else(|| ().into_any())}
            </div>
        </Show>
    }
}
