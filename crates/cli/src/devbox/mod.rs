use clap::Subcommand;

pub mod commands;
pub mod dns;
mod websocket_transport;

const DEFAULT_LAPDEV_URL: &str = "https://app.lap.dev";

#[derive(Subcommand)]
pub enum DevboxCommand {
    /// Connect to Lapdev devbox and establish port forwarding tunnels
    Connect {
        /// API server URL
        #[arg(long, env = "LAPDEV_API_URL", default_value = DEFAULT_LAPDEV_URL)]
        api_url: String,
    },
}

pub async fn handle_command(command: DevboxCommand) -> anyhow::Result<()> {
    match command {
        DevboxCommand::Connect { api_url } => {
            commands::connect::execute(&api_url).await?;
        }
    }

    Ok(())
}
