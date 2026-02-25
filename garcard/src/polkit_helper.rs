use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub const DEFAULT_HELPER_SOCKET: &str = "/run/polkit/agent-helper.socket";
const HELPER_TRANSPORT_ENV: &str = "GARCARD_POLKIT_HELPER_TRANSPORT";
const HELPER_SOCKET_PROTOCOL_ENV: &str = "GARCARD_POLKIT_SOCKET_PROTOCOL";
const SOCKET_FIRST_RESPONSE_TIMEOUT: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperOutcome {
    Authorized,
    Denied,
    Canceled,
    Timeout,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperEvent {
    PromptHidden(String),
    PromptVisible(String),
    Error(String),
    Info(String),
    Success,
    Failure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptResponse {
    Submitted(String),
    Canceled,
    TimedOut,
}

pub trait PromptProvider {
    fn prompt_secret(&mut self, prompt: &str) -> Result<PromptResponse>;
    fn prompt_plain(&mut self, prompt: &str) -> Result<PromptResponse>;

    fn show_error(&mut self, _message: &str) -> Result<()> {
        Ok(())
    }

    fn show_info(&mut self, _message: &str) -> Result<()> {
        Ok(())
    }

    fn auth_succeeded(&mut self) -> Result<()> {
        Ok(())
    }

    fn auth_failed(&mut self, _message: &str) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct HelperSocketClient {
    socket_path: PathBuf,
}

impl Default for HelperSocketClient {
    fn default() -> Self {
        Self {
            socket_path: PathBuf::from(DEFAULT_HELPER_SOCKET),
        }
    }
}

impl HelperSocketClient {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: path.into(),
        }
    }

    pub fn authenticate<P: PromptProvider>(
        &self,
        username: &str,
        cookie: &str,
        prompts: &mut P,
    ) -> Result<HelperOutcome> {
        let username_line = sanitize_control_line(username);
        let cookie_line = sanitize_control_line(cookie);
        let transport = helper_transport_mode();
        tracing::debug!(
            transport = %transport.as_str(),
            env_key = HELPER_TRANSPORT_ENV,
            "Selected polkit helper transport mode"
        );
        if matches!(transport, HelperTransportMode::Direct) {
            if let Some(helper) = resolve_direct_helper_path() {
                return self.authenticate_via_helper_process_with_helper(
                    &helper,
                    &username_line,
                    &cookie_line,
                    prompts,
                );
            }
            tracing::warn!(
                "Direct helper transport requested but no root setuid helper was found; falling back to socket transport"
            );
        }

        match helper_socket_protocol() {
            HelperSocketProtocol::Auto => {
                self.authenticate_via_socket_auto(&username_line, &cookie_line, prompts)
            }
            protocol => {
                match self.authenticate_via_socket(&username_line, &cookie_line, prompts, protocol)
                {
                    Ok(outcome) => Ok(outcome),
                    Err(err) if is_no_session_cookie_error(&err) => {
                        prompts
                            .auth_failed("Authentication failed")
                            .context("prompt failure callback failed")?;
                        Ok(HelperOutcome::Denied)
                    }
                    Err(err) => Err(err),
                }
            }
        }
    }

    fn authenticate_via_socket_auto<P: PromptProvider>(
        &self,
        username: &str,
        cookie: &str,
        prompts: &mut P,
    ) -> Result<HelperOutcome> {
        let protocols = [
            HelperSocketProtocol::CookieOnly,
            HelperSocketProtocol::UsernameCookie,
        ];

        for protocol in protocols {
            match self.authenticate_via_socket(username, cookie, prompts, protocol) {
                Ok(outcome) => return Ok(outcome),
                Err(err)
                    if is_no_session_cookie_error(&err)
                        || is_no_initial_helper_response_error(&err) =>
                {
                    tracing::warn!(
                        protocol = %protocol.as_str(),
                        error = %err,
                        "Socket helper protocol attempt failed; retrying with alternate socket protocol"
                    );
                }
                Err(err) => return Err(err),
            }
        }

        prompts
            .auth_failed("Authentication failed")
            .context("prompt failure callback failed")?;
        Ok(HelperOutcome::Denied)
    }

    fn authenticate_via_socket<P: PromptProvider>(
        &self,
        username_line: &str,
        cookie_line: &str,
        prompts: &mut P,
        protocol: HelperSocketProtocol,
    ) -> Result<HelperOutcome> {
        let mut stream = UnixStream::connect(&self.socket_path).with_context(|| {
            format!(
                "failed to connect to polkit helper socket at {}",
                self.socket_path.display()
            )
        })?;
        let cookie_preview: String = cookie_line.chars().take(16).collect();
        tracing::debug!(
            username = %username_line,
            cookie_len = cookie_line.len(),
            cookie_preview = %cookie_preview,
            socket = %self.socket_path.display(),
            protocol = %protocol.as_str(),
            "Connected to polkit helper socket"
        );
        let read_stream = stream
            .try_clone()
            .context("failed to clone helper socket stream")?;
        let _ = read_stream.set_read_timeout(Some(SOCKET_FIRST_RESPONSE_TIMEOUT));
        let mut reader = BufReader::new(read_stream);

        if matches!(protocol, HelperSocketProtocol::UsernameCookie) {
            write_line(&mut stream, &username_line).context("failed to send helper username")?;
        }
        write_line(&mut stream, &cookie_line).context("failed to send helper cookie")?;

        let mut saw_no_session_cookie = false;
        let mut saw_first_event = false;
        loop {
            let mut line = String::new();
            let bytes = match reader.read_line(&mut line) {
                Ok(bytes) => bytes,
                Err(err)
                    if !saw_first_event
                        && matches!(err.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) =>
                {
                    return Err(NoInitialHelperResponseError.into());
                }
                Err(err) => {
                    return Err(
                        anyhow::Error::new(err).context("failed to read helper response line")
                    );
                }
            };
            if bytes == 0 {
                anyhow::bail!("helper closed connection unexpectedly");
            }
            if !saw_first_event {
                saw_first_event = true;
                let _ = reader.get_mut().set_read_timeout(None);
            }
            tracing::debug!(
                helper_line = %line.trim_end_matches('\n').trim_end_matches('\r'),
                "Received helper protocol line"
            );

            let event = match parse_helper_line(&line) {
                Ok(event) => event,
                Err(err) => {
                    prompts
                        .show_error(&err.to_string())
                        .context("prompt error callback failed")?;
                    continue;
                }
            };

            match event {
                HelperEvent::PromptHidden(prompt) => {
                    match prompts
                        .prompt_secret(&prompt)
                        .context("prompt handler failed")?
                    {
                        PromptResponse::Submitted(mut response) => {
                            tracing::debug!(
                                response_len = response.chars().count(),
                                "Submitting secret prompt response to helper"
                            );
                            let mut sanitized = sanitize_response(&response);
                            write_line(&mut stream, &sanitized)
                                .context("failed to send helper secret response")?;
                            scrub_string(&mut sanitized);
                            scrub_string(&mut response);
                        }
                        PromptResponse::Canceled => return Ok(HelperOutcome::Canceled),
                        PromptResponse::TimedOut => return Ok(HelperOutcome::Timeout),
                    }
                }
                HelperEvent::PromptVisible(prompt) => {
                    match prompts
                        .prompt_plain(&prompt)
                        .context("prompt handler failed")?
                    {
                        PromptResponse::Submitted(mut response) => {
                            tracing::debug!(
                                response_len = response.chars().count(),
                                "Submitting visible prompt response to helper"
                            );
                            let mut sanitized = sanitize_response(&response);
                            write_line(&mut stream, &sanitized)
                                .context("failed to send helper visible response")?;
                            scrub_string(&mut sanitized);
                            scrub_string(&mut response);
                        }
                        PromptResponse::Canceled => return Ok(HelperOutcome::Canceled),
                        PromptResponse::TimedOut => return Ok(HelperOutcome::Timeout),
                    }
                }
                HelperEvent::Error(message) => {
                    if message
                        .to_ascii_lowercase()
                        .contains("no session for cookie")
                    {
                        saw_no_session_cookie = true;
                    }
                    prompts
                        .show_error(&message)
                        .context("prompt error callback failed")?;
                }
                HelperEvent::Info(message) => {
                    prompts
                        .show_info(&message)
                        .context("prompt info callback failed")?;
                }
                HelperEvent::Success => {
                    prompts
                        .auth_succeeded()
                        .context("prompt success callback failed")?;
                    return Ok(HelperOutcome::Authorized);
                }
                HelperEvent::Failure => {
                    if saw_no_session_cookie {
                        if let Some(helper) = resolve_direct_helper_path() {
                            tracing::warn!(
                                helper = %helper.display(),
                                "Socket helper reported no session for cookie; falling back to direct helper process"
                            );
                            return self.authenticate_via_helper_process_with_helper(
                                &helper,
                                &username_line,
                                &cookie_line,
                                prompts,
                            );
                        }
                        return Err(NoSessionForCookieError.into());
                    }
                    prompts
                        .auth_failed("Authentication failed")
                        .context("prompt failure callback failed")?;
                    return Ok(HelperOutcome::Denied);
                }
            }
        }
    }

    fn authenticate_via_helper_process_with_helper<P: PromptProvider>(
        &self,
        helper: &Path,
        username: &str,
        cookie: &str,
        prompts: &mut P,
    ) -> Result<HelperOutcome> {
        tracing::info!(
            helper = %helper.display(),
            "Starting direct polkit helper fallback process"
        );

        let mut child = Command::new(helper)
            .arg(username)
            .arg(cookie)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("failed to spawn helper process {}", helper.display()))?;

        let stdout = child
            .stdout
            .take()
            .context("failed to capture helper process stdout")?;
        let stdin = child
            .stdin
            .as_mut()
            .context("failed to capture helper process stdin")?;
        let mut reader = BufReader::new(stdout);

        loop {
            let mut line = String::new();
            let bytes = reader
                .read_line(&mut line)
                .context("failed to read direct helper response line")?;
            if bytes == 0 {
                let status = child
                    .wait()
                    .context("failed waiting for helper process exit")?;
                anyhow::bail!("direct helper closed stream unexpectedly (status: {status})");
            }
            tracing::debug!(
                helper_line = %line.trim_end_matches('\n').trim_end_matches('\r'),
                helper = %helper.display(),
                "Received direct helper protocol line"
            );

            let event = match parse_helper_line(&line) {
                Ok(event) => event,
                Err(err) => {
                    prompts
                        .show_error(&err.to_string())
                        .context("prompt error callback failed")?;
                    continue;
                }
            };

            match event {
                HelperEvent::PromptHidden(prompt) => {
                    match prompts
                        .prompt_secret(&prompt)
                        .context("prompt handler failed")?
                    {
                        PromptResponse::Submitted(mut response) => {
                            tracing::debug!(
                                response_len = response.chars().count(),
                                "Submitting secret prompt response to direct helper"
                            );
                            let mut sanitized = sanitize_response(&response);
                            write_line(stdin, &sanitized)
                                .context("failed to send direct helper secret response")?;
                            scrub_string(&mut sanitized);
                            scrub_string(&mut response);
                        }
                        PromptResponse::Canceled => return Ok(HelperOutcome::Canceled),
                        PromptResponse::TimedOut => return Ok(HelperOutcome::Timeout),
                    }
                }
                HelperEvent::PromptVisible(prompt) => {
                    match prompts
                        .prompt_plain(&prompt)
                        .context("prompt handler failed")?
                    {
                        PromptResponse::Submitted(mut response) => {
                            tracing::debug!(
                                response_len = response.chars().count(),
                                "Submitting visible prompt response to direct helper"
                            );
                            let mut sanitized = sanitize_response(&response);
                            write_line(stdin, &sanitized)
                                .context("failed to send direct helper visible response")?;
                            scrub_string(&mut sanitized);
                            scrub_string(&mut response);
                        }
                        PromptResponse::Canceled => return Ok(HelperOutcome::Canceled),
                        PromptResponse::TimedOut => return Ok(HelperOutcome::Timeout),
                    }
                }
                HelperEvent::Error(message) => {
                    prompts
                        .show_error(&message)
                        .context("prompt error callback failed")?;
                }
                HelperEvent::Info(message) => {
                    prompts
                        .show_info(&message)
                        .context("prompt info callback failed")?;
                }
                HelperEvent::Success => {
                    prompts
                        .auth_succeeded()
                        .context("prompt success callback failed")?;
                    return Ok(HelperOutcome::Authorized);
                }
                HelperEvent::Failure => {
                    prompts
                        .auth_failed("Authentication failed")
                        .context("prompt failure callback failed")?;
                    return Ok(HelperOutcome::Denied);
                }
            }
        }
    }
}

#[derive(Debug)]
struct NoSessionForCookieError;

impl std::fmt::Display for NoSessionForCookieError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("socket helper reported no session for cookie")
    }
}

impl std::error::Error for NoSessionForCookieError {}

fn is_no_session_cookie_error(err: &anyhow::Error) -> bool {
    err.downcast_ref::<NoSessionForCookieError>().is_some()
}

#[derive(Debug)]
struct NoInitialHelperResponseError;

impl std::fmt::Display for NoInitialHelperResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("socket helper produced no initial response")
    }
}

impl std::error::Error for NoInitialHelperResponseError {}

fn is_no_initial_helper_response_error(err: &anyhow::Error) -> bool {
    err.downcast_ref::<NoInitialHelperResponseError>().is_some()
}

fn write_line(stream: &mut impl Write, value: &str) -> Result<()> {
    stream.write_all(value.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

fn sanitize_response(raw: &str) -> String {
    raw.lines().collect::<Vec<_>>().join(" ")
}

fn sanitize_control_line(raw: &str) -> String {
    raw.lines().collect::<Vec<_>>().join(" ").trim().to_string()
}

fn resolve_direct_helper_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("GARCARD_POLKIT_HELPER_BIN") {
        let path = PathBuf::from(path);
        if is_viable_direct_helper(&path) {
            return Some(path);
        }
        tracing::warn!(
            helper = %path.display(),
            "Ignoring GARCARD_POLKIT_HELPER_BIN because helper is not root-owned setuid executable"
        );
    }

    let static_candidates = [
        "/run/wrappers/bin/polkit-agent-helper-1",
        "/run/current-system/sw/lib/polkit-1/polkit-agent-helper-1",
        "/usr/lib/polkit-1/polkit-agent-helper-1",
        "/usr/lib64/polkit-1/polkit-agent-helper-1",
        "/lib/polkit-1/polkit-agent-helper-1",
    ];
    for candidate in static_candidates {
        let path = PathBuf::from(candidate);
        if is_viable_direct_helper(&path) {
            return Some(path);
        }
    }

    if let Some(path) =
        command_in_path("polkit-agent-helper-1").filter(|path| is_viable_direct_helper(path))
    {
        return Some(path);
    }

    None
}

fn is_viable_direct_helper(path: &Path) -> bool {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return false,
    };
    let mode = metadata.permissions().mode();
    let is_setuid = mode & 0o4000 != 0;
    let is_executable = mode & 0o111 != 0;
    metadata.uid() == 0 && is_setuid && is_executable
}

fn command_in_path(command: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(command);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HelperTransportMode {
    Auto,
    Direct,
}

impl HelperTransportMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Direct => "direct",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HelperSocketProtocol {
    Auto,
    CookieOnly,
    UsernameCookie,
}

impl HelperSocketProtocol {
    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "socket-activated-auto",
            Self::CookieOnly => "socket-activated-cookie-only",
            Self::UsernameCookie => "socket-activated-username-cookie",
        }
    }
}

fn helper_transport_mode() -> HelperTransportMode {
    match std::env::var(HELPER_TRANSPORT_ENV)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("direct") => HelperTransportMode::Direct,
        _ => HelperTransportMode::Auto,
    }
}

fn helper_socket_protocol() -> HelperSocketProtocol {
    match std::env::var(HELPER_SOCKET_PROTOCOL_ENV)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("cookie-only") | Some("cookie_only") | Some("cookieonly") => {
            HelperSocketProtocol::CookieOnly
        }
        Some("username-cookie") | Some("username_cookie") | Some("usernamecookie") => {
            HelperSocketProtocol::UsernameCookie
        }
        _ => HelperSocketProtocol::Auto,
    }
}

fn scrub_string(value: &mut String) {
    if value.is_empty() {
        return;
    }
    let mut bytes = std::mem::take(value).into_bytes();
    bytes.fill(0);
}

pub fn parse_helper_line(raw: &str) -> Result<HelperEvent> {
    let line = raw.trim_end_matches('\n').trim_end_matches('\r');
    if line == "SUCCESS" {
        return Ok(HelperEvent::Success);
    }
    if line == "FAILURE" {
        return Ok(HelperEvent::Failure);
    }

    if let Some(message) = line.strip_prefix("PAM_PROMPT_ECHO_OFF ") {
        return Ok(HelperEvent::PromptHidden(message.to_string()));
    }
    if let Some(message) = line.strip_prefix("PAM_PROMPT_ECHO_ON ") {
        return Ok(HelperEvent::PromptVisible(message.to_string()));
    }
    if let Some(message) = line.strip_prefix("PAM_ERROR_MSG ") {
        return Ok(HelperEvent::Error(message.to_string()));
    }
    if let Some(message) = line.strip_prefix("PAM_TEXT_INFO ") {
        return Ok(HelperEvent::Info(message.to_string()));
    }

    if line.starts_with("polkit-agent-helper-1:") {
        let lower = line.to_ascii_lowercase();
        if lower.contains("pam_authenticate failed")
            || lower.contains("authentication failure")
            || lower.contains("no session for cookie")
        {
            return Ok(HelperEvent::Error(line.to_string()));
        }
        return Ok(HelperEvent::Info(line.to_string()));
    }

    anyhow::bail!("unsupported helper protocol line: {}", line);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct FakePrompt {
        secret_response: PromptResponse,
        plain_response: PromptResponse,
        infos: Vec<String>,
        errors: Vec<String>,
    }

    impl Default for FakePrompt {
        fn default() -> Self {
            Self {
                secret_response: PromptResponse::Canceled,
                plain_response: PromptResponse::Canceled,
                infos: Vec::new(),
                errors: Vec::new(),
            }
        }
    }

    impl PromptProvider for FakePrompt {
        fn prompt_secret(&mut self, _prompt: &str) -> Result<PromptResponse> {
            Ok(self.secret_response.clone())
        }

        fn prompt_plain(&mut self, _prompt: &str) -> Result<PromptResponse> {
            Ok(self.plain_response.clone())
        }

        fn show_error(&mut self, message: &str) -> Result<()> {
            self.errors.push(message.to_string());
            Ok(())
        }

        fn show_info(&mut self, message: &str) -> Result<()> {
            self.infos.push(message.to_string());
            Ok(())
        }
    }

    fn temp_socket_path() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "garcard-polkit-helper-test-{}-{}.sock",
            std::process::id(),
            nanos
        ))
    }

    #[test]
    fn parse_helper_prompt_lines() {
        assert_eq!(
            parse_helper_line("PAM_PROMPT_ECHO_OFF Password:").expect("parse hidden"),
            HelperEvent::PromptHidden("Password:".to_string())
        );
        assert_eq!(
            parse_helper_line("PAM_PROMPT_ECHO_ON OTP:").expect("parse visible"),
            HelperEvent::PromptVisible("OTP:".to_string())
        );
    }

    #[test]
    fn parse_helper_info_error_and_status() {
        assert_eq!(
            parse_helper_line("PAM_ERROR_MSG nope").expect("parse error"),
            HelperEvent::Error("nope".to_string())
        );
        assert_eq!(
            parse_helper_line("PAM_TEXT_INFO hint").expect("parse info"),
            HelperEvent::Info("hint".to_string())
        );
        assert_eq!(
            parse_helper_line("SUCCESS").expect("parse success"),
            HelperEvent::Success
        );
        assert_eq!(
            parse_helper_line("FAILURE").expect("parse failure"),
            HelperEvent::Failure
        );
    }

    #[test]
    fn parse_helper_line_rejects_unknown_prefix() {
        let err = parse_helper_line("WAT nope").expect_err("unknown line should fail");
        assert!(err.to_string().contains("unsupported helper protocol line"));
    }

    #[test]
    fn parse_helper_line_maps_plaintext_failure_diagnostics_to_error() {
        assert_eq!(
            parse_helper_line(
                "polkit-agent-helper-1: pam_authenticate failed: Authentication failure"
            )
            .expect("maps error"),
            HelperEvent::Error(
                "polkit-agent-helper-1: pam_authenticate failed: Authentication failure"
                    .to_string()
            )
        );
        assert_eq!(
            parse_helper_line("polkit-agent-helper-1: error response to PolicyKit daemon: GDBus.Error:org.freedesktop.PolicyKit1.Error.Failed: No session for cookie")
                .expect("maps cookie error"),
            HelperEvent::Error("polkit-agent-helper-1: error response to PolicyKit daemon: GDBus.Error:org.freedesktop.PolicyKit1.Error.Failed: No session for cookie".to_string())
        );
    }

    #[test]
    fn parse_helper_line_maps_plaintext_info() {
        assert_eq!(
            parse_helper_line("polkit-agent-helper-1: informational message").expect("maps info"),
            HelperEvent::Info("polkit-agent-helper-1: informational message".to_string())
        );
    }

    #[test]
    fn helper_client_drives_prompt_round_trip() {
        let socket_path = temp_socket_path();
        let listener = UnixListener::bind(&socket_path).expect("bind test socket");
        let transcript = Arc::new(Mutex::new(Vec::<String>::new()));
        let transcript_for_thread = Arc::clone(&transcript);

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let read_stream = stream.try_clone().expect("clone");
            let mut reader = BufReader::new(read_stream);

            let mut first_line = String::new();
            reader.read_line(&mut first_line).expect("read first line");
            let first = first_line.trim().to_string();
            let cookie = if first == "alice" {
                let mut cookie = String::new();
                reader.read_line(&mut cookie).expect("read cookie");
                cookie.trim().to_string()
            } else {
                first
            };

            stream
                .write_all(b"PAM_PROMPT_ECHO_OFF Password:\n")
                .expect("write prompt");
            stream.flush().expect("flush prompt");

            let mut secret = String::new();
            reader.read_line(&mut secret).expect("read secret");

            {
                let mut lines = transcript_for_thread.lock().expect("lock transcript");
                lines.push(cookie);
                lines.push(secret.trim().to_string());
            }

            stream.write_all(b"SUCCESS\n").expect("write success");
            stream.flush().expect("flush success");
        });

        let client = HelperSocketClient::new(&socket_path);
        let mut prompts = FakePrompt {
            secret_response: PromptResponse::Submitted("correct horse".to_string()),
            plain_response: PromptResponse::Canceled,
            infos: Vec::new(),
            errors: Vec::new(),
        };

        let result = client
            .authenticate("alice", "cookie-123", &mut prompts)
            .expect("client auth");
        assert_eq!(result, HelperOutcome::Authorized);
        server.join().expect("server join");

        let lines = transcript.lock().expect("lock transcript");
        assert_eq!(lines.as_slice(), ["cookie-123", "correct horse"]);

        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn helper_client_reports_timeout_from_prompt_provider() {
        let socket_path = temp_socket_path();
        let listener = UnixListener::bind(&socket_path).expect("bind test socket");

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let read_stream = stream.try_clone().expect("clone");
            let mut reader = BufReader::new(read_stream);

            let mut first_line = String::new();
            reader.read_line(&mut first_line).expect("read first line");
            if first_line.trim() == "alice" {
                let mut cookie = String::new();
                reader.read_line(&mut cookie).expect("read cookie");
            }

            stream
                .write_all(b"PAM_PROMPT_ECHO_OFF Password:\n")
                .expect("write prompt");
            stream.flush().expect("flush prompt");
        });

        let client = HelperSocketClient::new(&socket_path);
        let mut prompts = FakePrompt {
            secret_response: PromptResponse::TimedOut,
            plain_response: PromptResponse::Canceled,
            infos: Vec::new(),
            errors: Vec::new(),
        };

        let outcome = client
            .authenticate("alice", "cookie-timeout", &mut prompts)
            .expect("authenticate timeout");
        assert_eq!(outcome, HelperOutcome::Timeout);

        server.join().expect("server join");
        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn scrub_string_clears_input() {
        let mut value = "top-secret".to_string();
        scrub_string(&mut value);
        assert!(value.is_empty());
    }
}
