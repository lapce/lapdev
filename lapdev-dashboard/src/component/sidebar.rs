use leptos::prelude::*;
use lucide_leptos::PanelLeft;
use tailwind_fuse::*;

use crate::component::button::{Button, ButtonSize, ButtonVariant};

#[derive(Clone)]
pub struct SidebarData {
    pub open: RwSignal<bool>,
}

#[component]
pub fn SidebarProvider(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <div
            style="--sidebar-width: 16em"
            style=("--sidebar-width-icon", "3em")
            class=move || tw_merge!(
                "group/sidebar-wrapper has-data-[variant=inset]:bg-sidebar flex min-h-svh w-full",
                class.get()
            )
            data-slot="sidebar-wrapper"
        >
            {children}
        </div>
    }
}

#[component]
pub fn Sidebar(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    let data: SidebarData = expect_context();

    view! {
        <div
            class="group peer text-sidebar-foreground hidden md:block"
            data-state=move || match data.open.get() {
                true => "expanded",
                false => "collapsed",
            }
            data-collapsible=move || if !data.open.get() { "offcanvas" } else { "" }
            data-slot="sidebar"
            data-side="left"
            data-variant="sidebar"
        >
            <div
                class=move || tw_merge!(
                    "relative w-(--sidebar-width) bg-transparent transition-[width] duration-200 ease-linear",
                    "group-data-[collapsible=offcanvas]:w-0",
                    "group-data-[side=right]:rotate-180",
                    "group-data-[collapsible=icon]:w-(--sidebar-width-icon)",
                )
                data-slot="sidebar-gap"
            >
            </div>
            <div
                class=move || tw_merge!(
                    "fixed inset-y-0 z-10 hidden h-svh w-(--sidebar-width) transition-[left,right,width] duration-200 ease-linear md:flex",
                    "left-0 group-data-[collapsible=offcanvas]:left-[calc(var(--sidebar-width)*-1)]",
                    "group-data-[collapsible=icon]:w-(--sidebar-width-icon) group-data-[side=left]:border-r group-data-[side=right]:border-l",
                    class.get()
                )
                data-slot="sidebar-container"
            >
                <div
                    data-sidebar="sidebar"
                    data-slot="sidebar-inner"
                    class="bg-sidebar group-data-[variant=floating]:border-sidebar-border flex h-full w-full flex-col group-data-[variant=floating]:rounded-lg group-data-[variant=floating]:border group-data-[variant=floating]:shadow-sm"
                >
                    {children}
                </div>
            </div>
        </div>
    }
}

#[component]
pub fn SidebarHeader(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <div
            class=move || tw_merge!("flex flex-col gap-2 p-2", class.get())
            data-slot="sidebar-header"
            data-sidebar="header"
        >
            {children}
        </div>
    }
}

#[component]
pub fn SidebarFooter(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <div
            class=move || tw_merge!("flex flex-col gap-2 p-2", class.get())
            data-slot="sidebar-footer"
            data-sidebar="footer"
        >
            {children}
        </div>
    }
}

#[component]
pub fn SidebarContent(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <div
            class=move || tw_merge!("flex min-h-0 flex-1 flex-col gap-2 overflow-auto group-data-[collapsible=icon]:overflow-hidden", class.get())
            data-slot="sidebar-content"
            data-sidebar="content"
        >
            {children}
        </div>
    }
}

#[component]
pub fn SidebarGroup(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <div
            class=move || tw_merge!("relative flex w-full min-w-0 flex-col p-2", class.get())
            data-slot="sidebar-group"
            data-sidebar="group"
        >
            {children}
        </div>
    }
}

#[component]
pub fn SidebarGroupContent(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <div
            class=move || tw_merge!(
                "w-full text-sm",
                class.get()
            )
            data-slot="sidebar-group-content"
            data-sidebar="group-content"
        >
            {children}
        </div>
    }
}

#[component]
pub fn SidebarGroupLabel(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <div
            class=move || tw_merge!(
                    "text-sidebar-foreground/70 ring-sidebar-ring flex h-8 shrink-0 items-center rounded-md px-2 text-xs font-medium outline-hidden transition-[margin,opacity] duration-200 ease-linear focus-visible:ring-2 [&>svg]:size-4 [&>svg]:shrink-0",
                    "group-data-[collapsible=icon]:-mt-8 group-data-[collapsible=icon]:opacity-0",
                    class.get()
                )
            data-slot="sidebar-group-label"
            data-sidebar="group-label"
        >
            {children}
        </div>
    }
}

#[component]
pub fn SidebarMenu(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <ul
            class=move || tw_merge!(
                "flex w-full min-w-0 flex-col gap-1",
                class.get()
            )
            data-slot="sidebar-menu"
            data-sidebar="menu"
        >
            {children}
        </ul>
    }
}

#[component]
pub fn SidebarMenuItem(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <li
            class=move || tw_merge!(
                "group/menu-item relative",
                class.get()
            )
            data-slot="sidebar-menu-item"
            data-sidebar="menu-item"
        >
            {children}
        </li>
    }
}

#[component]
pub fn SidebarMenuSub(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <ul
            class=move || tw_merge!(
                "border-sidebar-border mx-3.5 flex min-w-0 translate-x-px flex-col gap-1 border-l px-2.5 py-0.5",
                "group-data-[collapsible=icon]:hidden",
                class.get()
            )
            data-slot="sidebar-menu-sub"
            data-sidebar="menu-sub"
        >
            {children}
        </ul>
    }
}

#[component]
pub fn SidebarMenuSubItem(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <li
            class=move || tw_merge!(
                "group/menu-sub-item relative",
                class.get()
            )
            data-slot="sidebar-menu-sub-item"
            data-sidebar="menu-sub-item"
        >
            {children}
        </li>
    }
}

#[component]
pub fn SidebarMenuSubButton(
    #[prop(into, optional)] size: Signal<SidebarSubButtonSize>,
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(into, optional)] href: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = Memo::new(move |_| {
        tw_merge!(
            "text-sidebar-foreground ring-sidebar-ring hover:bg-sidebar-accent hover:text-sidebar-accent-foreground active:bg-sidebar-accent active:text-sidebar-accent-foreground [&>svg]:text-sidebar-accent-foreground flex h-7 min-w-0 -translate-x-px items-center gap-2 overflow-hidden rounded-md px-2 outline-hidden focus-visible:ring-2 disabled:pointer-events-none disabled:opacity-50 aria-disabled:pointer-events-none aria-disabled:opacity-50 [&>span:last-child]:truncate [&>svg]:size-4 [&>svg]:shrink-0",
            "data-[active=true]:bg-sidebar-accent data-[active=true]:text-sidebar-accent-foreground",
            "group-data-[collapsible=icon]:hidden",
            size.get(),
            class.get()
        )
    });

    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <a
            class={move || class.get()}
            href=move || href.get().unwrap_or_default()
        >
            { children }
        </a>
    }
}

#[derive(TwClass)]
#[tw(
    class = "peer/menu-button flex w-full items-center gap-2 overflow-hidden rounded-md p-2 text-left text-sm outline-hidden ring-sidebar-ring transition-[width,height,padding] hover:bg-sidebar-accent hover:text-sidebar-accent-foreground focus-visible:ring-2 active:bg-sidebar-accent active:text-sidebar-accent-foreground disabled:pointer-events-none disabled:opacity-50 group-has-data-[sidebar=menu-action]/menu-item:pr-8 aria-disabled:pointer-events-none aria-disabled:opacity-50 data-[active=true]:bg-sidebar-accent data-[active=true]:font-medium data-[active=true]:text-sidebar-accent-foreground data-[state=open]:hover:bg-sidebar-accent data-[state=open]:hover:text-sidebar-accent-foreground group-data-[collapsible=icon]:size-8! group-data-[collapsible=icon]:p-2! [&>span:last-child]:truncate [&>svg]:size-4 [&>svg]:shrink-0"
)]
pub struct SidebarButtonClass {
    pub variant: SidebarButtonVariant,
    pub size: SidebarButtonSize,
}

#[derive(PartialEq, TwVariant)]
pub enum SidebarButtonVariant {
    #[tw(
        default,
        class = "hover:bg-sidebar-accent hover:text-sidebar-accent-foreground"
    )]
    Default,
    #[tw(
        class = "bg-background shadow-[0_0_0_1px_hsl(var(--sidebar-border))] hover:bg-sidebar-accent hover:text-sidebar-accent-foreground hover:shadow-[0_0_0_1px_hsl(var(--sidebar-accent))]"
    )]
    Outline,
}

#[derive(PartialEq, TwVariant)]
pub enum SidebarButtonSize {
    #[tw(default, class = "h-8 text-sm")]
    Default,
    #[tw(class = "h-7 text-xs")]
    Sm,
    #[tw(class = "h-12 text-sm group-data-[collapsible=icon]:p-0!")]
    Lg,
}

#[derive(PartialEq, TwVariant)]
pub enum SidebarSubButtonSize {
    #[tw(default, class = "text-sm")]
    Md,
    #[tw(class = "text-xs")]
    Sm,
}

#[component]
pub fn SidebarMenuButton(
    #[prop(into, optional)] variant: Signal<SidebarButtonVariant>,
    #[prop(into, optional)] size: Signal<SidebarButtonSize>,
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = Memo::new(move |_| {
        SidebarButtonClass {
            variant: variant.get(),
            size: size.get(),
        }
        .with_class(class.get().unwrap_or_default())
    });

    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <button
            class={move || class.get()}
        >
            { children }
        </button>
    }
}

#[component]
pub fn SidebarInset(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());

    view! {
        <main
            class=move || tw_merge!(
                "bg-background relative flex w-full flex-1 flex-col",
                "md:peer-data-[variant=inset]:m-2 md:peer-data-[variant=inset]:ml-0 md:peer-data-[variant=inset]:rounded-xl md:peer-data-[variant=inset]:shadow-sm md:peer-data-[variant=inset]:peer-data-[state=collapsed]:ml-2",
                class.get()
            )
            data-slot="sidebar-inset"
        >
            {children}
        </main>
    }
}

#[component]
pub fn SidebarTrigger(#[prop(into, optional)] class: MaybeProp<String>) -> impl IntoView {
    let class = MaybeProp::derive(move || Some(tw_merge!("size-7", class.get())));

    view! {
        <Button
            variant=ButtonVariant::Ghost
            size=ButtonSize::Icon
            class
        >
            <PanelLeft />
            <span class="sr-only">Toggle Sidebar</span>
        </Button>
    }
}
