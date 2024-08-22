use anyhow::{anyhow, Result};
use chrono::{DateTime, FixedOffset, Local, NaiveDate, TimeZone};
use gloo_net::http::Request;
use lapdev_common::{console::Organization, UsageRecord, UsageResult};
use leptos::{
    component, create_action, create_rw_signal, event_target_value, use_context, view, For,
    IntoView, Signal, SignalGet, SignalGetUntracked, SignalSet, SignalUpdate, SignalWith,
    SignalWithUntracked,
};

use crate::{
    datepicker::Datepicker,
    modal::{DatetimeModal, ErrorResponse},
};

async fn get_org_usage(
    start: Option<NaiveDate>,
    end: Option<NaiveDate>,
    page_size: String,
    page: u64,
) -> Result<UsageResult, ErrorResponse> {
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

    let org =
        use_context::<Signal<Option<Organization>>>().ok_or_else(|| anyhow!("can't get org"))?;
    let org = org
        .get_untracked()
        .ok_or_else(|| anyhow!("can't get org"))?;

    let page_size = page_size.parse::<u64>().unwrap_or(10);

    let resp = Request::get(&format!("/api/v1/organizations/{}/usage", org.id))
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

    let result: UsageResult = resp.json().await?;

    Ok(result)
}

#[component]
pub fn UsageView() -> impl IntoView {
    let from_date = create_rw_signal(Some(Local::now().date_naive()));
    let to_date = create_rw_signal(Some(Local::now().date_naive()));
    let page_size = create_rw_signal(String::new());
    let page = create_rw_signal(0);

    let error = create_rw_signal(None);

    let counter = create_rw_signal(0);

    let get_action = create_action(move |()| async move {
        counter.update(|c| {
            *c += 1;
        });
        error.set(None);
        let result = get_org_usage(
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

    let usage = Signal::derive(move || {
        let result = get_action.value().get();
        if let Some(Ok(result)) = result {
            return result;
        }

        UsageResult {
            num_pages: 0,
            page: 0,
            page_size: page_size.get_untracked().parse().unwrap_or(10),
            total_items: 0,
            total_cost: 0,
            records: vec![],
        }
    });

    let prev_page = move |_| {
        if usage.with_untracked(|u| u.page == 0) {
            return;
        }
        page.update(|p| *p = p.saturating_sub(1));
        get_action.dispatch(());
    };

    let next_page = move |_| {
        if usage.with_untracked(|u| u.page + 1 >= u.num_pages) {
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
            <h5 class="mr-3 text-2xl font-semibold dark:text-white">
                Organization Usage
            </h5>
            <p class="text-gray-700 dark:text-gray-400">{"View your organization's usage"}</p>
        </div>
        <div class="flex flex-row items-center">
            <Datepicker value=from_date />
            <span class="mx-2">to</span>
            <Datepicker value=to_date />
            <button
                type="button"
                class="ml-4 px-4 py-2 text-sm font-medium text-white rounded-lg bg-blue-700 hover:bg-blue-800 focus:ring-4 focus:ring-blue-300 dark:bg-blue-600 dark:hover:bg-blue-700 focus:outline-none dark:focus:ring-blue-800"
                on:click=move |_| get_action.dispatch(())
            >
                Search
            </button>
        </div>
        { move || if let Some(error) = error.get() {
            view! {
                <div class="my-4 p-4 rounded-lg bg-red-50 dark:bg-gray-800 ">
                    <span class="text-sm font-medium text-red-800 dark:text-red-400">{ error }</span>
                </div>
            }.into_view()
        } else {
            view!{}.into_view()
        }}
        <div class="mt-4 flex flex-row items-center justify-between">
            <span class="text-sm font-normal text-gray-500 dark:text-gray-400">
                {"Showing "}
                <span class="font-semibold text-gray-900 dark:text-white">
                    {move || format!("{}-{}", usage.with(|u| if u.records.is_empty() {0} else {1} + u.page * u.page_size ), usage.with(|u| if u.records.is_empty() {0} else {u.records.len() as u64} + u.page * u.page_size  ))}
                </span>
                {" of "}
                <span class="font-semibold text-gray-900 dark:text-white">{move || usage.with(|u| u.total_items)}</span>
            </span>
            <p><span class="text-gray-500 mr-1">{"Total Cost:"}</span>{move || usage.with(|u| u.total_cost)}</p>
            <div class="flex flex-row items-center">
                <p class="mr-2">{"rows per page"}</p>

                <select
                    class="bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-18 p-2.5 dark:bg-gray-700 dark:border-gray-600 dark:placeholder-gray-400 dark:text-white dark:focus:ring-blue-500 dark:focus:border-blue-500"
                    on:change=change_page_size
                >
                    <option selected>10</option>
                    <option>20</option>
                    <option>50</option>
                    <option>100</option>
                    <option>200</option>
                </select>

                <span class="ml-2 p-2 rounded"
                    class=("text-gray-300", move || usage.with(|u| u.page == 0))
                    class=("cursor-pointer", move || !usage.with(|u| u.page == 0))
                    class=("hover:bg-gray-100", move || !usage.with(|u| u.page == 0))
                    disabled=move || usage.with(|u| u.page == 0)
                    on:click=prev_page
                >
                    <svg class="w-5 h-5" aria-hidden="true" fill="currentColor" viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg">
                        <path fill-rule="evenodd" d="M12.707 5.293a1 1 0 010 1.414L9.414 10l3.293 3.293a1 1 0 01-1.414 1.414l-4-4a1 1 0 010-1.414l4-4a1 1 0 011.414 0z" clip-rule="evenodd"></path>
                    </svg>
                </span>
                <span class="p-2 rounded"
                    class=("text-gray-300", move || usage.with(|u| u.page + 1 >= u.num_pages))
                    class=("cursor-pointer", move || !usage.with(|u| u.page + 1 >= u.num_pages))
                    class=("hover:bg-gray-100", move || !usage.with(|u| u.page + 1 >= u.num_pages))
                    on:click=next_page
                >
                    <svg class="w-5 h-5" aria-hidden="true" fill="currentColor" viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg">
                        <path fill-rule="evenodd" d="M7.293 14.707a1 1 0 010-1.414L10.586 10 7.293 6.707a1 1 0 011.414-1.414l4 4a1 1 0 010 1.414l-4 4a1 1 0 01-1.414 0z" clip-rule="evenodd"></path>
                    </svg>
                </span>
            </div>
        </div>

        <div class="mt-2 flex items-center w-full px-4 py-2 text-gray-900 dark:text-white bg-gray-50 dark:bg-gray-700">
            <span class="w-1/4 truncate">Resource Type</span>
            <span class="w-1/4 truncate">Resource Name</span>
            <span class="w-1/4 truncate">Cost</span>
            <span class="w-1/4 truncate">User</span>
        </div>

        <For
            each=move || usage.get().records.into_iter().enumerate()
            key=move |(_, r)| (r.id, counter.get_untracked())
            children=move |(i, record)| {
                view! {
                    <RecordItemView i record />
                }
            }
        />
    }
}

#[component]
fn RecordItemView(i: usize, record: UsageRecord) -> impl IntoView {
    let duration = record.duration;
    let hour = duration / 3600;
    let remaining = duration - hour * 3600;
    let minute = remaining / 60;
    let remaining = remaining - minute * 60;
    let seconds = remaining;
    let duration = format!(
        "{}{}{seconds}s",
        if hour > 1 {
            format!("{hour}h ")
        } else {
            "".to_string()
        },
        if minute > 0 {
            format!("{minute}m ")
        } else {
            "".to_string()
        },
    );

    view! {
        <div
            class="flex items-center w-full px-4 py-2"
            class=("border-t", move || i > 0)
        >
            <div class="w-1/4 flex flex-col">
                <p>{record.resource_kind}</p>
            </div>
            <div class="w-1/4 flex flex-col">
                <p>{record.resource_name}</p>
                <p><span class="text-gray-500 mr-1">{"Machine Type:"}</span>{record.machine_type}</p>
            </div>
            <div class="w-1/4 flex flex-col">
                <p>{duration}</p>
                <p><span class="text-gray-500 mr-1">{"Cost:"}</span>{record.cost}</p>
            </div>
            <div class="w-1/4 flex flex-col">
                <span class="mb-2 truncate pr-2 text-gray-900 dark:text-white flex flex-row items-center">
                    <img
                        class="w-6 h-6 rounded-full mr-2"
                        src={ record.avatar.clone() }
                        alt="user photo"
                    />
                    {record.user.clone()}
                </span>
                <DatetimeModal time=record.start />
            </div>
        </div>
    }
}
