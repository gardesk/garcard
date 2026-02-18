use crate::agent::{AuthAgentBackend, PolkitAgent, PolkitBackendConfig, StubPolkitAgent};
use crate::config::{AgentBackendMode, Config};
use crate::state::{AuthState, RuntimeState};
use anyhow::{Context, Result};
use garcard_ipc::{Command, Response};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

/// Removes daemon socket on process exit.
struct SocketGuard {
    path: PathBuf,
}

impl SocketGuard {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub async fn run(config: Config) -> Result<()> {
    let auth_state = Arc::new(AuthState::default());
    let mut backend = init_backend(&config, Arc::clone(&auth_state))?;

    prepare_socket(&config.socket_path).await?;
    if let Some(parent) = config.socket_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("Failed to create socket directory {}", parent.display()))?;
    }

    let listener = UnixListener::bind(&config.socket_path)
        .with_context(|| format!("Failed to bind socket {}", config.socket_path.display()))?;
    std::fs::set_permissions(
        &config.socket_path,
        std::fs::Permissions::from_mode(config.socket_mode),
    )
    .with_context(|| {
        format!(
            "Failed to set socket permissions for {}",
            config.socket_path.display()
        )
    })?;

    let _socket_guard = SocketGuard::new(config.socket_path.clone());
    let state = Arc::new(RuntimeState::with_auth(
        config.socket_path.display().to_string(),
        backend.name(),
        auth_state,
    ));
    let (shutdown_tx, mut shutdown_rx) = mpsc::unbounded_channel::<()>();

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("Failed to register SIGTERM handler")?;
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .context("Failed to register SIGINT handler")?;

    tracing::info!(
        socket = %config.socket_path.display(),
        pid = state.status().pid,
        backend = backend.name(),
        "garcard daemon started"
    );

    loop {
        tokio::select! {
            _ = sigterm.recv() => {
                tracing::info!("Received SIGTERM");
                break;
            }
            _ = sigint.recv() => {
                tracing::info!("Received SIGINT");
                break;
            }
            maybe_shutdown = shutdown_rx.recv() => {
                if maybe_shutdown.is_some() {
                    tracing::info!("Shutdown requested via IPC");
                } else {
                    tracing::info!("Shutdown channel closed");
                }
                break;
            }
            accept = listener.accept() => {
                match accept {
                    Ok((stream, _)) => {
                        let runtime = Arc::clone(&state);
                        let shutdown = shutdown_tx.clone();
                        tokio::spawn(async move {
                            if let Err(err) = handle_client(stream, runtime, shutdown).await {
                                tracing::warn!(error = %err, "Client request failed");
                            }
                        });
                    }
                    Err(err) => {
                        tracing::error!(error = %err, "Failed to accept IPC client");
                    }
                }
            }
        }
    }

    backend.unregister()?;
    tracing::info!("garcard daemon stopped");
    Ok(())
}

fn init_backend(config: &Config, auth_state: Arc<AuthState>) -> Result<Box<dyn AuthAgentBackend>> {
    match config.agent_backend {
        AgentBackendMode::Stub => {
            let mut backend: Box<dyn AuthAgentBackend> = Box::new(StubPolkitAgent);
            backend.register()?;
            Ok(backend)
        }
        AgentBackendMode::Polkit => {
            let mut backend: Box<dyn AuthAgentBackend> =
                Box::new(PolkitAgent::new(PolkitBackendConfig {
                    object_path: config.polkit_object_path.clone(),
                    locale: config.locale.clone(),
                }, Arc::clone(&auth_state))?);
            backend.register()?;
            Ok(backend)
        }
        AgentBackendMode::Auto => {
            let attempt = (|| -> Result<Box<dyn AuthAgentBackend>> {
                let mut backend: Box<dyn AuthAgentBackend> =
                    Box::new(PolkitAgent::new(PolkitBackendConfig {
                        object_path: config.polkit_object_path.clone(),
                        locale: config.locale.clone(),
                    }, Arc::clone(&auth_state))?);
                backend.register()?;
                Ok(backend)
            })();

            match attempt {
                Ok(backend) => Ok(backend),
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        "Failed to initialize polkit backend, falling back to stub"
                    );
                    let mut fallback: Box<dyn AuthAgentBackend> = Box::new(StubPolkitAgent);
                    fallback.register()?;
                    Ok(fallback)
                }
            }
        }
    }
}

async fn prepare_socket(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    match UnixStream::connect(path).await {
        Ok(_) => {
            anyhow::bail!(
                "garcard daemon is already running (socket: {})",
                path.display()
            );
        }
        Err(_) => {
            tokio::fs::remove_file(path)
                .await
                .with_context(|| format!("Failed to remove stale socket {}", path.display()))?;
            tracing::warn!(socket = %path.display(), "Removed stale socket");
        }
    }

    Ok(())
}

async fn handle_client(
    stream: UnixStream,
    state: Arc<RuntimeState>,
    shutdown_tx: mpsc::UnboundedSender<()>,
) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    let bytes = reader
        .read_line(&mut line)
        .await
        .context("Failed to read command")?;
    if bytes == 0 {
        return Ok(());
    }

    let command: Command = serde_json::from_str(line.trim()).context("Invalid command payload")?;
    let response = dispatch(command, &state, &shutdown_tx);
    let payload = serde_json::to_string(&response).context("Failed to encode response")?;

    writer
        .write_all(payload.as_bytes())
        .await
        .context("Failed to write response")?;
    writer
        .write_all(b"\n")
        .await
        .context("Failed to terminate response line")?;
    writer.flush().await.context("Failed to flush response")?;

    Ok(())
}

fn dispatch(
    command: Command,
    state: &RuntimeState,
    shutdown_tx: &mpsc::UnboundedSender<()>,
) -> Response {
    match command {
        Command::Ping => Response::ok_with_data(json!({ "pong": true })),
        Command::Status => Response::ok_with_data(state.status()),
        Command::Version => Response::ok_with_data(state.version()),
        Command::AuthSummary => Response::ok_with_data(state.auth_summary()),
        Command::Quit => {
            if shutdown_tx.send(()).is_err() {
                Response::err("daemon shutdown channel unavailable")
            } else {
                Response::ok_with_data(json!({ "message": "shutdown_requested" }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_state() -> RuntimeState {
        RuntimeState::new("/tmp/garcard-test.sock".to_string(), "test-backend")
    }

    #[test]
    fn dispatch_ping_returns_pong() {
        let state = fake_state();
        let (shutdown_tx, _shutdown_rx) = mpsc::unbounded_channel();

        let response = dispatch(Command::Ping, &state, &shutdown_tx);
        assert!(response.success);

        let data = response.data.expect("data");
        assert_eq!(data.get("pong").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn dispatch_quit_signals_shutdown() {
        let state = fake_state();
        let (shutdown_tx, mut shutdown_rx) = mpsc::unbounded_channel();

        let response = dispatch(Command::Quit, &state, &shutdown_tx);
        assert!(response.success);
        assert!(shutdown_rx.try_recv().is_ok());
    }

    #[test]
    fn auto_backend_falls_back_to_stub_for_bad_object_path() {
        let config = Config {
            socket_path: PathBuf::from("/tmp/garcard-test.sock"),
            socket_mode: 0o600,
            agent_backend: AgentBackendMode::Auto,
            polkit_object_path: "invalid path".to_string(),
            locale: "C".to_string(),
        };

        let backend = init_backend(&config, Arc::new(AuthState::default()))
            .expect("auto mode should fall back");
        assert_eq!(backend.name(), "stub-polkit-agent");
    }
}
