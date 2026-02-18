mod agent;
mod config;
mod daemon;
mod polkit_helper;
mod prompt;
mod state;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "garcard",
    about = "Polkit auth agent daemon for the gar desktop suite"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Increase logging verbosity
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start daemon mode
    Daemon,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_logging(cli.verbose);

    let command = cli.command.unwrap_or(Commands::Daemon);
    match command {
        Commands::Daemon => {
            let config = config::Config::load()?;
            daemon::run(config).await
        }
    }
}

fn init_logging(verbosity: u8) {
    let filter = match verbosity {
        0 => "garcard=info",
        1 => "garcard=debug",
        _ => "garcard=trace",
    };

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(filter));
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .init();
}
