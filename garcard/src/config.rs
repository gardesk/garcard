use anyhow::{Context, Result};
use garcard_ipc::socket_path;
use serde::Deserialize;
use std::fmt;
use std::path::PathBuf;

/// Runtime configuration for the garcard daemon.
#[derive(Debug, Clone)]
pub struct Config {
    pub socket_path: PathBuf,
    pub socket_mode: u32,
    pub agent_backend: AgentBackendMode,
    pub polkit_object_path: String,
    pub locale: String,
    pub backend_healthcheck_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            socket_path: socket_path(),
            socket_mode: 0o600,
            agent_backend: AgentBackendMode::Auto,
            polkit_object_path: "/org/gardesk/Garcard/AuthAgent".to_string(),
            locale: std::env::var("LANG").unwrap_or_else(|_| "en_US.UTF-8".to_string()),
            backend_healthcheck_secs: 5,
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let mut cfg = Self::default();
        cfg.apply_file_config()?;

        if let Some(raw_mode) = std::env::var_os("GARCARD_SOCKET_MODE") {
            let parsed = parse_octal_mode(&raw_mode.to_string_lossy())
                .with_context(|| "Invalid GARCARD_SOCKET_MODE (expected octal, e.g. 600)")?;
            cfg.socket_mode = parsed;
        }

        if let Some(raw_socket) = std::env::var_os("GARCARD_SOCKET") {
            cfg.socket_path = PathBuf::from(raw_socket);
        }
        if let Some(raw_backend) = std::env::var_os("GARCARD_AGENT_BACKEND") {
            cfg.agent_backend = AgentBackendMode::from_str(&raw_backend.to_string_lossy())
                .with_context(|| "Invalid GARCARD_AGENT_BACKEND (expected auto|polkit|stub)")?;
        }
        if let Some(raw_object_path) = std::env::var_os("GARCARD_POLKIT_OBJECT_PATH") {
            cfg.polkit_object_path = raw_object_path.to_string_lossy().to_string();
        }
        if let Some(raw_locale) = std::env::var_os("GARCARD_LOCALE") {
            cfg.locale = raw_locale.to_string_lossy().to_string();
        }
        if let Some(raw_interval) = std::env::var_os("GARCARD_BACKEND_HEALTHCHECK_SECS") {
            cfg.backend_healthcheck_secs = parse_positive_secs(&raw_interval.to_string_lossy())
                .with_context(|| {
                    "Invalid GARCARD_BACKEND_HEALTHCHECK_SECS (expected positive integer seconds)"
                })?;
        }

        Ok(cfg)
    }

    fn apply_file_config(&mut self) -> Result<()> {
        let Some(path) = config_path() else {
            return Ok(());
        };
        if !path.exists() {
            return Ok(());
        }

        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config file {}", path.display()))?;
        let file_cfg: FileConfig = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file {}", path.display()))?;

        if let Some(socket_path) = file_cfg.socket_path {
            self.socket_path = PathBuf::from(socket_path);
        }
        if let Some(socket_mode) = file_cfg.socket_mode {
            self.socket_mode = parse_octal_mode(&socket_mode)
                .with_context(|| "Invalid socket_mode in config file (expected octal)")?;
        }
        if let Some(agent_backend) = file_cfg.agent_backend {
            self.agent_backend = AgentBackendMode::from_str(&agent_backend)
                .with_context(|| "Invalid agent_backend in config file")?;
        }
        if let Some(polkit_object_path) = file_cfg.polkit_object_path {
            self.polkit_object_path = polkit_object_path;
        }
        if let Some(locale) = file_cfg.locale {
            self.locale = locale;
        }
        if let Some(backend_healthcheck_secs) = file_cfg.backend_healthcheck_secs {
            self.backend_healthcheck_secs = parse_positive_secs(&backend_healthcheck_secs)
                .with_context(|| {
                "Invalid backend_healthcheck_secs in config file (expected positive integer seconds)"
            })?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentBackendMode {
    Auto,
    Polkit,
    Stub,
}

impl AgentBackendMode {
    fn from_str(raw: &str) -> Result<Self> {
        match raw.trim().to_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "polkit" => Ok(Self::Polkit),
            "stub" => Ok(Self::Stub),
            other => anyhow::bail!("unsupported backend mode: {}", other),
        }
    }
}

impl fmt::Display for AgentBackendMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Auto => "auto",
            Self::Polkit => "polkit",
            Self::Stub => "stub",
        };
        write!(f, "{}", value)
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct FileConfig {
    socket_path: Option<String>,
    socket_mode: Option<String>,
    agent_backend: Option<String>,
    polkit_object_path: Option<String>,
    locale: Option<String>,
    backend_healthcheck_secs: Option<String>,
}

fn config_path() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("GARCARD_CONFIG") {
        return Some(PathBuf::from(explicit));
    }

    dirs::config_dir().map(|base| base.join("garcard/config.toml"))
}

fn parse_octal_mode(raw: &str) -> Result<u32> {
    let trimmed = raw.trim();
    let mode = u32::from_str_radix(trimmed, 8)
        .with_context(|| format!("failed to parse mode value: {trimmed}"))?;
    Ok(mode)
}

fn parse_positive_secs(raw: &str) -> Result<u64> {
    let trimmed = raw.trim();
    let secs = trimmed
        .parse::<u64>()
        .with_context(|| format!("failed to parse seconds value: {trimmed}"))?;
    if secs == 0 {
        anyhow::bail!("seconds must be greater than zero");
    }
    Ok(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mode_octal() {
        let mode = parse_octal_mode("640").expect("valid mode");
        assert_eq!(mode, 0o640);
    }

    #[test]
    fn parse_mode_rejects_invalid() {
        let err = parse_octal_mode("x00").expect_err("should fail");
        assert!(err.to_string().contains("failed to parse mode value"));
    }

    #[test]
    fn parse_file_config_values() {
        let parsed: FileConfig = toml::from_str(
            r#"
socket_path = "/tmp/custom-garcard.sock"
socket_mode = "640"
agent_backend = "stub"
polkit_object_path = "/org/gardesk/Garcard/TestAgent"
locale = "C"
backend_healthcheck_secs = "7"
"#,
        )
        .expect("parse file config");

        assert_eq!(
            parsed.socket_path.as_deref(),
            Some("/tmp/custom-garcard.sock")
        );
        assert_eq!(parsed.socket_mode.as_deref(), Some("640"));
        assert_eq!(parsed.agent_backend.as_deref(), Some("stub"));
        assert_eq!(
            parsed.polkit_object_path.as_deref(),
            Some("/org/gardesk/Garcard/TestAgent")
        );
        assert_eq!(parsed.locale.as_deref(), Some("C"));
        assert_eq!(parsed.backend_healthcheck_secs.as_deref(), Some("7"));
    }

    #[test]
    fn backend_mode_parsing() {
        assert_eq!(
            AgentBackendMode::from_str("auto").expect("auto"),
            AgentBackendMode::Auto
        );
        assert_eq!(
            AgentBackendMode::from_str("polkit").expect("polkit"),
            AgentBackendMode::Polkit
        );
        assert_eq!(
            AgentBackendMode::from_str("stub").expect("stub"),
            AgentBackendMode::Stub
        );
    }

    #[test]
    fn backend_mode_rejects_unknown() {
        let err = AgentBackendMode::from_str("bad").expect_err("should fail");
        assert!(err.to_string().contains("unsupported backend mode"));
    }

    #[test]
    fn parse_positive_secs_rejects_zero() {
        let err = parse_positive_secs("0").expect_err("zero should fail");
        assert!(err.to_string().contains("greater than zero"));
    }

    #[test]
    fn parse_positive_secs_accepts_positive_integer() {
        assert_eq!(parse_positive_secs("9").expect("valid"), 9);
    }
}
