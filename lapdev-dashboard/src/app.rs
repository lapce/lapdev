use anyhow::Result;
use gloo_net::http::Request;
use lapdev_common::{console::MeUser, ClusterInfo};
use leptos::{
    component, create_local_resource, create_rw_signal, provide_context, use_context, view, window,
    IntoView, Resource, Show, Signal, SignalGet,
};
use leptos_router::{Route, Router, Routes};

use crate::{
    account::{get_login, AccountSettings, JoinView, Login},
    audit_log::AuditLogView,
    cluster::{ClusterSettings, ClusterUsersView, MachineTypeView, WorkspaceHostView},
    license::{LicenseView, SignLicenseView},
    nav::{AdminSideNav, NavExpanded, SideNav, TopNav},
    organization::{NewOrgModal, OrgMembers, OrgSettings},
    project::{ProjectDetails, Projects},
    quota::QuotaView,
    ssh_key::SshKeys,
    usage::UsageView,
    workspace::{WorkspaceDetails, Workspaces},
};

#[component]
fn Root() -> impl IntoView {
    view! {
        <Workspaces />
    }
}

async fn get_cluster_info() -> Result<ClusterInfo> {
    let resp = Request::get("/api/v1/cluster_info").send().await?;
    let info: ClusterInfo = resp.json().await?;
    Ok(info)
}

#[component]
pub fn App() -> impl IntoView {
    let login_counter = create_rw_signal(0);
    provide_context(login_counter);

    let login = create_local_resource(
        move || login_counter.get(),
        |_| async move { get_login().await.ok() },
    );
    provide_context(login);

    let login = create_local_resource(
        move || login_counter.get(),
        |_| async move { get_login().await.ok() },
    );
    provide_context(login);

    let cluster_info =
        create_local_resource(move || (), |_| async move { get_cluster_info().await.ok() });
    let cluster_info = Signal::derive(move || cluster_info.get().flatten());
    provide_context(cluster_info);

    let current_org = Signal::derive(move || {
        let login = login.get().flatten();
        login.map(|u| u.organization)
    });
    provide_context(current_org);

    let pathname = window().location().pathname().unwrap_or_default();
    let nav_expanded = NavExpanded {
        orgnization: create_rw_signal(pathname.starts_with("/organization")),
        account: create_rw_signal(pathname.starts_with("/account")),
    };
    provide_context(nav_expanded);

    view! {
        <Router>
            <Routes>
                <Route path="/" view=move || view! { <WrappedView element=Root /> } />
                <Route path="/projects" view=move || view! { <WrappedView element=Projects /> } />
                <Route path="/projects/:id" view=move || view! { <WrappedView element=ProjectDetails /> } />
                <Route path="/workspaces" view=move || view! { <WrappedView element=Workspaces /> } />
                <Route path="/workspaces/:name" view=move || view! { <WrappedView element=WorkspaceDetails /> } />
                <Route path="/organization/usage" view=move || view! { <WrappedView element=UsageView /> } />
                <Route path="/organization/members" view=move || view! { <WrappedView element=OrgMembers /> } />
                <Route path="/organization/quota" view=move || view! { <WrappedView element=QuotaView /> } />
                <Route path="/organization/audit_log" view=move || view! { <WrappedView element=AuditLogView /> } />
                <Route path="/organization/settings" view=move || view! { <WrappedView element=OrgSettings /> } />
                <Route path="/account" view=move || view! { <WrappedView element=AccountSettings /> } />
                <Route path="/join/:id" view=move || view! { <WrappedView element=JoinView /> } />
                <Route path="/account/ssh-keys" view=move || view! { <WrappedView element=SshKeys /> } />
                <Route path="/admin" view=move || view! { <AdminWrappedView element=WorkspaceHostView /> } />
                <Route path="/admin/workspace_hosts" view=move || view! { <AdminWrappedView element=WorkspaceHostView /> } />
                <Route path="/admin/machine_types" view=move || view! { <AdminWrappedView element=MachineTypeView /> } />
                <Route path="/admin/users" view=move || view! { <AdminWrappedView element=ClusterUsersView /> } />
                <Route path="/admin/settings" view=move || view! { <AdminWrappedView element=ClusterSettings /> } />
                <Route path="/admin/license" view=move || view! { <AdminWrappedView element=LicenseView /> } />
                <Route path="/admin/sign_license" view=move || view! { <AdminWrappedView element=SignLicenseView /> } />
            </Routes>
        </Router>

    }
}

#[component]
pub fn WrappedView<T>(element: T) -> impl IntoView
where
    T: IntoView + Copy + 'static,
{
    let login = use_context::<Resource<i32, Option<MeUser>>>().unwrap();
    let new_org_modal_hidden = create_rw_signal(true);
    view! {
        <Show
            when=move || { login.get().flatten().is_some() }
            fallback=move || view! { <Login /> }
        >
            <div class="flex flex-col h-screen">
                <TopNav />
                <div class="container mx-auto flex flex-row basis-0 grow">
                    <SideNav new_org_modal_hidden />
                    <div class="px-8 pt-8 w-[calc(100%-16rem)] h-full">
                        {element}
                    </div>
                </div>
                <NewOrgModal modal_hidden=new_org_modal_hidden />
            </div>
        </Show>
    }
}

#[component]
pub fn AdminWrappedView<T>(element: T) -> impl IntoView
where
    T: IntoView + Copy + 'static,
{
    let login = use_context::<Resource<i32, Option<MeUser>>>().unwrap();
    view! {
        <Show
            when=move || { login.get().flatten().is_some() }
            fallback=move || view! { <Login /> }
        >
            <div class="flex flex-col h-screen">
                <TopNav />
                <div class="container mx-auto flex flex-row basis-0 grow">
                    <AdminSideNav />
                    <div class="px-8 pt-8 w-[calc(100%-16rem)] h-full">
                        {element}
                    </div>
                </div>
            </div>
        </Show>
    }
}
