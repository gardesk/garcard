use crate::polkit_helper::{PromptProvider, PromptResponse};
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

#[derive(Debug, Clone)]
pub struct CommandPrompt {
    prompt_command: Option<String>,
    prompt_timeout_secs: u64,
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
        }
    }
}

impl CommandPrompt {
    fn run_prompt(&self, prompt: &str, visible: bool) -> Result<PromptResponse> {
        if let Some(command) = self.prompt_command.as_deref() {
            return run_custom_prompt_command(command, prompt, visible);
        }

        match run_gartk_prompt_subcommand(prompt, visible, self.prompt_timeout_secs) {
            Ok(response) => Ok(response),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "Failed to run built-in gartk prompt; falling back to systemd-ask-password"
                );
                run_systemd_ask_password(prompt, visible, self.prompt_timeout_secs)
            }
        }
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
        let _ = run_feedback_prompt_subcommand(message, PromptTone::Error, FEEDBACK_TIMEOUT_SECS);
        Ok(())
    }

    fn show_info(&mut self, message: &str) -> Result<()> {
        tracing::info!("polkit helper message: {}", message);
        Ok(())
    }

    fn auth_succeeded(&mut self) -> Result<()> {
        let _ = run_feedback_prompt_subcommand(
            "Authentication succeeded",
            PromptTone::Success,
            FEEDBACK_TIMEOUT_SECS,
        );
        Ok(())
    }

    fn auth_failed(&mut self, message: &str) -> Result<()> {
        let _ = run_feedback_prompt_subcommand(message, PromptTone::Error, FEEDBACK_TIMEOUT_SECS);
        Ok(())
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
