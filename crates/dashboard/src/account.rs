use anyhow::Result;
use gloo_net::http::Request;
use lapdev_common::{
    console::{MeUser, NewSessionResponse},
    AuthProvider,
};
use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

use crate::{
    cluster::{get_cluster_info, OauthSettings},
    modal::ErrorResponse,
};

pub async fn get_login() -> Result<MeUser> {
    let resp: MeUser = Request::get("/api/private/me").send().await?.json().await?;
    Ok(resp)
}

async fn now_login(provider: AuthProvider) -> Result<()> {
    let location = window().window().location();
    let next = location.href().unwrap_or_default();
    let next = urlencoding::encode(&next).to_string();
    let resp = Request::get("/api/private/session")
        .query([
            ("provider", &provider.to_string()),
            ("next", &next),
            ("host", &location.origin().unwrap_or_default()),
        ])
        .send()
        .await?;
    let resp: NewSessionResponse = resp.json().await?;
    let _ = window().location().set_href(&resp.url);

    Ok(())
}

async fn now_logout() {
    let _ = Request::delete("/api/private/session").send().await;
    let _ = window().location().set_href("/");
}

#[component]
pub fn Login() -> impl IntoView {
    let cluster_info = get_cluster_info();

    view! {
        <section class="min-h-screen bg-gray-50 text-slate-900">
            <div class="mx-auto flex min-h-screen w-full flex-col lg:flex-row">
                <div class="order-2 flex w-full items-center bg-gray-50 px-8 py-16 sm:px-12 lg:order-1 lg:w-1/2">
                    <div class="mx-auto w-full max-w-xl space-y-6">
                        <a href="#" class="inline-flex items-center gap-4 text-lg font-semibold text-slate-900">
                            <svg class="h-11 w-11 text-black" xmlns="http://www.w3.org/2000/svg" fill="currentColor" viewBox="0 0 1024 1024">
                                <path d="M512 0C390.759 0 279.426 42.2227 191.719 112.656L191.719 612L351.719 612L351.719 332L422 332L431.719 332L431.719 332.344C488.169 334.127 541.249 351.138 581.875 380.625C627.591 413.806 654.875 460.633 654.875 512C654.875 563.367 627.591 610.194 581.875 643.375C541.249 672.862 488.169 689.873 431.719 691.656L431.719 1017.69C457.88 1021.81 484.68 1024 512 1024C794.77 1024 1024 794.77 1024 512C1024 229.23 794.77 0 512 0ZM111.719 192.906C41.8539 280.432 0 391.303 0 512C0 738.766 147.487 930.96 351.719 998.25L351.719 692L151.719 692L151.719 691.844L111.719 691.844L111.719 192.906ZM738.219 372C741.597 372.107 745.042 373.02 748.312 374.812L946.281 483.312C952.311 486.616 956.692 492.393 959.188 499.156C959.821 500.874 960.317 502.641 960.688 504.469C960.764 504.834 960.841 505.196 960.906 505.562C961.12 506.807 961.225 508.071 961.312 509.344C961.378 510.235 961.498 511.112 961.5 512C961.498 512.888 961.378 513.765 961.312 514.656C961.226 515.929 961.12 517.193 960.906 518.438C960.841 518.804 960.764 519.166 960.688 519.531C960.317 521.359 959.821 523.126 959.188 524.844C956.692 531.608 952.311 537.384 946.281 540.688L748.312 649.188C735.232 656.355 719.818 649.367 713.875 633.594C707.932 617.82 713.7 599.23 726.781 592.062L872.875 512L726.781 431.938C713.7 424.771 707.932 406.18 713.875 390.406C718.332 378.576 728.085 371.678 738.219 372ZM431.719 412.344L431.719 611.656C513.56 608.208 574.875 561.985 574.875 512C574.875 462.015 513.56 415.792 431.719 412.344Z" />
                                <path d="M742 403.688C740.062 403.483 738.097 404.438 737.094 406.25C735.756 408.666 736.615 411.694 739.031 413.031L925.719 516.375C928.135 517.712 931.194 516.822 932.531 514.406C933.869 511.99 932.979 508.962 930.562 507.625L743.875 404.281C743.271 403.947 742.646 403.756 742 403.688Z" />
                                <path d="M927.5 507.031C926.856 507.115 926.221 507.339 925.625 507.688L738.938 616.906C736.554 618.301 735.762 621.335 737.156 623.719C738.551 626.102 741.616 626.926 744 625.531L930.688 516.312C933.071 514.918 933.863 511.852 932.469 509.469C931.423 507.681 929.432 506.78 927.5 507.031Z" />
                            </svg>
                            Lapdev
                        </a>
                        <div class="space-y-5">
                            <h1 class="max-w-xl text-4xl font-semibold tracking-tight text-slate-900 sm:text-5xl">
                                Spin up production-grade Kubernetes environments in minutes.
                            </h1>
                            <p class="max-w-lg text-lg text-slate-600">
                                Lapdev mirrors your live manifests, keeps every dev space in sync, and lets teams ship without wrestling YAML or waiting on pipelines.
                            </p>
                        </div>
                        <div class="flex flex-wrap items-center gap-3 text-sm text-slate-600">
                            <span class="inline-flex items-center gap-2 rounded-full border border-slate-200 bg-white px-3 py-2">
                                <svg class="h-4 w-4 text-emerald-500" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M4.5 12.75l6 6 9-13.5" />
                                </svg>
                                One-click replicas of prod workloads, config, and secrets.
                            </span>
                            <span class="inline-flex items-center gap-2 rounded-full border border-slate-200 bg-white px-3 py-2">
                                <svg class="h-4 w-4 text-sky-500" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M12 6v12m6-6H6" />
                                </svg>
                                Devbox intercepts traffic for true local debugging.
                            </span>
                            <span class="inline-flex items-center gap-2 rounded-full border border-slate-200 bg-white px-3 py-2">
                                <svg class="h-4 w-4 text-indigo-500" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M3 7.5l9-5.25 9 5.25M4.5 10.5v6.75L12 21l7.5-3.75V10.5" />
                                </svg>
                                Instant preview URLs with SSO-gated sharing.
                            </span>
                        </div>
                    </div>
                </div>
                <div class="order-1 flex w-full items-center bg-white px-8 py-16 shadow sm:px-12 lg:order-2 lg:w-1/2 lg:px-16">
                    <div class="mx-auto w-full max-w-md">
                        {move || match cluster_info.with(|i| i.as_ref().map(|i| i.auth_providers.clone())) {
                            Some(auth_providers) if !auth_providers.is_empty() => view! { <LoginWithView auth_providers /> }.into_any(),
                            Some(_) => view! {
                                <div class="space-y-4">
                                    <h2 class="text-2xl font-semibold text-slate-900">
                                        Configure your identity provider
                                    </h2>
                                    <p class="text-sm text-slate-600">
                                        Connect GitHub or GitLab to unlock Lapdev SSO for your organization.
                                    </p>
                                    <div class="rounded-2xl border border-slate-200 bg-gray-50 p-6">
                                        <InitAuthProvidersView />
                                    </div>
                                </div>
                            }.into_any(),
                            None => view! {
                                <div class="space-y-4 animate-pulse" aria-live="polite">
                                    <div class="h-4 w-40 rounded bg-slate-200"></div>
                                    <div class="h-10 rounded bg-slate-100"></div>
                                    <div class="h-10 rounded bg-slate-100"></div>
                                    <div class="h-10 w-2/3 rounded bg-slate-100"></div>
                                    <div class="h-4 w-48 rounded bg-slate-200"></div>
                                    <p class="pt-2 text-sm text-slate-500">
                                        Loading cluster SSO providers...
                                    </p>
                                </div>
                            }.into_any(),
                        }}
                    </div>
                </div>
            </div>
        </section>
    }
}

#[component]
pub fn LoginWithView(auth_providers: Vec<AuthProvider>) -> impl IntoView {
    let login = use_context::<LocalResource<Option<MeUser>>>().unwrap();
    view! {
        <div
            class="mx-auto w-full max-w-md"
            class:hidden=move || login.with(|l| l.is_none())
        >
            <div class="p-10 sm:p-12">
                <div class="relative">
                    <h1 class="text-center text-2xl font-semibold leading-tight tracking-tight text-slate-900">
                        Sign in to Lapdev
                    </h1>
                    <p class="mt-2 text-center text-sm text-slate-600">
                        Choose your identity provider to launch your personal or branch workspace.
                    </p>
                    <div class="mt-8 flex flex-col space-y-3">
                        <button type="button"
                            class="group relative flex w-full items-center justify-center gap-3 rounded-xl bg-[#24292F] px-5 py-3 text-sm font-semibold text-white transition hover:bg-[#1f2328] focus:outline-none focus:ring-2 focus:ring-sky-400 focus:ring-offset-2 focus:ring-offset-white"
                            class:hidden={ let auth_providers = auth_providers.clone(); move || !auth_providers.contains(&AuthProvider::Github) }
                            on:click=move |_| { Action::new_local(move |_| {now_login(AuthProvider::Github)}).dispatch(()); }
                        >
                            <svg class="h-5 w-5" aria-hidden="true" xmlns="http://www.w3.org/2000/svg" fill="currentColor" viewBox="0 0 20 20">
                                <path fill-rule="evenodd" d="M10 .333A9.911 9.911 0 0 0 6.866 19.65c.5.092.678-.215.678-.477 0-.237-.01-1.017-.014-1.845-2.757.6-3.338-1.169-3.338-1.169a2.627 2.627 0 0 0-1.1-1.451c-.9-.615.07-.6.07-.6a2.084 2.084 0 0 1 1.518 1.021 2.11 2.11 0 0 0 2.884.823c.044-.503.268-.973.63-1.325-2.2-.25-4.516-1.1-4.516-4.9A3.832 3.832 0 0 1 4.7 7.068a3.56 3.56 0 0 1 .095-2.623s.832-.266 2.726 1.016a9.409 9.409 0 0 1 4.962 0c1.89-1.282 2.717-1.016 2.717-1.016.366.83.402 1.768.1 2.623a3.827 3.827 0 0 1 1.02 2.659c0 3.807-2.319 4.644-4.525 4.889a2.366 2.366 0 0 1 .673 1.834c0 1.326-.012 2.394-.012 2.72 0 .263.18.572.681.475A9.911 9.911 0 0 0 10 .333Z" clip-rule="evenodd"/>
                            </svg>
                            Sign in with GitHub
                            <span class="pointer-events-none absolute inset-y-0 right-0 hidden items-center pr-5 text-slate-200/60 transition group-hover:flex">
                                <svg class="h-4 w-4" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M13.5 4.5L21 12l-7.5 7.5M21 12H3" />
                                </svg>
                            </span>
                        </button>

                        <button type="button"
                            class="group relative flex w-full items-center justify-center gap-3 rounded-xl bg-[#FC6D26] px-5 py-3 text-sm font-semibold text-white transition hover:bg-[#e75c1a] focus:outline-none focus:ring-2 focus:ring-amber-300 focus:ring-offset-2 focus:ring-offset-white"
                            class:hidden=move || !auth_providers.contains(&AuthProvider::Gitlab)
                            on:click=move |_| { Action::new_local(move |_| {now_login(AuthProvider::Gitlab)}).dispatch(()); }
                        >
                            <svg class="h-5 w-5" viewBox="0 0 25 24" xmlns="http://www.w3.org/2000/svg"><path d="M24.507 9.5l-.034-.09L21.082.562a.896.896 0 00-1.694.091l-2.29 7.01H7.825L5.535.653a.898.898 0 00-1.694-.09L.451 9.411.416 9.5a6.297 6.297 0 002.09 7.278l.012.01.03.022 5.16 3.867 2.56 1.935 1.554 1.176a1.051 1.051 0 001.268 0l1.555-1.176 2.56-1.935 5.197-3.89.014-.01A6.297 6.297 0 0024.507 9.5z" fill="#E24329"/><path d="M24.507 9.5l-.034-.09a11.44 11.44 0 00-4.56 2.051l-7.447 5.632 4.742 3.584 5.197-3.89.014-.01A6.297 6.297 0 0024.507 9.5z" fill="#FC6D26"/><path d="M7.707 20.677l2.56 1.935 1.555 1.176a1.051 1.051 0 001.268 0l1.555-1.176 2.56-1.935-4.743-3.584-4.755 3.584z" fill="#FCA326"/><path d="M5.01 11.461a11.43 11.43 0 00-4.56-2.05L.416 9.5a6.297 6.297 0 002.09 7.278l.012.01.03.022 5.16 3.867 4.745-3.584-7.444-5.632z" fill="#FC6D26"/></svg>
                            Sign in with GitLab
                            <span class="pointer-events-none absolute inset-y-0 right-0 hidden items-center pr-5 text-white/70 transition group-hover:flex">
                                <svg class="h-4 w-4" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M13.5 4.5L21 12l-7.5 7.5M21 12H3" />
                                </svg>
                            </span>
                        </button>
                    </div>
                </div>
            </div>
        </div>
    }
}

#[component]
pub fn InitAuthProvidersView() -> impl IntoView {
    view! {
        <OauthSettings reload=true />
    }
}

#[component]
pub fn NavUserControl(user_control_hidden: RwSignal<bool, LocalStorage>) -> impl IntoView {
    let login = use_context::<LocalResource<Option<MeUser>>>().unwrap();

    let logout = move |_| {
        user_control_hidden.set(true);
        Action::new_local(move |_| now_logout()).dispatch(());
    };

    view! {
      <div
        class="absolute z-50 my-2 right-0 w-56 text-base list-none bg-white rounded divide-y divide-gray-100 shadow border rounded-xl"
        class:hidden=move || user_control_hidden.get()
      >
        <div class="py-3 px-4">
          <span class="block text-sm font-semibold text-gray-900">
            { move || login.get().flatten().and_then(|l| l.name.clone()).unwrap_or("".to_string()) }
          </span>
          <span class="block text-sm text-gray-900 truncate">
            { move || login.get().flatten().and_then(|l| l.email.clone()).unwrap_or("".to_string()) }
          </span>
        </div>
        <ul
          class="py-1 text-gray-700"
          aria-labelledby="dropdown"
        >
          <li>
            <a
              href="/account"
              class="block py-2 px-4 text-sm hover:bg-gray-100"
              >User settings</a
            >
          </li>
        </ul>
        <ul
          class="py-1 text-gray-700"
          aria-labelledby="dropdown"
        >
          <li>
            <a
              href="#"
              class="block py-2 px-4 text-sm hover:bg-gray-100"
              on:click=logout
              >Sign out</a
            >
          </li>
        </ul>
      </div>
    }
}

#[component]
pub fn AccountSettings() -> impl IntoView {
    view! {
        <div class="border-b pb-4">
            <h5 class="mr-3 text-2xl font-semibold">
                User Settings
            </h5>
            <p class="text-gray-700">{"Manage your account settings"}</p>
        </div>
    }
}

async fn join_org(invitation_id: String) -> Result<Option<ErrorResponse>> {
    let resp = Request::put(&format!("/api/v1/join/{invitation_id}"))
        .send()
        .await?;
    if resp.status() != 204 {
        let error = resp
            .json::<ErrorResponse>()
            .await
            .unwrap_or_else(|_| ErrorResponse {
                error: "Internal Server Error".to_string(),
            });
        return Ok(Some(error));
    }
    let _ = window().location().set_href("/");
    Ok(None)
}

#[component]
pub fn JoinView() -> impl IntoView {
    let params = use_params_map();
    let id = Signal::derive(move || params.with(|params| params.get("id")).unwrap_or_default());

    let result = { LocalResource::new(move || async move { join_org(id.get_untracked()).await }) };

    view! {
        {
            move || if let Some(error) = result.with(|r| r.as_ref().and_then(|r| r.as_ref().ok().cloned())).flatten() {
                view! {
                    <div class="my-2 p-4 rounded-lg bg-red-50">
                        <span class="text-sm font-medium text-red-800">{ error.error }</span>
                    </div>
                }.into_any()
            } else {
                ().into_any()
            }
        }
    }
}
