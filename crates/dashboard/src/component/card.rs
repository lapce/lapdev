use leptos::prelude::*;
use tailwind_fuse::*;

#[component]
pub fn Card(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());
    view! {
        <div
            class=move || tw_merge!("rounded-lg border bg-card text-card-foreground shadow-sm", class.get())
        >
            { children }
        </div>
    }
}

#[component]
pub fn CardHeader(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());
    view! {
        <div
            class=move || tw_merge!("flex flex-col space-y-1.5 p-6", class.get())
        >
            { children }
        </div>
    }
}

#[component]
pub fn CardTitle(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());
    view! {
        <div
            class=move || tw_merge!("text-2xl font-semibold leading-none tracking-tight", class.get())
        >
            { children }
        </div>
    }
}

#[component]
pub fn CardDescription(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());
    view! {
        <div
            class=move || tw_merge!("text-sm text-muted-foreground", class.get())
        >
            { children }
        </div>
    }
}

#[component]
pub fn CardContent(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());
    view! {
        <div
            class=move || tw_merge!("p-6 pt-0", class.get())
        >
            { children }
        </div>
    }
}

#[component]
pub fn CardFooter(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());
    view! {
        <div
            class=move || tw_merge!("flex items-center p-6 pt-0", class.get())
        >
            { children }
        </div>
    }
}
