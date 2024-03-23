#[tokio::main]
pub async fn main() {
    let _result = setup_log().await;

    if let Err(e) = lapdev_ws::server::run().await {
        tracing::error!("lapdev-ws server run error: {e:#}");
    }
}

async fn setup_log() -> Result<tracing_appender::non_blocking::WorkerGuard, anyhow::Error> {
    let folder = "/var/lib/lapdev/logs";
    if !tokio::fs::try_exists(folder).await.unwrap_or(false) {
        tokio::fs::create_dir_all(folder).await?;
        let _ = tokio::process::Command::new("chown")
            .arg("-R")
            .arg("/var/lib/lapdev/")
            .output()
            .await;
    }
    let file_appender = tracing_appender::rolling::Builder::new()
        .max_log_files(30)
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("lapdev-ws.log")
        .build(folder)?;
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    let filter = tracing_subscriber::EnvFilter::default()
        .add_directive("lapdev_ws=info".parse()?)
        .add_directive("lapdev_rpc=info".parse()?)
        .add_directive("lapdev_common=info".parse()?);
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter(filter)
        .with_writer(non_blocking)
        .init();
    Ok(guard)
}
