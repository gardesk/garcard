use crate::polkit_helper::PromptProvider;
use anyhow::{Context, Result};
use std::process::Command;

const DEFAULT_ASK_TIMEOUT_SECS: u64 = 120;

#[derive(Debug, Clone)]
pub struct CommandPrompt {
    prompt_command: Option<String>,
}

impl Default for CommandPrompt {
    fn default() -> Self {
        Self {
            prompt_command: std::env::var("GARCARD_PROMPT_COMMAND").ok(),
        }
    }
}

impl CommandPrompt {
    fn run_prompt(&self, prompt: &str, visible: bool) -> Result<Option<String>> {
        if let Some(command) = self.prompt_command.as_deref() {
            return run_custom_prompt_command(command, prompt, visible);
        }

        run_systemd_ask_password(prompt, visible)
    }
}

impl PromptProvider for CommandPrompt {
    fn prompt_secret(&mut self, prompt: &str) -> Result<Option<String>> {
        self.run_prompt(prompt, false)
    }

    fn prompt_plain(&mut self, prompt: &str) -> Result<Option<String>> {
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
}

fn run_custom_prompt_command(command: &str, prompt: &str, visible: bool) -> Result<Option<String>> {
    let mode = if visible { "plain" } else { "secret" };
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .env("GARCARD_PROMPT", prompt)
        .env("GARCARD_PROMPT_MODE", mode)
        .output()
        .with_context(|| format!("failed to run custom prompt command: {}", command))?;

    if !output.status.success() {
        return Ok(None);
    }

    Ok(extract_response(&output.stdout))
}

fn run_systemd_ask_password(prompt: &str, visible: bool) -> Result<Option<String>> {
    let mut command = Command::new("systemd-ask-password");
    command.arg("--user");
    command.arg("--no-tty");
    command.arg(format!("--timeout={}", DEFAULT_ASK_TIMEOUT_SECS));
    if visible {
        command.arg("--echo=yes");
    }
    command.arg(prompt);

    let output = command
        .output()
        .context("failed to run systemd-ask-password")?;
    if !output.status.success() {
        return Ok(None);
    }

    Ok(extract_response(&output.stdout))
}

fn extract_response(stdout: &[u8]) -> Option<String> {
    let response = String::from_utf8_lossy(stdout).trim().to_string();
    if response.is_empty() {
        None
    } else {
        Some(response)
    }
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
}
