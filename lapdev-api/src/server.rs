use std::{path::PathBuf, sync::Arc};

use anyhow::{anyhow, Context, Result};
use axum::{extract::Request, Router};
use clap::Parser;
use futures_util::pin_mut;
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo};
use lapdev_conductor::Conductor;
use lapdev_db::api::DbApi;
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tower::Service;
use tracing::error;

use crate::{cert::tls_config, router, state::CoreState};

pub const LAPDEV_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
struct LapdevConfig {
    db: Option<String>,
    bind: Option<String>,
    http_port: Option<u16>,
    https_port: Option<u16>,
    ssh_proxy_port: Option<u16>,
}

#[derive(Parser)]
#[clap(name = "lapdev")]
#[clap(version = env!("CARGO_PKG_VERSION"))]
struct Cli {
    /// The config file path
    #[clap(short, long, action, value_hint = clap::ValueHint::AnyPath)]
    config_file: Option<PathBuf>,
    /// The folder for putting logs
    #[clap(short, long, action, value_hint = clap::ValueHint::AnyPath)]
    logs_folder: Option<PathBuf>,
    /// Don't run db migration on startup
    #[clap(short, long, action)]
    no_migration: bool,
}

pub async fn start(additional_router: Option<Router<CoreState>>) {
    let cli = Cli::parse();

    let _result = setup_log(&cli).await;

    if let Err(e) = run(&cli, additional_router).await {
        tracing::error!("lapdev api start server error: {e:#}");
    }
}

async fn run(cli: &Cli, additional_router: Option<Router<CoreState>>) -> Result<()> {
    let config_file = cli
        .config_file
        .clone()
        .unwrap_or_else(|| PathBuf::from("/etc/lapdev.conf"));
    let config_content = tokio::fs::read_to_string(&config_file)
        .await
        .with_context(|| format!("can't read config file {}", config_file.to_string_lossy()))?;
    let config: LapdevConfig =
        toml::from_str(&config_content).with_context(|| "wrong config file format")?;
    let db_url = config
        .db
        .ok_or_else(|| anyhow!("can't find database url in your config file"))?;

    let db = DbApi::new(&db_url, cli.no_migration).await?;
    let conductor = Conductor::new(LAPDEV_VERSION, db.clone()).await?;

    let ssh_proxy_port = config.ssh_proxy_port.unwrap_or(2222);
    {
        let conductor = conductor.clone();
        let bind = config.bind.clone();
        tokio::spawn(async move {
            if let Err(e) = lapdev_proxy_ssh::server::run(
                conductor,
                bind.as_deref().unwrap_or("0.0.0.0"),
                ssh_proxy_port,
            )
            .await
            {
                error!("ssh proxy error: {e}");
            }
        });
    }

    let state = CoreState::new(conductor, ssh_proxy_port).await;
    let app = router::build_router(state.clone(), additional_router).await;
    let certs = state.certs.clone();

    {
        // start http server
        let bind = format!(
            "{}:{}",
            config.bind.clone().unwrap_or_else(|| "0.0.0.0".to_string()),
            config.http_port.unwrap_or(80)
        );
        let tcp_listener = TcpListener::bind(&bind)
            .await
            .with_context(|| format!("bind to {bind}"))?;
        let app = app.clone();
        tokio::spawn(async move {
            if let Err(err) = axum::serve(tcp_listener, app.into_make_service()).await {
                tracing::error!("http server stopped error: {err}");
            }
        });
    }

    let bind = format!(
        "{}:{}",
        config.bind.unwrap_or_else(|| "0.0.0.0".to_string()),
        config.https_port.unwrap_or(443)
    );
    let tcp_listener = TcpListener::bind(&bind)
        .await
        .with_context(|| format!("bind to {bind}"))?;
    let tls_acceptor = TlsAcceptor::from(Arc::new(tls_config(certs)?));

    pin_mut!(tcp_listener);
    loop {
        let tower_service = app.clone();
        let tls_acceptor = tls_acceptor.clone();

        // Wait for new tcp connection
        let (cnx, addr) = tcp_listener.accept().await?;

        tokio::spawn(async move {
            // Wait for tls handshake to happen
            let stream = match tls_acceptor.accept(cnx).await {
                Err(_) => {
                    return;
                }
                Ok(stream) => stream,
            };

            // Hyper has its own `AsyncRead` and `AsyncWrite` traits and doesn't use tokio.
            // `TokioIo` converts between them.
            let stream = TokioIo::new(stream);

            // Hyper also has its own `Service` trait and doesn't use tower. We can use
            // `hyper::service::service_fn` to create a hyper `Service` that calls our app through
            // `tower::Service::call`.
            let hyper_service = hyper::service::service_fn(move |request: Request<Incoming>| {
                // We have to clone `tower_service` because hyper's `Service` uses `&self` whereas
                // tower's `Service` requires `&mut self`.
                //
                // We don't need to call `poll_ready` since `Router` is always ready.
                tower_service.clone().call(request)
            });

            let ret = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                .serve_connection_with_upgrades(stream, hyper_service)
                .await;

            if let Err(err) = ret {
                tracing::warn!("error serving connection from {}: {}", addr, err);
            }
        });
    }
}

async fn setup_log(
    cli: &Cli,
) -> Result<tracing_appender::non_blocking::WorkerGuard, anyhow::Error> {
    let folder = cli
        .logs_folder
        .clone()
        .unwrap_or_else(|| PathBuf::from("/var/lib/lapdev/logs"));
    tokio::fs::create_dir_all(&folder).await?;
    let file_appender = tracing_appender::rolling::Builder::new()
        .max_log_files(30)
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("lapdev.log")
        .build(folder)?;
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    let filter = tracing_subscriber::EnvFilter::default()
        .add_directive("lapdev=info".parse()?)
        .add_directive("lapdev_api=info".parse()?)
        .add_directive("lapdev_conductor=info".parse()?)
        .add_directive("lapdev_rpc=info".parse()?)
        .add_directive("lapdev_common=info".parse()?)
        .add_directive("lapdev_db=info".parse()?)
        .add_directive("lapdev_enterprise=info".parse()?)
        .add_directive("lapdev_proxy_ssh=info".parse()?)
        .add_directive("lapdev_proxy_http=info".parse()?);
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter(filter)
        .with_writer(non_blocking)
        .init();
    Ok(guard)
}
