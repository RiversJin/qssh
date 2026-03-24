mod cert;
mod client;
mod config;
mod error;
mod relay;
mod server;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "qssh", version, about = "QUIC-based SSH proxy")]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Path to config file (TOML)
    #[arg(long, short = 'c', global = true)]
    config: Option<PathBuf>,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, default_value = "warn", global = true)]
    log_level: String,
}

#[derive(Subcommand)]
enum Command {
    /// Run as client (SSH ProxyCommand)
    Client(config::ClientArgs),
    /// Run as server
    Server(config::ServerArgs),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Initialize tracing
    let filter = cli
        .log_level
        .parse::<tracing_subscriber::filter::EnvFilter>()
        .unwrap_or_else(|_| tracing_subscriber::filter::EnvFilter::new("warn"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    // Load config file
    let file_config = match &cli.config {
        Some(path) => config::load_file_config(path)?,
        None => config::FileConfig::default(),
    };

    // Build runtime and dispatch
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    match cli.command {
        Command::Client(args) => {
            let resolved = config::resolve_client(args, file_config.client)?;
            runtime.block_on(client::run(resolved))
        }
        Command::Server(args) => {
            let resolved = config::resolve_server(args, file_config.server)?;
            runtime.block_on(server::run(resolved))
        }
    }
}
