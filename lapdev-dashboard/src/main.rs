use lapdev_dashboard::App;
use leptos::{mount_to_body, view};

pub fn main() {
    leptos::document().body().unwrap().set_inner_html("");
    mount_to_body(move || {
        view! {
            <App />
        }
    })
}
