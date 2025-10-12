use anyhow::Result;
use clap::Parser;
use lapdev_kube_sidecar_proxy::SidecarProxyServer;
use tracing::{info, level_filters::LevelFilter};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Proxy listen address
    #[arg(long, default_value = "0.0.0.0:8080")]
    listen_addr: String,

    /// Target service address (where to proxy requests)
    #[arg(long, default_value = "127.0.0.1:3000")]
    target_addr: String,

    /// Kubernetes namespace to watch for services
    #[arg(long, env = "KUBERNETES_NAMESPACE")]
    namespace: Option<String>,

    /// Pod name (for self-identification)
    #[arg(long, env = "HOSTNAME")]
    pod_name: Option<String>,

    /// Lapdev environment ID
    #[arg(long, env = "LAPDEV_ENVIRONMENT_ID")]
    environment_id: Option<String>,

    /// Lapdev environment auth token
    #[arg(long, env = "LAPDEV_ENVIRONMENT_AUTH_TOKEN")]
    environment_auth_token: Option<String>,

    /// Log level
    #[arg(long, default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize tracing
    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy()
        .add_directive(format!("lapdev_kube_sidecar_proxy={}", args.log_level).parse()?);

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(filter)
        .init();

    info!("Starting Lapdev Kubernetes Sidecar Proxy");
    info!("Listen address: {}", args.listen_addr);
    info!("Target address: {}", args.target_addr);
    info!("Namespace: {:?}", args.namespace);
    info!("Pod name: {:?}", args.pod_name);

    // Validate that environment ID and auth token are present
    let environment_id = args
        .environment_id
        .ok_or_else(|| anyhow::anyhow!("LAPDEV_ENVIRONMENT_ID environment variable is required"))?;
    let environment_auth_token = args.environment_auth_token.ok_or_else(|| {
        anyhow::anyhow!("LAPDEV_ENVIRONMENT_AUTH_TOKEN environment variable is required")
    })?;

    info!("Environment ID: {}", environment_id);

    let server = SidecarProxyServer::new(
        args.listen_addr.parse()?,
        args.target_addr.parse()?,
        args.namespace,
        args.pod_name,
        environment_id,
        environment_auth_token,
    )
    .await?;

    // Graceful shutdown handling
    let shutdown_signal = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install CTRL+C signal handler");
        info!("Received shutdown signal");
    };

    server.serve_with_graceful_shutdown(shutdown_signal).await?;

    info!("Sidecar proxy shut down gracefully");
    Ok(())
}
