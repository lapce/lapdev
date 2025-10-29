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
        .add_directive("tarpc=warn".parse().unwrap()); // Only show tarpc warnings/errors

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(false).without_time())
        .init();

    // Handle commands
    match cli.command {
        Commands::Login {
            device_name,
            api_host,
        } => {
            let (_, api_url) = config::resolve_api_base_url(api_host);
            devbox::commands::login::execute(&api_url, device_name).await?;
        }
        Commands::Logout { api_host } => {
            let (_, api_url) = config::resolve_api_base_url(api_host);
            devbox::commands::logout::execute(&api_url).await?;
        }
        Commands::Whoami { api_host } => {
            let (_, api_url) = config::resolve_api_base_url(api_host);
            devbox::commands::whoami::execute(&api_url).await?;
        }
        Commands::Devbox { command } => {
            devbox::handle_command(command).await?;
        }
    }

    Ok(())
}
