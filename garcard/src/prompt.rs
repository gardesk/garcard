use crate::polkit_helper::{PromptProvider, PromptResponse};
use crate::prompt_ui::{
    PromptExit, PromptMode, PromptRequest, PromptSession, PromptTone as UiPromptTone,
};
use anyhow::{Context, Result};
use std::process::{Command, ExitStatus};

const DEFAULT_ASK_TIMEOUT_SECS: u64 = 120;
const FEEDBACK_TIMEOUT_SECS: u64 = 1;

#[derive(Debug, Clone, Copy)]
enum PromptTone {
    Default,
    Success,
    Error,
}

impl PromptTone {
    fn as_arg(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Success => "success",
            Self::Error => "error",
        }
    }
}

#[derive(Debug)]
pub struct CommandPrompt {
    prompt_command: Option<String>,
    prompt_timeout_secs: u64,
    session: Option<PromptSession>,
    session_unavailable: bool,
}

impl Default for CommandPrompt {
    fn default() -> Self {
        let prompt_timeout_secs = std::env::var("GARCARD_PROMPT_TIMEOUT_SECS")
            .ok()
            .and_then(|raw| raw.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_ASK_TIMEOUT_SECS);

        Self {
            prompt_command: std::env::var("GARCARD_PROMPT_COMMAND").ok(),
            prompt_timeout_secs,
            session: None,
            session_unavailable: false,
        }
    }
}

impl CommandPrompt {
    fn run_prompt(&mut self, prompt: &str, visible: bool) -> Result<PromptResponse> {
        if let Some(command) = self.prompt_command.as_deref() {
            return run_custom_prompt_command(command, prompt, visible);
        }

        let timeout_secs = self.prompt_timeout_secs;
        let session_result = match self.ensure_session() {
            Some(session) => {
                let request = PromptRequest {
                    message: prompt.to_string(),
                    mode: if visible {
                        PromptMode::Plain
                    } else {
                        PromptMode::Secret
                    },
                    timeout_secs,
                    tone: UiPromptTone::Default,
                };
                Some(session.run(request))
            }
            None => None,
        };

        if let Some(result) = session_result {
            match result {
                Ok(PromptExit::Submitted(value)) => return Ok(PromptResponse::Submitted(value)),
                Ok(PromptExit::Canceled) => return Ok(PromptResponse::Canceled),
                Ok(PromptExit::TimedOut) => return Ok(PromptResponse::TimedOut),
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        "Persistent prompt session failed; falling back to subprocess prompt"
                    );
                    self.session = None;
                    self.session_unavailable = true;
                }
            }
        }

        match run_gartk_prompt_subcommand(prompt, visible, timeout_secs) {
            Ok(response) => Ok(response),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "Failed to run built-in gartk prompt; falling back to systemd-ask-password"
                );
                run_systemd_ask_password(prompt, visible, timeout_secs)
            }
        }
    }

    fn run_feedback(&mut self, message: &str, tone: PromptTone) -> Result<()> {
        let session_result = match self.ensure_session() {
            Some(session) => {
                Some(session.show_feedback(message, to_ui_tone(tone), FEEDBACK_TIMEOUT_SECS))
            }
            None => None,
        };

        if let Some(result) = session_result {
            if let Err(err) = result {
                tracing::warn!(
                    error = %err,
                    "Persistent prompt feedback failed; falling back to subprocess prompt"
                );
                self.session = None;
                self.session_unavailable = true;
            } else {
                return Ok(());
            }
        }

        let _ = run_feedback_prompt_subcommand(message, tone, FEEDBACK_TIMEOUT_SECS);
        Ok(())
    }

    fn ensure_session(&mut self) -> Option<&mut PromptSession> {
        if self.prompt_command.is_some() || self.session_unavailable {
            return None;
        }
        if self.session.is_none() {
            match PromptSession::connect() {
                Ok(session) => {
                    self.session = Some(session);
                }
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        "Built-in persistent prompt unavailable; using subprocess prompt path"
                    );
                    self.session_unavailable = true;
                    return None;
                }
            }
        }
        self.session.as_mut()
    }
}

impl PromptProvider for CommandPrompt {
    fn prompt_secret(&mut self, prompt: &str) -> Result<PromptResponse> {
        self.run_prompt(prompt, false)
    }

    fn prompt_plain(&mut self, prompt: &str) -> Result<PromptResponse> {
        self.run_prompt(prompt, true)
    }

    fn show_error(&mut self, message: &str) -> Result<()> {
        tracing::warn!("polkit helper message: {}", message);
        Ok(())
    }

    fn show_info(&mut self, message: &str) -> Result<()> {
        tracing::info!("polkit helper message: {}", message);
        Ok(())
    }

    fn auth_succeeded(&mut self) -> Result<()> {
        self.run_feedback("Authentication succeeded", PromptTone::Success)
    }

    fn auth_failed(&mut self, message: &str) -> Result<()> {
        self.run_feedback(message, PromptTone::Error)
    }
}

fn to_ui_tone(tone: PromptTone) -> UiPromptTone {
    match tone {
        PromptTone::Default => UiPromptTone::Default,
        PromptTone::Success => UiPromptTone::Success,
        PromptTone::Error => UiPromptTone::Error,
    }
}

fn run_custom_prompt_command(command: &str, prompt: &str, visible: bool) -> Result<PromptResponse> {
    let mode = if visible { "plain" } else { "secret" };
    let mut output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .env("GARCARD_PROMPT", prompt)
        .env("GARCARD_PROMPT_MODE", mode)
        .output()
        .with_context(|| format!("failed to run custom prompt command: {}", command))?;

    let response = map_output_to_prompt_response(&output.status, &output.stdout);
    scrub_bytes(&mut output.stdout);
    Ok(response)
}

fn run_gartk_prompt_subcommand(
    prompt: &str,
    visible: bool,
    timeout_secs: u64,
) -> Result<PromptResponse> {
    let mode = if visible { "plain" } else { "secret" };
    let executable =
        std::env::current_exe().context("failed to resolve current executable path")?;
    let mut output = Command::new(executable)
        .arg("prompt")
        .arg("--mode")
        .arg(mode)
        .arg("--message")
        .arg(prompt)
        .arg("--timeout-secs")
        .arg(timeout_secs.to_string())
        .arg("--tone")
        .arg(PromptTone::Default.as_arg())
        .output()
        .context("failed to launch garcard prompt subcommand")?;

    if output.status.code() == Some(2) {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        scrub_bytes(&mut output.stdout);
        scrub_bytes(&mut output.stderr);
        anyhow::bail!("garcard prompt subcommand unavailable: {}", stderr);
    }

    let response = map_output_to_prompt_response(&output.status, &output.stdout);
    scrub_bytes(&mut output.stdout);
    scrub_bytes(&mut output.stderr);
    Ok(response)
}

fn run_feedback_prompt_subcommand(
    message: &str,
    tone: PromptTone,
    timeout_secs: u64,
) -> Result<()> {
    let executable =
        std::env::current_exe().context("failed to resolve current executable path")?;
    let _output = Command::new(executable)
        .arg("prompt")
        .arg("--mode")
        .arg("plain")
        .arg("--message")
        .arg(message)
        .arg("--timeout-secs")
        .arg(timeout_secs.to_string())
        .arg("--tone")
        .arg(tone.as_arg())
        .output()
        .context("failed to launch feedback prompt subcommand")?;
    Ok(())
}

fn run_systemd_ask_password(
    prompt: &str,
    visible: bool,
    timeout_secs: u64,
) -> Result<PromptResponse> {
    let mut command = Command::new("systemd-ask-password");
    command.arg("--user");
    command.arg("--no-tty");
    command.arg(format!("--timeout={}", timeout_secs));
    if visible {
        command.arg("--echo=yes");
    }
    command.arg(prompt);

    let mut output = command
        .output()
        .context("failed to run systemd-ask-password")?;
    let response = map_output_to_prompt_response(&output.status, &output.stdout);
    scrub_bytes(&mut output.stdout);
    scrub_bytes(&mut output.stderr);
    Ok(response)
}

fn map_output_to_prompt_response(status: &ExitStatus, stdout: &[u8]) -> PromptResponse {
    if status.success() {
        return extract_response(stdout)
            .map(PromptResponse::Submitted)
            .unwrap_or(PromptResponse::Canceled);
    }

    if status.code() == Some(124) {
        PromptResponse::TimedOut
    } else {
        PromptResponse::Canceled
    }
}

fn extract_response(stdout: &[u8]) -> Option<String> {
    let response = String::from_utf8_lossy(stdout).trim().to_string();
    if response.is_empty() {
        None
    } else {
        Some(response)
    }
}

fn scrub_bytes(value: &mut Vec<u8>) {
    if value.is_empty() {
        return;
    }
    value.fill(0);
    value.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_response_trims_whitespace() {
        let response = extract_response(b"  hello world \n");
        assert_eq!(response.as_deref(), Some("hello world"));
    }

    #[test]
    fn extract_response_rejects_empty_value() {
        assert_eq!(extract_response(b" \n\t"), None);
    }

    #[test]
    fn map_output_maps_timeout_status_code() {
        let status = Command::new("sh")
            .arg("-c")
            .arg("exit 124")
            .status()
            .expect("run shell");
        let mapped = map_output_to_prompt_response(&status, b"");
        assert_eq!(mapped, PromptResponse::TimedOut);
    }

    #[test]
    fn scrub_bytes_clears_vec() {
        let mut value = b"top-secret".to_vec();
        scrub_bytes(&mut value);
        assert!(value.is_empty());
    }
}
