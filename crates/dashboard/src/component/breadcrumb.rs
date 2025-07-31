use leptos::prelude::*;
use lucide_leptos::ChevronRight;
use tailwind_fuse::*;

#[component]
pub fn Breadcrumb(#[prop(optional)] children: Option<Children>) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <nav
            aria-label="breadcrumb"
            data-slot="breadcrumb"
        >
            {children}
        </nav>
    }
}

#[component]
pub fn BreadcrumbList(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <ol
            class=move || tw_merge!(
                "text-muted-foreground flex flex-wrap items-center gap-1.5 text-sm break-words sm:gap-2.5",
                class.get()
            )
            data-slot="breadcrumb-list"
        >
            {children}
        </ol>
    }
}

#[component]
pub fn BreadcrumbItem(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <li
            class=move || tw_merge!(
                "inline-flex items-center gap-1.5",
                class.get()
            )
            data-slot="breadcrumb-item"
        >
            {children}
        </li>
    }
}

#[component]
pub fn BreadcrumbPage(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <span
            class=move || tw_merge!(
                "text-foreground font-normal",
                class.get()
            )
            data-slot="breadcrumb-page"
            role="link"
            aria-disabled="true"
            aria-current="page"
        >
            {children}
        </span>
    }
}

#[component]
pub fn BreadcrumbSeparator(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| view! { <ChevronRight /> }.into_any());

    view! {
        <span
            class=move || tw_merge!(
                "[&>svg]:size-3.5",
                class.get()
            )
            data-slot="breadcrumb-separator"
            role="presentation"
            aria-hidden="true"
        >
            {children}
        </span>
    }
}

#[component]
pub fn BreadcrumbLink(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(into, optional)] href: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <a
            class=move || tw_merge!(
                "hover:text-foreground transition-colors",
                class.get()
            )
            href=move || href.get()
            data-slot="breadcrumb-link"
        >
            {children}
        </a>
    }
}
