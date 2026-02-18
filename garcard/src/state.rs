use garcard_ipc::{AuthSummary, PROTOCOL_VERSION, StatusData, VersionData};
use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

/// Typed phases for auth flow transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthPhase {
    Idle,
    PendingPrompt,
    Verifying,
    Success,
    Failure,
    Canceled,
    Timeout,
}

impl AuthPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::PendingPrompt => "pending_prompt",
            Self::Verifying => "verifying",
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Canceled => "canceled",
            Self::Timeout => "timeout",
        }
    }

    pub fn from_label(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "idle" => Some(Self::Idle),
            "pending" | "pending_prompt" | "pending-prompt" | "pending prompt" => {
                Some(Self::PendingPrompt)
            }
            "verifying" => Some(Self::Verifying),
            "success" => Some(Self::Success),
            "failure" | "failed" => Some(Self::Failure),
            "canceled" | "cancelled" => Some(Self::Canceled),
            "timeout" | "timed_out" => Some(Self::Timeout),
            _ => None,
        }
    }
}

impl fmt::Display for AuthPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

fn can_transition(current: AuthPhase, next: AuthPhase) -> bool {
    if current == next {
        return true;
    }

    match current {
        AuthPhase::Idle => matches!(next, AuthPhase::PendingPrompt),
        AuthPhase::PendingPrompt => matches!(
            next,
            AuthPhase::Verifying | AuthPhase::Canceled | AuthPhase::Timeout
        ),
        AuthPhase::Verifying => matches!(
            next,
            AuthPhase::Success | AuthPhase::Failure | AuthPhase::Canceled | AuthPhase::Timeout
        ),
        AuthPhase::Success | AuthPhase::Failure | AuthPhase::Canceled | AuthPhase::Timeout => {
            matches!(next, AuthPhase::Idle | AuthPhase::PendingPrompt)
        }
    }
}

/// Tracks auth workflow state with no sensitive data.
#[derive(Debug)]
pub struct AuthState {
    current_phase: RwLock<AuthPhase>,
    active_requests: AtomicUsize,
    queued_requests: AtomicUsize,
}

impl Default for AuthState {
    fn default() -> Self {
        Self {
            current_phase: RwLock::new(AuthPhase::Idle),
            active_requests: AtomicUsize::new(0),
            queued_requests: AtomicUsize::new(0),
        }
    }
}

impl AuthState {
    pub fn phase(&self) -> AuthPhase {
        self.current_phase.read().map(|phase| *phase).unwrap_or(AuthPhase::Idle)
    }

    pub fn set_phase(&self, next: AuthPhase) {
        if let Ok(mut phase) = self.current_phase.write() {
            *phase = next;
        }
    }

    pub fn transition(&self, next: AuthPhase) -> bool {
        if let Ok(mut phase) = self.current_phase.write() {
            if can_transition(*phase, next) {
                *phase = next;
                return true;
            }
            return false;
        }

        false
    }

    pub fn set_state(&self, next: impl AsRef<str>) {
        if let Some(phase) = AuthPhase::from_label(next.as_ref()) {
            self.set_phase(phase);
        }
    }

    pub fn set_active_requests(&self, count: usize) {
        self.active_requests.store(count, Ordering::Relaxed);
    }

    pub fn set_queued_requests(&self, count: usize) {
        self.queued_requests.store(count, Ordering::Relaxed);
    }

    pub fn sync_queue_counts(&self, active: usize, queued: usize) {
        self.set_active_requests(active);
        self.set_queued_requests(queued);
    }

    pub fn summary(&self) -> AuthSummary {
        AuthSummary {
            state: self.phase().to_string(),
            active_requests: self.active_requests.load(Ordering::Relaxed),
            queued_requests: self.queued_requests.load(Ordering::Relaxed),
        }
    }
}

/// Queue policy for concurrent auth requests: one active request and FIFO backlog.
#[derive(Debug, Clone)]
pub struct AuthQueue<T> {
    active: Option<T>,
    queued: VecDeque<T>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueInsert {
    Activated,
    Queued { position: usize },
}

impl<T> Default for AuthQueue<T> {
    fn default() -> Self {
        Self {
            active: None,
            queued: VecDeque::new(),
        }
    }
}

impl<T> AuthQueue<T> {
    pub fn push(&mut self, request: T) -> QueueInsert {
        if self.active.is_none() {
            self.active = Some(request);
            QueueInsert::Activated
        } else {
            self.queued.push_back(request);
            QueueInsert::Queued {
                position: self.queued.len(),
            }
        }
    }

    pub fn active(&self) -> Option<&T> {
        self.active.as_ref()
    }

    pub fn active_mut(&mut self) -> Option<&mut T> {
        self.active.as_mut()
    }

    pub fn take_active_if<F>(&mut self, mut predicate: F) -> Option<T>
    where
        F: FnMut(&T) -> bool,
    {
        if self.active.as_ref().is_some_and(&mut predicate) {
            return self.complete_active();
        }

        None
    }

    pub fn remove_queued_if<F>(&mut self, mut predicate: F) -> bool
    where
        F: FnMut(&T) -> bool,
    {
        if let Some(index) = self.queued.iter().position(&mut predicate) {
            self.queued.remove(index);
            return true;
        }

        false
    }

    pub fn complete_active(&mut self) -> Option<T> {
        let finished = self.active.take();
        self.promote_next();
        finished
    }

    pub fn cancel_active(&mut self) -> Option<T> {
        self.complete_active()
    }

    pub fn active_len(&self) -> usize {
        usize::from(self.active.is_some())
    }

    pub fn queued_len(&self) -> usize {
        self.queued.len()
    }

    pub fn is_empty(&self) -> bool {
        self.active.is_none() && self.queued.is_empty()
    }

    pub fn clear(&mut self) {
        self.active = None;
        self.queued.clear();
    }

    pub fn counts(&self) -> (usize, usize) {
        (self.active_len(), self.queued_len())
    }

    fn promote_next(&mut self) {
        if self.active.is_none() {
            self.active = self.queued.pop_front();
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
    auth: Arc<AuthState>,
}

impl RuntimeState {
    pub fn new(socket_path: String, backend_name: &'static str) -> Self {
        Self::with_auth(socket_path, backend_name, Arc::new(AuthState::default()))
    }

    pub fn with_auth(socket_path: String, backend_name: &'static str, auth: Arc<AuthState>) -> Self {
        auth.set_phase(AuthPhase::Idle);
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
    pub fn auth_mutation(&self) -> &Arc<AuthState> {
        &self.auth
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_state_defaults_to_idle_phase() {
        let state = AuthState::default();
        let summary = state.summary();
        assert_eq!(summary.state, "idle");
        assert_eq!(summary.active_requests, 0);
        assert_eq!(summary.queued_requests, 0);
    }

    #[test]
    fn auth_state_updates_summary() {
        let state = AuthState::default();
        state.set_phase(AuthPhase::Verifying);
        state.set_active_requests(2);
        state.set_queued_requests(3);

        let summary = state.summary();
        assert_eq!(summary.state, "verifying");
        assert_eq!(summary.active_requests, 2);
        assert_eq!(summary.queued_requests, 3);
    }

    #[test]
    fn auth_phase_transition_rules_are_enforced() {
        let state = AuthState::default();
        assert!(state.transition(AuthPhase::PendingPrompt));
        assert!(state.transition(AuthPhase::Verifying));
        assert!(state.transition(AuthPhase::Failure));
        assert!(state.transition(AuthPhase::Idle));
        assert!(!state.transition(AuthPhase::Verifying));
        assert_eq!(state.phase(), AuthPhase::Idle);
    }

    #[test]
    fn auth_queue_activates_then_queues() {
        let mut queue = AuthQueue::default();
        assert_eq!(queue.push(10), QueueInsert::Activated);
        assert_eq!(queue.push(20), QueueInsert::Queued { position: 1 });
        assert_eq!(queue.push(30), QueueInsert::Queued { position: 2 });
        assert_eq!(queue.active(), Some(&10));
        assert_eq!(queue.counts(), (1, 2));
    }

    #[test]
    fn auth_queue_completion_promotes_next_request() {
        let mut queue = AuthQueue::default();
        queue.push("first");
        queue.push("second");
        queue.push("third");

        assert_eq!(queue.complete_active(), Some("first"));
        assert_eq!(queue.active(), Some(&"second"));
        assert_eq!(queue.complete_active(), Some("second"));
        assert_eq!(queue.active(), Some(&"third"));
        assert_eq!(queue.complete_active(), Some("third"));
        assert!(queue.is_empty());
    }

    #[test]
    fn auth_queue_take_active_if_promotes_next() {
        let mut queue = AuthQueue::default();
        queue.push("cookie-a");
        queue.push("cookie-b");

        let removed = queue.take_active_if(|value| *value == "cookie-a");
        assert_eq!(removed, Some("cookie-a"));
        assert_eq!(queue.active(), Some(&"cookie-b"));
    }

    #[test]
    fn auth_queue_remove_queued_if_removes_specific_item() {
        let mut queue = AuthQueue::default();
        queue.push("cookie-a");
        queue.push("cookie-b");
        queue.push("cookie-c");

        assert!(queue.remove_queued_if(|value| *value == "cookie-b"));
        assert_eq!(queue.counts(), (1, 1));
        assert!(!queue.remove_queued_if(|value| *value == "missing"));
    }
}
