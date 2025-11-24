use anyhow::Result;
use chrono::{DateTime, FixedOffset};
use gloo_net::http::Request;
use lapdev_common::{EnterpriseLicense, NewLicense, NewLicenseKey};
use leptos::prelude::*;

use crate::modal::{CreationInput, ErrorResponse, Modal};

async fn get_license() -> Result<EnterpriseLicense> {
    let resp = Request::get("/api/v1/admin/license").send().await?;
    let license: EnterpriseLicense = resp.json().await?;
    Ok(license)
}

#[component]
pub fn LicenseView() -> impl IntoView {
    let update_counter = RwSignal::new_local(0);
    let license = LocalResource::new(move || async move { get_license().await });
    Effect::new(move |_| {
        update_counter.track();
        license.refetch();
    });

    view! {
        <div class="border-b pb-4 mb-8">
            <h5 class="mr-3 text-2xl font-semibold">
                Enterprise License
            </h5>
            <p class="text-gray-700">{"Manage your Lapdev Enterprise License for the cluster"}</p>
        </div>

        <UpdateLicenseView update_counter />

        <div class="mt-4 border rounded-lg p-8">
        {
            move || {
                if let Some(license) = license.with(|l| l.as_ref().and_then(|l| l.as_ref().ok()).cloned()) {
                    view! {
                        <div class="flex flex-col space-y-8 lg:space-y-0 lg:flex-row items-center lg:justify-between">
                            <p class="text-lg px-4">
                            Enterprise License
                            </p>
                            <div class="flex flex-col items-center px-4">
                                <p class="text-gray-500">
                                    Hostname
                                </p>
                                <p>{license.hostname}</p>
                            </div>
                            <div class="flex flex-col items-center px-4">
                                <p class="text-gray-500">
                                    Users Limit
                                </p>
                                <p>{license.users}</p>
                            </div>
                            <div class="flex flex-col items-center px-4">
                                <p class="text-gray-500">
                                    Valid Until
                                </p>
                                <p>{ license.expires_at.to_rfc2822() }</p>
                            </div>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <p>{"You don't have a valid Enterprise License"}</p>
                    }.into_any()
                }
            }
    }
    </div>
    }
}

async fn update_license(
    secret: String,
    modal_open: RwSignal<bool>,
    update_counter: RwSignal<i32, LocalStorage>,
) -> Result<(), ErrorResponse> {
    let resp = Request::put("/api/v1/admin/license")
        .json(&NewLicenseKey { key: secret })?
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
    let _ = location().reload();

    Ok(())
}

#[component]
pub fn UpdateLicenseView(update_counter: RwSignal<i32, LocalStorage>) -> impl IntoView {
    let modal_open = RwSignal::new(false);
    let secret = RwSignal::new_local(String::new());
    let action =
        Action::new_local(move |_| update_license(secret.get(), modal_open, update_counter));
    view! {
        <button
            type="button"
            class="flex items-center justify-center px-4 py-2 text-sm font-medium text-white rounded-lg bg-blue-700 hover:bg-blue-800 focus:ring-4 focus:ring-blue-300 focus:outline-none"
            on:click=move |_| modal_open.set(true)
        >
            Update Enterprise License
        </button>
        <Modal
            title="Update Enterprise License"
            open=modal_open
            action
            action_text="Update"
            action_progress_text="Updating"
        >
            <CreationInput label="New License Key".to_string() value=secret placeholder="".to_string() />
        </Modal>
    }
}

async fn sign_new_license(
    secret: String,
    expires_at: String,
    users: String,
    hostname: String,
) -> Result<(), ErrorResponse> {
    let users: usize = match users.parse() {
        Ok(users) => users,
        Err(_) => {
            return Err(ErrorResponse {
                error: "can't parse users to number".to_string(),
            })
        }
    };

    let expires_at: DateTime<FixedOffset> = match expires_at.parse() {
        Ok(v) => v,
        Err(_) => {
            return Err(ErrorResponse {
                error: "can't parse expires to date".to_string(),
            })
        }
    };

    let resp = Request::post("/api/v1/admin/new_license")
        .json(&NewLicense {
            secret,
            expires_at,
            users,
            hostname,
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

    Ok(())
}

#[component]
pub fn SignLicenseView() -> impl IntoView {
    let secret = RwSignal::new_local(String::new());
    let expires_at = RwSignal::new_local(String::new());
    let users = RwSignal::new_local(String::new());
    let hostname = RwSignal::new_local(String::new());
    let modal_open = RwSignal::new(false);
    let action = Action::new_local(move |_| {
        sign_new_license(
            secret.get_untracked(),
            expires_at.get_untracked(),
            users.get_untracked(),
            hostname.get_untracked(),
        )
    });
    view! {
        <div class="border-b pb-4 mb-8">
            <h5 class="mr-3 text-2xl font-semibold">
                Sign a new Enterprise License
            </h5>
        </div>
        <button
            type="button"
            class="flex items-center justify-center px-4 py-2 text-sm font-medium text-white rounded-lg bg-blue-700 hover:bg-blue-800 focus:ring-4 focus:ring-blue-300 focus:outline-none"
            on:click=move |_| modal_open.set(true)
        >
            Sign New License
        </button>
        <Modal
            title="Sign Enterprise License"
            open=modal_open
            action
        >
            <CreationInput label="Signing Secret".to_string() value=secret placeholder="".to_string() />
            <CreationInput label="Expires".to_string() value=expires_at placeholder="".to_string() />
            <CreationInput label="Users".to_string() value=users placeholder="".to_string() />
            <CreationInput label="Hostname".to_string() value=hostname placeholder="".to_string() />
        </Modal>
    }
}
