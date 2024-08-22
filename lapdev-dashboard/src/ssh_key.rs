use anyhow::Result;
use gloo_net::http::Request;
use lapdev_common::{NewSshKey, SshKey};
use leptos::{
    component, create_action, create_local_resource, create_rw_signal, event_target_value, view,
    For, IntoView, RwSignal, SignalGet, SignalGetUntracked, SignalSet, SignalUpdate,
};

use crate::modal::{CreationInput, CreationModal, DeletionModal, ErrorResponse};

async fn delete_ssh_key(
    id: String,
    delete_modal_hidden: RwSignal<bool>,
    update_counter: RwSignal<i32>,
) -> Result<(), ErrorResponse> {
    let resp = Request::delete(&format!("/api/v1/account/ssh_keys/{id}"))
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

#[component]
pub fn SshKeyItem(key: SshKey, update_counter: RwSignal<i32>) -> impl IntoView {
    let id = key.id;
    let name = key.name.clone();
    let delete_modal_hidden = create_rw_signal(true);
    let delete_action =
        create_action(move |_| delete_ssh_key(id.to_string(), delete_modal_hidden, update_counter));

    view! {
        <div class="flex flex-row items-center border rounded-xl px-4 py-2">
            <span class="w-1/6">{key.name}</span>
            <span class="w-3/6 truncate p-2">{key.key}</span>
            <span class="w-1/6 truncate p-2">{key.created_at.to_rfc2822()}</span>
            <div class="w-1/6 flex justify-end items-center">
                <button class="px-4 py-2 text-sm text-white rounded-lg bg-red-700 hover:bg-red-800 focus:ring-4 focus:ring-red-300 dark:bg-red-600 dark:hover:bg-red-700 focus:outline-none dark:focus:ring-red-800"
                    on:click=move |_| delete_modal_hidden.set(false)
                >Delete</button>
            </div>
            <DeletionModal resource=name  modal_hidden=delete_modal_hidden delete_action=delete_action />
        </div>
    }
}

async fn all_ssh_keys() -> Result<Vec<SshKey>> {
    let resp = Request::get("/api/v1/account/ssh_keys").send().await?;
    let ssh_keys: Vec<SshKey> = resp.json().await?;
    Ok(ssh_keys)
}

#[component]
pub fn SshKeys() -> impl IntoView {
    let update_counter = create_rw_signal(0);
    let ssh_keys = create_local_resource(
        move || update_counter.get(),
        |_| async move { all_ssh_keys().await.unwrap_or_default() },
    );

    view! {
        <section class="w-full h-full flex flex-col">
            <div class="border-b pb-4">
                <div class="flex items-end justify-between">
                    <div class="min-w-0 mr-4">
                        <h5 class="mr-3 text-2xl font-semibold dark:text-white">
                            SSH Keys
                        </h5>
                        <p class="text-gray-700 dark:text-gray-400">{"Manage your SSH keys. Add your SSH public keys to your account will enable you to SSH into all your workspaces."}</p>
                    </div>
                    <NewSshKey update_counter />
                </div>
            </div>
            <div class="relative w-full basis-0 grow">
                <div class="absolute w-full h-full flex flex-col py-4 space-y-4 overflow-y-auto">
                    <For
                        each=move || ssh_keys.get().unwrap_or_default()
                        key=|key|  key.id
                        children=move |key| {
                            view! {
                                <SshKeyItem key update_counter />
                            }
                        }
                    />
                </div>
            </div>
        </section>
    }
}

async fn create_ssh_key(
    name: RwSignal<String>,
    key: RwSignal<String>,
    modal_hidden: RwSignal<bool>,
    update_counter: RwSignal<i32>,
) -> Result<(), ErrorResponse> {
    let resp = Request::post("/api/v1/account/ssh_keys")
        .json(&NewSshKey {
            name: name.get_untracked(),
            key: key.get_untracked(),
        })?
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

    modal_hidden.set(true);
    update_counter.update(|c| *c += 1);
    name.set(String::new());
    key.set(String::new());

    Ok(())
}

#[component]
pub fn NewSshKey(update_counter: RwSignal<i32>) -> impl IntoView {
    let name = create_rw_signal(String::new());
    let key = create_rw_signal(String::new());

    let modal_hidden = create_rw_signal(true);
    let action = create_action(move |_| create_ssh_key(name, key, modal_hidden, update_counter));

    let body = view! {
        <CreationInput label="Name".to_string() value=name placeholder="".to_string() />
        <div>
            <label class="block mb-2 text-sm font-medium text-gray-900 dark:text-white">SSH Public Key</label>
            <textarea
                rows=4
                prop:value={move || key.get()}
                on:input=move |ev| { key.set(event_target_value(&ev)); }
                class="bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5 dark:bg-gray-600 dark:border-gray-500 dark:placeholder-gray-400 dark:text-white"
            />
        </div>
    };

    view! {
        <button
            type="button"
            class="flex items-center justify-center whitespace-nowrap px-4 py-2 text-sm font-medium text-white rounded-lg bg-blue-700 hover:bg-blue-800 focus:ring-4 focus:ring-blue-300 dark:bg-blue-600 dark:hover:bg-blue-700 focus:outline-none dark:focus:ring-blue-800"
            on:click=move |_| modal_hidden.set(false)
        >
            New SSH Key
        </button>
        <CreationModal title="New SSH Key".to_string() modal_hidden action body update_text=Some("Create".to_string()) updating_text=Some("Creating".to_string()) create_button_hidden=false />
    }
}
