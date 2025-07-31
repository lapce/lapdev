use leptos::prelude::*;
use tailwind_fuse::*;

#[component]
pub fn H2(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());
    view! {
        <h2
            class=move || tw_merge!(
                "scroll-m-20 border-b pb-2 text-3xl font-semibold tracking-tight first:mt-0",
                class.get()
            )
        >
            {children}
        </h2>
    }
}

#[component]
pub fn H3(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());
    view! {
        <h3
            class=move || tw_merge!(
                "scroll-m-20 text-2xl font-semibold tracking-tight",
                class.get()
            )
        >
            {children}
        </h3>
    }
}

#[component]
pub fn Lead(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());
    view! {
        <p
            class=move || tw_merge!(
                "text-muted-foreground text-xl",
                class.get()
            )
        >
            {children}
        </p>
    }
}

#[component]
pub fn P(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());
    view! {
        <p
            class=move || tw_merge!(
                "leading-7",
                class.get()
            )
        >
            {children}
        </p>
    }
}
