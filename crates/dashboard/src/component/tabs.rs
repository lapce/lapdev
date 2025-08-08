use leptos::{context::Provider, prelude::*};
use tailwind_fuse::*;

#[derive(Clone)]
struct TabsContext<T: PartialEq + Clone + Send + Sync + 'static> {
    value: RwSignal<T>,
}

#[component]
pub fn Tabs<T>(
    #[prop()] default_value: RwSignal<T>,
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<ChildrenFn>,
) -> impl IntoView
where
    T: PartialEq + Clone + Send + Sync + 'static,
{
    view! {
        <Provider value=TabsContext { value: default_value }>
            <div
                data-slot="tabs"
                class=move || tw_merge!(
                    "flex flex-col gap-2",
                    class.get()
                )
            >
                {children.clone().map(|c| c().into_any()).unwrap_or_else(|| ().into_any())}
            </div>
        </Provider>
    }
}

#[component]
pub fn TabsList(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<ChildrenFn>,
) -> impl IntoView {
    view! {
        <div
            data-slot="tabs-list"
            class=move || tw_merge!(
                "bg-muted text-muted-foreground inline-flex h-9 w-fit items-center justify-center rounded-lg p-[3px]",
                class.get()
            )
        >
            {children.clone().map(|c| c().into_any()).unwrap_or_else(|| ().into_any())}
        </div>
    }
}

#[component]
pub fn TabsTrigger<T>(
    #[prop()] value: T,
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<ChildrenFn>,
) -> impl IntoView
where
    T: PartialEq + Clone + Send + Sync + 'static,
{
    let context = use_context::<TabsContext<T>>();
    let is_active = {
        let value = value.clone();
        let context = context.clone();
        Signal::derive(move || context.as_ref().map(|c| c.value.get()).as_ref() == Some(&value))
    };

    view! {
        <button
            data-slot="tabs-trigger"
            class=move || tw_merge!(
                "data-[state=active]:bg-background dark:data-[state=active]:text-foreground focus-visible:border-ring focus-visible:ring-ring/50 focus-visible:outline-ring dark:data-[state=active]:border-input dark:data-[state=active]:bg-input/30 text-foreground dark:text-muted-foreground inline-flex h-[calc(100%-1px)] flex-1 items-center justify-center gap-1.5 rounded-md border border-transparent px-2 py-1 text-sm font-medium whitespace-nowrap transition-[color,box-shadow] focus-visible:ring-[3px] focus-visible:outline-1 disabled:pointer-events-none disabled:opacity-50 data-[state=active]:shadow-sm [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
                class.get()
            )
            data-state=move || { if is_active.get() { "active" } else { "inactive" } }
            on:click=move |_| {
                if let Some(context) = &context {
                    context.value.set(value.clone());
                }
            }
        >
            {children.clone().map(|c| c().into_any()).unwrap_or_else(|| ().into_any())}
        </button>
    }
}

#[component]
pub fn TabsContent<T>(
    #[prop()] value: T,
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<ChildrenFn>,
) -> impl IntoView
where
    T: PartialEq + Clone + Send + Sync + 'static,
{
    let context = use_context::<TabsContext<T>>();
    let is_active = {
        let value = value.clone();
        Signal::derive(move || context.as_ref().map(|c| c.value.get()).as_ref() == Some(&value))
    };

    view! {
        <Show when=move || is_active.get()>
            <div
                data-slot="tabs-content"
                class=move || tw_merge!(
                    "flex-1 outline-none",
                    class.get()
                )
            >
                {children.clone().map(|c| c().into_any()).unwrap_or_else(|| ().into_any())}
            </div>
        </Show>
    }
}
