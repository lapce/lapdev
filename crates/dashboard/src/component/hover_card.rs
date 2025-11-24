use leptos::{context::Provider, prelude::*};
use tailwind_fuse::*;

#[derive(Clone)]
struct HoverCardContext {
    position: RwSignal<(f64, f64)>,
    width: RwSignal<f64>,
    height: RwSignal<f64>,
    // dictates whether the hovercard is actually showing or not
    show: RwSignal<bool>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum HoverCardPlacement {
    BottomLeft,
    BottomRight,
    TopLeft,
    TopRight,
    RightTop,
    RightBottom,
    LeftTop,
    LeftBottom,
}

impl Default for HoverCardPlacement {
    fn default() -> Self {
        Self::BottomRight
    }
}

#[component]
pub fn HoverCard(
    #[prop()] open: RwSignal<bool>,
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let show_delay = std::time::Duration::from_millis(500);
    let hide_delay = std::time::Duration::from_millis(500);
    let show_handle: StoredValue<Option<TimeoutHandle>> = StoredValue::new(None);
    let hide_handle: StoredValue<Option<TimeoutHandle>> = StoredValue::new(None);
    let show = RwSignal::new(open.get_untracked());

    let eff = RenderEffect::new(move |_| {
        if open.get() {
            // clear any possibly active hide timer
            if let Some(h) = hide_handle.get_value() {
                h.clear();
            }
            // set show timer
            let h = leptos::leptos_dom::helpers::set_timeout_with_handle(
                move || show.set(true),
                show_delay,
            )
            .expect("set timeout for show");
            show_handle.set_value(Some(h));
        } else {
            // clear any possibly active show timer
            if let Some(h) = show_handle.get_value() {
                h.clear();
            }
            // set hide timer
            let h = leptos::leptos_dom::helpers::set_timeout_with_handle(
                move || show.set(false),
                hide_delay,
            )
            .expect("set timeout for hide");
            hide_handle.set_value(Some(h));
        }
    });

    on_cleanup(move || {
        if let Some(Some(h)) = show_handle.try_get_value() {
            h.clear();
        }
        if let Some(Some(h)) = hide_handle.try_get_value() {
            h.clear();
        }
        drop(eff);
    });

    let context = HoverCardContext {
        position: RwSignal::new((0.0, 0.0)),
        width: RwSignal::new(0.0),
        height: RwSignal::new(0.0),
        show,
    };
    view! {
        <Provider value=context>
            <div
                class=move || tw_merge!("", class.get())
                on:mouseenter=move |_| open.set(true)
                on:mouseleave=move |_| open.set(false)
                data-slot="hover-card"
            >
                {children .map(|c| c().into_any()) .unwrap_or_else(|| ().into_any())}
            </div>
        </Provider>
    }
}

#[component]
pub fn HoverCardTrigger(
    #[prop(default = HoverCardPlacement::default())] placement: HoverCardPlacement,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let context = use_context::<HoverCardContext>().unwrap();
    let trigger_ref = NodeRef::<leptos::html::Div>::new();

    Effect::new(move || {
        if context.show.get() {
            // Also trigger when width/height changes
            let hover_card_width = context.width.get();
            let hover_card_height = context.height.get();

            if let Some(element) = trigger_ref.get() {
                let element: &web_sys::Element = element.as_ref();
                let rect = element.get_bounding_client_rect();
                if let Some(window) = web_sys::window() {
                    let viewport_width = window.inner_width().unwrap().as_f64().unwrap();
                    let viewport_height = window.inner_height().unwrap().as_f64().unwrap();

                    let (mut x, mut y) = match placement {
                        HoverCardPlacement::BottomLeft => (rect.x(), rect.y() + rect.height()),
                        HoverCardPlacement::BottomRight => (
                            rect.x() + rect.width() - hover_card_width,
                            rect.y() + rect.height(),
                        ),
                        HoverCardPlacement::TopLeft => (rect.x(), rect.y() - hover_card_height),
                        HoverCardPlacement::TopRight => (
                            rect.x() + rect.width() - hover_card_width,
                            rect.y() - hover_card_height,
                        ),
                        HoverCardPlacement::RightTop => (rect.x() + rect.width(), rect.y()),
                        HoverCardPlacement::RightBottom => (
                            rect.x() + rect.width(),
                            rect.y() + rect.height() - hover_card_height,
                        ),
                        HoverCardPlacement::LeftTop => (rect.x() - hover_card_width, rect.y()),
                        HoverCardPlacement::LeftBottom => (
                            rect.x() - hover_card_width,
                            rect.y() + rect.height() - hover_card_height,
                        ),
                    };

                    // Viewport boundary detection and adjustment
                    if hover_card_width > 0.0 && x + hover_card_width > viewport_width {
                        x = viewport_width - hover_card_width - 10.0; // 10px margin
                    }
                    if x < 10.0 {
                        x = 10.0;
                    }

                    if hover_card_height > 0.0 && y + hover_card_height > viewport_height {
                        y = viewport_height - hover_card_height - 10.0;
                    }
                    if y < 10.0 {
                        y = 10.0;
                    }

                    context.position.set((x, y));
                }
            }
        }
    });

    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <div
            node_ref=trigger_ref
            data-slot="hover-card-trigger"
        >
            {children}
        </div>
    }
}

#[component]
pub fn HoverCardContent(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<ChildrenFn>,
) -> impl IntoView {
    let content_ref = NodeRef::<leptos::html::Div>::new();
    let context = use_context::<HoverCardContext>().unwrap();

    // Measure hover card size when it becomes visible
    Effect::new(move || {
        if context.show.get() {
            if let Some(element) = content_ref.get() {
                let element: &web_sys::Element = element.as_ref();
                let rect = element.get_bounding_client_rect();
                context.width.set(rect.width());
                context.height.set(rect.height());
            }
        }
    });

    view! {
        <Show when=move || context.show.get() fallback=|| ()>
            <div
                node_ref=content_ref
                class=move || {
                    tw_merge!(
                        "fixed z-50 outline-none bg-popover text-popover-foreground data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 z-50 w-64 origin-(--radix-hover-card-content-transform-origin) rounded-md border p-4 shadow-md",
                         class.get()
                    )
                }
                style=move || {
                    let (x, y) = context.position.get();
                    format!("left: {x}px; top: {y}px;")
                }
                data-state=move || { if context.show.get() { "open" } else { "closed" } }
                data-slot="hover-card-content"
            >
                {children.clone().map(|c| c().into_any()).unwrap_or_else(|| ().into_any())}
            </div>
        </Show>
    }
}
