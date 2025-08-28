use thiserror::Error;

#[derive(Error, Debug)]
pub enum SidecarProxyError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Kubernetes API error: {0}")]
    Kubernetes(#[from] kube::Error),

    #[error("HTTP proxy error: {0}")]
    Http(#[from] hyper::Error),

    #[error("HTTP error: {0}")]
    HttpRequest(#[from] hyper::http::Error),

    #[error("Kubernetes watcher error: {0}")]
    Watcher(#[from] kube::runtime::watcher::Error),

    #[error("Service discovery error: {0}")]
    ServiceDiscovery(String),

    #[error("Target service not found: {service_name}")]
    TargetNotFound { service_name: String },

    #[error("Authorization error: {0}")]
    Authorization(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("YAML error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("Connection error: {0}")]
    Connection(String),

    #[error("RPC error: {0}")]
    Rpc(String),

    #[error("Generic error: {0}")]
    Generic(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, SidecarProxyError>;