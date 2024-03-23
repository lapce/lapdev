use std::collections::HashMap;

use serde::Deserialize;

pub type DevContainerCwd = std::path::PathBuf;

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DevContainerConfig {
    pub name: Option<String>,
    pub image: Option<String>,
    pub build: Option<BuildConfig>,
    #[serde(default)]
    pub forward_ports: Vec<u16>,
    pub on_create_command: Option<DevContainerLifeCycleCmd>,
    pub update_content_command: Option<DevContainerLifeCycleCmd>,
    pub post_create_command: Option<DevContainerLifeCycleCmd>,
    #[serde(default)]
    pub run_args: Vec<String>,
    pub docker_compose_file: Option<String>,

    pub service: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum DevContainerCmd {
    Simple(String),
    Args(Vec<String>),
}

#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum DevContainerLifeCycleCmd {
    Simple(String),
    Args(Vec<String>),
    Object(HashMap<String, DevContainerCmd>),
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BuildConfig {
    pub dockerfile: Option<String>,
    pub context: Option<String>,
    #[serde(default)]
    pub args: HashMap<String, String>,
}
