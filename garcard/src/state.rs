use garcard_ipc::{AuthSummary, PROTOCOL_VERSION, StatusData, VersionData};
use std::sync::RwLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

/// Tracks auth workflow state with no sensitive data.
#[derive(Debug)]
pub struct AuthState {
    current_state: RwLock<String>,
    active_requests: AtomicUsize,
    queued_requests: AtomicUsize,
}

impl Default for AuthState {
    fn default() -> Self {
        Self {
            current_state: RwLock::new("idle".to_string()),
            active_requests: AtomicUsize::new(0),
            queued_requests: AtomicUsize::new(0),
        }
    }
}

impl AuthState {
    pub fn set_state(&self, next: impl Into<String>) {
        if let Ok(mut state) = self.current_state.write() {
            *state = next.into();
        }
    }

    pub fn set_active_requests(&self, count: usize) {
        self.active_requests.store(count, Ordering::Relaxed);
    }

    pub fn set_queued_requests(&self, count: usize) {
        self.queued_requests.store(count, Ordering::Relaxed);
    }

    pub fn summary(&self) -> AuthSummary {
        let state = self
            .current_state
            .read()
            .map(|value| value.clone())
            .unwrap_or_else(|_| "unknown".to_string());

        AuthSummary {
            state,
            active_requests: self.active_requests.load(Ordering::Relaxed),
            queued_requests: self.queued_requests.load(Ordering::Relaxed),
        }
    }
}

/// Immutable process/runtime metadata for IPC status.
#[derive(Debug)]
pub struct RuntimeState {
    started_at: Instant,
    pid: u32,
    socket_path: String,
    backend_name: &'static str,
    auth: AuthState,
}

impl RuntimeState {
    pub fn new(socket_path: String, backend_name: &'static str) -> Self {
        let auth = AuthState::default();
        auth.set_state("idle");
        auth.set_active_requests(0);
        auth.set_queued_requests(0);

        Self {
            started_at: Instant::now(),
            pid: std::process::id(),
            socket_path,
            backend_name,
            auth,
        }
    }

    pub fn status(&self) -> StatusData {
        StatusData {
            running: true,
            pid: self.pid,
            uptime_secs: self.started_at.elapsed().as_secs(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_version: PROTOCOL_VERSION,
            socket_path: self.socket_path.clone(),
            agent_backend: self.backend_name.to_string(),
        }
    }

    pub fn version(&self) -> VersionData {
        VersionData {
            component: "garcard".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_version: PROTOCOL_VERSION,
        }
    }

    pub fn auth_summary(&self) -> AuthSummary {
        self.auth.summary()
    }

    #[allow(dead_code)]
    pub fn auth_mutation(&self) -> &AuthState {
        &self.auth
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_state_defaults_to_idle() {
        let state = AuthState::default();
        let summary = state.summary();
        assert_eq!(summary.state, "idle");
        assert_eq!(summary.active_requests, 0);
        assert_eq!(summary.queued_requests, 0);
    }

    #[test]
    fn auth_state_updates_summary() {
        let state = AuthState::default();
        state.set_state("verifying");
        state.set_active_requests(2);
        state.set_queued_requests(3);

        let summary = state.summary();
        assert_eq!(summary.state, "verifying");
        assert_eq!(summary.active_requests, 2);
        assert_eq!(summary.queued_requests, 3);
    }
}
