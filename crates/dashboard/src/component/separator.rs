use leptos::prelude::*;
use tailwind_fuse::*;

#[derive(Default)]
pub enum SeparatorOrientation {
    #[default]
    Horizontal,
    Vertical,
}

impl std::fmt::Display for SeparatorOrientation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            SeparatorOrientation::Horizontal => "horizontal",
            SeparatorOrientation::Vertical => "vertical",
        };
        f.write_str(s)
    }
}

#[component]
pub fn Separator(
    #[prop(into, optional)] orientation: Signal<SeparatorOrientation>,
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <div
            class=move || tw_merge!(
                "bg-border shrink-0 data-[orientation=horizontal]:h-px data-[orientation=horizontal]:w-full data-[orientation=vertical]:h-full data-[orientation=vertical]:w-px",
                class.get()
            )
            data-slot="separator-root"
            data-orientation=move || orientation.with(|o| o.to_string())
        >
            {children}
        </div>
    }
}
