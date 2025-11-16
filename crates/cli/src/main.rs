use std::{fs, path::PathBuf};

use anyhow::Context;
use clap::{Parser, Subcommand};

mod api;
mod auth;
mod config;
mod devbox;

#[derive(Parser)]
#[command(name = "lapdev")]
#[command(about = "Lapdev CLI - Self-Hosted Remote Dev Environment", long_about = None)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Directory for lapdev-cli log files
    #[arg(long = "log-dir", env = "LAPDEV_LOG_DIR", value_name = "PATH")]
    log_dir: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// Authenticate with Lapdev
    Login {
        /// Device name (defaults to hostname)
        #[arg(long)]
        device_name: Option<String>,

        /// API host (e.g. app.lap.dev)
        #[arg(long = "api-host", alias = "api-url", env = "LAPDEV_API_HOST")]
        api_host: Option<String>,
    },

    /// Sign out (deletes token from keychain)
    Logout {
        /// API host (e.g. app.lap.dev)
        #[arg(long = "api-host", alias = "api-url", env = "LAPDEV_API_HOST")]
        api_host: Option<String>,
    },

    /// Show current session info
    Whoami {
        /// API host (e.g. app.lap.dev)
        #[arg(long = "api-host", alias = "api-url", env = "LAPDEV_API_HOST")]
        api_host: Option<String>,
    },

    /// Devbox commands
    Devbox {
        #[command(subcommand)]
        command: devbox::DevboxCommand,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let log_dir = resolve_log_dir(cli.log_dir.clone())?;
    fs::create_dir_all(&log_dir)
        .with_context(|| format!("failed to create log directory at {}", log_dir.display()))?;
    let file_appender = tracing_appender::rolling::daily(&log_dir, "lapdev-cli.log");
    let (file_writer, _file_guard) = tracing_appender::non_blocking(file_appender);

    // Setup logging
    let log_level = if cli.verbose {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };

    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    // Filter out tarpc internal tracing
    let filter = EnvFilter::from_default_env()
        .add_directive(log_level.into())
        .add_directive("tarpc=warn".parse().unwrap())
        .add_directive("tarpc::client=warn".parse().unwrap()); // Only show tarpc warnings/errors

    let console_layer = fmt::layer()
        .with_target(false)
        .without_time()
        .with_writer(std::io::stderr);

    let file_layer = fmt::layer()
        .with_target(true)
        .with_ansi(false)
        .with_writer(file_writer);

    tracing_subscriber::registry()
        .with(filter)
        .with(console_layer)
        .with(file_layer)
        .init();

    tracing::debug!("writing logs to {}", log_dir.display());

    // Handle commands
    match cli.command {
        Commands::Login {
            device_name,
            api_host,
        } => {
            let (api_host, _) = config::resolve_api_base_url(api_host);
            devbox::commands::login::execute(&api_host, device_name).await?;
        }
        Commands::Logout { api_host } => {
            let (api_host, _) = config::resolve_api_base_url(api_host);
            devbox::commands::logout::execute(&api_host).await?;
        }
        Commands::Whoami { api_host } => {
            let (api_host, api_url) = config::resolve_api_base_url(api_host);
            devbox::commands::whoami::execute(&api_host, &api_url).await?;
        }
        Commands::Devbox { command } => {
            devbox::handle_command(command).await?;
        }
    }

    Ok(())
}

fn resolve_log_dir(override_dir: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    override_dir
        .or_else(default_log_dir)
        .ok_or_else(|| anyhow::anyhow!("unable to determine log directory"))
}

#[cfg(target_os = "macos")]
fn default_log_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join("Library").join("Logs").join("Lapdev"))
}

#[cfg(target_os = "windows")]
fn default_log_dir() -> Option<PathBuf> {
    dirs::data_local_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join("AppData").join("Local")))
        .map(|base| base.join("Lapdev").join("logs"))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn default_log_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".local").join("state")))
        .map(|base| base.join("lapdev").join("logs"))
}
