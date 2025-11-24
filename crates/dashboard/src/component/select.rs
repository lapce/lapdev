use leptos::{context::Provider, leptos_dom::logging::console_log, prelude::*};
use tailwind_fuse::*;

use crate::component::{fade_effect::fade_effect, input::Input};

#[derive(Clone)]
pub struct SelectValueContext<T: PartialEq + Clone + Send + Sync + 'static> {
    value: RwSignal<T>,
}

#[derive(Clone)]
pub struct SelectContext {
    open: RwSignal<bool>,
    search_filter: RwSignal<String>,
}

#[derive(TwVariant)]
pub enum SelectTriggerSize {
    #[tw(default, class = "")]
    Default,
    #[tw(class = "")]
    Sm,
}

#[component]
pub fn Select<T>(
    #[prop()] value: RwSignal<T>,
    // #[prop()] on_change: impl Fn(String) + 'static + Send + Sync,
    #[prop()] open: RwSignal<bool>,
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView
where
    T: PartialEq + Clone + Send + Sync + 'static,
{
    let search_filter = RwSignal::new("".to_string());
    Effect::new(move |_| {
        open.track();
        search_filter.set("".to_string());
    });
    view! {
        <Provider value=SelectValueContext { value }>
            <Provider value=SelectContext { open, search_filter }>
                <div class=move || tw_merge!("relative", class.get()) data-slot="select">
                    <Show when=move || open.get() fallback=|| ()>
                        <div
                            class="fixed inset-0 z-50 overflow-y-auto flex items-center justify-center w-full h-full"
                            on:click=move |_| open.set(false)
                        >
                        </div>
                    </Show>
                    { children.map(|c| c().into_any()).unwrap_or_else(|| ().into_any())}
                </div>
            </Provider>
        </Provider>
    }
}

#[component]
pub fn SelectTrigger(
    #[prop(into, optional)] size: Signal<SelectTriggerSize>,
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let context = use_context::<SelectContext>().expect("SelectItem must be used within Select");

    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <button
            type="button"
            class=move || tw_merge!(
                "border-input data-[placeholder]:text-muted-foreground [&_svg:not([class*='text-'])]:text-muted-foreground focus-visible:border-ring focus-visible:ring-ring/50 aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive dark:bg-input/30 dark:hover:bg-input/50 flex w-fit items-center justify-between gap-2 rounded-md border bg-transparent px-3 py-2 text-sm whitespace-nowrap shadow-xs transition-[color,box-shadow] outline-none focus-visible:ring-[3px] disabled:cursor-not-allowed disabled:opacity-50 data-[size=default]:h-9 data-[size=sm]:h-8 *:data-[slot=select-value]:line-clamp-1 *:data-[slot=select-value]:flex *:data-[slot=select-value]:items-center *:data-[slot=select-value]:gap-2 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
                class.get()
            )
            data-slot="select-trigger"
            data-size={move || match size.get() {
                SelectTriggerSize::Sm => "sm",
                SelectTriggerSize::Default => "default",
            }}
            on:click=move |_| {
                context.open.update(|open| *open = !*open);
            }
        >
            { children }
            // <span class="pointer-events-none">
            //     {move || context.current_value.get()}
            // </span>
            <lucide_leptos::ChevronDown />
        </button>
    }
}

#[component]
pub fn SelectContent(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<ChildrenFn>,
) -> impl IntoView {
    let open_context =
        use_context::<SelectContext>().expect("SelectItem needs to have SelectOpenContext");
    let open = open_context.open;
    let show = fade_effect(open.read_only());

    view! {
        <Show when=move || show.get() fallback=|| ()>
            <div
                class=move || tw_merge!(
                    "absolute z-50 bg-popover text-popover-foreground data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 max-h-(--radix-select-content-available-height) min-w-[8rem] origin-(--radix-select-content-transform-origin) overflow-x-hidden overflow-y-auto rounded-md border shadow-md",
                    "data-[side=bottom]:translate-y-1 data-[side=left]:-translate-x-1 data-[side=right]:translate-x-1 data-[side=top]:-translate-y-1",
                    class.get()
                )
                data-side="bottom"
                data-state=move || { if open.get() { "open" } else { "closed" } }
                data-slot="select-content"
            >
                <div
                    class=move || tw_merge!(
                        "p-1",
                        "h-[var(--radix-select-trigger-height)] w-full min-w-[var(--radix-select-trigger-width)] scroll-my-1",
                    )
                >
                    {children.clone().map(|c| c().into_any()).unwrap_or_else(|| ().into_any())}
                </div>
            </div>
        </Show>
    }
}

#[component]
pub fn SelectItem<T>(
    #[prop()] value: T,
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(into, optional)] display: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView
where
    T: PartialEq + std::fmt::Debug + Clone + Send + Sync + 'static,
{
    let value_context = use_context::<SelectValueContext<T>>();
    let context =
        use_context::<SelectContext>().expect("SelectItem needs to have SelectOpenContext");
    if value_context.is_none() {
        console_log(&format!(
            "Select Item {value:?} won't be selected because it's the different type of value in Select"
        ));
    }
    // let item_value = value.clone();
    // let display_value = value.clone();

    let is_selected = {
        let context = value_context.clone();
        let value = value.clone();
        Signal::derive(move || context.as_ref().map(|c| c.value.get()).as_ref() == Some(&value))
    };

    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <div
            class=move || tw_merge!(
                "hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='text-'])]:text-muted-foreground relative flex w-full cursor-default items-center gap-2 rounded-sm py-1.5 pr-8 pl-2 text-sm outline-hidden select-none data-[disabled]:pointer-events-none data-[disabled]:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4 *:[span]:last:flex *:[span]:last:items-center *:[span]:last:gap-2",
                // if is_selected.get() { "bg-accent text-accent-foreground" } else { "" },
                class.get()
            )
            class:hidden=move || {
                if let Some(display) = display.get() {
                    if !display.contains(context.search_filter.get().as_str()) {
                        return true
                    }
                }
                false
            }
            // data-value=display_value.clone()
            // data-selected=move || is_selected.get()
            data-slot="select-item"
            on:click=move |_| {
                if let Some(context) = &value_context {
                    context.value.set(value.clone());
                }
                context.open.set(false);
            }
        >
            <span class="absolute right-2 flex size-3.5 items-center justify-center">
                <Show when=move || is_selected.get() fallback=|| ()>
                    <lucide_leptos::Check  />
                </Show>
            </span>
            {children}
        </div>
    }
}

#[component]
pub fn SelectLabel(
    #[prop(into, optional)] class: MaybeProp<String>,
    children: Children,
) -> impl IntoView {
    view! {
        <div
            class=move || tw_merge!(
                "px-2 py-1.5 text-sm font-semibold text-foreground",
                class.get()
            )
            data-slot="select-label"
        >
            {children()}
        </div>
    }
}

#[component]
pub fn SelectSearchInput(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(into, optional)] placeholder: MaybeProp<String>,
) -> impl IntoView {
    let class = Signal::derive(move || {
        tw_merge!(
            "w-full px-2 border-none focus-visible:ring-[0px] focus-visible:border-0 shadow-none",
            class.get()
        )
    });

    let context = use_context::<SelectContext>().expect("SelectItem must be used within Select");

    view! {
        <div class="flex items-center pl-2 [&_svg:not([class*='text-'])]:text-muted-foreground [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4">
            <lucide_leptos::Search />
            <Input
                class
                attr:placeholder=move || placeholder.get().unwrap_or_else(|| "Search ...".to_string())
                prop:value=move || context.search_filter.get()
                on:input=move |ev| {
                    context.search_filter.set(event_target_value(&ev));
                }
            />
        </div>
    }
}

#[component]
pub fn SelectSeparator(#[prop(into, optional)] class: MaybeProp<String>) -> impl IntoView {
    view! {
        <div
            class=move || tw_merge!(
                "bg-muted -mx-1 my-1 h-px",
                class.get()
            )
            data-slot="select-separator"
        />
    }
}
