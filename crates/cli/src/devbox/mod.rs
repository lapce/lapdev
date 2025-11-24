use clap::Subcommand;

use crate::config;

pub mod commands;
pub mod dns;

#[derive(Subcommand)]
pub enum DevboxCommand {
    /// Connect to Lapdev devbox and establish port forwarding tunnels
    Connect {
        /// API host (e.g. app.lap.dev)
        #[arg(long = "api-host", alias = "api-url", env = "LAPDEV_API_HOST")]
        api_host: Option<String>,
    },
}

pub async fn handle_command(command: DevboxCommand) -> anyhow::Result<()> {
    match command {
        DevboxCommand::Connect { api_host } => {
            let (api_host, _) = config::resolve_api_base_url(api_host);
            commands::connect::execute(&api_host).await?;
        }
    }

    Ok(())
}
