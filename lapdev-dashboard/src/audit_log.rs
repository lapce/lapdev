use anyhow::{anyhow, Result};
use chrono::{DateTime, FixedOffset, Local, NaiveDate, TimeZone};
use gloo_net::http::Request;
use lapdev_common::{console::Organization, AuditLogRecord, AuditLogResult};
use leptos::prelude::*;

use crate::{
    component::button::Button,
    datepicker::Datepicker,
    modal::{DatetimeModal, ErrorResponse},
    organization::get_current_org,
};

async fn get_audit_logs(
    org: Signal<Option<Organization>>,
    start: Option<NaiveDate>,
    end: Option<NaiveDate>,
    page_size: String,
    page: u64,
) -> Result<AuditLogResult, ErrorResponse> {
    let org = org.get().ok_or_else(|| anyhow!("can't get org"))?;
    let start: DateTime<FixedOffset> = start
        .and_then(|t| {
            Local
                .from_local_datetime(&t.and_hms_opt(0, 0, 0).unwrap())
                .single()
        })
        .unwrap_or_else(|| {
            Local
                .from_local_datetime(&Local::now().date_naive().and_hms_opt(0, 0, 0).unwrap())
                .unwrap()
        })
        .into();
    let end: DateTime<FixedOffset> = end
        .and_then(|t| {
            Local
                .from_local_datetime(&t.and_hms_opt(23, 59, 59).unwrap())
                .single()
        })
        .unwrap_or_else(|| {
            Local
                .from_local_datetime(&Local::now().date_naive().and_hms_opt(23, 59, 59).unwrap())
                .unwrap()
        })
        .into();

    let page_size = page_size.parse::<u64>().unwrap_or(10);

    let resp = Request::get(&format!("/api/v1/organizations/{}/audit_logs", org.id))
        .query([
            ("start", &start.to_rfc3339()),
            ("end", &end.to_rfc3339()),
            ("page", &page.to_string()),
            ("page_size", &page_size.to_string()),
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

    let result: AuditLogResult = resp.json().await?;

    Ok(result)
}

#[component]
pub fn AuditLogView() -> impl IntoView {
    let from_date = RwSignal::new(Some(Local::now().date_naive()));
    let to_date = RwSignal::new(Some(Local::now().date_naive()));
    let page_size = RwSignal::new(String::new());
    let page = RwSignal::new(0);
    let org = get_current_org();

    let error = RwSignal::new(None);

    let get_action = Action::new_local(move |()| async move {
        error.set(None);
        let result = get_audit_logs(
            org,
            from_date.get_untracked(),
            to_date.get_untracked(),
            page_size.get_untracked(),
            page.get_untracked(),
        )
        .await;

        if let Err(e) = &result {
            error.set(Some(e.error.clone()));
        }

        result
    });

    let audit_logs = Signal::derive(move || {
        let result = get_action.value().get();
        if let Some(Ok(result)) = result {
            return result;
        }

        AuditLogResult {
            num_pages: 0,
            page: 0,
            page_size: page_size.get_untracked().parse().unwrap_or(10),
            total_items: 0,
            records: vec![],
        }
    });

    let prev_page = move |_| {
        if audit_logs.with_untracked(|a| a.page == 0) {
            return;
        }
        page.update(|p| *p = p.saturating_sub(1));
        get_action.dispatch(());
    };

    let next_page = move |_| {
        if audit_logs.with_untracked(|a| a.page + 1 >= a.num_pages) {
            return;
        }
        page.update(|p| *p += 1);
        get_action.dispatch(());
    };

    let change_page_size = move |e| {
        page_size.set(event_target_value(&e));
        page.set(0);
        get_action.dispatch(());
    };

    get_action.dispatch(());

    view! {
        <div class="border-b pb-4 mb-8">
            <h5 class="mr-3 text-2xl font-semibold">
                Audit Log
            </h5>
            <p class="text-gray-700">{"View your organization's audit logs"}</p>
        </div>

        <div class="pb-8">
            <div class="flex flex-row items-center">
                <Datepicker value=from_date />
                <span class="mx-2">to</span>
                <Datepicker value=to_date />
                <Button
                    class="ml-4"
                    on:click=move |_| {get_action.dispatch(());}
                >
                    Search
                </Button>
            </div>
            { move || if let Some(error) = error.get() {
                view! {
                    <div class="my-4 p-4 rounded-lg bg-red-50">
                        <span class="text-sm font-medium text-red-800">{ error }</span>
                    </div>
                }.into_any()
            } else {
                ().into_any()
            }}

            <div class="mt-4 flex flex-row items-center justify-between">
                <span class="text-sm font-normal text-gray-500">
                    {"Showing "}
                    <span class="font-semibold text-gray-900">
                        {move || format!("{}-{}", audit_logs.with(|a| if a.records.is_empty() {0} else {1} + a.page * a.page_size ), audit_logs.with(|a| if a.records.is_empty() {0} else {a.records.len() as u64} + a.page * a.page_size  ))}
                    </span>
                    {" of "}
                    <span class="font-semibold text-gray-900">{move || audit_logs.with(|a| a.total_items)}</span>
                </span>
                <div class="flex flex-row items-center">
                    <p class="mr-2">{"rows per page"}</p>

                    <select
                        class="bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-18 p-2.5"
                        on:change=change_page_size
                    >
                        <option selected>10</option>
                        <option>20</option>
                        <option>50</option>
                        <option>100</option>
                        <option>200</option>
                    </select>

                    <span class="ml-2 p-2 rounded"
                        class=("text-gray-300", move || audit_logs.with(|a| a.page == 0))
                        class=("cursor-pointer", move || !audit_logs.with(|a| a.page == 0))
                        class=("hover:bg-gray-100", move || !audit_logs.with(|a| a.page == 0))
                        class=("disabled", move || audit_logs.with(|a| a.page == 0))
                        on:click=prev_page
                    >
                        <svg class="w-5 h-5" aria-hidden="true" fill="currentColor" viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg">
                            <path fill-rule="evenodd" d="M12.707 5.293a1 1 0 010 1.414L9.414 10l3.293 3.293a1 1 0 01-1.414 1.414l-4-4a1 1 0 010-1.414l4-4a1 1 0 011.414 0z" clip-rule="evenodd"></path>
                        </svg>
                    </span>
                    <span class="p-2 rounded"
                        class=("text-gray-300", move || audit_logs.with(|a| a.page + 1 >= a.num_pages))
                        class=("cursor-pointer", move || !audit_logs.with(|a| a.page + 1 >= a.num_pages))
                        class=("hover:bg-gray-100", move || !audit_logs.with(|a| a.page + 1 >= a.num_pages))
                        on:click=next_page
                    >
                        <svg class="w-5 h-5" aria-hidden="true" fill="currentColor" viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg">
                            <path fill-rule="evenodd" d="M7.293 14.707a1 1 0 010-1.414L10.586 10 7.293 6.707a1 1 0 011.414-1.414l4 4a1 1 0 010 1.414l-4 4a1 1 0 01-1.414 0z" clip-rule="evenodd"></path>
                        </svg>
                    </span>
                </div>
            </div>

            <div class="mt-2 overflow-x-auto w-full">
                <table class="w-full text-left text-gray-700 bg-gray-50">
                    <thead>
                        <tr>
                            <th scope="col" class="py-2 px-4">Time</th>
                            <th scope="col" class="py-2 px-4">User</th>
                            <th scope="col" class="py-2 px-4">Action</th>
                            <th scope="col" class="py-2 px-4">Resource</th>
                        </tr>
                    </thead>
                    <tbody>
                        <For
                            each=move || audit_logs.get().records.into_iter().enumerate()
                            key=|(_, a)| a.id
                            children=move |(i, record)| {
                                view! {
                                    <RecordItemView i record />
                                }
                            }
                        />
                    </tbody>
                </table>
            </div>
        </div>
    }
}

#[component]
fn RecordItemView(i: usize, record: AuditLogRecord) -> impl IntoView {
    view! {
        <tr
            class="w-full bg-white"
            class=("border-t", move || i > 0)
        >
            <td class="px-4 py-2">
                <DatetimeModal time=record.time />
            </td>
            <td class="px-4 py-2">
                <div class="text-gray-900 flex flex-row items-center">
                    <img
                        class="w-6 h-6 rounded-full mr-2"
                        src={ record.avatar.clone() }
                        alt="user photo"
                    />
                    {record.user.clone()}
                </div>
            </td>
            <td class="px-4 py-2">
                <p>{record.action}</p>
            </td>
            <td class="px-4 py-2">
                <p>{record.resource_kind}</p>
                <p class="text-gray-500">{record.resource_name}</p>
            </td>
        </tr>
    }
}
