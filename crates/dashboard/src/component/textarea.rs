use leptos::prelude::*;
use tailwind_fuse::*;

#[component]
pub fn Textarea(#[prop(into, optional)] class: MaybeProp<String>) -> impl IntoView {
    view! {
        <textarea
            class=move || tw_join!(
                "focus-visible:ring-ring/50",
                tw_merge!(
                    "border-input placeholder:text-muted-foreground focus-visible:border-ring aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive dark:bg-input/30 flex field-sizing-content min-h-16 w-full rounded-md border bg-transparent px-3 py-2 text-base shadow-xs transition-[color,box-shadow] outline-none focus-visible:ring-[3px] disabled:cursor-not-allowed disabled:opacity-50 md:text-sm",
                    class.get(),
                )
            )
            data-slot="textarea"
        />
    }
}
