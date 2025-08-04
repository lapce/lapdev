use tracing_subscriber::prelude::*;

#[tokio::main]
pub async fn main() {
    // Initialize tracing with journald
    let var = std::env::var("RUST_LOG").unwrap_or_default();
    let var = format!(
        "error,lapdev_kube_manager=info,lapdev_kube=info,lapdev_rpc=info,lapdev_common=info,{var}"
    );
    let filter = tracing_subscriber::EnvFilter::builder().parse_lossy(var);

    match tracing_journald::layer() {
        Ok(journald_layer) => {
            tracing_subscriber::registry()
                .with(filter)
                .with(journald_layer)
                .init();
        }
        Err(_) => {
            // Fallback to stdout if journald is not available
            tracing_subscriber::fmt().with_env_filter(filter).init();
        }
    }

    if let Err(e) = lapdev_kube_manager::manager::KubeManager::connect_cluster().await {
        tracing::error!("error: {e:?}");
    }
}
