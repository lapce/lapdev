use anyhow::Result;
use gloo_net::http::Request;
use lapdev_common::{console::MeUser, ClusterInfo};
use leptos::prelude::*;
use leptos_router::{
    components::{Route, Router, Routes},
    path,
};

use crate::{
    account::{get_login, AccountSettings, JoinView, Login},
    audit_log::AuditLogView,
    cluster::{ClusterSettings, ClusterUsersView, MachineTypeView, WorkspaceHostView},
    git_provider::GitProviderView,
    license::{LicenseView, SignLicenseView},
    nav::{AdminSideNav, NavExpanded, SideNav, TopNav},
    organization::{NewOrgModal, OrgMembers, OrgSettings},
    project::{ProjectDetails, Projects},
    quota::QuotaView,
    ssh_key::SshKeys,
    usage::UsageView,
    workspace::{WorkspaceDetails, Workspaces},
};

#[derive(Clone)]
pub struct AppConfig {
    pub show_lapdev_website: bool,
}

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

pub fn set_context() {
    let login_counter = RwSignal::new(0);
    provide_context(login_counter);

    let login = LocalResource::new(|| async move { get_login().await.ok() });
    Effect::new(move |_| {
        login_counter.track();
        login.refetch();
    });
    provide_context(login);

    let cluster_info = LocalResource::new(|| async move { get_cluster_info().await.ok() });
    let cluster_info = Signal::derive(move || cluster_info.get().as_deref().cloned().flatten());
    provide_context(cluster_info);

    let current_org = Signal::derive(move || {
        let login = login.get().as_deref().cloned().flatten();
        login.map(|u| u.organization)
    });
    provide_context(current_org);

    let pathname = window().location().pathname().unwrap_or_default();
    let nav_expanded = NavExpanded {
        orgnization: RwSignal::new(pathname.starts_with("/organization")),
        account: RwSignal::new(pathname.starts_with("/account")),
    };
    provide_context(nav_expanded);

    provide_context(RwSignal::new(AppConfig {
        show_lapdev_website: false,
    }));
}

#[component]
pub fn App() -> impl IntoView {
    set_context();
    view! {
        <Router>
            <Routes fallback=|| "Not found.">
                <Route path=path!("/") view=move || view! { <WrappedView element=Root /> } />
                <Route path=path!("/projects") view=move || view! { <WrappedView element=Projects /> } />
                <Route path=path!("/projects/:id") view=move || view! { <WrappedView element=ProjectDetails /> } />
                <Route path=path!("/workspaces") view=move || view! { <WrappedView element=Workspaces /> } />
                <Route path=path!("/workspaces/:name") view=move || view! { <WrappedView element=WorkspaceDetails /> } />
                <Route path=path!("/organization/usage") view=move || view! { <WrappedView element=UsageView /> } />
                <Route path=path!("/organization/members") view=move || view! { <WrappedView element=OrgMembers /> } />
                <Route path=path!("/organization/quota") view=move || view! { <WrappedView element=QuotaView /> } />
                <Route path=path!("/organization/audit_log") view=move || view! { <WrappedView element=AuditLogView /> } />
                <Route path=path!("/organization/settings") view=move || view! { <WrappedView element=OrgSettings /> } />
                <Route path=path!("/account") view=move || view! { <WrappedView element=AccountSettings /> } />
                <Route path=path!("/join/:id") view=move || view! { <WrappedView element=JoinView /> } />
                <Route path=path!("/account/ssh-keys") view=move || view! { <WrappedView element=SshKeys /> } />
                <Route path=path!("/account/git-providers") view=move || view! { <WrappedView element=GitProviderView /> } />
                <Route path=path!("/admin") view=move || view! { <AdminWrappedView element=WorkspaceHostView /> } />
                <Route path=path!("/admin/workspace_hosts") view=move || view! { <AdminWrappedView element=WorkspaceHostView /> } />
                <Route path=path!("/admin/machine_types") view=move || view! { <AdminWrappedView element=MachineTypeView /> } />
                <Route path=path!("/admin/users") view=move || view! { <AdminWrappedView element=ClusterUsersView /> } />
                <Route path=path!("/admin/settings") view=move || view! { <AdminWrappedView element=ClusterSettings /> } />
                <Route path=path!("/admin/license") view=move || view! { <AdminWrappedView element=LicenseView /> } />
                <Route path=path!("/admin/sign_license") view=move || view! { <AdminWrappedView element=SignLicenseView /> } />
            </Routes>
        </Router>
    }
}

#[component]
pub fn WrappedView<T>(element: T) -> impl IntoView
where
    T: IntoView + Copy + 'static + Send + Sync,
{
    let login = use_context::<LocalResource<Option<MeUser>>>().unwrap();
    let new_org_modal_hidden = RwSignal::new(true);
    view! {
        <Show
            when=move || { login.get().as_deref().flatten().is_some() }
            fallback=move || view! { <Login /> }
        >
            <div class="flex flex-col h-screen">
                <TopNav />
                <div class="container mx-auto flex flex-row basis-0 grow">
                    <SideNav new_org_modal_hidden />
                    <div class="p-8 w-[calc(100%-16rem)] h-full">
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
    T: IntoView + Copy + 'static + Send + Sync,
{
    let login = use_context::<LocalResource<Option<MeUser>>>().unwrap();
    view! {
        <Show
            when=move || { login.get().as_deref().flatten().is_some() }
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
