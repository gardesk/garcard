use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

pub const DEFAULT_HELPER_SOCKET: &str = "/run/polkit/agent-helper.socket";

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
        let mut stream = UnixStream::connect(&self.socket_path).with_context(|| {
            format!(
                "failed to connect to polkit helper socket at {}",
                self.socket_path.display()
            )
        })?;
        let read_stream = stream
            .try_clone()
            .context("failed to clone helper socket stream")?;
        let mut reader = BufReader::new(read_stream);

        write_line(&mut stream, username).context("failed to send helper username")?;
        write_line(&mut stream, cookie).context("failed to send helper cookie")?;

        loop {
            let mut line = String::new();
            let bytes = reader
                .read_line(&mut line)
                .context("failed to read helper response line")?;
            if bytes == 0 {
                anyhow::bail!("helper closed connection unexpectedly");
            }

            match parse_helper_line(&line)? {
                HelperEvent::PromptHidden(prompt) => {
                    match prompts
                        .prompt_secret(&prompt)
                        .context("prompt handler failed")?
                    {
                        PromptResponse::Submitted(response) => {
                            write_line(&mut stream, &sanitize_response(&response))
                                .context("failed to send helper secret response")?
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
                        PromptResponse::Submitted(response) => {
                            write_line(&mut stream, &sanitize_response(&response))
                                .context("failed to send helper visible response")?
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
                HelperEvent::Success => return Ok(HelperOutcome::Authorized),
                HelperEvent::Failure => return Ok(HelperOutcome::Denied),
            }
        }
    }
}

fn write_line(stream: &mut UnixStream, value: &str) -> Result<()> {
    stream.write_all(value.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

fn sanitize_response(raw: &str) -> String {
    raw.lines().collect::<Vec<_>>().join(" ")
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
    fn helper_client_drives_prompt_round_trip() {
        let socket_path = temp_socket_path();
        let listener = UnixListener::bind(&socket_path).expect("bind test socket");
        let transcript = Arc::new(Mutex::new(Vec::<String>::new()));
        let transcript_for_thread = Arc::clone(&transcript);

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let read_stream = stream.try_clone().expect("clone");
            let mut reader = BufReader::new(read_stream);

            let mut username = String::new();
            reader.read_line(&mut username).expect("read username");
            let mut cookie = String::new();
            reader.read_line(&mut cookie).expect("read cookie");

            stream
                .write_all(b"PAM_PROMPT_ECHO_OFF Password:\n")
                .expect("write prompt");
            stream.flush().expect("flush prompt");

            let mut secret = String::new();
            reader.read_line(&mut secret).expect("read secret");

            {
                let mut lines = transcript_for_thread.lock().expect("lock transcript");
                lines.push(username.trim().to_string());
                lines.push(cookie.trim().to_string());
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
        assert_eq!(lines.as_slice(), ["alice", "cookie-123", "correct horse"]);

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

            let mut username = String::new();
            reader.read_line(&mut username).expect("read username");
            let mut cookie = String::new();
            reader.read_line(&mut cookie).expect("read cookie");

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
}
