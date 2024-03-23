use std::{str::FromStr, time::Duration};

use anyhow::{anyhow, Result};
use gloo_net::http::Request;
use lapdev_common::{
    console::{MeUser, Organization, OrganizationMember},
    ClusterInfo, NewOrganization, UpdateOrganizationAutoStartStop, UpdateOrganizationMember,
    UpdateOrganizationName, UserRole,
};
use leptos::{
    component, create_action, create_effect, create_local_resource, create_rw_signal, document,
    event_target_checked, event_target_value, expect_context, leptos_dom::logging::console_log,
    set_timeout, use_context, view, window, Action, For, IntoView, Resource, RwSignal, Signal,
    SignalGet, SignalGetUntracked, SignalSet, SignalUpdate, SignalWith,
};
use uuid::Uuid;
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use web_sys::FocusEvent;

use crate::modal::{CreationModal, DatetimeModal, DeletionModal, ErrorResponse, SettingView};

async fn create_org(name: RwSignal<String>) -> Result<()> {
    let org_name = name.get_untracked();
    let resp = Request::post("/api/v1/organizations")
        .json(&NewOrganization { name: org_name })?
        .send()
        .await?;
    if resp.status() != 200 {
        return Err(anyhow!("can't create organization"));
    }
    let _resp: Organization = resp.json().await?;

    let _ = window().location().set_href("/");

    Ok(())
}

async fn switch_org(org_id: String) -> Result<()> {
    let _ = Request::put(&format!("/api/private/me/organization/{org_id}"))
        .send()
        .await;
    let _ = window().location().set_href("/");
    Ok(())
}

#[component]
pub fn OrgSelector(new_org_modal_hidden: RwSignal<bool>) -> impl IntoView {
    let login = use_context::<Resource<i32, Option<MeUser>>>().unwrap();
    let hidden = create_rw_signal(true);
    let toggle_dropdown = move |_| {
        if hidden.get_untracked() {
            hidden.set(false);
        } else {
            hidden.set(true);
        }
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
                if !has_focus && !hidden.get_untracked() {
                    hidden.set(true);
                }
            },
            Duration::from_secs(0),
        );
    };

    view! {
        <div
            class="w-full mb-5"
            on:focusout=on_focusout
        >
        <button
            class="flex items-center justify-between w-full border rounded-xl px-5"
            type="button"
            on:click=toggle_dropdown
        >
            <div class="flex items-center grow basis-0 min-w-0">
                <div class="rounded-full flex items-center justify-center flex-shrink-0 w-10 h-10 bg-gradient-to-tr from-sky-500 to-indigo-500 from-30% to-70% mr-2">
                    <span class="text-white font-semibold text-xl">{ move || login.get().flatten().and_then(|l| l.organization.name.chars().next()).unwrap_or('P') }</span>
                </div>
                <span class="py-4 truncate">{ move || login.get().flatten().map(|l| l.organization.name).unwrap_or_else(|| "Personal".to_string()) }</span>
            </div>
            <svg class="w-5 h-5" fill="currentColor" viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg"><path fill-rule="evenodd" d="M10 3a1 1 0 01.707.293l3 3a1 1 0 01-1.414 1.414L10 5.414 7.707 7.707a1 1 0 01-1.414-1.414l3-3A1 1 0 0110 3zm-3.707 9.293a1 1 0 011.414 0L10 14.586l2.293-2.293a1 1 0 011.414 1.414l-3 3a1 1 0 01-1.414 0l-3-3a1 1 0 010-1.414z" clip-rule="evenodd"></path></svg>
        </button>
        <div
            class="relative"
            class:hidden=move || hidden.get()
        >
        <div
            class="absolute w-full mr-4 mt-2 z-10 bg-white divide-y divide-gray-100 rounded-xl border shadow w-full dark:bg-gray-700"
        >
            <ul class="py-2 text-sm text-gray-700 dark:text-gray-200">
                <For
                    each=move || login.get().flatten().map(|l| {
                        let mut orgs = l.all_organizations;
                        orgs.retain(|o| o.id != l.organization.id);
                        orgs
                    }).unwrap_or_default()
                    key=|o| o.id
                    children=move |org| {
                        view! {
                            <li>
                                <a href="#"
                                    class="flex items-center px-5 hover:bg-gray-100 dark:hover:bg-gray-600 dark:hover:text-white"
                                    on:click=move |_| {
                                        hidden.set(true);
                                        create_action(move |org_id: &String| switch_org(org_id.clone())).dispatch(org.id.to_string());
                                    }
                                >
                                    <div class="rounded-full flex items-center justify-center flex-shrink-0 w-10 h-10 bg-gradient-to-tr from-sky-500 to-indigo-500 from-30% to-70% mr-2">
                                        <span class="text-white font-semibold text-xl">{ org.name.chars().next().unwrap_or('O') }</span>
                                    </div>
                                    <span class="py-4 truncate">{org.name}</span>
                                </a>
                            </li>
                        }
                    }
                />
                <li>
                    <a href="#"
                        class="flex items-center justify-between px-5 py-4 hover:bg-gray-100 dark:hover:bg-gray-600 dark:hover:text-white"
                        on:click=move |_| { new_org_modal_hidden.set(false); hidden.set(true); }
                    >
                        Create New Organization
                        <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 14 14" class="w-3.5">
                            <path fill="currentColor" fill-rule="evenodd" d="M7 0a1 1 0 011 1v5h5a1 1 0 110 2H8v5a1 1 0 11-2 0V8H1a1 1 0 010-2h5V1a1 1 0 011-1z" clip-rule="evenodd" />
                        </svg>
                    </a>
                </li>
            </ul>
        </div>
        </div>
        </div>
    }
}

#[component]
pub fn NewOrgModal(modal_hidden: RwSignal<bool>) -> impl IntoView {
    let org_name = create_rw_signal("".to_string());
    let action = create_action(move |_| create_org(org_name));
    let handle_create_org = move |_| {
        action.dispatch(());
    };
    let create_pending = action.pending();
    view! {
        <div
            id="default-modal"
            tabindex="-1"
            aria-hidden="true"
            class="bg-gray-900/50 dark:bg-gray-900/80 flex overflow-y-auto overflow-x-hidden fixed top-0 right-0 left-0 z-50 justify-center items-center w-full h-full"
            class:hidden=move || modal_hidden.get()
            on:click=move |_| modal_hidden.set(true)
        >
            <div
                class="relative p-4 w-full max-w-2xl max-h-full"
                on:click=move |e| e.stop_propagation()
            >
                <div class="relative bg-white rounded-lg shadow dark:bg-gray-700">
                    <div class="flex items-center justify-between p-4 md:p-5 border-b rounded-t dark:border-gray-600">
                        <h3 class="text-xl font-semibold text-gray-900 dark:text-white">
                            Create New Organization
                        </h3>
                        <button
                            type="button"
                            class="text-gray-400 bg-transparent hover:bg-gray-200 hover:text-gray-900 rounded-lg text-sm w-8 h-8 ms-auto inline-flex justify-center items-center dark:hover:bg-gray-600 dark:hover:text-white" data-modal-hide="default-modal"
                            on:click=move |_| modal_hidden.set(true)
                        >
                            <svg class="w-3 h-3" aria-hidden="true" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 14 14">
                                <path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="m1 1 6 6m0 0 6 6M7 7l6-6M7 7l-6 6"/>
                            </svg>
                            <span class="sr-only">Close modal</span>
                        </button>
                    </div>
                    <div class="p-4 md:p-5 space-y-4">
                        <div>
                            <label class="block mb-2 text-sm font-medium text-gray-900 dark:text-white">Your Organization Name</label>
                            <input
                                prop:value={move || org_name.get()}
                                on:input=move |ev| { org_name.set(event_target_value(&ev)); }
                                class="bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5 dark:bg-gray-600 dark:border-gray-500 dark:placeholder-gray-400 dark:text-white"
                                required
                            />
                        </div>
                    </div>
                    <div class="flex items-center p-4 md:p-5 border-t border-gray-200 rounded-b dark:border-gray-600">
                        <button
                            type="button"
                            class="flex flex-row items-center text-white bg-blue-700 hover:bg-blue-800 focus:ring-4 focus:outline-none focus:ring-blue-300 font-medium rounded-lg text-sm px-5 py-2.5 text-center dark:bg-blue-600 dark:hover:bg-blue-700 dark:focus:ring-blue-800"
                            disabled=move || create_pending.get()
                            on:click=handle_create_org
                        >
                            <svg aria-hidden="true" role="status"
                                class="inline w-4 h-4 me-3 text-white animate-spin" viewBox="0 0 100 101" fill="none" xmlns="http://www.w3.org/2000/svg"
                                class:hidden=move || !create_pending.get()
                            >
                            <path d="M100 50.5908C100 78.2051 77.6142 100.591 50 100.591C22.3858 100.591 0 78.2051 0 50.5908C0 22.9766 22.3858 0.59082 50 0.59082C77.6142 0.59082 100 22.9766 100 50.5908ZM9.08144 50.5908C9.08144 73.1895 27.4013 91.5094 50 91.5094C72.5987 91.5094 90.9186 73.1895 90.9186 50.5908C90.9186 27.9921 72.5987 9.67226 50 9.67226C27.4013 9.67226 9.08144 27.9921 9.08144 50.5908Z" fill="#E5E7EB"/>
                            <path d="M93.9676 39.0409C96.393 38.4038 97.8624 35.9116 97.0079 33.5539C95.2932 28.8227 92.871 24.3692 89.8167 20.348C85.8452 15.1192 80.8826 10.7238 75.2124 7.41289C69.5422 4.10194 63.2754 1.94025 56.7698 1.05124C51.7666 0.367541 46.6976 0.446843 41.7345 1.27873C39.2613 1.69328 37.813 4.19778 38.4501 6.62326C39.0873 9.04874 41.5694 10.4717 44.0505 10.1071C47.8511 9.54855 51.7191 9.52689 55.5402 10.0491C60.8642 10.7766 65.9928 12.5457 70.6331 15.2552C75.2735 17.9648 79.3347 21.5619 82.5849 25.841C84.9175 28.9121 86.7997 32.2913 88.1811 35.8758C89.083 38.2158 91.5421 39.6781 93.9676 39.0409Z" fill="currentColor"/>
                            </svg>
                            { move || if create_pending.get() { "Creating" } else { "Create" } }
                        </button>
                        <button
                            type="button"
                            class="ms-3 text-gray-500 bg-white hover:bg-gray-100 focus:ring-4 focus:outline-none focus:ring-blue-300 rounded-lg border border-gray-200 text-sm font-medium px-5 py-2.5 hover:text-gray-900 focus:z-10 dark:bg-gray-700 dark:text-gray-300 dark:border-gray-500 dark:hover:text-white dark:hover:bg-gray-600 dark:focus:ring-gray-600"
                            disabled=move || create_pending.get()
                            on:click=move |_| modal_hidden.set(true)
                        >Cancel</button>
                    </div>
                </div>
            </div>
        </div>
    }
}

async fn delete_org() -> Result<Option<ErrorResponse>> {
    let org =
        use_context::<Signal<Option<Organization>>>().ok_or_else(|| anyhow!("can't get org"))?;
    let org = org
        .get_untracked()
        .ok_or_else(|| anyhow!("can't get org"))?;
    let resp = Request::delete(&format!("/api/v1/organizations/{}", org.id))
        .send()
        .await?;
    if resp.status() == 200 {
        let _ = window().location().set_href("/");
    } else {
        let error = resp
            .json::<ErrorResponse>()
            .await
            .unwrap_or_else(|_| ErrorResponse {
                error: "Internal Server Error".to_string(),
            });
        return Ok(Some(error));
    }
    Ok(None)
}

#[component]
pub fn OrgSettings() -> impl IntoView {
    let modal_hidden = create_rw_signal(true);
    let delete_action = create_action(|()| delete_org());
    let login = use_context::<Resource<i32, Option<MeUser>>>().unwrap();
    let cluster_info = expect_context::<Signal<Option<ClusterInfo>>>();
    view! {
        <div class="border-b pb-4 mb-8">
            <h5 class="mr-3 text-2xl font-semibold dark:text-white">
                Organization Settings
            </h5>
            <p class="text-gray-700 dark:text-gray-400">{"Manage your organization's settings"}</p>
        </div>
        <div class="mb-8">
            <div class="w-full p-8 border rounded-xl">
                <UpdateNameView />
            </div>
            <div
                class="mt-4 w-full p-8 border rounded-xl"
                class:hidden=move || !cluster_info.with(|i| i.as_ref().map(|i| i.has_enterprise)).unwrap_or(false)
            >
                <AutoStartStopView />
            </div>

            <div
                class="mt-4 w-full p-8 border rounded-xl"
                class:hidden=move || !login.with(|l| { l.as_ref() .and_then(|l| l.as_ref().map(|l| l.organization.role == UserRole::Owner)) .unwrap_or(false) })
            >
                <h5 class="text-lg font-semibold dark:text-white">
                    Delete Organization
                </h5>
                <button
                    type="button"
                    class="flex items-center justify-center mt-2 px-5 py-2.5 text-sm font-medium text-white rounded-lg bg-red-600 hover:bg-red-800 focus:ring-4 focus:ring-red-300 dark:bg-red-600 dark:hover:bg-red-700 focus:outline-none dark:focus:ring-red-800"
                    on:click=move |_| modal_hidden.set(false)
                >
                    Delete
                </button>
            </div>
        </div>
        <DeleteOrgModal modal_hidden delete_action />
    }
}

#[component]
pub fn DeleteOrgModal(
    modal_hidden: RwSignal<bool>,
    delete_action: Action<(), Result<Option<ErrorResponse>>>,
) -> impl IntoView {
    let error = create_rw_signal(None);
    let confirmation = create_rw_signal(String::new());
    let handle_delete = move |_| {
        delete_action.dispatch(());
    };
    let delete_pending = delete_action.pending();
    let org = use_context::<Signal<Option<Organization>>>().unwrap();
    create_effect(move |_| {
        delete_action.value().with(|result| {
            if let Some(result) = result {
                match result {
                    Ok(err) => {
                        if let Some(err) = err {
                            error.set(Some(err.error.to_string()));
                        }
                    }
                    Err(e) => {
                        console_log(&format!("creation error: {e}"));
                    }
                }
            }
        })
    });
    view! {
        <div tabindex="-1" class="bg-gray-900/50 dark:bg-gray-900/80 flex overflow-y-auto overflow-x-hidden fixed top-0 right-0 left-0 z-50 justify-center items-center w-full h-full"
            class:hidden=move || modal_hidden.get()
            on:click=move |_| modal_hidden.set(true)
        >
            <div
                class="relative p-4 w-full max-w-md max-h-full"
                on:click=move |e| e.stop_propagation()
            >
                <div class="relative bg-white rounded-lg shadow dark:bg-gray-700">
                    <button type="button"
                        class="absolute top-3 end-2.5 text-gray-400 bg-transparent hover:bg-gray-200 hover:text-gray-900 rounded-lg text-sm w-8 h-8 ms-auto inline-flex justify-center items-center dark:hover:bg-gray-600 dark:hover:text-white"
                        on:click=move |_| modal_hidden.set(true)
                    >
                        <svg class="w-3 h-3" aria-hidden="true" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 14 14">
                            <path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="m1 1 6 6m0 0 6 6M7 7l6-6M7 7l-6 6"/>
                        </svg>
                        <span class="sr-only">Close modal</span>
                    </button>
                    <div class="p-4 md:p-5 text-center text-gray-500 dark:text-gray-400">
                        <svg class="mx-auto mb-4 text-gray-400 w-12 h-12 dark:text-gray-200" aria-hidden="true" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 20 20">
                            <path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10 11V6m0 8h.01M19 10a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z"/>
                        </svg>
                        <h3 class="mb-5 text-lg font-normalflex flex-col items-center">
                            <p>Are you sure you want to delete organization</p>
                            <p class="text-bold text-gray-600">{ move || org.get().map(|org| org.name).unwrap_or_default() }</p>
                        </h3>
                        { move || if let Some(error) = error.get() {
                                view! {
                                    <div class="text-left p-4 mb-4 rounded-lg bg-red-50 dark:bg-gray-800 ">
                                        <span class="text-sm font-medium text-red-800 dark:text-red-400">{ error }</span>
                                    </div>
                                }.into_view()
                            } else {
                                view! {}.into_view()
                            }
                        }
                        <div class="text-left">
                        <p>{"1. You'll lose all the projects in this organization and this cannot be restored."}</p>
                        <p>{"2. All organization members will lose access to this organization and all the associated workspaces."}</p>
                        </div>
                        <p class="mt-4 text-left">
                        Type
                        <span class="text-semibold mx-1 px-1 text-gray-800 bg-gray-200 rounded">{ move || org.get().map(|org| org.name).unwrap_or_default() }</span>
                        To Confirm
                        </p>
                        <input
                            class="w-full border rounded py-1 px-1 my-2"
                            prop:value={move || confirmation.get()}
                            on:input=move |ev| { confirmation.set(event_target_value(&ev)); }
                        />
                        <button
                            type="button"
                            class="text-white bg-red-600 disabled:bg-red-400 hover:bg-red-800 focus:ring-4 focus:outline-none focus:ring-red-300 dark:focus:ring-red-800 font-medium rounded-lg text-sm inline-flex items-center px-5 py-2.5 text-center me-2"
                            on:click=handle_delete
                            disabled=move || delete_pending.get() || {
                                let confirmation = confirmation.get();
                                let org = org.get().map(|org| org.name).unwrap_or_default();
                                confirmation != org
                            }
                        >
                            <svg aria-hidden="true" role="status"
                                class="inline w-3 h-3 me-3 text-white animate-spin" viewBox="0 0 100 101" fill="none" xmlns="http://www.w3.org/2000/svg"
                                class:hidden=move || !delete_pending.get()
                            >
                            <path d="M100 50.5908C100 78.2051 77.6142 100.591 50 100.591C22.3858 100.591 0 78.2051 0 50.5908C0 22.9766 22.3858 0.59082 50 0.59082C77.6142 0.59082 100 22.9766 100 50.5908ZM9.08144 50.5908C9.08144 73.1895 27.4013 91.5094 50 91.5094C72.5987 91.5094 90.9186 73.1895 90.9186 50.5908C90.9186 27.9921 72.5987 9.67226 50 9.67226C27.4013 9.67226 9.08144 27.9921 9.08144 50.5908Z" fill="#E5E7EB"/>
                            <path d="M93.9676 39.0409C96.393 38.4038 97.8624 35.9116 97.0079 33.5539C95.2932 28.8227 92.871 24.3692 89.8167 20.348C85.8452 15.1192 80.8826 10.7238 75.2124 7.41289C69.5422 4.10194 63.2754 1.94025 56.7698 1.05124C51.7666 0.367541 46.6976 0.446843 41.7345 1.27873C39.2613 1.69328 37.813 4.19778 38.4501 6.62326C39.0873 9.04874 41.5694 10.4717 44.0505 10.1071C47.8511 9.54855 51.7191 9.52689 55.5402 10.0491C60.8642 10.7766 65.9928 12.5457 70.6331 15.2552C75.2735 17.9648 79.3347 21.5619 82.5849 25.841C84.9175 28.9121 86.7997 32.2913 88.1811 35.8758C89.083 38.2158 91.5421 39.6781 93.9676 39.0409Z" fill="currentColor"/>
                            </svg>
                            { move || if delete_pending.get() { "Deleting" } else { "Yes, I'm sure" } }
                        </button>
                        <button
                            type="button"
                            class="text-gray-500 bg-white hover:bg-gray-100 focus:ring-4 focus:outline-none focus:ring-gray-200 rounded-lg border border-gray-200 text-sm font-medium px-5 py-2.5 hover:text-gray-900 focus:z-10 dark:bg-gray-700 dark:text-gray-300 dark:border-gray-500 dark:hover:text-white dark:hover:bg-gray-600 dark:focus:ring-gray-600"
                            disabled=move || delete_pending.get()
                            on:click=move |_| modal_hidden.set(true)
                        >No, cancel</button>
                    </div>
                </div>
            </div>
        </div>
    }
}

async fn update_name(id: Uuid, name: String) -> Result<(), ErrorResponse> {
    let resp = Request::put(&format!("/api/v1/organizations/{id}/name"))
        .json(&UpdateOrganizationName { name })?
        .send()
        .await?;

    if resp.status() != 204 {
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

async fn update_auto_start_stop(
    id: Uuid,
    auto_start: bool,
    allow_workspace_change_auto_start: bool,
    auto_stop: Option<i32>,
    allow_workspace_change_auto_stop: bool,
) -> Result<(), ErrorResponse> {
    let resp = Request::put(&format!("/api/v1/organizations/{id}/auto_start_stop"))
        .json(&UpdateOrganizationAutoStartStop {
            auto_start,
            auto_stop,
            allow_workspace_change_auto_start,
            allow_workspace_change_auto_stop,
        })?
        .send()
        .await?;

    if resp.status() != 204 {
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
fn AutoStartStopView() -> impl IntoView {
    let org = use_context::<Signal<Option<Organization>>>().unwrap();
    let login_counter = expect_context::<RwSignal<i32>>();

    let auto_start_enabled = create_rw_signal(false);
    create_effect(move |_| {
        if let Some(enabled) = org.with(|o| o.as_ref().map(|o| o.auto_start)) {
            auto_start_enabled.set(enabled);
        }
    });

    let allow_workspace_change_auto_start = create_rw_signal(false);
    create_effect(move |_| {
        if let Some(enabled) = org.with(|o| o.as_ref().map(|o| o.allow_workspace_change_auto_start))
        {
            allow_workspace_change_auto_start.set(enabled);
        }
    });

    let auto_stop_enabled = create_rw_signal(false);
    create_effect(move |_| {
        if let Some(enabled) = org.with(|o| o.as_ref().map(|o| o.auto_stop.is_some())) {
            auto_stop_enabled.set(enabled);
        }
    });

    let auto_stop_seconds = create_rw_signal(3600);
    create_effect(move |_| {
        if let Some(seconds) = org.with(|o| o.as_ref().map(|o| o.auto_stop)).flatten() {
            auto_stop_seconds.set(seconds);
        }
    });

    let allow_workspace_change_auto_stop = create_rw_signal(false);
    create_effect(move |_| {
        if let Some(enabled) = org.with(|o| o.as_ref().map(|o| o.allow_workspace_change_auto_stop))
        {
            allow_workspace_change_auto_stop.set(enabled);
        }
    });

    let save_action = create_action(move |_| async move {
        if let Some(id) = org.with(|o| o.as_ref().map(|o| o.id)) {
            update_auto_start_stop(
                id,
                auto_start_enabled.get_untracked(),
                allow_workspace_change_auto_start.get_untracked(),
                if auto_stop_enabled.get_untracked() {
                    Some(auto_stop_seconds.get_untracked())
                } else {
                    None
                },
                allow_workspace_change_auto_stop.get_untracked(),
            )
            .await
        } else {
            Err(ErrorResponse {
                error: "Organization not loaded yet".to_string(),
            })
        }
    });

    let body = view! {
        <div class="mt-2">
            <label class="inline-flex items-center cursor-pointer">
                <input type="checkbox" value="" class="sr-only peer"
                    prop:checked=move || auto_start_enabled.get()
                    on:change=move |e| auto_start_enabled.set(event_target_checked(&e))
                />
                <div class="relative w-11 h-6 bg-gray-200 peer-focus:outline-none peer-focus:ring-4 peer-focus:ring-blue-300 dark:peer-focus:ring-blue-800 rounded-full peer dark:bg-gray-700 peer-checked:after:translate-x-full rtl:peer-checked:after:-translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:start-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-5 after:w-5 after:transition-all dark:border-gray-600 peer-checked:bg-blue-600"></div>
                <span class="ms-3 text-sm font-medium text-gray-900 dark:text-gray-300">Auto Start Enabled</span>
            </label>
        </div>
        <div class="mt-2">
            <label class="inline-flex items-center cursor-pointer">
                <input type="checkbox" value="" class="sr-only peer"
                    prop:checked=move || allow_workspace_change_auto_start.get()
                    on:change=move |e| allow_workspace_change_auto_start.set(event_target_checked(&e))
                />
                <div class="relative w-11 h-6 bg-gray-200 peer-focus:outline-none peer-focus:ring-4 peer-focus:ring-blue-300 dark:peer-focus:ring-blue-800 rounded-full peer dark:bg-gray-700 peer-checked:after:translate-x-full rtl:peer-checked:after:-translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:start-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-5 after:w-5 after:transition-all dark:border-gray-600 peer-checked:bg-blue-600"></div>
                <span class="ms-3 text-sm font-medium text-gray-900 dark:text-gray-300">Allow Changing Auto Start on Workspace</span>
            </label>
        </div>
        <div class="mt-2">
            <label class="inline-flex items-center cursor-pointer">
                <input type="checkbox" value="" class="sr-only peer"
                    prop:checked=move || auto_stop_enabled.get()
                    on:change=move |e| auto_stop_enabled.set(event_target_checked(&e))
                />
                <div class="relative w-11 h-6 bg-gray-200 peer-focus:outline-none peer-focus:ring-4 peer-focus:ring-blue-300 dark:peer-focus:ring-blue-800 rounded-full peer dark:bg-gray-700 peer-checked:after:translate-x-full rtl:peer-checked:after:-translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:start-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-5 after:w-5 after:transition-all dark:border-gray-600 peer-checked:bg-blue-600"></div>
                <span class="ms-3 text-sm font-medium text-gray-900 dark:text-gray-300">Auto Stop Enabled</span>
            </label>
        </div>
        <div class="mt-2">
            <label class="inline-flex items-center cursor-pointer">
                <input type="checkbox" value="" class="sr-only peer"
                    prop:checked=move || allow_workspace_change_auto_stop.get()
                    on:change=move |e| allow_workspace_change_auto_stop.set(event_target_checked(&e))
                />
                <div class="relative w-11 h-6 bg-gray-200 peer-focus:outline-none peer-focus:ring-4 peer-focus:ring-blue-300 dark:peer-focus:ring-blue-800 rounded-full peer dark:bg-gray-700 peer-checked:after:translate-x-full rtl:peer-checked:after:-translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:start-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-5 after:w-5 after:transition-all dark:border-gray-600 peer-checked:bg-blue-600"></div>
                <span class="ms-3 text-sm font-medium text-gray-900 dark:text-gray-300">Allow Changing Auto Stop on Workspace</span>
            </label>
        </div>
        {
            move || if auto_stop_enabled.get() {
                view! {
                    <div class="mt-2">
                        <label class="block mb-2 text-sm font-medium text-gray-900 dark:text-white">Auto Stop Seconds</label>
                        <input
                            prop:value={move || auto_stop_seconds.get()}
                            on:input=move |ev| {
                                if let Ok(seconds) = event_target_value(&ev).parse() {
                                    auto_stop_seconds.set(seconds);
                                }
                            }
                            class="max-w-96 bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5 dark:bg-gray-600 dark:border-gray-500 dark:placeholder-gray-400 dark:text-white"
                            // placeholder={placeholder}
                        />
                    </div>
                }.into_view()
            } else {
                view! {}.into_view()
            }
        }
    };

    view! {
        <SettingView title="Auto Start Stop Settings".to_string() action=save_action body update_counter=login_counter extra=None />
    }
}

#[component]
fn UpdateNameView() -> impl IntoView {
    let org = use_context::<Signal<Option<Organization>>>().unwrap();
    let login_counter = expect_context::<RwSignal<i32>>();

    let org_name = create_rw_signal(String::new());
    create_effect(move |_| {
        if let Some(name) = org.with(|o| o.as_ref().map(|o| o.name.clone())) {
            org_name.set(name);
        }
    });

    let save_action = create_action(move |_| async move {
        if let Some(id) = org.with(|o| o.as_ref().map(|o| o.id)) {
            update_name(id, org_name.get_untracked()).await
        } else {
            Err(ErrorResponse {
                error: "Organization not loaded yet".to_string(),
            })
        }
    });

    let body = view! {
        <div class="mt-2">
            <input
                prop:value={move || org_name.get()}
                on:input=move |ev| {
                    org_name.set(event_target_value(&ev));
                }
                class="max-w-96 bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5 dark:bg-gray-600 dark:border-gray-500 dark:placeholder-gray-400 dark:text-white"
                // placeholder={placeholder}
            />
        </div>
    };

    view! {
        <SettingView title="Organization Name".to_string() action=save_action body update_counter=login_counter extra=None />
    }
}

async fn get_org_members() -> Result<Vec<OrganizationMember>> {
    let org =
        use_context::<Signal<Option<Organization>>>().ok_or_else(|| anyhow!("can't get org"))?;
    let org = org
        .get_untracked()
        .ok_or_else(|| anyhow!("can't get org"))?;
    let resp = Request::get(&format!("/api/v1/organizations/{}/members", org.id))
        .send()
        .await?;
    let members: Vec<OrganizationMember> = resp.json().await?;
    Ok(members)
}

#[component]
pub fn OrgMembers() -> impl IntoView {
    let invite_member_modal_hidden = create_rw_signal(true);
    let member_filter = create_rw_signal(String::new());

    let update_counter = create_rw_signal(0);
    let members = create_local_resource(
        move || update_counter.get(),
        move |_| async move { get_org_members().await },
    );
    let members = Signal::derive(move || {
        let filter = member_filter.get();
        let mut members = members
            .with(|m| m.as_ref().and_then(|m| m.as_ref().ok().cloned()))
            .unwrap_or_default();
        if !filter.trim().is_empty() {
            members.retain(|m| {
                m.name
                    .as_ref()
                    .map(|n| n.contains(&filter))
                    .unwrap_or(false)
            });
        }
        members
    });

    view! {
        <div class="pb-4">
            <h5 class="mr-3 text-2xl font-semibold dark:text-white">
                Organization Members
            </h5>
            <p class="text-gray-700 dark:text-gray-400">{"Manage your organization's members"}</p>
            <div class="flex flex-col items-center justify-between py-4 gap-y-3 md:flex-row md:space-y-0 md:space-x-4">
                <div class="w-full md:w-1/2">
                    <form class="flex items-center">
                        <label for="simple-search" class="sr-only">Filter Members</label>
                        <div class="relative w-full">
                        <div class="absolute inset-y-0 left-0 flex items-center pl-3 pointer-events-none">
                            <svg aria-hidden="true" class="w-5 h-5 text-gray-500 dark:text-gray-400" fill="currentColor" viewbox="0 0 20 20" xmlns="http://www.w3.org/2000/svg">
                            <path fill-rule="evenodd" d="M8 4a4 4 0 100 8 4 4 0 000-8zM2 8a6 6 0 1110.89 3.476l4.817 4.817a1 1 0 01-1.414 1.414l-4.816-4.816A6 6 0 012 8z" clip-rule="evenodd" />
                            </svg>
                        </div>
                        <input
                            prop:value={move || member_filter.get()}
                            on:input=move |ev| { member_filter.set(event_target_value(&ev)); }
                            type="text"
                            class="bg-white block w-full p-2 pl-10 text-sm text-gray-900 border border-gray-300 rounded-lg bg-gray-50 focus:ring-blue-500 focus:border-blue-500 dark:bg-gray-700 dark:border-gray-600 dark:placeholder-gray-400 dark:text-white dark:focus:ring-blue-500 dark:focus:border-blue-500"
                            placeholder="Filter Members"
                        />
                        </div>
                    </form>
                </div>
                <button
                    type="button"
                    class="px-4 py-2 text-sm font-medium text-white rounded-lg bg-blue-700 hover:bg-blue-800 focus:ring-4 focus:ring-blue-300 dark:bg-blue-600 dark:hover:bg-blue-700 focus:outline-none dark:focus:ring-blue-800"
                    on:click=move |_| invite_member_modal_hidden.set(false)
                >
                    Invite New Member
                </button>
            </div>
        </div>

        <div class="flex items-center w-full px-4 py-2 text-gray-900 dark:text-white bg-gray-50 dark:bg-gray-700">
            <span class="w-1/3 truncate pr-2">Name</span>
            <span class="w-1/3 truncate">Joined</span>
            <span class="w-1/3 truncate pl-2 flex flex-row items-center">
                <div class="w-2/3 flex flex-row justify-center">
                    <span>Role</span>
                </div>
                <span class="w-1/3">
                </span>
            </span>
        </div>

        <For
            each=move || members.get().into_iter().enumerate()
            key=|(i, m)| (*i, m.clone())
            children=move |(i, m)| {
                view! {
                    <MemberItemView i member=m update_counter />
                }
            }
        />

        <InviteMemberView invite_member_modal_hidden />
    }
}

async fn delete_org_member(
    user_id: Uuid,
    delete_modal_hidden: RwSignal<bool>,
    update_counter: RwSignal<i32>,
) -> Result<(), ErrorResponse> {
    let current_org =
        use_context::<Signal<Option<Organization>>>().ok_or_else(|| anyhow!("can't get org"))?;
    let org = current_org
        .get_untracked()
        .ok_or_else(|| anyhow!("can't get org"))?;
    let resp = Request::delete(&format!(
        "/api/v1/organizations/{}/members/{user_id}",
        org.id
    ))
    .send()
    .await?;
    if resp.status() != 204 {
        let error = resp
            .json::<ErrorResponse>()
            .await
            .unwrap_or_else(|_| ErrorResponse {
                error: "Internal Server Error".to_string(),
            });
        return Err(error);
    }
    delete_modal_hidden.set(true);
    update_counter.update(|c| *c += 1);
    Ok(())
}

async fn update_org_member(
    user_id: Uuid,
    role: String,
    update_modal_hidden: RwSignal<bool>,
    update_counter: RwSignal<i32>,
) -> Result<(), ErrorResponse> {
    let current_org =
        use_context::<Signal<Option<Organization>>>().ok_or_else(|| anyhow!("can't get org"))?;
    let org = current_org
        .get_untracked()
        .ok_or_else(|| anyhow!("can't get org"))?;

    let role: UserRole = match UserRole::from_str(&role) {
        Ok(r) => r,
        Err(_) => {
            return Err(ErrorResponse {
                error: "role is invalid".to_string(),
            })
        }
    };

    let resp = Request::put(&format!(
        "/api/v1/organizations/{}/members/{user_id}",
        org.id
    ))
    .json(&UpdateOrganizationMember { role })?
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
    update_modal_hidden.set(true);
    update_counter.update(|c| *c += 1);
    Ok(())
}

#[component]
fn MemberItemView(
    i: usize,
    member: OrganizationMember,
    update_counter: RwSignal<i32>,
) -> impl IntoView {
    let update_member_modal_hidden = create_rw_signal(true);
    let delete_modal_hidden = create_rw_signal(true);
    let delete_action = create_action(move |_| {
        delete_org_member(member.user_id, delete_modal_hidden, update_counter)
    });

    view! {
        <div
            class="flex items-center w-full px-4 py-2"
            class=("border-t", move || i > 0)
        >
            <span class="w-1/3 truncate pr-2 text-gray-900 dark:text-white flex flex-row items-center">
                <img
                    class="w-8 h-8 rounded-full mr-2"
                    src={ member.avatar_url.clone().unwrap_or("https://flowbite.s3.amazonaws.com/blocks/marketing-ui/avatars/michael-gough.png".to_string()) }
                    alt="user photo"
                />
                {member.name.clone()}
            </span>
            <div class="w-1/3 flex flex-col">
                <DatetimeModal time=member.joined />
            </div>
            <span class="w-1/3 pl-2 flex flex-row items-center">
                <div class="w-2/3 flex flex-row justify-center">
                    <span>{member.role.to_string()}</span>
                </div>
                <span class="w-1/3">
                    <MemberControl delete_modal_hidden update_member_modal_hidden align_right=true />
                </span>
            </span>
        </div>
        <DeletionModal resource=member.name.clone().unwrap_or_default() modal_hidden=delete_modal_hidden delete_action />
        <UpdateMemberView member=member.clone() update_modal_hidden=update_member_modal_hidden update_counter />
    }
}

#[component]
fn UpdateMemberView(
    member: OrganizationMember,
    update_modal_hidden: RwSignal<bool>,
    update_counter: RwSignal<i32>,
) -> impl IntoView {
    let role = create_rw_signal(member.role.to_string());
    let update_action = create_action(move |_| {
        update_org_member(
            member.user_id,
            role.get_untracked(),
            update_modal_hidden,
            update_counter,
        )
    });
    let select_on_change = move |ev: web_sys::Event| {
        role.set(event_target_value(&ev));
    };
    let body = view! {
        <div>
            <label class="block mb-2 text-sm font-medium text-gray-900 dark:text-white">
                Role
            </label>
            <select
                class="bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5 dark:bg-gray-700 dark:border-gray-600 dark:placeholder-gray-400 dark:text-white dark:focus:ring-blue-500 dark:focus:border-blue-500"
                on:change=select_on_change
            >
                <For
                    each=move || vec![UserRole::Admin, UserRole::Member]
                    key=|b| b.clone()
                    children=move |r| {
                        let current_role = r.to_string();
                        view! {
                            <option
                                selected=move || current_role == role.get().as_ref()
                            >{r.to_string()}</option>
                        }
                    }
                />
            </select>
        </div>
    };
    view! {
        <CreationModal title="Update Member".to_string() modal_hidden=update_modal_hidden body action=update_action is_update=true create_button_hidden=false />
    }
}

#[component]
fn MemberControl(
    delete_modal_hidden: RwSignal<bool>,
    update_member_modal_hidden: RwSignal<bool>,
    align_right: bool,
) -> impl IntoView {
    let dropdown_hidden = create_rw_signal(true);

    let toggle_dropdown = move |_| {
        if dropdown_hidden.get_untracked() {
            dropdown_hidden.set(false);
        } else {
            dropdown_hidden.set(true);
        }
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

    let delete_member = {
        move |_| {
            dropdown_hidden.set(true);
            delete_modal_hidden.set(false);
        }
    };

    view! {
        <div
            class="relative w-9"
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
                class="absolute pt-2 z-10 divide-y divide-gray-100"
                class:hidden=move || dropdown_hidden.get()
                class=("right-0", move || align_right)
            >
                <ul class="py-2 text-sm text-gray-700 dark:text-gray-200 bg-white rounded-lg border shadow w-44 dark:bg-gray-700">
                    <li>
                        <a
                            href="#"
                            class="block px-4 py-2 hover:bg-gray-100 dark:hover:bg-gray-600 dark:hover:text-white"
                            on:click=move |_| {
                                dropdown_hidden.set(true);
                                update_member_modal_hidden.set(false);
                            }
                        >
                            Update Member
                        </a>
                    </li>
                    <li>
                        <a
                            href="#"
                            class="block px-4 py-2 hover:bg-gray-100 dark:hover:bg-gray-600 text-red-700"
                            on:click=delete_member
                        >
                            Delete Member
                        </a>
                    </li>
                </ul>
            </div>
        </div>
    }
}

async fn create_user_invitation() -> Result<String> {
    let org =
        use_context::<Signal<Option<Organization>>>().ok_or_else(|| anyhow!("can't get org"))?;
    let org = org
        .get_untracked()
        .ok_or_else(|| anyhow!("can't get org"))?;
    let resp = Request::post(&format!("/api/v1/organizations/{}/invitations", org.id))
        .send()
        .await?;
    if resp.status() != 200 {
        return Err(anyhow!(resp.text().await?));
    }
    let invitation: String = resp.text().await?;
    Ok(invitation)
}

#[component]
fn InviteMemberView(invite_member_modal_hidden: RwSignal<bool>) -> impl IntoView {
    let action = create_action(move |_| async move { Ok(()) });

    let update_counter = create_rw_signal(0);
    let invitation_resource = create_local_resource(
        move || update_counter.get(),
        move |_| async move { create_user_invitation().await },
    );
    let invitation = create_rw_signal(String::new());
    create_effect(move |_| {
        let hidden = invite_member_modal_hidden.get();
        invitation.set(String::new());
        if !hidden {
            update_counter.update(|c| *c += 1);
        }
    });

    create_effect(move |_| {
        if let Some(i) =
            invitation_resource.with(|i| i.as_ref().and_then(|i| i.as_ref().ok().cloned()))
        {
            invitation.set(i);
        }
    });

    let body = view! {
        <div>
            <label class="block mb-2 text-sm font-medium text-gray-900 dark:text-white">
                Invite URL
            </label>
            <span class="bg-gray-50 text-gray-900 text-sm rounded-lg block w-full p-2.5 dark:bg-gray-600 dark:border-gray-500 dark:placeholder-gray-400 dark:text-white">
                { move || format!("{}/join/{}", window().location().origin().unwrap(), invitation.get()) }
            </span>
            <label class="block mt-2 text-sm font-medium text-yellow-500">
                This URL will expiry in 30 minutes
            </label>
        </div>
    };
    view! {
        <CreationModal title="Invite New Member".to_string() modal_hidden=invite_member_modal_hidden action body is_update=false create_button_hidden=true />
    }
}
