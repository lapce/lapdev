#[tokio::main]
pub async fn main() {
    let _result = setup_log().await;

    if let Err(e) = lapdev_api::server::run().await {
        tracing::error!("lapdev api start server error: {e:#}");
    }
}

async fn setup_log() -> Result<tracing_appender::non_blocking::WorkerGuard, anyhow::Error> {
    let folder = "/var/lib/lapdev/logs";
    tokio::fs::create_dir_all(folder).await?;
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
