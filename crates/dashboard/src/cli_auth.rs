use gloo_net::http::Request;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_query_map;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize)]
struct GenerateTokenRequest {
    session_id: Uuid,
    device_name: String,
}

#[derive(Debug, Deserialize)]
struct GenerateTokenResponse {
    token: String,
    expires_in: u32,
}

#[component]
pub fn CliAuth() -> impl IntoView {
    let query = use_query_map();
    let session_id = move || {
        query
            .read()
            .get("session_id")
            .and_then(|s| Uuid::parse_str(&s).ok())
    };
    let device_name = move || query.read().get("device_name").unwrap_or_default();

    let (generating, set_generating) = signal(false);
    let (error, set_error) = signal(None::<String>);

    // Auto-generate token when component mounts (user is already authenticated by WrappedView)
    Effect::new(move |_| {
        let sid = session_id();
        let dname = device_name();

        if let Some(session_id) = sid {
            if !dname.is_empty() && !generating.get() {
                set_generating.set(true);
                spawn_local(async move {
                    match generate_token(session_id, dname).await {
                        Ok(_) => {
                            // Redirect to success page
                            if let Some(window) = web_sys::window() {
                                let _ = window.location().set_href("/cli/success");
                            }
                        }
                        Err(e) => {
                            set_error.set(Some(format!("Failed to generate token: {}", e)));
                            set_generating.set(false);
                        }
                    }
                });
            }
        } else {
            set_error.set(Some("Missing session_id parameter".to_string()));
        }
    });

    view! {
        <div class="flex flex-col items-center justify-center min-h-screen bg-gray-50">
            <div class="text-center p-8 bg-white rounded-lg shadow-lg max-w-md">
                <div class="mb-6 flex items-center justify-center">
                    <svg class="w-12 h-12 mr-3" xmlns="http://www.w3.org/2000/svg" fill="currentColor" viewBox="0 0 1024 1024">
                        <path d="M512 0C390.759 0 279.426 42.2227 191.719 112.656L191.719 612L351.719 612L351.719 332L422 332L431.719 332L431.719 332.344C488.169 334.127 541.249 351.138 581.875 380.625C627.591 413.806 654.875 460.633 654.875 512C654.875 563.367 627.591 610.194 581.875 643.375C541.249 672.862 488.169 689.873 431.719 691.656L431.719 1017.69C457.88 1021.81 484.68 1024 512 1024C794.77 1024 1024 794.77 1024 512C1024 229.23 794.77 0 512 0ZM111.719 192.906C41.8539 280.432 0 391.303 0 512C0 738.766 147.487 930.96 351.719 998.25L351.719 692L151.719 692L151.719 691.844L111.719 691.844L111.719 192.906ZM738.219 372C741.597 372.107 745.042 373.02 748.312 374.812L946.281 483.312C952.311 486.616 956.692 492.393 959.188 499.156C959.821 500.874 960.317 502.641 960.688 504.469C960.764 504.834 960.841 505.196 960.906 505.562C961.12 506.807 961.225 508.071 961.312 509.344C961.378 510.235 961.498 511.112 961.5 512C961.498 512.888 961.378 513.765 961.312 514.656C961.226 515.929 961.12 517.193 960.906 518.438C960.841 518.804 960.764 519.166 960.688 519.531C960.317 521.359 959.821 523.126 959.188 524.844C956.692 531.608 952.311 537.384 946.281 540.688L748.312 649.188C735.232 656.355 719.818 649.367 713.875 633.594C707.932 617.82 713.7 599.23 726.781 592.062L872.875 512L726.781 431.938C713.7 424.771 707.932 406.18 713.875 390.406C718.332 378.576 728.085 371.678 738.219 372ZM431.719 412.344L431.719 611.656C513.56 608.208 574.875 561.985 574.875 512C574.875 462.015 513.56 415.792 431.719 412.344Z" />
                        <path d="M742 403.688C740.062 403.483 738.097 404.438 737.094 406.25C735.756 408.666 736.615 411.694 739.031 413.031L925.719 516.375C928.135 517.712 931.194 516.822 932.531 514.406C933.869 511.99 932.979 508.962 930.562 507.625L743.875 404.281C743.271 403.947 742.646 403.756 742 403.688Z" />
                        <path d="M927.5 507.031C926.856 507.115 926.221 507.339 925.625 507.688L738.938 616.906C736.554 618.301 735.762 621.335 737.156 623.719C738.551 626.102 741.616 626.926 744 625.531L930.688 516.312C933.071 514.918 933.863 511.852 932.469 509.469C931.423 507.681 929.432 506.78 927.5 507.031Z" />
                    </svg>
                    <span class="text-3xl font-semibold">Lapdev</span>
                </div>
                <Show
                    when=move || error.get().is_none()
                    fallback=move || {
                        view! {
                            <div class="mb-4">
                                <svg
                                    class="w-16 h-16 mx-auto text-red-500"
                                    fill="none"
                                    stroke="currentColor"
                                    viewBox="0 0 24 24"
                                >
                                    <path
                                        stroke-linecap="round"
                                        stroke-linejoin="round"
                                        stroke-width="2"
                                        d="M12 8v4m0 4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z"
                                    />
                                </svg>
                            </div>
                            <h1 class="text-3xl font-bold text-red-600 mb-4">"Error"</h1>
                            <p class="text-gray-700">{move || error.get().unwrap_or_default()}</p>
                        }
                    }
                >
                    <div class="mb-4">
                        <svg
                            class="w-16 h-16 mx-auto text-blue-500 animate-spin"
                            fill="none"
                            stroke="currentColor"
                            viewBox="0 0 24 24"
                        >
                            <path
                                stroke-linecap="round"
                                stroke-linejoin="round"
                                stroke-width="2"
                                d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15"
                            />
                        </svg>
                    </div>
                    <h1 class="text-3xl font-bold text-gray-800 mb-4">
                        "Authenticating CLI..."
                    </h1>
                    <p class="text-gray-600 mb-2">
                        "Generating your authentication token."
                    </p>
                    <p class="text-sm text-gray-500">
                        "Please wait, redirecting to success page..."
                    </p>
                </Show>
            </div>
        </div>
    }
}

async fn generate_token(session_id: Uuid, device_name: String) -> Result<(), String> {
    let req = GenerateTokenRequest {
        session_id,
        device_name,
    };

    let _response: GenerateTokenResponse = Request::post("/api/v1/auth/cli/generate-token")
        .json(&req)
        .map_err(|e| format!("Failed to serialize request: {}", e))?
        .send()
        .await
        .map_err(|e| format!("Failed to send request: {}", e))?
        .json()
        .await
        .map_err(|e| format!("Failed to parse response: {}", e))?;

    Ok(())
}
