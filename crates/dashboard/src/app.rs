use std::sync::Arc;

use anyhow::Result;
use gloo_net::http::Request;
use lapdev_api_hrpc::HrpcServiceClient;
use lapdev_common::{console::MeUser, ClusterInfo};
use leptos::prelude::*;
use leptos_router::{
    components::{Route, Router, Routes},
    path,
};

use crate::{
    account::{get_login, AccountSettings, JoinView, Login},
    audit_log::AuditLogView,
    cli_auth::CliAuth,
    cli_success::CliSuccess,
    cluster::{ClusterSettings, ClusterUsersView, MachineTypeView, WorkspaceHostView},
    component::sidebar::SidebarData,
    git_provider::GitProviderView,
    home::DashboardHome,
    kube_app_catalog::KubeAppCatalog,
    kube_app_catalog_detail::KubeAppCatalogDetail,
    kube_app_catalog_workload::KubeAppCatalogWorkload,
    kube_cluster::KubeCluster,
    kube_environment::{EnvironmentType, KubeEnvironment},
    kube_environment_detail::KubeEnvironmentDetail,
    kube_environment_workload::KubeEnvironmentWorkload,
    kube_resource::KubeResource,
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
    pub show_lapdev_website: RwSignal<bool>,
    pub environment_type: RwSignal<EnvironmentType>,
    pub current_page: RwSignal<String>,
    pub header_links: RwSignal<Vec<(String, String)>>,
}

#[component]
fn Root() -> impl IntoView {
    view! {
        <DashboardHome />
    }
}

pub fn get_hrpc_client() -> HrpcServiceClient {
    HrpcServiceClient::new("/api/rpc".to_string())
}

async fn get_cluster_info() -> Result<ClusterInfo> {
    let resp = Request::get("/api/v1/cluster_info").send().await?;
    let info: ClusterInfo = resp.json().await?;
    Ok(info)
}

pub fn set_context() {
    let login_counter = RwSignal::new_local(0);
    provide_context(login_counter);

    let login_fetching = RwSignal::new_local(true);
    provide_context(login_fetching);

    let login_fetching_for_resource = login_fetching;
    let login = LocalResource::new(move || {
        login_fetching_for_resource.set(true);
        async move {
            let result = get_login().await.ok();
            login_fetching_for_resource.set(false);
            result
        }
    });
    Effect::new(move |_| {
        login_counter.track();
        login_fetching.set(true);
        login.refetch();
    });
    provide_context(login);

    let cluster_info = LocalResource::new(|| async move { get_cluster_info().await.ok() });
    let cluster_info = Signal::derive_local(move || cluster_info.get().flatten());
    provide_context(cluster_info);

    let current_org = Signal::derive_local(move || {
        let login = login.get().flatten();
        login.map(|u| u.organization)
    });
    provide_context(current_org);

    let pathname = window().location().pathname().unwrap_or_default();
    let nav_expanded = NavExpanded {
        orgnization: RwSignal::new(pathname.starts_with("/organization")),
        account: RwSignal::new(pathname.starts_with("/account")),
        k8s_environments: RwSignal::new(pathname.starts_with("/kubernetes/environments")),
    };
    provide_context(nav_expanded);

    provide_context(AppConfig {
        show_lapdev_website: RwSignal::new(false),
        environment_type: RwSignal::new(EnvironmentType::Personal),
        current_page: RwSignal::new(String::new()),
        header_links: RwSignal::new(Vec::new()),
    });

    provide_context(SidebarData {
        open: RwSignal::new(true),
    });

    provide_context(Arc::new(HrpcServiceClient::new("/api/rpc".to_string())));
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
                <Route path=path!("/kubernetes/catalogs") view=move || view! { <WrappedView element=KubeAppCatalog /> } />
                <Route path=path!("/kubernetes/catalogs/:catalog_id") view=move || view! { <WrappedView element=KubeAppCatalogDetail /> } />
                <Route path=path!("/kubernetes/catalogs/:catalog_id/workloads/:workload_id") view=move || view! { <WrappedView element=KubeAppCatalogWorkload /> } />
                <Route path=path!("/kubernetes/environments") view=move || view! { <WrappedView element=KubeEnvironment /> } />
                <Route path=path!("/kubernetes/environments/personal") view=move || view! { <WrappedView element=KubeEnvironment /> } />
                <Route path=path!("/kubernetes/environments/shared") view=move || view! { <WrappedView element=KubeEnvironment /> } />
                <Route path=path!("/kubernetes/environments/branch") view=move || view! { <WrappedView element=KubeEnvironment /> } />
                <Route path=path!("/kubernetes/environments/:environment_id") view=move || view! { <WrappedView element=KubeEnvironmentDetail /> } />
                <Route path=path!("/kubernetes/environments/:environment_id/workloads/:workload_id") view=move || view! { <WrappedView element=KubeEnvironmentWorkload /> } />
                <Route path=path!("/kubernetes/clusters/:cluster_id") view=move || view! { <WrappedView element=KubeResource /> } />
                <Route path=path!("/kubernetes/clusters") view=move || view! { <WrappedView element=KubeCluster /> } />
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
                <Route path=path!("/auth/cli") view=move || view! { <WrappedView element=CliAuth /> } />
                <Route path=path!("/cli/success") view=CliSuccess />
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
    let login_fetching = expect_context::<RwSignal<bool, LocalStorage>>();
    let new_org_modal_hidden = RwSignal::new_local(true);
    view! {
        <Show
            when=move || { login.get().flatten().is_some() }
            fallback=move || {
                let login_fetching = login_fetching;
                view! {
                    <Show
                        when=move || { !login_fetching.get() }
                        fallback=move || view! { <AppLoading /> }
                    >
                        <Login />
                    </Show>
                }
            }
        >
            <div class="flex flex-col h-screen">
                <TopNav />
                <div class="container mx-auto flex flex-row basis-0 grow">
                    <SideNav />
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
    let login_fetching = expect_context::<RwSignal<bool, LocalStorage>>();
    view! {
        <Show
            when=move || { login.get().flatten().is_some() }
            fallback=move || {
                let login_fetching = login_fetching;
                view! {
                    <Show
                        when=move || { !login_fetching.get() }
                        fallback=move || view! { <AppLoading /> }
                    >
                        <Login />
                    </Show>
                }
            }
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

#[component]
fn AppLoading() -> impl IntoView {
    view! {
        <div class="flex h-screen items-center justify-center bg-slate-50">
            <div class="flex flex-col items-center gap-3 text-sm text-slate-500">
                <span class="inline-block h-6 w-6 animate-spin rounded-full border-2 border-slate-300 border-t-sky-500"></span>
                Loading your workspace...
            </div>
        </div>
    }
}
