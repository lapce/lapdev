use lapdev_dashboard::app::App;
use leptos::{mount::mount_to_body, view};

pub fn main() {
    leptos::prelude::document()
        .body()
        .unwrap()
        .set_inner_html("");
    mount_to_body(move || {
        view! {
            <App />
        }
    })
}
