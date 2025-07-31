use leptos::{prelude::*, svg::Svg};
use tailwind_fuse::*;

#[component]
pub fn LoaderCircle(
    #[prop(default = 24.into(), into)] size: Signal<usize>,
    #[prop(default = "currentColor".into(), into)] color: Signal<String>,
    #[prop(default = "none".into(), into)] fill: Signal<String>,
    #[prop(default = 2.into(), into)] stroke_width: Signal<usize>,
    #[prop(default = false.into(), into)] absolute_stroke_width: Signal<bool>,
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] node_ref: NodeRef<Svg>,
) -> impl IntoView {
    let stroke_width = Signal::derive(move || {
        if absolute_stroke_width.get() {
            stroke_width.get() * 24 / size.get()
        } else {
            stroke_width.get()
        }
    });
    view! {
        <svg
            node_ref=node_ref
            class=move || tw_merge!("lucide",
                class.get())
            xmlns="http://www.w3.org/2000/svg"
            width=size
            height=size
            viewBox="0 0 24 24"
            fill=fill
            stroke=color
            stroke-width=stroke_width
            stroke-linecap="round"
            stroke-linejoin="round"
        >
            <path d="M21 12a9 9 0 1 1-6.219-8.56" />
        </svg>
    }
}

#[component]
pub fn BoxMargin() -> impl IntoView {
    view! {
        <svg
            xmlns="http://www.w3.org/2000/svg"
            width="24"
            height="24"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            class="tabler-icon tabler-icon-box-margin "
        >
            <path d="M8 8h8v8h-8z"></path>
            <path d="M4 4v.01"></path>
            <path d="M8 4v.01"></path>
            <path d="M12 4v.01"></path>
            <path d="M16 4v.01"></path>
            <path d="M20 4v.01"></path>
            <path d="M4 20v.01"></path>
            <path d="M8 20v.01"></path>
            <path d="M12 20v.01"></path>
            <path d="M16 20v.01"></path>
            <path d="M20 20v.01"></path>
            <path d="M20 16v.01"></path>
            <path d="M20 12v.01"></path>
            <path d="M20 8v.01"></path>
            <path d="M4 16v.01"></path>
            <path d="M4 12v.01"></path>
            <path d="M4 8v.01"></path>
        </svg>
    }
}

#[component]
pub fn BrandGoogle() -> impl IntoView {
    view! {
        <svg
            xmlns="http://www.w3.org/2000/svg"
            width="24"
            height="24"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            class="tabler-icon tabler-icon-brand-google "
        >
            <path d="M20.945 11a9 9 0 1 1 -3.284 -5.997l-2.655 2.392a5.5 5.5 0 1 0 2.119 6.605h-4.125v-3h7.945z"></path>
        </svg>
    }
}
