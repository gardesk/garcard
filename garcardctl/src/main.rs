use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use garcard_ipc::{Command, Response, socket_path};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

#[derive(Parser, Debug)]
#[command(name = "garcardctl", about = "Control and inspect garcard daemon")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Ping,
    Status,
    Version,
    AuthSummary,
    Quit,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let command = to_protocol_command(cli.command);
    let response = send_command(&command)?;

    if response.success {
        if let Some(data) = response.data {
            println!("{}", serde_json::to_string_pretty(&data)?);
        } else {
            println!("OK");
        }
        return Ok(());
    }

    let message = response
        .error
        .unwrap_or_else(|| "unknown daemon error".to_string());
    eprintln!("Error: {}", message);
    std::process::exit(1);
}

fn to_protocol_command(command: Commands) -> Command {
    match command {
        Commands::Ping => Command::Ping,
        Commands::Status => Command::Status,
        Commands::Version => Command::Version,
        Commands::AuthSummary => Command::AuthSummary,
        Commands::Quit => Command::Quit,
    }
}

fn send_command(command: &Command) -> Result<Response> {
    let socket = socket_path();
    let mut stream = UnixStream::connect(&socket).with_context(|| {
        format!(
            "failed to connect to garcard daemon at {} (is it running?)",
            socket.display()
        )
    })?;

    let request = serde_json::to_string(command).context("failed to serialize command")?;
    writeln!(stream, "{}", request).context("failed to send command")?;
    stream.flush().context("failed to flush command")?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let bytes = reader
        .read_line(&mut line)
        .context("failed to read daemon response")?;
    if bytes == 0 {
        anyhow::bail!("daemon closed socket before responding");
    }

    let response: Response =
        serde_json::from_str(line.trim()).context("failed to decode daemon response")?;
    Ok(response)
}
