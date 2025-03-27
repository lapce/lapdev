use anyhow::{anyhow, Result};
use gloo_net::http::Request;
use lapdev_common::{console::Organization, OrgQuota, OrgQuotaValue, QuotaKind, UpdateOrgQuota};
use leptos::prelude::*;

use crate::modal::{CreationInput, CreationModal, ErrorResponse};

async fn get_org_quota() -> Result<OrgQuota, ErrorResponse> {
    let org =
        use_context::<Signal<Option<Organization>>>().ok_or_else(|| anyhow!("can't get org"))?;
    let org = org
        .get_untracked()
        .ok_or_else(|| anyhow!("can't get org"))?;

    let resp = Request::get(&format!("/api/v1/organizations/{}/quota", org.id))
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

    let result: OrgQuota = resp.json().await?;

    Ok(result)
}

#[component]
pub fn QuotaView() -> impl IntoView {
    let error = RwSignal::new(None);

    let get_action = Action::new_local(move |()| async move {
        error.set(None);
        let result = get_org_quota().await;

        if let Err(e) = &result {
            error.set(Some(e.error.clone()));
        }

        result
    });

    let org_quota = Signal::derive(move || {
        let result = get_action.value().get();
        if let Some(Ok(result)) = result {
            return result;
        }
        OrgQuota::default()
    });

    get_action.dispatch(());

    view! {
        <div class="border-b pb-4 mb-8">
            <h5 class="mr-3 text-2xl font-semibold">
                Organization Quota
            </h5>
            <p class="text-gray-700">{"Manage your organization's quota settings"}</p>
        </div>

        <div class="mt-2 flex items-center w-full px-4 py-2 text-gray-900 bg-gray-50">
            <span class="w-1/4 truncate">Quota Type</span>
            <span class="w-1/4 truncate">Quota Value</span>
            <div class="w-1/2 flex flex-row">
                <span class="w-1/3 truncate">Current Usage</span>
                <span class="w-2/3 truncate">Usage Percentage</span>
            </div>
        </div>
        {
            move || {
                let org_quota = org_quota.get();
                view! {
                    <QuotaItemView i=0 kind=QuotaKind::Workspace value=org_quota.workspace get_action />
                    <QuotaItemView i=1 kind=QuotaKind::RunningWorkspace value=org_quota.running_workspace get_action />
                    <QuotaItemView i=2 kind=QuotaKind::Project value=org_quota.project get_action />
                    <QuotaItemView i=3 kind=QuotaKind::DailyCost value=org_quota.daily_cost get_action />
                    <QuotaItemView i=4 kind=QuotaKind::MonthlyCost value=org_quota.monthly_cost get_action />
                }
            }
        }
    }
}

async fn update_org_quota(
    kind: QuotaKind,
    default_user_quota: String,
    org_quota: String,
    get_action: Action<(), Result<OrgQuota, ErrorResponse>, LocalStorage>,
    update_modal_hidden: RwSignal<bool>,
) -> Result<(), ErrorResponse> {
    let default_user_quota: usize = match default_user_quota.parse() {
        Ok(n) => n,
        Err(_) => {
            return Err(ErrorResponse {
                error: "default user quota is invalid".to_string(),
            })
        }
    };
    let org_quota: usize = match org_quota.parse() {
        Ok(n) => n,
        Err(_) => {
            return Err(ErrorResponse {
                error: "org quota is invalid".to_string(),
            })
        }
    };

    let org =
        use_context::<Signal<Option<Organization>>>().ok_or_else(|| anyhow!("can't get org"))?;
    let org = org
        .get_untracked()
        .ok_or_else(|| anyhow!("can't get org"))?;

    let resp = Request::put(&format!("/api/v1/organizations/{}/quota", org.id))
        .json(&UpdateOrgQuota {
            kind,
            default_user_quota,
            org_quota,
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

    update_modal_hidden.set(true);
    get_action.dispatch(());

    Ok(())
}

#[component]
fn QuotaItemView(
    i: usize,
    kind: QuotaKind,
    value: OrgQuotaValue,
    get_action: Action<(), Result<OrgQuota, ErrorResponse>, LocalStorage>,
) -> impl IntoView {
    let percentage = if value.org_quota == 0 {
        0
    } else {
        value.existing * 100 / value.org_quota
    };
    let color = if percentage >= 100 {
        "bg-red-600"
    } else if percentage >= 75 {
        "bg-yellow-400"
    } else {
        "bg-green-600"
    };
    let update_modal_hidden = RwSignal::new(true);

    let org_quota = RwSignal::new(value.org_quota.to_string());
    let default_user_quota = RwSignal::new(value.default_user_quota.to_string());

    let update_modal_body = view! {
        <CreationInput label="Default User Quota".to_string() value=default_user_quota placeholder="quota number, 0 means disabled".to_string() />
        <CreationInput label="Organization Quota".to_string() value=org_quota placeholder="quota number, 0 means disabled".to_string() />
    };

    let update_action = Action::new_local(move |()| async move {
        update_org_quota(
            kind,
            default_user_quota.get_untracked(),
            org_quota.get_untracked(),
            get_action,
            update_modal_hidden,
        )
        .await
    });

    let current_usage = match kind {
        QuotaKind::Workspace => value.existing.to_string(),
        QuotaKind::RunningWorkspace => value.existing.to_string(),
        QuotaKind::Project => value.existing.to_string(),
        QuotaKind::DailyCost => format!("{} Hours", value.existing),
        QuotaKind::MonthlyCost => format!("{} Hours", value.existing),
    };

    view! {
        <div
            class="flex items-center w-full px-4 py-2"
            class=("border-t", move || i > 0)
        >
            <div class="w-1/4 flex flex-col">
                <p>{kind.to_string()}</p>
            </div>
            <div class="w-1/4 flex flex-col">
                <p><span class="text-gray-500 mr-1">{"Default User Quota:"}</span>{value.default_user_quota}</p>
                <p><span class="text-gray-500 mr-1">{"Organization Quota:"}</span>{value.org_quota}</p>
            </div>
            <div class="w-1/2 flex flex-row">
                <div class="w-1/3 flex flex-col">
                    <p>{current_usage}</p>
                </div>
                <div class="w-2/3 flex flex-row items-center justify-between">
                    <div class="flex flex-row items-center">
                        <div class="w-36 bg-gray-200 rounded-full h-1.5">
                            <div class={format!("{color} h-1.5 rounded-full")} style={format!("width: {}%", percentage.min(100))} > </div>
                        </div>
                        <span class="ml-2">{format!("{percentage}%")}</span>
                    </div>
                    <button
                        class="px-4 py-2 text-sm font-medium text-white rounded-lg bg-green-700 hover:bg-green-800 focus:ring-4 focus:ring-green-300 focus:outline-none"
                        on:click=move |_| update_modal_hidden.set(false)
                    >
                        Update
                    </button>
                </div>
            </div>
        </div>
        <CreationModal title=format!("Update {kind} Quota") modal_hidden=update_modal_hidden action=update_action body=update_modal_body update_text=None updating_text=None width_class=None create_button_hidden=Box::new(|| false) />
    }
}
