pub mod account;
pub mod app;
pub mod audit_log;
pub mod cli_auth;
pub mod cli_success;
pub mod cluster;
pub mod component;
pub mod datepicker;
pub mod git_provider;
pub mod home;
pub mod kube_app_catalog;
pub mod kube_app_catalog_detail;
pub mod kube_app_catalog_workload;
pub mod kube_cluster;
pub mod kube_container;
pub mod kube_environment;
pub mod kube_environment_detail;
pub mod kube_environment_preview_url;
pub mod kube_environment_workload;
pub mod kube_resource;
pub mod license;
pub mod modal;
pub mod nav;
pub mod organization;
pub mod project;
pub mod quota;
pub mod sse;
pub mod ssh_key;
pub mod usage;
pub mod workspace;

pub const DOCS_URL: &str = "https://lapdev.gitbook.io/docs/";
pub const DOCS_ENVIRONMENT_PATH: &str = "how-to-guides/create-lapdev-environment";
pub const DOCS_APP_CATALOG_PATH: &str = "how-to-guides/create-an-app-catalog";
pub const DOCS_CLUSTER_PATH: &str = "how-to-guides/connect-your-kubernetes-cluster";
pub const DOCS_DEVBOX_PATH: &str = "how-to-guides/local-development-with-devbox";

#[inline]
pub fn docs_url(path: &str) -> String {
    format!("{DOCS_URL}{path}")
}
