use std::{env, net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::{anyhow, Context, Result};
use axum::Router;
use clap::Parser;
use lapdev_conductor::Conductor;
use lapdev_db::api::DbApi;
use tokio::{net::TcpListener, time::Duration};
use tracing::{error, info, warn};

use crate::{
    router::{self, add_forward_middleware},
    state::CoreState,
};

pub const LAPDEV_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone)]
struct LapdevConfig {
    db_url: String,
    bind_addr: String,
    http_port: u16,
    ssh_proxy_port: u16,
    ssh_proxy_display_port: u16,
    force_osuser: Option<String>,
    preview_url_proxy_port: u16,
}

const DB_URL_ENV: &str = "LAPDEV_DB_URL";
const BIND_ADDR_ENV: &str = "LAPDEV_BIND_ADDR";
const HTTP_PORT_ENV: &str = "LAPDEV_HTTP_PORT";
const SSH_PROXY_PORT_ENV: &str = "LAPDEV_SSH_PROXY_PORT";
const SSH_PROXY_DISPLAY_PORT_ENV: &str = "LAPDEV_SSH_PROXY_DISPLAY_PORT";
const FORCE_OSUSER_ENV: &str = "LAPDEV_FORCE_OSUSER";
const PREVIEW_PROXY_PORT_ENV: &str = "LAPDEV_PREVIEW_URL_PROXY_PORT";

impl LapdevConfig {
    fn from_env() -> Result<Self> {
        let db_url = env::var(DB_URL_ENV)
            .map_err(|_| anyhow!("environment variable {DB_URL_ENV} is required"))?;
        let bind_addr = env::var(BIND_ADDR_ENV).unwrap_or_else(|_| "0.0.0.0".to_string());
        let http_port = Self::parse_port(HTTP_PORT_ENV, 8080)?;
        let ssh_proxy_port = Self::parse_port(SSH_PROXY_PORT_ENV, 2222)?;
        let ssh_proxy_display_port = Self::parse_port(SSH_PROXY_DISPLAY_PORT_ENV, 2222)?;
        let preview_url_proxy_port = Self::parse_port(PREVIEW_PROXY_PORT_ENV, 8443)?;

        let force_osuser = match env::var(FORCE_OSUSER_ENV) {
            Ok(value) if value.trim().is_empty() => None,
            Ok(value) => Some(value),
            Err(env::VarError::NotPresent) => None,
            Err(err) => return Err(anyhow!("error reading {FORCE_OSUSER_ENV}: {err}")),
        };

        Ok(Self {
            db_url,
            bind_addr,
            http_port,
            ssh_proxy_port,
            ssh_proxy_display_port,
            force_osuser,
            preview_url_proxy_port,
        })
    }

    fn parse_port(var: &str, default: u16) -> Result<u16> {
        match env::var(var) {
            Ok(value) => value
                .parse::<u16>()
                .map_err(|err| anyhow!("{var} must be a valid u16: {err}")),
            Err(env::VarError::NotPresent) => Ok(default),
            Err(err) => Err(anyhow!("error reading {var}: {err}")),
        }
    }
}

#[derive(Parser)]
#[clap(name = "lapdev")]
#[clap(version = env!("CARGO_PKG_VERSION"))]
struct Cli {
    /// The folder for putting data
    #[clap(short, long, action, value_hint = clap::ValueHint::AnyPath)]
    data_folder: Option<PathBuf>,
}

pub struct ApiServer {
    config: LapdevConfig,
    pub state: Arc<CoreState>,
    pub app: Router,
    pub conductor: Conductor,
}

impl ApiServer {
    pub async fn new(static_dir: Option<include_dir::Dir<'static>>) -> Result<Self> {
        let cli = Cli::parse();
        let data_folder = cli
            .data_folder
            .clone()
            .unwrap_or_else(|| PathBuf::from("/var/lib/lapdev"));

        init_tracing();

        let config = LapdevConfig::from_env()?;
        let db = DbApi::new(&config.db_url).await?;
        let conductor = Conductor::new(
            LAPDEV_VERSION,
            db.clone(),
            data_folder,
            config.force_osuser.clone(),
        )
        .await?;

        let state = Arc::new(
            CoreState::new(
                conductor.clone(),
                config.ssh_proxy_port,
                config.ssh_proxy_display_port,
                static_dir,
            )
            .await,
        );

        let app = router::build_router(state.clone());
        Ok(Self {
            config,
            state,
            conductor,
            app,
        })
    }

    pub async fn run(&self) -> Result<()> {
        {
            let db = self.conductor.db.clone();
            let tunnel_registry = self.state.kube_controller.tunnel_registry.clone();
            let host = self.config.bind_addr.clone();
            let port = self.config.preview_url_proxy_port;
            tokio::spawn(async move {
                loop {
                    let db_clone = db.clone();
                    let tunnel_clone = tunnel_registry.clone();
                    let bind = format!("{host}:{port}");
                    match lapdev_kube::start_preview_url_proxy_server(db_clone, tunnel_clone, &bind)
                        .await
                    {
                        Ok(_) => {
                            info!("Preview URL proxy server exited gracefully");
                            break;
                        }
                        Err(err) => {
                            error!("Preview URL proxy server failed: {err:?}");
                            warn!("Retrying preview URL proxy server in 5 seconds");
                            tokio::time::sleep(Duration::from_secs(5)).await;
                        }
                    }
                }
            });
        }

        let app_with_forward = add_forward_middleware(self.state.clone(), self.app.clone());

        let bind = format!(
            "{}:{}",
            self.config.bind_addr.clone(),
            self.config.http_port
        );
        let tcp_listener = TcpListener::bind(&bind)
            .await
            .with_context(|| format!("bind to {bind}"))?;

        info!(%bind, "Starting Axum HTTP server");

        if let Err(err) = axum::serve(
            tcp_listener,
            app_with_forward.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        {
            tracing::error!("http server stopped error: {err}");
        }

        Ok(())
    }
}

pub async fn start(static_dir: Option<include_dir::Dir<'static>>) {
    match ApiServer::new(static_dir).await {
        Ok(server) => {
            if let Err(e) = server.run().await {
                tracing::error!("lapdev api server error: {e:#}");
            }
        }
        Err(e) => tracing::error!("failed to initialize lapdev api server: {e:#}"),
    }
}

fn init_tracing() {
    let var = std::env::var("RUST_LOG").unwrap_or_default();
    let var =
        format!("error,lapdev=info,lapdev_api=info,lapdev_conductor=info,lapdev_rpc=info,lapdev_common=info,lapdev_db=info,lapdev_enterprise=info,lapdev_proxy_ssh=info,lapdev_proxy_http=info,lapdev_kube=info,lapdev_kube_manager=info,{var}");
    let filter = tracing_subscriber::EnvFilter::builder().parse_lossy(var);
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
