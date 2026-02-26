//! Shared IPC types for garcard daemon and control clients.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::ffi::OsString;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Protocol version for compatibility checks.
pub const PROTOCOL_VERSION: u32 = 1;

/// Runtime socket filename.
pub const SOCKET_BASENAME: &str = "garcard.sock";

/// Commands accepted by garcard daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    Ping,
    Status,
    Version,
    AuthSummary,
    Quit,
}

/// Standard daemon response envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn ok() -> Self {
        Self {
            success: true,
            data: None,
            error: None,
        }
    }

    pub fn ok_with_data(data: impl Serialize) -> Self {
        Self {
            success: true,
            data: serde_json::to_value(data).ok(),
            error: None,
        }
    }

    pub fn err(message: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(message.into()),
        }
    }
}

/// Minimal health and runtime status summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusData {
    pub running: bool,
    pub pid: u32,
    pub uptime_secs: u64,
    pub version: String,
    pub protocol_version: u32,
    pub socket_path: String,
    pub agent_backend: String,
}

/// Version handshake payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionData {
    pub component: String,
    pub version: String,
    pub protocol_version: u32,
}

/// Auth workflow summary with no sensitive data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSummary {
    pub state: String,
    pub active_requests: usize,
    pub queued_requests: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_action_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_retention_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_retention_enforced: Option<bool>,
}

/// Resolve the daemon socket path from `XDG_RUNTIME_DIR` with `/tmp` fallback.
pub fn socket_path() -> PathBuf {
    if let Some(explicit) = std::env::var_os("GARCARD_SOCKET") {
        return PathBuf::from(explicit);
    }

    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR");
    if runtime_dir.as_ref().is_some_and(runtime_dir_is_usable) {
        return socket_path_from_runtime_dir(runtime_dir);
    }

    socket_path_from_runtime_dir(None)
}

/// Build socket path from an explicit runtime dir value.
pub fn socket_path_from_runtime_dir(runtime_dir: Option<OsString>) -> PathBuf {
    match runtime_dir {
        Some(dir) => PathBuf::from(dir).join(SOCKET_BASENAME),
        None => PathBuf::from("/tmp").join(SOCKET_BASENAME),
    }
}

/// Check if we can create a socket in runtime dir.
///
/// In sandboxed environments, `XDG_RUNTIME_DIR` can exist but still reject
/// socket creation. We probe with a unique temporary socket path so caller
/// code can safely fall back to `/tmp`.
fn runtime_dir_is_usable(runtime_dir: &OsString) -> bool {
    let base = PathBuf::from(runtime_dir);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let probe = base.join(format!(
        ".garcard-probe-{}-{}.sock",
        std::process::id(),
        timestamp
    ));

    match UnixListener::bind(&probe) {
        Ok(listener) => {
            drop(listener);
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_round_trip() {
        let cmd = Command::AuthSummary;
        let encoded = serde_json::to_string(&cmd).expect("encode command");
        let decoded: Command = serde_json::from_str(&encoded).expect("decode command");
        assert!(matches!(decoded, Command::AuthSummary));
    }

    #[test]
    fn socket_path_falls_back_to_tmp() {
        let path = socket_path_from_runtime_dir(None);
        assert_eq!(path, PathBuf::from("/tmp/garcard.sock"));
    }

    #[test]
    fn socket_path_uses_runtime_dir() {
        let path = socket_path_from_runtime_dir(Some(OsString::from("/run/user/1000")));
        assert_eq!(path, PathBuf::from("/run/user/1000/garcard.sock"));
    }
}
