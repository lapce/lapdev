use std::time::Duration;

use lapdev_common::{console::MeUser, ClusterInfo, UserRole};
use leptos::{
    component, create_rw_signal, document, expect_context, set_timeout, use_context, view,
    IntoView, Resource, RwSignal, Signal, SignalGet, SignalGetUntracked, SignalSet, SignalUpdate,
    SignalWith,
};
use leptos_router::use_location;
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use web_sys::FocusEvent;

use crate::{account::NavUserControl, organization::OrgSelector};

#[derive(Clone, Copy)]
pub struct NavExpanded {
    pub orgnization: RwSignal<bool>,
    pub account: RwSignal<bool>,
}

#[component]
pub fn TopNav() -> impl IntoView {
    let login = use_context::<Resource<i32, Option<MeUser>>>().unwrap();
    let user_control_hidden = create_rw_signal(true);
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
        <nav class="bg-white border-b border-gray-200 dark:bg-gray-800 dark:border-gray-700 left-0 right-0 top-0 z-50">
            <div class="container mx-auto px-8 py-4 flex flex-wrap justify-between items-center">
                <div class="flex justify-start items-center">
                    <button
                        data-drawer-target="drawer-navigation"
                        data-drawer-toggle="drawer-navigation"
                        aria-controls="drawer-navigation"
                        class="p-2 mr-2 text-gray-600 rounded-lg cursor-pointer md:hidden hover:text-gray-900 hover:bg-gray-100 focus:bg-gray-100 dark:focus:bg-gray-700 focus:ring-2 focus:ring-gray-100 dark:focus:ring-gray-700 dark:text-gray-400 dark:hover:bg-gray-700 dark:hover:text-white"
                    >
                    <svg
                    aria-hidden="true"
                    class="w-6 h-6"
                    fill="currentColor"
                    viewBox="0 0 20 20"
                    xmlns="http://www.w3.org/2000/svg"
                    >
                    <path
                        fill-rule="evenodd"
                        d="M3 5a1 1 0 011-1h12a1 1 0 110 2H4a1 1 0 01-1-1zM3 10a1 1 0 011-1h6a1 1 0 110 2H4a1 1 0 01-1-1zM3 15a1 1 0 011-1h12a1 1 0 110 2H4a1 1 0 01-1-1z"
                        clip-rule="evenodd"
                    ></path>
                        </svg>
                        <svg
                        aria-hidden="true"
                        class="hidden w-6 h-6"
                        fill="currentColor"
                        viewBox="0 0 20 20"
                        xmlns="http://www.w3.org/2000/svg"
                        >
                        <path
                            fill-rule="evenodd"
                            d="M4.293 4.293a1 1 0 011.414 0L10 8.586l4.293-4.293a1 1 0 111.414 1.414L11.414 10l4.293 4.293a1 1 0 01-1.414 1.414L10 11.414l-4.293 4.293a1 1 0 01-1.414-1.414L8.586 10 4.293 5.707a1 1 0 010-1.414z"
                            clip-rule="evenodd"
                        ></path>
                        </svg>
                        <span class="sr-only">Toggle sidebar</span>
                    </button>
                    <a href="/" class="flex items-center justify-between mr-4">
                        <svg class="w-10 h-10 mr-4" xmlns="http://www.w3.org/2000/svg" fill="currentColor" viewBox="0 0 1024 1024">
                            <path d="M512 0C390.759 0 279.426 42.2227 191.719 112.656L191.719 612L351.719 612L351.719 332L422 332L431.719 332L431.719 332.344C488.169 334.127 541.249 351.138 581.875 380.625C627.591 413.806 654.875 460.633 654.875 512C654.875 563.367 627.591 610.194 581.875 643.375C541.249 672.862 488.169 689.873 431.719 691.656L431.719 1017.69C457.88 1021.81 484.68 1024 512 1024C794.77 1024 1024 794.77 1024 512C1024 229.23 794.77 0 512 0ZM111.719 192.906C41.8539 280.432 0 391.303 0 512C0 738.766 147.487 930.96 351.719 998.25L351.719 692L151.719 692L151.719 691.844L111.719 691.844L111.719 192.906ZM738.219 372C741.597 372.107 745.042 373.02 748.312 374.812L946.281 483.312C952.311 486.616 956.692 492.393 959.188 499.156C959.821 500.874 960.317 502.641 960.688 504.469C960.764 504.834 960.841 505.196 960.906 505.562C961.12 506.807 961.225 508.071 961.312 509.344C961.378 510.235 961.498 511.112 961.5 512C961.498 512.888 961.378 513.765 961.312 514.656C961.226 515.929 961.12 517.193 960.906 518.438C960.841 518.804 960.764 519.166 960.688 519.531C960.317 521.359 959.821 523.126 959.188 524.844C956.692 531.608 952.311 537.384 946.281 540.688L748.312 649.188C735.232 656.355 719.818 649.367 713.875 633.594C707.932 617.82 713.7 599.23 726.781 592.062L872.875 512L726.781 431.938C713.7 424.771 707.932 406.18 713.875 390.406C718.332 378.576 728.085 371.678 738.219 372ZM431.719 412.344L431.719 611.656C513.56 608.208 574.875 561.985 574.875 512C574.875 462.015 513.56 415.792 431.719 412.344Z" />
                            <path d="M742 403.688C740.062 403.483 738.097 404.438 737.094 406.25C735.756 408.666 736.615 411.694 739.031 413.031L925.719 516.375C928.135 517.712 931.194 516.822 932.531 514.406C933.869 511.99 932.979 508.962 930.562 507.625L743.875 404.281C743.271 403.947 742.646 403.756 742 403.688Z" />
                            <path d="M927.5 507.031C926.856 507.115 926.221 507.339 925.625 507.688L738.938 616.906C736.554 618.301 735.762 621.335 737.156 623.719C738.551 626.102 741.616 626.926 744 625.531L930.688 516.312C933.071 514.918 933.863 511.852 932.469 509.469C931.423 507.681 929.432 506.78 927.5 507.031Z" />
                        </svg>
                        <span class="self-center text-2xl font-semibold whitespace-nowrap dark:text-white">Lapdev</span>
                    </a>
                </div>
        <div class="flex flex-row items-center">
            <ul
                class="font-medium flex flex-row space-x-8 mr-8"
                class:hidden=move || !login.with(|l| l.as_ref().and_then(|l| l.as_ref().map(|l| l.cluster_admin))).unwrap_or(false)
            >
                <li>
                    <a href="/admin"
                        class="block"
                        class=("text-blue-700", move || admin_path)
                        class=("dark:text-blue-500", move || !admin_path)
                    >Cluster Admin</a>
                </li>
                <li>
                    <a href="/"
                        class="block"
                        class=("text-blue-700", move || !admin_path)
                        class=("dark:text-blue-500", move || !admin_path)
                    >Dashboard</a>
                </li>
            </ul>
            <div on:focusout=on_focusout>
            <button
                type="button"
                class="flex text-sm bg-gray-800 rounded-full focus:ring-4 focus:ring-gray-300 dark:focus:ring-gray-600"
                on:click=toggle_user_control
            >
                <span class="sr-only">Open user menu</span>
                <img
                class="w-8 h-8 rounded-full"
                src=move ||  { login.get().flatten().and_then(|l| l.avatar_url).unwrap_or("https://flowbite.s3.amazonaws.com/blocks/marketing-ui/avatars/michael-gough.png".to_string()) }
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
    view! {
        <ul class="space-y-2">
            <li>
                <a href="/workspaces" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg dark:text-white hover:bg-gray-100 dark:hover:bg-gray-700 group">
                    <span class="ml-3">Workspaces</span>
                </a>
            </li>
            <li>
                <a href="/projects" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg dark:text-white hover:bg-gray-100 dark:hover:bg-gray-700 group">
                    <span class="ml-3">Projects</span>
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
            <a href="#" class="flex items-center justify-between p-2 text-base font-normal text-gray-900 rounded-lg transition duration-75 hover:bg-gray-100 dark:hover:bg-gray-700 dark:text-white group"
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
            <a href="/account/ssh-keys" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg transition duration-75 hover:bg-gray-100 dark:hover:bg-gray-700 dark:text-white group">
                <span class="ml-8">SSH Keys</span>
            </a>
        </li>
    }
}

#[component]
pub fn SideNavOrg() -> impl IntoView {
    let cluster_info = expect_context::<Signal<Option<ClusterInfo>>>();
    let nav_expanded: NavExpanded = expect_context();
    let login = use_context::<Resource<i32, Option<MeUser>>>().unwrap();
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
            <a href="#" class="flex items-center justify-between p-2 text-base font-normal text-gray-900 rounded-lg transition duration-75 hover:bg-gray-100 dark:hover:bg-gray-700 dark:text-white group"
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
            <a href="/organization/usage" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg transition duration-75 hover:bg-gray-100 dark:hover:bg-gray-700 dark:text-white group">
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
            <a href="/organization/members" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg transition duration-75 hover:bg-gray-100 dark:hover:bg-gray-700 dark:text-white group">
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
            <a href="/organization/quota" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg transition duration-75 hover:bg-gray-100 dark:hover:bg-gray-700 dark:text-white group">
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
            <a href="/organization/audit_log" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg transition duration-75 hover:bg-gray-100 dark:hover:bg-gray-700 dark:text-white group">
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
            <a href="/organization/settings" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg transition duration-75 hover:bg-gray-100 dark:hover:bg-gray-700 dark:text-white group">
                <span class="ml-8">Settings</span>
            </a>
        </li>
    }
}

#[component]
pub fn SideNav(new_org_modal_hidden: RwSignal<bool>) -> impl IntoView {
    view! {
        <aside class="top-0 left-0 z-40 w-64 h-full transition-transform -translate-x-full sm:translate-x-0" aria-label="Sidenav">
            <div class="overflow-y-auto py-5 px-3 h-full bg-white border-r border-gray-200 dark:bg-gray-800 dark:border-gray-700">
                <OrgSelector new_org_modal_hidden />
                <SideNavMain />
                <ul class="pt-5 mt-5 space-y-2 border-t border-gray-200 dark:border-gray-700">
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
            <div class="overflow-y-auto py-5 px-3 h-full bg-white border-r border-gray-200 dark:bg-gray-800 dark:border-gray-700">
                <ul class="space-y-2">
                    <li>
                        <a href="/admin/workspace_hosts" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg dark:text-white hover:bg-gray-100 dark:hover:bg-gray-700 group">
                            <span class="ml-3">Workspace Hosts</span>
                        </a>
                    </li>
                    <li>
                        <a href="/admin/machine_types" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg dark:text-white hover:bg-gray-100 dark:hover:bg-gray-700 group">
                            <span class="ml-3">Machine Types</span>
                        </a>
                    </li>
                    <li>
                        <a href="/admin/users" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg dark:text-white hover:bg-gray-100 dark:hover:bg-gray-700 group">
                            <span class="ml-3">Cluster Users</span>
                        </a>
                    </li>
                </ul>
                <ul class="pt-5 mt-5 space-y-2 border-t border-gray-200 dark:border-gray-700">
                    <li>
                        <a href="/admin/license" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg transition duration-75 hover:bg-gray-100 dark:hover:bg-gray-700 dark:text-white group">
                            <span class="ml-3">Enterprise License</span>
                        </a>
                        <a href="/admin/settings" class="flex items-center p-2 text-base font-normal text-gray-900 rounded-lg transition duration-75 hover:bg-gray-100 dark:hover:bg-gray-700 dark:text-white group">
                            <span class="ml-3">Cluster Settings</span>
                        </a>
                    </li>
                </ul>
            </div>
        </aside>
    }
}
