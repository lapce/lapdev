use leptos::prelude::*;

pub fn fade_effect(open: ReadSignal<bool>) -> RwSignal<bool> {
    let handle: StoredValue<Option<TimeoutHandle>> = StoredValue::new(None);
    let hide_delay = std::time::Duration::from_millis(100);
    let show = RwSignal::new(open.get_untracked());

    let eff = RenderEffect::new(move |_| {
        if open.get() {
            // clear any possibly active timer
            if let Some(h) = handle.get_value() {
                h.clear();
            }

            show.set(true);
        } else {
            let h = leptos::leptos_dom::helpers::set_timeout_with_handle(
                move || show.set(false),
                hide_delay,
            )
            .expect("set timeout in AnimatedShow");
            handle.set_value(Some(h));
        }
    });

    on_cleanup(move || {
        if let Some(Some(h)) = handle.try_get_value() {
            h.clear();
        }
        drop(eff);
    });

    show
}
