use leptos::{html::Div, prelude::*};
use tailwind_fuse::*;

#[component]
pub fn DropdownMenu(
    #[prop()] open: RwSignal<bool>,
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    let node: NodeRef<Div> = NodeRef::new();

    view! {
        <div
            class=move || tw_merge!("relative", class.get())
            node_ref=node
            data-slot="dropdown-menu"
        >
            <Show when=move || open.get() fallback=|| ()>
                <div
                    class="fixed inset-0 z-50 overflow-y-auto flex items-center justify-center w-full h-full"
                    on:click=move |_| open.set(false)
                >
                </div>
            </Show>
            {children}
        </div>
    }
}

#[component]
pub fn DropdownMenuTrigger(
    #[prop()] open: RwSignal<bool>,
    #[prop(into, optional)] disabled: MaybeProp<bool>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <div
            data-slot="dropdown-menu-trigger"
            data-disabled=move || disabled.get()
            class="data-[disabled]:pointer-events-none"
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
pub fn DropdownMenuContent(
    #[prop()] open: ReadSignal<bool>,
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
                        "absolute z-50 outline-none bg-popover text-popover-foreground data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 z-50 max-h-(--radix-dropdown-menu-content-available-height) min-w-[8rem] origin-(--radix-dropdown-menu-content-transform-origin) overflow-x-hidden overflow-y-auto rounded-md border p-1 shadow-md",
                            class.get()
                    )
                }
                style="--radix-popper-transform-origin: 0px 0%; will-change: transform; --radix-popper-available-width: 1669px; --radix-popper-available-height: 932px; --radix-popper-anchor-width: 239px; --radix-popper-anchor-height: 48px;"
                data-state=move || { if open.get() { "open" } else { "closed" } }
                data-slot="dropdown-menu-content"
                tabindex="0"
            >
                {children.clone().map(|c| c().into_any()).unwrap_or_else(|| ().into_any())}
            </div>
        </Show>
    }
}

#[component]
pub fn DropdownMenuLabel(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <div
            class=move || {
                tw_merge!(
                    "px-2 py-1.5 text-sm font-medium data-[inset]:pl-8",
                class.get()
                )
            }
            data-slot="dropdown-menu-label"
        >
            {children}
        </div>
    }
}

#[component]
pub fn DropdownMenuItem(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(into, optional)] disabled: MaybeProp<bool>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <div
            class=move || {
                tw_merge!(
                    "hover:bg-accent hover:text-accent-foreground focus:bg-accent focus:text-accent-foreground data-[variant=destructive]:text-destructive data-[variant=destructive]:focus:bg-destructive/10 dark:data-[variant=destructive]:focus:bg-destructive/20 data-[variant=destructive]:focus:text-destructive data-[variant=destructive]:*:[svg]:!text-destructive [&_svg:not([class*='text-'])]:text-muted-foreground relative flex cursor-default items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-hidden select-none data-[disabled]:pointer-events-none data-[disabled]:opacity-50 data-[inset]:pl-8 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
                class.get()
                )
            }
            data-slot="dropdown-menu-item"
            data-disabled=move || disabled.get()
        >
            {children}
        </div>
    }
}

#[component]
pub fn DropdownMenuSeparator(#[prop(into, optional)] class: MaybeProp<String>) -> impl IntoView {
    view! {
        <div
            class=move || tw_merge!("bg-border -mx-1 my-1 h-px",
                class.get())
            data-slot="dropdown-menu-separator"
        />
    }
}
