use std::{ops::Deref, time::Duration};

use chrono::{Datelike, Days, Local, Month, Months, NaiveDate};
use leptos::prelude::*;
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use web_sys::FocusEvent;

#[derive(Default, Clone)]
pub enum PanelKind {
    #[default]
    Date,
    #[allow(dead_code)]
    Month,
    #[allow(dead_code)]
    Year,
}

#[derive(Clone)]
pub struct PanelRef {
    show_date: RwSignal<NaiveDate>,
    kind: RwSignal<PanelKind>,
}

impl PanelRef {
    pub fn init_panel(&self, show_date: NaiveDate) {
        self.show_date.set(show_date);
        self.kind.set(PanelKind::Date);
    }
}

#[component]
pub fn Datepicker(value: RwSignal<Option<NaiveDate>>) -> impl IntoView {
    let is_show_panel = RwSignal::new(false);

    let show_date_text = RwSignal::new(String::new());
    let show_date_format = "%Y-%m-%d";

    let update_show_date_text = move || {
        value.with_untracked(move |date| {
            let text = date.as_ref().map_or(String::new(), |date| {
                date.format(show_date_format).to_string()
            });
            show_date_text.set(text);
        });
    };
    update_show_date_text();

    let panel_ref: RwSignal<Option<PanelRef>> = RwSignal::new(None);
    let panel_selected_date = RwSignal::new(None::<NaiveDate>);
    _ = panel_selected_date.watch(move |date| {
        let text = date.as_ref().map_or(String::new(), |date| {
            date.format(show_date_format).to_string()
        });
        show_date_text.set(text);
    });

    let open_panel = Callback::new(move |_| {
        panel_selected_date.set(value.get_untracked());
        if let Some(panel_ref) = panel_ref.get_untracked() {
            panel_ref.init_panel(value.get_untracked().unwrap_or(now_date()));
        }
        is_show_panel.set(true);
    });

    let clear_input = Callback::new(move |()| {
        value.set(None);
        show_date_text.set("".to_string());
        is_show_panel.set(false);
    });

    let close_panel = Callback::new(move |date: Option<NaiveDate>| {
        if value.get_untracked() != date {
            if date.is_some() {
                value.set(date);
            }
            update_show_date_text();
        }
        is_show_panel.set(false);
    });

    let on_input_blur = Callback::new(move |_| {
        if let Ok(date) =
            NaiveDate::parse_from_str(&show_date_text.get_untracked(), show_date_format)
        {
            if value.get_untracked() != Some(date) {
                value.set(Some(date));
                update_show_date_text();
            }
        } else {
            update_show_date_text();
        }
    });

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
                if !has_focus && is_show_panel.get_untracked() {
                    close_panel.run(None);
                }
            },
            Duration::from_millis(0),
        );
    };

    view! {
        <div class="relative max-w-48"
            on:focusout=on_focusout
        >
            <div class="absolute inset-y-0 start-0 flex items-center ps-3 pointer-events-none">
                <svg class="w-4 h-4 text-gray-500" aria-hidden="true" xmlns="http://www.w3.org/2000/svg" fill="currentColor" viewBox="0 0 20 20">
                    <path d="M20 4a2 2 0 0 0-2-2h-2V1a1 1 0 0 0-2 0v1h-3V1a1 1 0 0 0-2 0v1H6V1a1 1 0 0 0-2 0v1H2a2 2 0 0 0-2 2v2h20V4ZM0 18a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V8H0v10Zm5-8h10a1 1 0 0 1 0 2H5a1 1 0 0 1 0-2Z"></path>
                </svg>
            </div>
            <input type="text" class="bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full ps-10 p-2.5 datepicker-input" placeholder="Select date"
                prop:value=move || show_date_text.get()
                on:input=move |e| show_date_text.set(event_target_value(&e))
                on:focus=move |e| open_panel.run(e)
                on:blur=move |e| on_input_blur.run(e)
            />
            <Panel selected_date=panel_selected_date is_show_panel close_panel clear_input comp_ref=panel_ref />
        </div>
    }
}

#[component]
fn Panel(
    selected_date: RwSignal<Option<NaiveDate>>,
    is_show_panel: RwSignal<bool>,
    close_panel: Callback<Option<NaiveDate>>,
    clear_input: Callback<()>,
    comp_ref: RwSignal<Option<PanelRef>>,
) -> impl IntoView {
    let panel_kind = RwSignal::new(PanelKind::Date);
    let show_date = RwSignal::new(selected_date.get_untracked().unwrap_or(now_date()));

    comp_ref.update(|current| {
        *current = Some(PanelRef {
            show_date,
            kind: panel_kind,
        });
    });

    view! {
        <div
            class="absolute top-12 z-20"
            class:hidden=move || !is_show_panel.get()
        >
        {move || {
            match panel_kind.get() {
                PanelKind::Date => view! { <DatePanel value=selected_date show_date close_panel clear_input /> }.into_any(),
                PanelKind::Month => view! { <MonthPanel /> }.into_any(),
                PanelKind::Year => view! { <YearPanel /> }.into_any(),
            }

        }}
        </div>
    }
}

#[component]
fn DatePanel(
    value: RwSignal<Option<NaiveDate>>,
    show_date: RwSignal<NaiveDate>,
    close_panel: Callback<Option<NaiveDate>>,
    clear_input: Callback<()>,
) -> impl IntoView {
    let dates = Memo::new(move |_| {
        let show_date = show_date.get();
        let show_date_month = show_date.month();
        let mut dates = vec![];

        let mut current_date = show_date;
        let mut current_weekday_number = None::<u32>;
        loop {
            let date = current_date - Days::new(1);
            if date.month() != show_date_month {
                if current_weekday_number.is_none() {
                    current_weekday_number = Some(current_date.weekday().num_days_from_sunday());
                }
                let weekday_number = current_weekday_number.unwrap();
                if weekday_number == 0 {
                    break;
                }
                current_weekday_number = Some(weekday_number - 1);

                dates.push(CalendarItemDate::Previous(date));
            } else {
                dates.push(CalendarItemDate::Current(date));
            }
            current_date = date;
        }
        dates.reverse();
        dates.push(CalendarItemDate::Current(show_date));
        current_date = show_date;
        current_weekday_number = None;
        loop {
            let date = current_date + Days::new(1);
            if date.month() != show_date_month {
                if current_weekday_number.is_none() {
                    current_weekday_number = Some(current_date.weekday().num_days_from_sunday());
                }
                let weekday_number = current_weekday_number.unwrap();
                if weekday_number == 6 {
                    break;
                }
                current_weekday_number = Some(weekday_number + 1);
                dates.push(CalendarItemDate::Next(date));
            } else {
                dates.push(CalendarItemDate::Current(date));
            }
            current_date = date;
        }
        dates
    });

    let previous_month = move |_| {
        show_date.update(|date| {
            *date = *date - Months::new(1);
        });
    };

    let next_month = move |_| {
        show_date.update(|date| {
            *date = *date + Months::new(1);
        });
    };

    let pick_today = move |_| {
        close_panel.run(Some(now_date()));
    };

    view! {
        <div class="datepicker-picker inline-block rounded-lg bg-white shadow-lg p-4">
            <div class="datepicker-header">
                <div class="datepicker-title bg-white px-2 py-3 text-center font-semibold" style="display: none;">
                </div>
                <div class="datepicker-controls flex justify-between mb-2">
                    <button
                        type="button" class="bg-white rounded-lg text-gray-500 hover:bg-gray-100 hover:text-gray-900 text-lg p-2.5 focus:outline-none focus:ring-2 focus:ring-gray-200 prev-btn"
                        on:click=previous_month
                    >
                        <svg class="w-4 h-4 rtl:rotate-180 text-gray-800" aria-hidden="true" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 14 10"><path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 5H1m0 0 4 4M1 5l4-4"></path></svg>
                    </button>
                    <button type="button" class="text-sm rounded-lg text-gray-900 bg-white font-semibold py-2.5 px-5 hover:bg-gray-100 focus:outline-none focus:ring-2 focus:ring-gray-200 view-switch">
                        {move || format!("{} {}", Month::try_from(show_date.get().month() as u8).unwrap().name(), show_date.get().year())}
                    </button>
                    <button
                        type="button" class="bg-white rounded-lg text-gray-500 hover:bg-gray-100 hover:text-gray-900 text-lg p-2.5 focus:outline-none focus:ring-2 focus:ring-gray-200 next-btn"
                        on:click=next_month
                    >
                        <svg class="w-4 h-4 rtl:rotate-180 text-gray-800" aria-hidden="true" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 14 10"><path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M1 5h12m0 0L9 1m4 4L9 9"></path></svg>
                    </button>
                </div>
            </div>
            <div class="p-1">
                <div class="flex">
                    <div>
                        <div class="grid grid-cols-7 mb-1 text-sm font-medium text-gray-900">
                            <span class="text-center h-6 leading-6">"Su"</span>
                            <span class="text-center h-6 leading-6">"Mo"</span>
                            <span class="text-center h-6 leading-6">"Tu"</span>
                            <span class="text-center h-6 leading-6">"We"</span>
                            <span class="text-center h-6 leading-6">"Th"</span>
                            <span class="text-center h-6 leading-6">"Fr"</span>
                            <span class="text-center h-6 leading-6">"Sa"</span>
                        </div>

                        <div class="w-64 grid grid-cols-7">
                            {move || {
                                dates
                                    .get()
                                    .into_iter()
                                    .map(|date| {
                                        view! { <DatePanelItem value date=date close_panel /> }
                                    })
                                    .collect_view()
                            }}
                        </div>
                    </div>
                </div>
            </div>
            <div class="datepicker-footer">
                <div class="datepicker-controls flex space-x-2 rtl:space-x-reverse mt-2">
                    <button type="button" class="button today-btn text-white bg-blue-700 !bg-primary-700 hover:bg-blue-800 hover:!bg-primary-800 focus:ring-4 focus:ring-blue-300 focus:!ring-primary-300 font-medium rounded-lg text-sm px-5 py-2 text-center w-1/2"
                        on:click=pick_today
                    >
                        Today
                    </button>
                    <button type="button" class="button clear-btn text-gray-900 bg-white border border-gray-300 hover:bg-gray-100 focus:ring-4 focus:ring-blue-300 focus:!ring-primary-300 font-medium rounded-lg text-sm px-5 py-2 text-center w-1/2"
                        on:click=move |_| clear_input.run(())
                    >
                        Clear
                    </button>
                </div>
            </div>
        </div>
    }
}

#[component]
fn DatePanelItem(
    value: RwSignal<Option<NaiveDate>>,
    date: CalendarItemDate,
    close_panel: Callback<Option<NaiveDate>>,
) -> impl IntoView {
    let is_selected = Memo::new({
        let date = date.clone();
        move |_| value.with(|value_date| value_date.as_ref() == Some(date.deref()))
    });

    let on_click = {
        let date = date.clone();
        move |_| {
            close_panel.run(Some(*date.deref()));
        }
    };
    view! {
        <button
            class="datepicker-cell block flex-1 leading-9 border-0 rounded-lg cursor-pointer text-center font-semibold text-sm day prev"
            class=("text-gray-400", date.is_other_month())
            class=("text-gray-900", !date.is_other_month())
            class=("bg-blue-700", move || is_selected.get())
            class=("text-white", move || is_selected.get())
            class=("hover:bg-gray-100", move || !is_selected.get())
            on:click=on_click
        >
            {date.day()}
            {if date.is_today() {
                view! { <div class="thaw-date-picker-date-panel__item-sup"></div> }.into()
            } else {
                None
            }}
        </button>
    }
}

#[component]
fn MonthPanel() -> impl IntoView {
    {}
}

#[component]
fn YearPanel() -> impl IntoView {
    {}
}

#[derive(Clone, PartialEq)]
pub(crate) enum CalendarItemDate {
    Previous(NaiveDate),
    Current(NaiveDate),
    Next(NaiveDate),
}

impl CalendarItemDate {
    pub fn is_other_month(&self) -> bool {
        match self {
            CalendarItemDate::Previous(_) | CalendarItemDate::Next(_) => true,
            CalendarItemDate::Current(_) => false,
        }
    }

    pub fn is_today(&self) -> bool {
        let date = self.deref();
        let now_date = now_date();
        &now_date == date
    }
}

impl Deref for CalendarItemDate {
    type Target = NaiveDate;

    fn deref(&self) -> &Self::Target {
        match self {
            CalendarItemDate::Previous(date)
            | CalendarItemDate::Current(date)
            | CalendarItemDate::Next(date) => date,
        }
    }
}

pub(crate) fn now_date() -> NaiveDate {
    Local::now().date_naive()
}

pub trait SignalWatch {
    type Value;

    fn watch(&self, f: impl Fn(&Self::Value) + 'static) -> Box<dyn FnOnce()>;
}

impl<T, S> SignalWatch for RwSignal<T, S>
where
    T: 'static,
    S: Storage<ArcRwSignal<T>>,
{
    type Value = T;
    fn watch(&self, f: impl Fn(&Self::Value) + 'static) -> Box<dyn FnOnce()> {
        let signal = *self;

        let effect = Effect::new(move |prev: Option<()>| {
            signal.with(|value| {
                if prev.is_some() {
                    untrack(|| f(value));
                }
            });
        });

        Box::new(move || {
            effect.dispose();
        })
    }
}
