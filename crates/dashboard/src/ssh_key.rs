use anyhow::Result;
use gloo_net::http::Request;
use lapdev_common::{NewSshKey, SshKey};
use leptos::prelude::*;

use crate::{
    component::button::{Button, ButtonVariant},
    modal::{CreationInput, DeleteModal, ErrorResponse, Modal},
};

async fn delete_ssh_key(
    id: String,
    delete_modal_open: RwSignal<bool>,
    update_counter: RwSignal<i32, LocalStorage>,
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
    delete_modal_open.set(false);
    update_counter.update(|c| *c += 1);
    Ok(())
}

#[component]
pub fn SshKeyItem(key: SshKey, update_counter: RwSignal<i32, LocalStorage>) -> impl IntoView {
    let id = key.id;
    let name = key.name.clone();
    let delete_modal_open = RwSignal::new(false);
    let delete_action = Action::new_local(move |_| {
        delete_ssh_key(id.to_string(), delete_modal_open, update_counter)
    });

    view! {
        <div class="flex flex-row items-center border rounded-xl px-4 py-2">
            <div class="w-1/6">{key.name}</div>
            <div class="w-3/6 truncate p-2">{key.key}</div>
            <div class="w-1/6 truncate p-2">{key.created_at.to_rfc2822()}</div>
            <div class="w-1/6 flex justify-end items-center">
                <Button variant=ButtonVariant::Destructive
                    on:click=move |_| delete_modal_open.set(true)
                >Delete</Button>
            </div>
            <DeleteModal
            resource=name
            open=delete_modal_open delete_action />
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
    let update_counter = RwSignal::new_local(0);
    let ssh_keys = LocalResource::new(|| async move { all_ssh_keys().await.unwrap_or_default() });
    Effect::new(move |_| {
        update_counter.track();
        ssh_keys.refetch();
    });

    view! {
        <section class="w-full flex flex-col">
            <div class="border-b pb-4 w-full">
                <div class="flex items-end justify-between">
                    <div class="min-w-0 mr-4">
                        <h5 class="mr-3 text-2xl font-semibold">
                            SSH Keys
                        </h5>
                        <p class="text-gray-700">{"Manage your SSH keys. Add your SSH public keys to your account will enable you to SSH into all your workspaces."}</p>
                    </div>
                    <NewSshKey update_counter />
                </div>
            </div>
            <div class="flex flex-col py-4 space-y-4">
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
        </section>
    }
}

async fn create_ssh_key(
    name: RwSignal<String, LocalStorage>,
    key: RwSignal<String, LocalStorage>,
    modal_open: RwSignal<bool>,
    update_counter: RwSignal<i32, LocalStorage>,
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

    modal_open.set(false);
    update_counter.update(|c| *c += 1);
    name.set(String::new());
    key.set(String::new());

    Ok(())
}

#[component]
pub fn NewSshKey(update_counter: RwSignal<i32, LocalStorage>) -> impl IntoView {
    let name = RwSignal::new_local(String::new());
    let key = RwSignal::new_local(String::new());

    let modal_open = RwSignal::new(false);
    let action = Action::new_local(move |_| create_ssh_key(name, key, modal_open, update_counter));

    view! {
        <Button
            on:click=move |_| modal_open.set(true)
        >
            New SSH Key
        </Button>
        <Modal
            title="New SSH Key"
            open=modal_open
            action
        >
            <CreationInput label="Name".to_string() value=name placeholder="".to_string() />
            <div>
                <label class="block mb-2 text-sm font-medium text-gray-900">SSH Public Key</label>
                <textarea
                    rows=4
                    prop:value={move || key.get()}
                    on:input=move |ev| { key.set(event_target_value(&ev)); }
                    class="bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5"
                />
            </div>
        </Modal>
    }
}
