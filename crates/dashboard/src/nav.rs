use std::time::Duration;

use lapdev_common::{console::MeUser, UserRole};
use leptos::prelude::*;
use leptos_router::hooks::use_location;
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use web_sys::FocusEvent;

use crate::{
    account::NavUserControl,
    app::AppConfig,
    cluster::get_cluster_info,
    component::badge::{Badge, BadgeVariant},
    organization::OrgSelector,
    DOCS_URL,
};

#[derive(Clone, Copy)]
pub struct NavExpanded {
    pub orgnization: RwSignal<bool>,
    pub account: RwSignal<bool>,
    pub k8s_environments: RwSignal<bool>,
}

#[component]
pub fn TopNav() -> impl IntoView {
    let login = use_context::<LocalResource<Option<MeUser>>>().unwrap();
    let config = use_context::<AppConfig>().unwrap();
    let user_control_hidden = RwSignal::new_local(true);
    let toggle_user_control = move |_| {
        if user_control_hidden.get_untracked() {
            user_control_hidden.set(false);
        } else {
            user_control_hidden.set(true);
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
                if !has_focus && !user_control_hidden.get_untracked() {
                    user_control_hidden.set(true);
                }
            },
            Duration::from_secs(0),
        );
    };

    let pathname = use_location().pathname.get_untracked();
    let admin_path = pathname.starts_with("/admin");

    view! {
        <nav class="bg-white border-b border-gray-200 left-0 right-0 top-0 z-50">
            <div class="px-8 py-4 flex flex-wrap justify-between items-center">
                <div class="flex justify-start items-center">
                    <a href="/" class="flex items-center gap-3 mr-4">
                        <svg class="w-10 h-10" xmlns="http://www.w3.org/2000/svg" fill="currentColor" viewBox="0 0 1024 1024">
                            <path d="M512 0C390.759 0 279.426 42.2227 191.719 112.656L191.719 612L351.719 612L351.719 332L422 332L431.719 332L431.719 332.344C488.169 334.127 541.249 351.138 581.875 380.625C627.591 413.806 654.875 460.633 654.875 512C654.875 563.367 627.591 610.194 581.875 643.375C541.249 672.862 488.169 689.873 431.719 691.656L431.719 1017.69C457.88 1021.81 484.68 1024 512 1024C794.77 1024 1024 794.77 1024 512C1024 229.23 794.77 0 512 0ZM111.719 192.906C41.8539 280.432 0 391.303 0 512C0 738.766 147.487 930.96 351.719 998.25L351.719 692L151.719 692L151.719 691.844L111.719 691.844L111.719 192.906ZM738.219 372C741.597 372.107 745.042 373.02 748.312 374.812L946.281 483.312C952.311 486.616 956.692 492.393 959.188 499.156C959.821 500.874 960.317 502.641 960.688 504.469C960.764 504.834 960.841 505.196 960.906 505.562C961.12 506.807 961.225 508.071 961.312 509.344C961.378 510.235 961.498 511.112 961.5 512C961.498 512.888 961.378 513.765 961.312 514.656C961.226 515.929 961.12 517.193 960.906 518.438C960.841 518.804 960.764 519.166 960.688 519.531C960.317 521.359 959.821 523.126 959.188 524.844C956.692 531.608 952.311 537.384 946.281 540.688L748.312 649.188C735.232 656.355 719.818 649.367 713.875 633.594C707.932 617.82 713.7 599.23 726.781 592.062L872.875 512L726.781 431.938C713.7 424.771 707.932 406.18 713.875 390.406C718.332 378.576 728.085 371.678 738.219 372ZM431.719 412.344L431.719 611.656C513.56 608.208 574.875 561.985 574.875 512C574.875 462.015 513.56 415.792 431.719 412.344Z" />
                            <path d="M742 403.688C740.062 403.483 738.097 404.438 737.094 406.25C735.756 408.666 736.615 411.694 739.031 413.031L925.719 516.375C928.135 517.712 931.194 516.822 932.531 514.406C933.869 511.99 932.979 508.962 930.562 507.625L743.875 404.281C743.271 403.947 742.646 403.756 742 403.688Z" />
                            <path d="M927.5 507.031C926.856 507.115 926.221 507.339 925.625 507.688L738.938 616.906C736.554 618.301 735.762 621.335 737.156 623.719C738.551 626.102 741.616 626.926 744 625.531L930.688 516.312C933.071 514.918 933.863 511.852 932.469 509.469C931.423 507.681 929.432 506.78 927.5 507.031Z" />
                        </svg>
                        <div class="flex items-center gap-2">
                            <span class="self-center text-2xl font-semibold whitespace-nowrap">Lapdev</span>
                            <Badge
                                variant=BadgeVariant::Secondary
                                class="text-[10px] uppercase tracking-wide"
                            >
                                "Public beta"
                            </Badge>
                        </div>
                    </a>
                </div>
        <div class="flex flex-row items-center">
            <ul
                class="font-medium flex flex-row space-x-8 mr-8"
            >
                <li
                    class:hidden=move || !config.show_lapdev_website.get()
                >
                    <a href="https://lap.dev/">Home</a>
                </li>
                <li
                    class:hidden=move || !config.show_lapdev_website.get()
                >
                    <a href=DOCS_URL target="_blank" rel="noopener noreferrer">Docs</a>
                </li>
                <li
                    class:hidden=move || !login.with(|l| l.as_ref().and_then(|l| l.as_ref().map(|l| l.cluster_admin))).unwrap_or(false)
                >
                    <a href="/admin"
                        class="block"
                        class=("text-blue-700", move || admin_path)
                    >Cluster Admin</a>
                </li>
                <li
                    class:hidden=move || !login.with(|l| l.as_ref().and_then(|l| l.as_ref().map(|l| l.cluster_admin))).unwrap_or(false)
                >
                    <a href="/"
                        class="block"
                        class=("text-blue-700", move || !admin_path)
                    >Dashboard</a>
                </li>
            </ul>
            <div on:focusout=on_focusout>
            <button
                type="button"
                class="flex text-sm rounded-full focus:ring-4 focus:ring-gray-300"
                on:click=toggle_user_control
            >
                <span class="sr-only">Open user menu</span>
                <img
                class="w-8 h-8 rounded-full"
                src=move ||  { login.get().flatten().and_then(|l| l.avatar_url.clone()).unwrap_or("https://flowbite.s3.amazonaws.com/blocks/marketing-ui/avatars/michael-gough.png".to_string()) }
                alt="user photo"
                />
            </button>
            <div class="relative">
                <NavUserControl user_control_hidden />
            </div>
            </div>
        </div>
      </div>
    </nav>
    }
}

#[component]
pub fn SideNavMain() -> impl IntoView {
    let nav_expanded: NavExpanded = expect_context();
    view! {
        <ul class="space-y-2">
            <li>
                <a href="/" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg hover:bg-gray-100 group">
                    <span class="ml-3">Dashboard</span>
                </a>
            </li>
            <li>
                <a href="/workspaces" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg hover:bg-gray-100 group">
                    <span class="ml-3">Workspaces</span>
                </a>
            </li>
            <li>
                <a href="/projects" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg hover:bg-gray-100 group">
                    <span class="ml-3">Projects</span>
                </a>
            </li>
            <li>
                <a href="/kubernetes/clusters" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg hover:bg-gray-100 group">
                    <span class="ml-3">K8s Clusters</span>
                </a>
            </li>
            <li>
                <a href="#" class="flex items-center justify-between p-2 text-base font-normal text-gray-900 rounded-lg transition duration-75 hover:bg-gray-100 group"
                    on:click=move |_| nav_expanded.k8s_environments.update(|expanded| *expanded = !*expanded)
                >
                    <span class="ml-3">Dev Environments</span>
                    <svg class="w-3 h-3"  xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 6 10"
                        class:hidden=move || nav_expanded.k8s_environments.get()
                    >
                        <path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="m1 9 4-4-4-4"/>
                    </svg>
                    <svg class="w-3 h-3"  xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 10 6"
                        class:hidden=move || !nav_expanded.k8s_environments.get()
                    >
                        <path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="m1 1 4 4 4-4"/>
                    </svg>
                </a>
            </li>

            <li
                class:hidden=move || !nav_expanded.k8s_environments.get()
            >
                <a href="/kubernetes/environments/personal" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg transition duration-75 hover:bg-gray-100 group">
                    <span class="ml-8">Personal</span>
                </a>
            </li>

            <li
                class:hidden=move || !nav_expanded.k8s_environments.get()
            >
                <a href="/kubernetes/environments/shared" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg transition duration-75 hover:bg-gray-100 group">
                    <span class="ml-8">Shared</span>
                </a>
            </li>

            <li
                class:hidden=move || !nav_expanded.k8s_environments.get()
            >
                <a href="/kubernetes/environments/branch" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg transition duration-75 hover:bg-gray-100 group">
                    <span class="ml-8">Branch</span>
                </a>
            </li>
        </ul>
    }
}

#[component]
pub fn SideNavAccount() -> impl IntoView {
    let nav_expanded: NavExpanded = expect_context();
    view! {
        <li>
            <a href="#" class="flex items-center justify-between p-2 text-base font-normal text-gray-900 rounded-lg transition duration-75 hover:bg-gray-100 group"
                on:click=move |_| nav_expanded.account.update(|expanded| *expanded = !*expanded)
            >
                <span class="ml-3">User Settings</span>
                <svg class="w-3 h-3"  xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 6 10"
                    class:hidden=move || nav_expanded.account.get()
                >
                    <path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="m1 9 4-4-4-4"/>
                </svg>
                <svg class="w-3 h-3"  xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 10 6"
                    class:hidden=move || !nav_expanded.account.get()
                >
                    <path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="m1 1 4 4 4-4"/>
                </svg>
            </a>
        </li>

        <li
            class:hidden=move || !nav_expanded.account.get()
        >
            <a href="/account/ssh-keys" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg transition duration-75 hover:bg-gray-100  group">
                <span class="ml-8">SSH Keys</span>
            </a>
        </li>

        <li
            class:hidden=move || !nav_expanded.account.get()
        >
            <a href="/account/git-providers" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg transition duration-75 hover:bg-gray-100 group">
                <span class="ml-8">Git Providers</span>
            </a>
        </li>
    }
}

#[component]
pub fn SideNavOrg() -> impl IntoView {
    let cluster_info = get_cluster_info();
    let nav_expanded: NavExpanded = expect_context();
    let login = use_context::<LocalResource<Option<MeUser>>>().unwrap();
    let role = Signal::derive(move || {
        let role = login.with(|l| {
            l.as_ref()
                .and_then(|l| l.as_ref().map(|l| l.organization.role.clone()))
        });
        role.unwrap_or(UserRole::Member)
    });
    view! {
        <li
            class:hidden=move || {
                let role = role.get();
                role != UserRole::Owner && role != UserRole::Admin
            }
        >
            <a href="#" class="flex items-center justify-between p-2 text-base font-normal text-gray-900 rounded-lg transition duration-75 hover:bg-gray-100 group"
                on:click=move |_| nav_expanded.orgnization.update(|expanded| *expanded = !*expanded)
            >
                <span class="ml-3">Organization</span>
                <svg class="w-3 h-3"  xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 6 10"
                    class:hidden=move || nav_expanded.orgnization.get()
                >
                    <path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="m1 9 4-4-4-4"/>
                </svg>
                <svg class="w-3 h-3"  xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 10 6"
                    class:hidden=move || !nav_expanded.orgnization.get()
                >
                    <path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="m1 1 4 4 4-4"/>
                </svg>
            </a>
        </li>
        <li
            class:hidden=move || {
                let role = role.get();
                if role != UserRole::Owner && role != UserRole::Admin {
                    return true;
                }
                if !nav_expanded.orgnization.get() {
                    return true;
                }
                if !cluster_info.with(|i| i.as_ref().map(|i| i.has_enterprise)).unwrap_or(false) {
                    return true;
                }
                false
            }
        >
            <a href="/organization/usage" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg transition duration-75 hover:bg-gray-100 group">
                <span class="ml-8">Usage</span>
            </a>
        </li>

        <li
            class:hidden=move || {
                let role = role.get();
                if role != UserRole::Owner && role != UserRole::Admin {
                    return true;
                }
                if !nav_expanded.orgnization.get() {
                    return true;
                }
                false
            }
        >
            <a href="/organization/members" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg transition duration-75 hover:bg-gray-100 group">
                <span class="ml-8">Members</span>
            </a>
        </li>

        <li
            class:hidden=move || {
                let role = role.get();
                if role != UserRole::Owner && role != UserRole::Admin {
                    return true;
                }
                if !nav_expanded.orgnization.get() {
                    return true;
                }
                if !cluster_info.with(|i| i.as_ref().map(|i| i.has_enterprise)).unwrap_or(false) {
                    return true;
                }
                false
            }
        >
            <a href="/organization/quota" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg transition duration-75 hover:bg-gray-100 group">
                <span class="ml-8">Quota</span>
            </a>
        </li>

        <li
            class:hidden=move || {
                let role = role.get();
                if role != UserRole::Owner && role != UserRole::Admin {
                    return true;
                }
                if !nav_expanded.orgnization.get() {
                    return true;
                }
                if !cluster_info.with(|i| i.as_ref().map(|i| i.has_enterprise)).unwrap_or(false) {
                    return true;
                }
                false
            }
        >
            <a href="/organization/audit_log" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg transition duration-75 hover:bg-gray-100 group">
                <span class="ml-8">Audit Log</span>
            </a>
        </li>

        <li
            class:hidden=move || {
                let role = role.get();
                if role != UserRole::Owner && role != UserRole::Admin {
                    return true;
                }
                if !nav_expanded.orgnization.get() {
                    return true;
                }
                false
            }
        >
            <a href="/organization/settings" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg transition duration-75 hover:bg-gray-100 group">
                <span class="ml-8">Settings</span>
            </a>
        </li>
    }
}

#[component]
pub fn SideNav() -> impl IntoView {
    view! {
        <aside class="top-0 left-0 z-40 w-64 h-full transition-transform -translate-x-full sm:translate-x-0" aria-label="Sidenav">
            <div class="overflow-y-auto py-5 px-3 h-full bg-white border-r border-gray-200">
                <OrgSelector />
                <SideNavMain />
                <ul class="pt-5 mt-5 space-y-2 border-t border-gray-200">
                    <SideNavAccount />
                    <SideNavOrg />
                </ul>
            </div>
        </aside>
    }
}

#[component]
pub fn AdminSideNav() -> impl IntoView {
    view! {
        <aside class="top-0 left-0 z-40 w-64 h-full transition-transform -translate-x-full sm:translate-x-0" aria-label="Sidenav">
            <div class="overflow-y-auto py-5 px-3 h-full bg-white border-r border-gray-200">
                <ul class="space-y-2">
                    <li>
                        <a href="/admin/workspace_hosts" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg hover:bg-gray-100 group">
                            <span class="ml-3">Workspace Hosts</span>
                        </a>
                    </li>
                    <li>
                        <a href="/admin/machine_types" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg hover:bg-gray-100 group">
                            <span class="ml-3">Machine Types</span>
                        </a>
                    </li>
                    <li>
                        <a href="/admin/users" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg hover:bg-gray-100 group">
                            <span class="ml-3">Cluster Users</span>
                        </a>
                    </li>
                </ul>
                <ul class="pt-5 mt-5 space-y-2 border-t border-gray-200">
                    <li>
                        <a href="/admin/license" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg transition duration-75 hover:bg-gray-100 group">
                            <span class="ml-3">Enterprise License</span>
                        </a>
                        <a href="/admin/settings" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg transition duration-75 hover:bg-gray-100 group">
                            <span class="ml-3">Cluster Settings</span>
                        </a>
                    </li>
                </ul>
            </div>
        </aside>
    }
}
