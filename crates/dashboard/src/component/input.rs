use leptos::prelude::*;
use tailwind_fuse::*;

#[component]
pub fn Input(#[prop(into, optional)] class: MaybeProp<String>) -> impl IntoView {
    view! {
        <input
            class=move || tw_join!(
                "focus-visible:ring-ring/50",
                tw_merge!(
                    "file:text-foreground placeholder:text-muted-foreground selection:bg-primary selection:text-primary-foreground dark:bg-input/30 border-input flex h-9 w-full min-w-0 rounded-md border bg-transparent px-3 py-1 text-base shadow-xs transition-[color,box-shadow] outline-none file:inline-flex file:h-7 file:border-0 file:bg-transparent file:text-sm file:font-medium disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50 md:text-sm",
                    "focus-visible:border-ring focus-visible:ring-[3px] aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive",
                    class.get(),
                )
            )
            data-slot="input"
        />
    }
}
