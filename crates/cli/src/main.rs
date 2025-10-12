use clap::{Parser, Subcommand};

mod api;
mod auth;
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

        /// API server URL
        #[arg(long, env = "LAPDEV_API_URL", default_value = "https://app.lap.dev")]
        api_url: String,
    },

    /// Sign out (deletes token from keychain)
    Logout {
        /// API server URL
        #[arg(long, env = "LAPDEV_API_URL", default_value = "https://app.lap.dev")]
        api_url: String,
    },

    /// Show current session info
    Whoami {
        /// API server URL
        #[arg(long, env = "LAPDEV_API_URL", default_value = "https://app.lap.dev")]
        api_url: String,
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

    use tracing_subscriber::{fmt, EnvFilter, prelude::*};

    // Filter out tarpc internal tracing
    let filter = EnvFilter::from_default_env()
        .add_directive(log_level.into())
        .add_directive("tarpc=warn".parse().unwrap()); // Only show tarpc warnings/errors

    tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .with_target(false)
                .without_time()
        )
        .init();

    // Handle commands
    match cli.command {
        Commands::Login {
            device_name,
            api_url,
        } => {
            devbox::commands::login::execute(&api_url, device_name).await?;
        }
        Commands::Logout { api_url } => {
            devbox::commands::logout::execute(&api_url).await?;
        }
        Commands::Whoami { api_url } => {
            devbox::commands::whoami::execute(&api_url).await?;
        }
        Commands::Devbox { command } => {
            devbox::handle_command(command).await?;
        }
    }

    Ok(())
}
