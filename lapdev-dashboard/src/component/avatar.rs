use leptos::prelude::*;
use tailwind_fuse::*;

#[component]
pub fn Avatar(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <span
            class=move || tw_merge!(
                "relative flex size-8 shrink-0 overflow-hidden rounded-full",
                class.get()
            )
            data-slot="avatar"
        >
            {children}
        </span>
    }
}

#[component]
pub fn AvatarImage(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(into, optional)] src: MaybeProp<String>,
) -> impl IntoView {
    view! {
        <img
            class=move || tw_merge!(
                "aspect-square size-full",
                class.get()
            )
            src=move || src.get().unwrap_or_default()
            data-slot="avatar-image"
        />
    }
}
