mod agent;
mod config;
mod daemon;
mod polkit_helper;
mod prompt;
mod prompt_ui;
mod state;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
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
    /// Run interactive auth prompt mode (used internally by daemon)
    Prompt(PromptArgs),
}

#[derive(Parser, Debug)]
struct PromptArgs {
    /// Prompt message to display
    #[arg(long)]
    message: String,
    /// Input mode
    #[arg(long, value_enum, default_value_t = PromptModeArg::Secret)]
    mode: PromptModeArg,
    /// Prompt timeout in seconds
    #[arg(long, default_value_t = 120)]
    timeout_secs: u64,
    /// Visual prompt tone
    #[arg(long, value_enum, default_value_t = PromptToneArg::Default)]
    tone: PromptToneArg,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum PromptModeArg {
    Secret,
    Plain,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum PromptToneArg {
    Default,
    Success,
    Error,
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
        Commands::Prompt(args) => {
            let request = prompt_ui::PromptRequest {
                message: args.message,
                mode: match args.mode {
                    PromptModeArg::Secret => prompt_ui::PromptMode::Secret,
                    PromptModeArg::Plain => prompt_ui::PromptMode::Plain,
                },
                timeout_secs: args.timeout_secs,
                tone: match args.tone {
                    PromptToneArg::Default => prompt_ui::PromptTone::Default,
                    PromptToneArg::Success => prompt_ui::PromptTone::Success,
                    PromptToneArg::Error => prompt_ui::PromptTone::Error,
                },
                feedback_only: false,
            };

            let outcome = match prompt_ui::run_prompt_dialog(request) {
                Ok(outcome) => outcome,
                Err(err) => {
                    eprintln!("garcard prompt backend unavailable: {}", err);
                    std::process::exit(2);
                }
            };

            match outcome {
                prompt_ui::PromptExit::Submitted(value) => {
                    println!("{}", value);
                    Ok(())
                }
                prompt_ui::PromptExit::Canceled => {
                    std::process::exit(1);
                }
                prompt_ui::PromptExit::TimedOut => {
                    std::process::exit(124);
                }
            }
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
