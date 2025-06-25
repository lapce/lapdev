use leptos::prelude::*;

#[component]
pub fn CollapsibleTrigger(
    #[prop()] open: RwSignal<bool>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <div
            data-slot="collapsible-trigger"
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
pub fn CollapsibleContent(
    #[prop()] open: ReadSignal<bool>,
    #[prop(optional)] children: Option<ChildrenFn>,
) -> impl IntoView {
    view! {
        <Show when=move || open.get() fallback=|| ()>
            {children.clone().map(|c| c().into_any()).unwrap_or_else(|| ().into_any())}
        </Show>
    }
}
