use leptos::prelude::*;
use tailwind_fuse::*;

#[derive(PartialEq, TwVariant)]
pub enum AlertVariant {
    #[tw(default, class = "bg-card text-card-foreground")]
    Default,
    #[tw(
        class = "text-destructive bg-card [&>svg]:text-current *:data-[slot=alert-description]:text-destructive/90"
    )]
    Destructive,
}

#[component]
pub fn Alert(
    #[prop(into, optional)] variant: Signal<AlertVariant>,
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <div
            class=move || tw_merge!(
                "relative w-full rounded-lg border px-4 py-3 text-sm grid has-[>svg]:grid-cols-[calc(var(--spacing)*4)_1fr] grid-cols-[0_1fr] has-[>svg]:gap-x-3 gap-y-0.5 items-start [&>svg]:size-4 [&>svg]:translate-y-0.5 [&>svg]:text-current",
                variant.get(),
                class.get()
            )
            data-slot="alert"
            role="alert"
        >
            {children}
        </div>
    }
}

#[component]
pub fn AlertTitle(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <div
            class=move || tw_merge!(
                "col-start-2 line-clamp-1 min-h-4 font-medium tracking-tight",
                class.get()
            )
            data-slot="alert-title"
        >
            {children}
        </div>
    }
}

#[component]
pub fn AlertDescription(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <div
            class=move || tw_merge!(
                "text-muted-foreground col-start-2 grid justify-items-start gap-1 text-sm [&_p]:leading-relaxed",
                class.get()
            )
            data-slot="alert-description"
        >
            {children}
        </div>
    }
}
