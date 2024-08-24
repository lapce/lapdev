use std::time::Duration;

use anyhow::Result;
use gloo_net::http::Request;
use lapdev_common::{console::NewSessionResponse, AuthProvider, GitProvider};
use leptos::{
    component, create_action, create_local_resource, create_rw_signal, document,
    event_target_checked, set_timeout, view, window, For, IntoView, RwSignal, Signal, SignalGet,
    SignalGetUntracked, SignalSet, SignalUpdate,
};
use leptos_router::use_location;
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use web_sys::FocusEvent;

use crate::modal::{CreationModal, ErrorResponse};

async fn get_git_providers() -> Result<Vec<GitProvider>> {
    let resp = Request::get("/api/v1/account/git_providers").send().await?;
    let machine_types: Vec<GitProvider> = resp.json().await?;
    Ok(machine_types)
}

async fn connect_oauth(
    provider: AuthProvider,
    update_read_repo: Option<bool>,
) -> Result<(), ErrorResponse> {
    let location = use_location();
    let next = format!(
        "{}{}",
        location.pathname.get_untracked(),
        location.search.get_untracked()
    );
    let location = window().window().location();
    let url = if update_read_repo.is_some() {
        "/api/v1/account/git_providers/update_scope"
    } else {
        "/api/v1/account/git_providers/connect"
    };

    let read_repo = if update_read_repo.unwrap_or(false) {
        "yes".to_string()
    } else {
        "no".to_string()
    };

    let resp = Request::put(url)
        .query([
            ("provider", &provider.to_string()),
            ("next", &next),
            ("host", &location.origin().unwrap_or_default()),
            ("read_repo", &read_repo),
        ])
        .send()
        .await?;

    if resp.status() != 200 {
        let error = resp
            .json::<ErrorResponse>()
            .await
            .unwrap_or_else(|_| ErrorResponse {
                error: "Internal Server Error".to_string(),
            });
        return Err(error);
    }

    let resp: NewSessionResponse = resp.json().await?;
    let _ = window().location().set_href(&resp.url);

    Ok(())
}

async fn disconnect_oauth(provider: AuthProvider) -> Result<(), ErrorResponse> {
    let location = use_location();
    let next = format!(
        "{}{}{}",
        window().location().origin().unwrap_or_default(),
        location.pathname.get_untracked(),
        location.search.get_untracked(),
    );

    let resp = Request::put("/api/v1/account/git_providers/disconnect")
        .query([("provider", &provider.to_string()), ("next", &next)])
        .send()
        .await?;

    if resp.status() != 200 {
        let error = resp
            .json::<ErrorResponse>()
            .await
            .unwrap_or_else(|_| ErrorResponse {
                error: "Internal Server Error".to_string(),
            });
        return Err(error);
    }

    Ok(())
}

#[component]
pub fn GitProviderView() -> impl IntoView {
    let error = create_rw_signal(None);

    let git_provider_counter = create_rw_signal(0);
    let git_providers = create_local_resource(
        move || git_provider_counter.get(),
        |_| async move { get_git_providers().await.unwrap_or_default() },
    );
    let git_providers = Signal::derive(move || git_providers.get().unwrap_or_default());

    view! {
        <section class="w-full h-full flex flex-col">
            <div class="border-b pb-4">
                <div class="flex items-end justify-between">
                    <div class="min-w-0 mr-4">
                        <h5 class="mr-3 text-2xl font-semibold dark:text-white">
                            Git Providers
                        </h5>
                        <p class="text-gray-700 dark:text-gray-400">{"Connect to a git provider or manage your permissions of a git provider."}</p>
                    </div>
                </div>
            </div>

            { move || if let Some(error) = error.get() {
                view! {
                    <div class="w-full mt-8 p-4 rounded-lg bg-red-50 dark:bg-gray-800 ">
                        <span class="text-sm font-medium text-red-800 dark:text-red-400">{ error }</span>
                    </div>
                }.into_view()
            } else {
                view!{}.into_view()
            }}

            <div class="my-8 flex flex-col gap-y-6">
                <For
                    each=move || git_providers.get()
                    key=|m| m.clone()
                    children=move |git_provider| {
                        view! {
                            <GitProviderItem provider=git_provider error git_provider_counter />
                        }
                    }
                />
            </div>

        </section>
    }
}

#[component]
fn GitProviderControl(
    provider: AuthProvider,
    error: RwSignal<Option<String>>,
    git_provider_counter: RwSignal<i32>,
    update_scope_modal_hidden: RwSignal<bool>,
) -> impl IntoView {
    let dropdown_hidden = create_rw_signal(true);
    let toggle_dropdown = move |_| {
        if dropdown_hidden.get_untracked() {
            dropdown_hidden.set(false);
        } else {
            dropdown_hidden.set(true);
        }
    };

    let connect_action = create_action(move |_| async move {
        if let Err(e) = connect_oauth(provider, None).await {
            error.set(Some(e.error));
        }
    });
    let connect = move |_| {
        dropdown_hidden.set(true);
        error.set(None);
        connect_action.dispatch(());
    };

    let disconnect_action = create_action(move |_| async move {
        if let Err(e) = disconnect_oauth(provider).await {
            error.set(Some(e.error));
        } else {
            git_provider_counter.update(|c| {
                *c += 1;
            });
        }
    });
    let disconnect = move |_| {
        dropdown_hidden.set(true);
        error.set(None);
        disconnect_action.dispatch(());
    };

    let on_focusout = move |e: FocusEvent| {
        let node = e
            .current_target()
            .unwrap_throw()
            .unchecked_into::<web_sys::HtmlElement>();

        set_timeout(
            move || {
                let has_focus = if let Some(active) = document().active_element() {
                    let active: web_sys::Node = active.into();
                    node.contains(Some(&active))
                } else {
                    false
                };
                if !has_focus && !dropdown_hidden.get_untracked() {
                    dropdown_hidden.set(true);
                }
            },
            Duration::from_secs(0),
        );
    };

    view! {
        <div class="flex flex-row items-center">
            // <OpenWorkspaceView workspace_name workspace_status workspace_folder workspace_hostname align_right />
            <div
                class="ml-2 relative"
                on:focusout=on_focusout
            >
                <button
                    class="hover:bg-gray-100 focus:outline-none font-medium rounded-lg text-sm px-2.5 py-2.5 text-center inline-flex items-center dark:bg-blue-600 dark:hover:bg-blue-700 dark:focus:ring-blue-800"
                    type="button"
                    on:click=toggle_dropdown
                >
                    <svg class="w-4 h-4 text-white" viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg">
                        <path d="M9.5 13a1.5 1.5 0 1 1-3 0 1.5 1.5 0 0 1 3 0zm0-5a1.5 1.5 0 1 1-3 0 1.5 1.5 0 0 1 3 0zm0-5a1.5 1.5 0 1 1-3 0 1.5 1.5 0 0 1 3 0z"/>
                    </svg>
                </button>
                <div
                    class="absolute pt-2 z-10 divide-y divide-gray-100 right-0"
                    class:hidden=move || dropdown_hidden.get()
                >
                    <ul class="py-2 text-sm text-gray-700 dark:text-gray-200 bg-white rounded-lg border shadow w-44 dark:bg-gray-700">
                    <li>
                        <a
                            href="#"
                            class="block px-4 py-2 hover:bg-gray-100 dark:hover:bg-gray-600 dark:hover:text-white"
                            on:click=connect
                            // class:hidden=move || workspace_status != WorkspaceStatus::Running && workspace_status != WorkspaceStatus::Stopping
                        >
                            Connect
                        </a>
                    </li>
                    <li>
                        <a
                            href="#"
                            class="block px-4 py-2 hover:bg-gray-100 dark:hover:bg-gray-600 dark:hover:text-white"
                            on:click=disconnect
                        >
                            Disconnect
                        </a>
                    </li>
                    <li>
                        <a
                            href="#"
                            class="block px-4 py-2 hover:bg-gray-100 dark:hover:bg-gray-600 text-red-700"
                            on:click=move |_| {
                                dropdown_hidden.set(true);
                                update_scope_modal_hidden.set(false);
                            }
                        >
                            Update Permission
                        </a>
                    </li>
                    </ul>
                </div>
            </div>
        </div>
    }
}

#[component]
fn GitProviderItem(
    provider: GitProvider,
    error: RwSignal<Option<String>>,
    git_provider_counter: RwSignal<i32>,
) -> impl IntoView {
    let update_scope_modal_hidden = create_rw_signal(true);

    let icon = match provider.auth_provider {
        AuthProvider::Github => view! {
            <svg class="w-4 h-4 me-2" aria-hidden="true" xmlns="http://www.w3.org/2000/svg" fill="currentColor" viewBox="0 0 20 20">
                <path fill-rule="evenodd" d="M10 .333A9.911 9.911 0 0 0 6.866 19.65c.5.092.678-.215.678-.477 0-.237-.01-1.017-.014-1.845-2.757.6-3.338-1.169-3.338-1.169a2.627 2.627 0 0 0-1.1-1.451c-.9-.615.07-.6.07-.6a2.084 2.084 0 0 1 1.518 1.021 2.11 2.11 0 0 0 2.884.823c.044-.503.268-.973.63-1.325-2.2-.25-4.516-1.1-4.516-4.9A3.832 3.832 0 0 1 4.7 7.068a3.56 3.56 0 0 1 .095-2.623s.832-.266 2.726 1.016a9.409 9.409 0 0 1 4.962 0c1.89-1.282 2.717-1.016 2.717-1.016.366.83.402 1.768.1 2.623a3.827 3.827 0 0 1 1.02 2.659c0 3.807-2.319 4.644-4.525 4.889a2.366 2.366 0 0 1 .673 1.834c0 1.326-.012 2.394-.012 2.72 0 .263.18.572.681.475A9.911 9.911 0 0 0 10 .333Z" clip-rule="evenodd"/>
            </svg>
        },
        AuthProvider::Gitlab => view! {
            <svg class="w-4 h-4 me-2" viewBox="0 0 25 24" xmlns="http://www.w3.org/2000/svg"><path d="M24.507 9.5l-.034-.09L21.082.562a.896.896 0 00-1.694.091l-2.29 7.01H7.825L5.535.653a.898.898 0 00-1.694-.09L.451 9.411.416 9.5a6.297 6.297 0 002.09 7.278l.012.01.03.022 5.16 3.867 2.56 1.935 1.554 1.176a1.051 1.051 0 001.268 0l1.555-1.176 2.56-1.935 5.197-3.89.014-.01A6.297 6.297 0 0024.507 9.5z" fill="#E24329"/><path d="M24.507 9.5l-.034-.09a11.44 11.44 0 00-4.56 2.051l-7.447 5.632 4.742 3.584 5.197-3.89.014-.01A6.297 6.297 0 0024.507 9.5z" fill="#FC6D26"/><path d="M7.707 20.677l2.56 1.935 1.555 1.176a1.051 1.051 0 001.268 0l1.555-1.176 2.56-1.935-4.743-3.584-4.755 3.584z" fill="#FCA326"/><path d="M5.01 11.461a11.43 11.43 0 00-4.56-2.05L.416 9.5a6.297 6.297 0 002.09 7.278l.012.01.03.022 5.16 3.867 4.745-3.584-7.444-5.632z" fill="#FC6D26"/></svg>
        },
    };

    view! {
        <div class="border rounded-xl px-8 py-4 flex flex-row items-center">
            <div class="w-1/3 flex flex-row items-center">
                <div class="w-1/4 flex flex-row items-center">
                    {icon}
                    <p>{provider.auth_provider.to_string()}</p>
                </div>
            </div>
            <div class="w-1/3">
                <div class="flex flex-row items-center"
                    class:hidden={!provider.connected}
                >
                    <img
                        class="w-8 h-8 rounded-full mr-2"
                        src={ provider.avatar_url.clone() }
                        alt="user photo"
                    />
                    <div class="flex flex-col">
                        <p>{provider.name}</p>
                        <p>{provider.email}</p>
                    </div>
                </div>
                <p
                    class:hidden={provider.connected}
                >
                    Not Connected
                </p>
            </div>
            <div class="w-1/3 flex flex-row items-center">
                <div class="w-5/6 flex flex-col">
                    <div
                        class:hidden={!provider.connected}
                    >
                        <p class="text-gray-500">
                            Permissions
                        </p>
                        <p>
                        { if provider.read_repo == Some(true) { provider.all_scopes.join(", ") } else { provider.scopes.join(", ") } }
                        </p>
                    </div>
                </div>
                <div class="w-1/6">
                    <GitProviderControl
                        provider=provider.auth_provider
                        error
                        git_provider_counter
                        update_scope_modal_hidden
                    />
                </div>
            </div>
        </div>
        <UpdateScopeModal
            provider=provider.auth_provider
            read_repo=provider.read_repo.unwrap_or(false)
            update_scope_modal_hidden
        />
    }
}

#[component]
pub fn UpdateScopeModal(
    provider: AuthProvider,
    read_repo: bool,
    update_scope_modal_hidden: RwSignal<bool>,
) -> impl IntoView {
    let read_repo = create_rw_signal(read_repo);

    let body = view! {
        <div class="block mb-2 text-sm font-medium text-gray-900 dark:text-white">
            Turn this option on if you want to open private repo in Lapdev
        </div>
        <div
            class="flex items-start"
        >
            <div class="flex items-center h-5">
            <input
                id={format!("read_repo_{provider}")}
                type="checkbox"
                value=""
                class="w-4 h-4 border border-gray-300 rounded bg-gray-50 focus:ring-3 focus:ring-blue-300 dark:bg-gray-700 dark:border-gray-600 dark:focus:ring-blue-600 dark:ring-offset-gray-800"
                prop:checked=move || read_repo.get()
                on:change=move |e| read_repo.set(event_target_checked(&e))
            />
            </div>
            <label for={format!("read_repo_{provider}")} class="ml-2 text-sm font-medium text-gray-900 dark:text-gray-300">Can read private repo</label>
        </div>
    };

    let action = create_action(move |_| connect_oauth(provider, Some(read_repo.get_untracked())));
    view! {
        <CreationModal
            title="Update Permission".to_string()
            modal_hidden=update_scope_modal_hidden
            action
            body
            update_text=None
            updating_text=None
            create_button_hidden=false
        />
    }
}
