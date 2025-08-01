use leptos::prelude::*;
use tailwind_fuse::*;

#[component]
pub fn Checkbox(#[prop(into, optional)] class: MaybeProp<String>) -> impl IntoView {
    view! {
        <input
            type="checkbox"
            class=move || tw_merge!(
                "peer border-input dark:bg-input/30 data-[state=checked]:bg-primary data-[state=checked]:text-primary-foreground dark:data-[state=checked]:bg-primary data-[state=checked]:border-primary focus-visible:border-ring focus-visible:ring-ring/50 aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive size-4 shrink-0 rounded-[4px] border shadow-xs transition-shadow outline-none focus-visible:ring-[3px] disabled:cursor-not-allowed disabled:opacity-50",
                class.get(),
            )
            data-slot="checkbox"
        />
    }
}
