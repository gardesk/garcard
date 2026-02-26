use crate::polkit_helper::{
    DEFAULT_HELPER_SOCKET, HelperOutcome, HelperSocketClient, PromptProvider, PromptResponse,
};
use crate::prompt::CommandPrompt;
use crate::state::{AuthPhase, AuthQueue, AuthState, QueueInsert};
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};
use zbus::blocking::{Connection, Proxy};
use zbus::fdo;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Str};

/// Backend interface for Polkit agent integration.
pub trait AuthAgentBackend {
    fn name(&self) -> &'static str;
    fn register(&mut self) -> Result<()>;
    fn unregister(&mut self) -> Result<()>;
    fn has_active_auth(&self) -> bool {
        false
    }
    fn maintain(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Placeholder backend used during Sprint 01 scaffolding.
#[derive(Default)]
pub struct StubPolkitAgent;

impl AuthAgentBackend for StubPolkitAgent {
    fn name(&self) -> &'static str {
        "stub-polkit-agent"
    }

    fn register(&mut self) -> Result<()> {
        tracing::info!("Registered stub auth-agent backend");
        Ok(())
    }

    fn unregister(&mut self) -> Result<()> {
        tracing::info!("Unregistered stub auth-agent backend");
        Ok(())
    }
}

/// Runtime configuration for the real Polkit backend.
#[derive(Debug, Clone)]
pub struct PolkitBackendConfig {
    pub object_path: String,
    pub locale: String,
}

type Subject = (String, HashMap<String, OwnedValue>);
type Details = HashMap<String, String>;
type TemporaryAuthorization = (String, String, Subject, u64, u64);

#[derive(Debug, Clone, serde::Serialize)]
pub struct TemporaryAuthorizationRecord {
    pub authorization_id: String,
    pub action_id: String,
    pub obtained_at_unix: u64,
    pub expires_at_unix: u64,
    pub expires_in_secs: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SubjectResolution {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time_ticks: Option<u64>,
    pub has_start_time: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AuthorityDiagnostics {
    pub authority_connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority_error: Option<String>,
    pub subject: SubjectResolution,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temporary_authorization_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temporary_authorization_error: Option<String>,
}
const DEFAULT_AUTH_MAX_ATTEMPTS: usize = 3;
const DEFAULT_IDENTITY_SELECTION_ATTEMPTS: usize = 3;
const DEFAULT_RETENTION_SELECTION_ATTEMPTS: usize = 3;

#[derive(Debug)]
struct AuthRequest {
    action_id: String,
    message: String,
    icon_name: String,
    details: Details,
    cookie: String,
    identities: Vec<Subject>,
}

#[derive(Debug, Clone)]
struct ActiveRequest {
    action_id: String,
    message: String,
    icon_name: String,
    detail_count: usize,
    details: Details,
    cookie: String,
    username: String,
    identity_options: Vec<String>,
    retention_options: Vec<RetentionPolicy>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetentionPolicy {
    OneShot,
    Session,
    Always,
}

impl RetentionPolicy {
    fn label(self) -> &'static str {
        match self {
            Self::OneShot => "one-shot",
            Self::Session => "keep-session",
            Self::Always => "keep-always",
        }
    }

    fn prompt_label(self) -> &'static str {
        match self {
            Self::OneShot => "One-shot",
            Self::Session => "Keep for session",
            Self::Always => "Keep always",
        }
    }
}

#[derive(Debug)]
struct PolkitRuntime {
    auth_state: Arc<AuthState>,
    queue: Mutex<AuthQueue<AuthRequest>>,
    outcomes: Mutex<HashMap<String, HelperOutcome>>,
    canceled_cookies: Mutex<HashSet<String>>,
    outcome_signal: Condvar,
    helper_client: HelperSocketClient,
    processing: AtomicBool,
    worker_enabled: bool,
}

trait RetryPromptProvider: PromptProvider {
    fn on_retry(&mut self) {}
}

impl RetryPromptProvider for CommandPrompt {
    fn on_retry(&mut self) {
        self.set_next_prompt_error_tone();
    }
}

struct CancellationAwarePrompt<'a, P> {
    inner: &'a mut P,
    runtime: &'a PolkitRuntime,
    cookie: &'a str,
}

impl<'a, P> CancellationAwarePrompt<'a, P> {
    fn new(inner: &'a mut P, runtime: &'a PolkitRuntime, cookie: &'a str) -> Self {
        Self {
            inner,
            runtime,
            cookie,
        }
    }

    fn canceled(&self) -> bool {
        self.runtime.is_canceled(self.cookie)
    }
}

impl<P: PromptProvider> PromptProvider for CancellationAwarePrompt<'_, P> {
    fn prompt_secret(&mut self, prompt: &str) -> Result<crate::polkit_helper::PromptResponse> {
        if self.canceled() {
            return Ok(crate::polkit_helper::PromptResponse::Canceled);
        }
        self.inner.prompt_secret(prompt)
    }

    fn prompt_plain(&mut self, prompt: &str) -> Result<crate::polkit_helper::PromptResponse> {
        if self.canceled() {
            return Ok(crate::polkit_helper::PromptResponse::Canceled);
        }
        self.inner.prompt_plain(prompt)
    }

    fn show_error(&mut self, message: &str) -> Result<()> {
        if self.canceled() {
            return Ok(());
        }
        self.inner.show_error(message)
    }

    fn show_info(&mut self, message: &str) -> Result<()> {
        if self.canceled() {
            return Ok(());
        }
        self.inner.show_info(message)
    }

    fn auth_succeeded(&mut self) -> Result<()> {
        if self.canceled() {
            return Ok(());
        }
        self.inner.auth_succeeded()
    }

    fn auth_failed(&mut self, message: &str) -> Result<()> {
        if self.canceled() {
            return Ok(());
        }
        self.inner.auth_failed(message)
    }
}

impl PolkitRuntime {
    fn new(auth_state: Arc<AuthState>) -> Self {
        Self::new_with_worker(auth_state, true)
    }

    #[cfg(test)]
    fn new_without_worker(auth_state: Arc<AuthState>) -> Self {
        Self::new_with_worker(auth_state, false)
    }

    fn new_with_worker(auth_state: Arc<AuthState>, worker_enabled: bool) -> Self {
        let helper_socket = std::env::var_os("GARCARD_POLKIT_HELPER_SOCKET")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_HELPER_SOCKET));
        Self {
            auth_state,
            queue: Mutex::new(AuthQueue::default()),
            outcomes: Mutex::new(HashMap::new()),
            canceled_cookies: Mutex::new(HashSet::new()),
            outcome_signal: Condvar::new(),
            helper_client: HelperSocketClient::new(helper_socket),
            processing: AtomicBool::new(false),
            worker_enabled,
        }
    }

    fn begin_authentication(self: &Arc<Self>, request: AuthRequest) -> Result<QueueInsert> {
        let cookie = request.cookie.clone();
        let (insert, active, queued) = {
            let mut queue = self
                .queue
                .lock()
                .map_err(|_| anyhow::anyhow!("auth request queue lock poisoned"))?;
            let insert = queue.push(request);
            let (active, queued) = queue.counts();
            (insert, active, queued)
        };
        self.clear_canceled(&cookie);

        self.auth_state.sync_queue_counts(active, queued);
        if matches!(
            self.auth_state.phase(),
            AuthPhase::Idle
                | AuthPhase::Success
                | AuthPhase::Failure
                | AuthPhase::Canceled
                | AuthPhase::Timeout
        ) {
            self.auth_state.set_phase(AuthPhase::PendingPrompt);
        }

        if self.worker_enabled && active > 0 {
            self.ensure_worker();
        }

        Ok(insert)
    }

    fn cancel_authentication(self: &Arc<Self>, cookie: &str) -> Result<bool> {
        let (canceled_active, removed_queued, active, queued) = {
            let mut queue = self
                .queue
                .lock()
                .map_err(|_| anyhow::anyhow!("auth request queue lock poisoned"))?;
            let canceled_active = queue
                .take_active_if(|request| request.cookie == cookie)
                .is_some();
            let removed_queued = if canceled_active {
                false
            } else {
                queue.remove_queued_if(|request| request.cookie == cookie)
            };
            let (active, queued) = queue.counts();
            (canceled_active, removed_queued, active, queued)
        };

        if canceled_active || removed_queued {
            self.mark_canceled(cookie);
            self.record_outcome(cookie, HelperOutcome::Canceled);
        }

        self.auth_state.sync_queue_counts(active, queued);
        if canceled_active {
            if active > 0 {
                self.auth_state.set_phase(AuthPhase::PendingPrompt);
            } else {
                self.auth_state.set_phase(AuthPhase::Canceled);
            }
            if self.worker_enabled && active > 0 {
                self.ensure_worker();
            }
            return Ok(true);
        }

        if removed_queued {
            if active == 0 && queued == 0 {
                self.auth_state.set_phase(AuthPhase::Idle);
            }
            return Ok(true);
        }

        Ok(false)
    }

    fn ensure_worker(self: &Arc<Self>) {
        if self.processing.swap(true, Ordering::AcqRel) {
            return;
        }

        let runtime = Arc::clone(self);
        let spawn_result = thread::Builder::new()
            .name("garcard-auth-worker".to_string())
            .spawn(move || runtime.process_loop());
        if let Err(err) = spawn_result {
            self.processing.store(false, Ordering::Release);
            tracing::error!(error = %err, "Failed to spawn garcard auth worker");
        }
    }

    fn process_loop(self: Arc<Self>) {
        loop {
            let Some(active) = self.active_request_snapshot() else {
                break;
            };

            self.auth_state.set_phase(AuthPhase::Verifying);
            tracing::info!(
                action_id = %active.action_id,
                icon_name = %active.icon_name,
                detail_count = active.detail_count,
                username = %active.username,
                "Processing polkit auth request"
            );

            let outcome = self.authenticate_active_request(&active);
            self.complete_request(&active.cookie, outcome);
        }

        self.processing.store(false, Ordering::Release);
        if self.has_active_request() {
            self.ensure_worker();
        }
    }

    fn has_active_request(&self) -> bool {
        self.queue
            .lock()
            .map(|queue| queue.active().is_some())
            .unwrap_or(false)
    }

    fn active_request_snapshot(&self) -> Option<ActiveRequest> {
        let queue = self.queue.lock().ok()?;
        let request = queue.active()?;

        let username = resolve_identity_username(&request.identities)
            .or_else(current_username)
            .or_else(|| std::env::var("USER").ok())
            .unwrap_or_else(|| "unknown".to_string());
        let mut identity_options = identity_options_from_subjects(&request.identities);
        if !identity_options
            .iter()
            .any(|candidate| candidate == &username)
        {
            identity_options.insert(0, username.clone());
        }
        let retention_options = retention_options_from_details(&request.details);

        tracing::debug!(
            identity_summary = %summarize_identities(&request.identities),
            selected_username = %username,
            "Resolved polkit auth identity"
        );

        Some(ActiveRequest {
            action_id: request.action_id.clone(),
            message: request.message.clone(),
            icon_name: request.icon_name.clone(),
            detail_count: request.details.len(),
            details: request.details.clone(),
            cookie: request.cookie.clone(),
            username,
            identity_options,
            retention_options,
        })
    }

    fn authenticate_active_request(&self, request: &ActiveRequest) -> HelperOutcome {
        let mut prompts = CommandPrompt::default();
        self.authenticate_active_request_with_prompts(request, &mut prompts)
    }

    fn authenticate_active_request_with_prompts<P: RetryPromptProvider>(
        &self,
        request: &ActiveRequest,
        prompts: &mut P,
    ) -> HelperOutcome {
        let prompt_context = render_prompt_context(request);
        let max_attempts = auth_max_attempts();
        let username = match select_identity_for_request(request, prompts) {
            IdentitySelection::Selected(selected) => selected,
            IdentitySelection::Terminal(outcome) => {
                return self.finalize_auth_attempt(request, outcome, None);
            }
        };
        let retention = match select_retention_for_request(request, prompts) {
            RetentionSelection::Selected(selected) => selected,
            RetentionSelection::Terminal(outcome) => {
                return self.finalize_auth_attempt(request, outcome, None);
            }
        };
        tracing::info!(
            action_id = %request.action_id,
            retention = retention.label(),
            "Selected retention policy for authentication request"
        );

        for attempt in 1..=max_attempts {
            if self.is_canceled(&request.cookie) {
                tracing::info!(
                    action_id = %request.action_id,
                    "Authentication request canceled before helper attempt"
                );
                return self.finalize_auth_attempt(
                    request,
                    HelperOutcome::Canceled,
                    Some(retention),
                );
            }
            tracing::info!(
                context = %prompt_context,
                attempt,
                max_attempts,
                "Starting helper authentication dialog"
            );

            let mut cancel_aware =
                CancellationAwarePrompt::new(prompts, self, request.cookie.as_str());
            match self
                .helper_client
                .authenticate(&username, &request.cookie, &mut cancel_aware)
            {
                Ok(outcome) if self.is_canceled(&request.cookie) => {
                    tracing::info!(
                        action_id = %request.action_id,
                        "Authentication request canceled during helper attempt"
                    );
                    return self.finalize_auth_attempt(
                        request,
                        HelperOutcome::Canceled,
                        Some(retention),
                    );
                }
                Ok(HelperOutcome::Denied) if attempt < max_attempts => {
                    prompts.on_retry();
                    self.auth_state.set_phase(AuthPhase::PendingPrompt);
                    tracing::warn!(
                        action_id = %request.action_id,
                        attempt,
                        max_attempts,
                        "Authentication denied; retrying prompt"
                    );
                    continue;
                }
                Ok(outcome) => {
                    return self.finalize_auth_attempt(request, outcome, Some(retention));
                }
                Err(err) => {
                    tracing::warn!(
                        action_id = %request.action_id,
                        attempt,
                        max_attempts,
                        error = %err,
                        "Polkit helper authentication failed"
                    );
                    if attempt < max_attempts {
                        prompts.on_retry();
                        self.auth_state.set_phase(AuthPhase::PendingPrompt);
                        continue;
                    }
                    return self.finalize_auth_attempt(
                        request,
                        HelperOutcome::Denied,
                        Some(retention),
                    );
                }
            }
        }

        self.finalize_auth_attempt(request, HelperOutcome::Denied, Some(retention))
    }

    fn finalize_auth_attempt(
        &self,
        request: &ActiveRequest,
        outcome: HelperOutcome,
        retention: Option<RetentionPolicy>,
    ) -> HelperOutcome {
        let retention_enforced = self.enforce_retention_policy(request, outcome, retention);
        self.auth_state.set_last_decision(
            request.action_id.clone(),
            helper_outcome_label(outcome),
            retention.map(|policy| policy.label().to_string()),
            retention_enforced,
        );
        outcome
    }

    fn enforce_retention_policy(
        &self,
        request: &ActiveRequest,
        outcome: HelperOutcome,
        retention: Option<RetentionPolicy>,
    ) -> bool {
        if outcome != HelperOutcome::Authorized {
            return false;
        }
        let Some(retention) = retention else {
            return false;
        };
        if retention != RetentionPolicy::OneShot {
            return false;
        }
        if request.retention_options.len() <= 1 {
            return false;
        }

        match revoke_temporary_authorizations_for_action(&request.action_id) {
            Ok(revoked) => {
                tracing::info!(
                    action_id = %request.action_id,
                    revoked_count = revoked,
                    "Applied one-shot retention policy by revoking temporary authorizations"
                );
                true
            }
            Err(err) => {
                tracing::warn!(
                    action_id = %request.action_id,
                    error = %err,
                    "Failed to enforce one-shot retention policy"
                );
                false
            }
        }
    }

    fn complete_request(&self, cookie: &str, outcome: HelperOutcome) {
        let (removed, active, queued) = {
            let mut queue = match self.queue.lock() {
                Ok(queue) => queue,
                Err(_) => {
                    tracing::error!("auth request queue lock poisoned");
                    return;
                }
            };
            let removed = queue
                .complete_active_if(|request| request.cookie == cookie)
                .is_some();
            let (active, queued) = queue.counts();
            (removed, active, queued)
        };

        self.auth_state.sync_queue_counts(active, queued);
        if !removed {
            if active == 0 && queued == 0 && self.auth_state.phase() == AuthPhase::Verifying {
                self.auth_state.set_phase(AuthPhase::Idle);
            }
            return;
        }
        self.clear_canceled(cookie);
        self.record_outcome(cookie, outcome);

        if active > 0 {
            self.auth_state.set_phase(AuthPhase::PendingPrompt);
            return;
        }

        let phase = match outcome {
            HelperOutcome::Authorized => AuthPhase::Success,
            HelperOutcome::Denied => AuthPhase::Failure,
            HelperOutcome::Canceled => AuthPhase::Canceled,
            HelperOutcome::Timeout => AuthPhase::Timeout,
        };
        self.auth_state.set_phase(phase);
    }

    fn record_outcome(&self, cookie: &str, outcome: HelperOutcome) {
        let mut outcomes = match self.outcomes.lock() {
            Ok(outcomes) => outcomes,
            Err(_) => {
                tracing::error!("auth outcome map lock poisoned");
                return;
            }
        };
        outcomes.insert(cookie.to_string(), outcome);
        self.outcome_signal.notify_all();
    }

    fn mark_canceled(&self, cookie: &str) {
        if let Ok(mut canceled) = self.canceled_cookies.lock() {
            canceled.insert(cookie.to_string());
        } else {
            tracing::error!("auth canceled-cookie set lock poisoned");
        }
    }

    fn clear_canceled(&self, cookie: &str) {
        if let Ok(mut canceled) = self.canceled_cookies.lock() {
            canceled.remove(cookie);
        } else {
            tracing::error!("auth canceled-cookie set lock poisoned");
        }
    }

    fn is_canceled(&self, cookie: &str) -> bool {
        self.canceled_cookies
            .lock()
            .map(|canceled| canceled.contains(cookie))
            .unwrap_or(false)
    }

    fn wait_for_completion(&self, cookie: &str) -> Result<HelperOutcome> {
        let mut outcomes = self
            .outcomes
            .lock()
            .map_err(|_| anyhow::anyhow!("auth outcome map lock poisoned"))?;
        loop {
            if let Some(outcome) = outcomes.remove(cookie) {
                drop(outcomes);
                self.clear_canceled(cookie);
                return Ok(outcome);
            }
            outcomes = self
                .outcome_signal
                .wait(outcomes)
                .map_err(|_| anyhow::anyhow!("auth outcome map lock poisoned"))?;
        }
    }

    fn reset(&self) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.clear();
        }
        if let Ok(mut outcomes) = self.outcomes.lock() {
            outcomes.clear();
        }
        if let Ok(mut canceled) = self.canceled_cookies.lock() {
            canceled.clear();
        }
        self.processing.store(false, Ordering::Release);
        self.auth_state.set_phase(AuthPhase::Idle);
        self.auth_state.sync_queue_counts(0, 0);
    }
}

fn helper_outcome_label(outcome: HelperOutcome) -> &'static str {
    match outcome {
        HelperOutcome::Authorized => "success",
        HelperOutcome::Denied => "failure",
        HelperOutcome::Canceled => "canceled",
        HelperOutcome::Timeout => "timeout",
    }
}

pub fn enumerate_temporary_authorizations() -> Result<Vec<TemporaryAuthorizationRecord>> {
    let connection = Connection::system().context("failed to connect to system bus")?;
    let subject = build_subject();
    enumerate_temporary_authorizations_for_subject(&connection, &subject)
}

pub fn revoke_temporary_authorization_by_id(authorization_id: &str) -> Result<()> {
    let connection = Connection::system().context("failed to connect to system bus")?;
    let proxy = PolkitAgent::proxy(&connection)?;
    let _: () = proxy.call("RevokeTemporaryAuthorizationById", &authorization_id)?;
    Ok(())
}

pub fn revoke_all_temporary_authorizations() -> Result<usize> {
    let connection = Connection::system().context("failed to connect to system bus")?;
    let subject = build_subject();
    let proxy = PolkitAgent::proxy(&connection)?;
    let authorizations: Vec<TemporaryAuthorization> =
        proxy.call("EnumerateTemporaryAuthorizations", &subject)?;

    let mut revoked = 0_usize;
    for (authorization_id, _action_id, _subject, _obtained, _expires) in authorizations {
        let _: () = proxy.call("RevokeTemporaryAuthorizationById", &authorization_id)?;
        revoked += 1;
    }

    Ok(revoked)
}

fn revoke_temporary_authorizations_for_action(action_id: &str) -> Result<usize> {
    let connection = Connection::system().context("failed to connect to system bus")?;
    let subject = build_subject();
    let proxy = PolkitAgent::proxy(&connection)?;
    let authorizations: Vec<TemporaryAuthorization> =
        proxy.call("EnumerateTemporaryAuthorizations", &subject)?;

    let mut revoked = 0_usize;
    for (authorization_id, auth_action_id, _subject, _obtained, _expires) in authorizations {
        if auth_action_id != action_id {
            continue;
        }

        let _: () = proxy.call("RevokeTemporaryAuthorizationById", &authorization_id)?;
        revoked += 1;
    }

    Ok(revoked)
}

pub fn current_subject_resolution() -> SubjectResolution {
    if let Some(session_id) = current_session_id() {
        return SubjectResolution {
            kind: "unix-session".to_string(),
            session_id: Some(session_id),
            pid: None,
            uid: None,
            start_time_ticks: None,
            has_start_time: false,
        };
    }

    let start_time_ticks = process_start_time_ticks();
    SubjectResolution {
        kind: "unix-process".to_string(),
        session_id: None,
        pid: Some(std::process::id()),
        uid: Some(nix::unistd::geteuid().as_raw()),
        start_time_ticks,
        has_start_time: start_time_ticks.is_some(),
    }
}

pub fn collect_authority_diagnostics() -> AuthorityDiagnostics {
    let subject = current_subject_resolution();
    let connection = match Connection::system() {
        Ok(connection) => connection,
        Err(err) => {
            return AuthorityDiagnostics {
                authority_connected: false,
                authority_error: Some(format!("failed to connect to system bus: {}", err)),
                subject,
                temporary_authorization_count: None,
                temporary_authorization_error: None,
            };
        }
    };

    if let Err(err) = PolkitAgent::ping_authority(&connection) {
        return AuthorityDiagnostics {
            authority_connected: false,
            authority_error: Some(format!("polkit authority ping failed: {}", err)),
            subject,
            temporary_authorization_count: None,
            temporary_authorization_error: None,
        };
    }

    let polkit_subject = subject_to_polkit_subject(&subject);
    match enumerate_temporary_authorizations_for_subject(&connection, &polkit_subject) {
        Ok(authorizations) => AuthorityDiagnostics {
            authority_connected: true,
            authority_error: None,
            subject,
            temporary_authorization_count: Some(authorizations.len()),
            temporary_authorization_error: None,
        },
        Err(err) => AuthorityDiagnostics {
            authority_connected: true,
            authority_error: None,
            subject,
            temporary_authorization_count: None,
            temporary_authorization_error: Some(err.to_string()),
        },
    }
}

fn enumerate_temporary_authorizations_for_subject(
    connection: &Connection,
    subject: &Subject,
) -> Result<Vec<TemporaryAuthorizationRecord>> {
    let proxy = PolkitAgent::proxy(connection)?;
    let authorizations: Vec<TemporaryAuthorization> =
        proxy.call("EnumerateTemporaryAuthorizations", subject)?;
    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);

    let mut entries = Vec::with_capacity(authorizations.len());
    for (authorization_id, action_id, _subject, obtained_at_unix, expires_at_unix) in authorizations
    {
        let expires_in_secs = expires_at_unix.saturating_sub(now_unix);
        entries.push(TemporaryAuthorizationRecord {
            authorization_id,
            action_id,
            obtained_at_unix,
            expires_at_unix,
            expires_in_secs,
        });
    }
    entries.sort_by(|left, right| left.action_id.cmp(&right.action_id));
    Ok(entries)
}

fn render_prompt_context(request: &ActiveRequest) -> String {
    let mut lines = Vec::new();
    let message = request.message.trim();
    if !message.is_empty() {
        lines.push(message.to_string());
    } else {
        lines.push("Authentication is required".to_string());
    }

    lines.push(format!("Action: {}", request.action_id));
    if !request.icon_name.trim().is_empty() {
        lines.push(format!("Icon: {}", request.icon_name.trim()));
    }

    if let Some(vendor) = first_detail_value(
        &request.details,
        &[
            "vendor",
            "vendor_name",
            "polkit.vendor",
            "polkit.vendor_name",
        ],
    ) {
        lines.push(format!("Vendor: {}", vendor));
    }

    if let Some(application) = first_detail_value(
        &request.details,
        &[
            "application",
            "application_name",
            "program_name",
            "polkit.program_name",
        ],
    ) {
        lines.push(format!("Application: {}", application));
    }

    if !request.retention_options.is_empty() {
        let supported = request
            .retention_options
            .iter()
            .map(|policy| policy.label())
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("Retention options: {}", supported));
    }

    let detail_keys = [
        "program",
        "polkit.exec.path",
        "polkit.exec.argv1",
        "command_line",
        "unit",
        "verb",
        "polkit.message",
        "polkit.gettext_domain",
        "polkit.retains_authorization_after_challenge",
    ];
    for key in detail_keys {
        if let Some(value) = request.details.get(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                lines.push(format!("{}: {}", detail_key_label(key), trimmed));
            }
        }
    }

    lines.join("\n")
}

enum IdentitySelection {
    Selected(String),
    Terminal(HelperOutcome),
}

enum RetentionSelection {
    Selected(RetentionPolicy),
    Terminal(HelperOutcome),
}

fn select_identity_for_request<P: PromptProvider>(
    request: &ActiveRequest,
    prompts: &mut P,
) -> IdentitySelection {
    if request.identity_options.len() <= 1 {
        return IdentitySelection::Selected(request.username.clone());
    }

    let prompt = render_identity_selection_prompt(request);
    for attempt in 1..=DEFAULT_IDENTITY_SELECTION_ATTEMPTS {
        match prompts.prompt_plain(&prompt) {
            Ok(PromptResponse::Submitted(mut raw)) => {
                if let Some(selected) = parse_identity_selection(
                    raw.as_str(),
                    &request.identity_options,
                    &request.username,
                ) {
                    raw.clear();
                    return IdentitySelection::Selected(selected);
                }

                raw.clear();
                let _ = prompts.show_error("Invalid identity selection");
                if attempt == DEFAULT_IDENTITY_SELECTION_ATTEMPTS {
                    tracing::warn!(
                        action_id = %request.action_id,
                        default_identity = %request.username,
                        "Identity selection failed repeatedly; using default identity"
                    );
                    return IdentitySelection::Selected(request.username.clone());
                }
            }
            Ok(PromptResponse::Canceled) => {
                return IdentitySelection::Terminal(HelperOutcome::Canceled);
            }
            Ok(PromptResponse::TimedOut) => {
                return IdentitySelection::Terminal(HelperOutcome::Timeout);
            }
            Err(err) => {
                tracing::warn!(
                    action_id = %request.action_id,
                    error = %err,
                    default_identity = %request.username,
                    "Identity selection prompt failed; using default identity"
                );
                return IdentitySelection::Selected(request.username.clone());
            }
        }
    }

    IdentitySelection::Selected(request.username.clone())
}

fn select_retention_for_request<P: PromptProvider>(
    request: &ActiveRequest,
    prompts: &mut P,
) -> RetentionSelection {
    if request.retention_options.len() <= 1 {
        let fallback = request
            .retention_options
            .first()
            .copied()
            .unwrap_or(RetentionPolicy::OneShot);
        return RetentionSelection::Selected(fallback);
    }

    let prompt = render_retention_selection_prompt(request);
    for attempt in 1..=DEFAULT_RETENTION_SELECTION_ATTEMPTS {
        match prompts.prompt_plain(&prompt) {
            Ok(PromptResponse::Submitted(mut raw)) => {
                if let Some(selected) =
                    parse_retention_selection(raw.as_str(), &request.retention_options)
                {
                    raw.clear();
                    return RetentionSelection::Selected(selected);
                }

                raw.clear();
                let _ = prompts.show_error("Invalid retention selection");
                if attempt == DEFAULT_RETENTION_SELECTION_ATTEMPTS {
                    tracing::warn!(
                        action_id = %request.action_id,
                        default_retention = RetentionPolicy::OneShot.label(),
                        "Retention selection failed repeatedly; using default retention"
                    );
                    return RetentionSelection::Selected(RetentionPolicy::OneShot);
                }
            }
            Ok(PromptResponse::Canceled) => {
                return RetentionSelection::Terminal(HelperOutcome::Canceled);
            }
            Ok(PromptResponse::TimedOut) => {
                return RetentionSelection::Terminal(HelperOutcome::Timeout);
            }
            Err(err) => {
                tracing::warn!(
                    action_id = %request.action_id,
                    error = %err,
                    default_retention = RetentionPolicy::OneShot.label(),
                    "Retention selection prompt failed; using default retention"
                );
                return RetentionSelection::Selected(RetentionPolicy::OneShot);
            }
        }
    }

    RetentionSelection::Selected(RetentionPolicy::OneShot)
}

fn render_identity_selection_prompt(request: &ActiveRequest) -> String {
    let mut lines = vec![
        "Select authentication identity".to_string(),
        format!("Action: {}", request.action_id),
    ];
    for (index, option) in request.identity_options.iter().enumerate() {
        if option == &request.username {
            lines.push(format!("{}: {} (default)", index + 1, option));
        } else {
            lines.push(format!("{}: {}", index + 1, option));
        }
    }
    lines.push("Enter number or username (blank for default)".to_string());
    lines.join("\n")
}

fn parse_identity_selection(input: &str, options: &[String], default: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Some(default.to_string());
    }

    if let Ok(index) = trimmed.parse::<usize>() {
        if (1..=options.len()).contains(&index) {
            return options.get(index - 1).cloned();
        }
    }

    options
        .iter()
        .find(|option| option.eq_ignore_ascii_case(trimmed))
        .cloned()
}

fn render_retention_selection_prompt(request: &ActiveRequest) -> String {
    let mut lines = vec![
        "Select authorization retention".to_string(),
        format!("Action: {}", request.action_id),
    ];
    for (index, option) in request.retention_options.iter().enumerate() {
        if *option == RetentionPolicy::OneShot {
            lines.push(format!(
                "{}: {} (default)",
                index + 1,
                option.prompt_label()
            ));
        } else {
            lines.push(format!("{}: {}", index + 1, option.prompt_label()));
        }
    }
    lines.push("Enter number or label (blank for default)".to_string());
    lines.join("\n")
}

fn parse_retention_selection(input: &str, options: &[RetentionPolicy]) -> Option<RetentionPolicy> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Some(RetentionPolicy::OneShot);
    }

    if let Ok(index) = trimmed.parse::<usize>() {
        if (1..=options.len()).contains(&index) {
            return options.get(index - 1).copied();
        }
    }

    let normalized = trimmed.to_ascii_lowercase();
    if normalized == "default" || normalized == "one-shot" || normalized == "oneshot" {
        return Some(RetentionPolicy::OneShot);
    }
    if normalized == "session"
        || normalized == "keep-session"
        || normalized == "keep for session"
        || normalized == "session-only"
    {
        return options
            .iter()
            .copied()
            .find(|option| *option == RetentionPolicy::Session);
    }
    if normalized == "always"
        || normalized == "keep-always"
        || normalized == "keep always"
        || normalized == "persistent"
    {
        return options
            .iter()
            .copied()
            .find(|option| *option == RetentionPolicy::Always);
    }

    None
}

fn identity_options_from_subjects(identities: &[Subject]) -> Vec<String> {
    let mut options = Vec::new();
    let mut seen = HashSet::new();

    for (kind, details) in identities {
        if kind != "unix-user" {
            continue;
        }

        let Some(name) = identity_name(details) else {
            continue;
        };
        let trimmed = name.trim();
        if trimmed.is_empty() {
            continue;
        }
        let dedupe_key = trimmed.to_ascii_lowercase();
        if seen.insert(dedupe_key) {
            options.push(trimmed.to_string());
        }
    }

    options
}

fn retention_options_from_details(details: &Details) -> Vec<RetentionPolicy> {
    let mut options = vec![RetentionPolicy::OneShot];

    let raw = first_detail_value(
        details,
        &[
            "polkit.retains_authorization_after_challenge",
            "retains_authorization_after_challenge",
            "polkit.retention",
            "retention",
        ],
    );
    let Some(raw) = raw else {
        return options;
    };
    let normalized = raw.trim().to_ascii_lowercase();

    if normalized.is_empty()
        || normalized == "0"
        || normalized == "false"
        || normalized == "no"
        || normalized == "never"
        || normalized == "none"
    {
        return options;
    }

    if normalized == "always"
        || normalized == "persistent"
        || normalized == "keep-always"
        || normalized == "2"
    {
        options.push(RetentionPolicy::Session);
        options.push(RetentionPolicy::Always);
        return options;
    }

    options.push(RetentionPolicy::Session);
    options
}

fn first_detail_value(details: &Details, keys: &[&str]) -> Option<String> {
    for key in keys {
        let Some(value) = details.get(*key) else {
            continue;
        };
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    None
}

fn detail_key_label(key: &str) -> &'static str {
    match key {
        "program" => "Program",
        "polkit.exec.path" => "Executable",
        "polkit.exec.argv1" => "Executable Arg",
        "command_line" => "Command",
        "unit" => "Unit",
        "verb" => "Verb",
        "polkit.message" => "Policy Message",
        "polkit.gettext_domain" => "Text Domain",
        "polkit.retains_authorization_after_challenge" => "Retains authorization",
        _ => "Detail",
    }
}

fn resolve_identity_username(identities: &[Subject]) -> Option<String> {
    let current = current_username();
    if let Some(current_name) = current.as_deref() {
        for (kind, details) in identities {
            if kind != "unix-user" {
                continue;
            }

            if let Some(name) = identity_name(details) {
                if name == current_name {
                    return Some(name);
                }
            }
        }
    }

    for (kind, details) in identities {
        if kind != "unix-user" {
            continue;
        }

        if let Some(name) = identity_name(details) {
            return Some(name);
        }
    }

    None
}

fn identity_name(details: &HashMap<String, OwnedValue>) -> Option<String> {
    if let Some(value) = details.get("name") {
        if let Ok(name) = <&str>::try_from(value) {
            return Some(name.to_string());
        }
    }

    if let Some(uid) = details.get("uid").and_then(parse_uid) {
        return username_for_uid(uid);
    }

    None
}

fn parse_uid(value: &OwnedValue) -> Option<u32> {
    if let Ok(uid) = u32::try_from(value) {
        return Some(uid);
    }
    if let Ok(uid) = u64::try_from(value) {
        return u32::try_from(uid).ok();
    }
    if let Ok(uid) = i64::try_from(value) {
        return u32::try_from(uid).ok();
    }
    if let Ok(uid) = i32::try_from(value) {
        return u32::try_from(uid).ok();
    }
    if let Ok(uid) = <&str>::try_from(value) {
        return uid.trim().parse::<u32>().ok();
    }

    None
}

fn username_for_uid(uid: u32) -> Option<String> {
    nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(uid))
        .ok()
        .flatten()
        .map(|user| user.name)
}

fn current_username() -> Option<String> {
    username_for_uid(nix::unistd::geteuid().as_raw())
}

fn summarize_identities(identities: &[Subject]) -> String {
    let mut summary = Vec::new();

    for (kind, details) in identities {
        let name = identity_name(details).unwrap_or_else(|| "unknown".to_string());
        let uid = details
            .get("uid")
            .and_then(parse_uid)
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string());
        summary.push(format!("{}(name={},uid={})", kind, name, uid));
    }

    if summary.is_empty() {
        "<none>".to_string()
    } else {
        summary.join(",")
    }
}

#[derive(Debug, Clone)]
struct PolkitAuthAgentObject {
    runtime: Arc<PolkitRuntime>,
}

impl PolkitAuthAgentObject {
    fn new(runtime: Arc<PolkitRuntime>) -> Self {
        Self { runtime }
    }
}

#[zbus::interface(name = "org.freedesktop.PolicyKit1.AuthenticationAgent")]
impl PolkitAuthAgentObject {
    fn begin_authentication(
        &self,
        action_id: &str,
        message: &str,
        icon_name: &str,
        details: Details,
        cookie: &str,
        identities: Vec<Subject>,
    ) -> fdo::Result<()> {
        let detail_count = details.len();
        let identity_count = identities.len();
        let identity_summary = summarize_identities(&identities);
        let request = AuthRequest {
            action_id: action_id.to_string(),
            message: message.to_string(),
            icon_name: icon_name.to_string(),
            details,
            cookie: cookie.to_string(),
            identities,
        };

        tracing::debug!(
            action_id = %action_id,
            cookie_len = request.cookie.len(),
            identities = ?request.identities,
            "Received raw polkit auth callback payload"
        );

        let queue_insert = self
            .runtime
            .begin_authentication(request)
            .map_err(|err| fdo::Error::Failed(err.to_string()))?;

        match queue_insert {
            QueueInsert::Activated => {
                tracing::info!(
                    action_id = %action_id,
                    icon_name = %icon_name,
                    detail_count,
                    identity_count,
                    identity_summary = %identity_summary,
                    "Started active polkit auth request"
                );
            }
            QueueInsert::Queued { position } => {
                tracing::info!(
                    action_id = %action_id,
                    icon_name = %icon_name,
                    detail_count,
                    identity_count,
                    queue_position = position,
                    identity_summary = %identity_summary,
                    "Queued polkit auth request"
                );
            }
        }

        if self.runtime.worker_enabled {
            let outcome = self
                .runtime
                .wait_for_completion(cookie)
                .map_err(|err| fdo::Error::Failed(err.to_string()))?;
            tracing::debug!(
                action_id = %action_id,
                outcome = ?outcome,
                "Completed polkit auth request callback"
            );
        }

        Ok(())
    }

    fn cancel_authentication(&self, cookie: &str) -> fdo::Result<()> {
        let canceled = self
            .runtime
            .cancel_authentication(cookie)
            .map_err(|err| fdo::Error::Failed(err.to_string()))?;

        if canceled {
            tracing::info!("Canceled polkit auth request");
        } else {
            tracing::debug!("Ignoring cancel for unknown polkit auth request");
        }

        Ok(())
    }
}

/// Real Polkit backend scaffold based on org.freedesktop.PolicyKit1 APIs.
///
/// This registers/unregisters an authentication agent with the authority and
/// exports the `AuthenticationAgent` object for request/cancel callbacks.
pub struct PolkitAgent {
    object_path: OwnedObjectPath,
    locale: String,
    subject: Subject,
    connection: Option<Connection>,
    registered: bool,
    runtime: Arc<PolkitRuntime>,
}

impl PolkitAgent {
    pub fn new(config: PolkitBackendConfig, auth_state: Arc<AuthState>) -> Result<Self> {
        let object_path = OwnedObjectPath::try_from(config.object_path)
            .map_err(|err| anyhow::anyhow!("invalid polkit object path: {}", err))?;

        Ok(Self {
            object_path,
            locale: config.locale,
            subject: build_subject(),
            connection: None,
            registered: false,
            runtime: Arc::new(PolkitRuntime::new(auth_state)),
        })
    }

    fn proxy(connection: &Connection) -> Result<Proxy<'_>> {
        let proxy = Proxy::new(
            connection,
            "org.freedesktop.PolicyKit1",
            "/org/freedesktop/PolicyKit1/Authority",
            "org.freedesktop.PolicyKit1.Authority",
        )?;
        Ok(proxy)
    }

    fn peer_proxy(connection: &Connection) -> Result<Proxy<'_>> {
        let proxy = Proxy::new(
            connection,
            "org.freedesktop.PolicyKit1",
            "/org/freedesktop/PolicyKit1/Authority",
            "org.freedesktop.DBus.Peer",
        )?;
        Ok(proxy)
    }

    fn ping_authority(connection: &Connection) -> Result<()> {
        let _: () = Self::peer_proxy(connection)?.call("Ping", &())?;
        Ok(())
    }

    fn register_with_authority(
        connection: &Connection,
        subject: &Subject,
        locale: &str,
        object_path: &str,
    ) -> Result<()> {
        let proxy = Self::proxy(connection)?;
        let _: () = proxy.call(
            "RegisterAuthenticationAgent",
            &(subject, locale, object_path),
        )?;
        Ok(())
    }
}

impl AuthAgentBackend for PolkitAgent {
    fn name(&self) -> &'static str {
        "polkit"
    }

    fn register(&mut self) -> Result<()> {
        if self.registered {
            return Ok(());
        }

        let connection = Connection::system()?;
        let exported = connection
            .object_server()
            .at(
                self.object_path.clone(),
                PolkitAuthAgentObject::new(Arc::clone(&self.runtime)),
            )
            .context("Failed to export polkit auth agent object")?;
        if !exported {
            anyhow::bail!(
                "polkit auth agent object already exists at {}",
                self.object_path
            );
        }

        let register_result = Self::register_with_authority(
            &connection,
            &self.subject,
            self.locale.as_str(),
            self.object_path.as_str(),
        );

        if let Err(err) = register_result {
            let _ = connection
                .object_server()
                .remove::<PolkitAuthAgentObject, _>(self.object_path.clone());
            return Err(err);
        }

        self.connection = Some(connection);
        self.registered = true;
        tracing::info!(
            backend = self.name(),
            "Registered polkit authentication agent"
        );
        Ok(())
    }

    fn unregister(&mut self) -> Result<()> {
        if !self.registered {
            return Ok(());
        }

        if let Some(connection) = &self.connection {
            match Self::proxy(connection) {
                Ok(proxy) => match proxy.call::<_, _, ()>(
                    "UnregisterAuthenticationAgent",
                    &(&self.subject, self.object_path.to_string()),
                ) {
                    Ok(()) => {
                        tracing::info!(
                            backend = self.name(),
                            "Unregistered polkit authentication agent"
                        );
                    }
                    Err(err) => {
                        tracing::warn!(
                            error = %err,
                            "Failed to unregister polkit authentication agent"
                        );
                    }
                },
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        "Failed to create polkit authority proxy during unregister"
                    );
                }
            }

            if let Err(err) = connection
                .object_server()
                .remove::<PolkitAuthAgentObject, _>(self.object_path.clone())
            {
                tracing::warn!(error = %err, "Failed to remove polkit auth agent object");
            }
        }

        self.runtime.reset();
        self.registered = false;
        self.connection = None;
        Ok(())
    }

    fn has_active_auth(&self) -> bool {
        self.runtime.has_active_request()
    }

    fn maintain(&mut self) -> Result<()> {
        if !self.registered {
            return self.register();
        }

        let is_healthy = self
            .connection
            .as_ref()
            .map(|connection| Self::ping_authority(connection).is_ok())
            .unwrap_or(false);
        if is_healthy {
            return Ok(());
        }

        tracing::warn!(
            backend = self.name(),
            "Polkit backend health check failed; attempting reconnect"
        );
        let _ = self.unregister();
        self.register()
    }
}

fn build_subject() -> Subject {
    subject_from_session_id(current_session_id())
}

fn subject_to_polkit_subject(subject: &SubjectResolution) -> Subject {
    if subject.kind == "unix-session" {
        return subject_from_session_id(subject.session_id.clone());
    }

    build_unix_process_subject()
}

fn subject_from_session_id(session_id: Option<String>) -> Subject {
    if let Some(session_id) = session_id {
        let mut details = HashMap::new();
        details.insert(
            "session-id".to_string(),
            OwnedValue::from(Str::from(session_id.as_str())),
        );
        tracing::info!(
            session_id = %session_id,
            "Registering polkit authentication agent for unix-session subject"
        );
        return ("unix-session".to_string(), details);
    }

    tracing::warn!(
        "Unable to resolve session id for polkit agent registration; falling back to unix-process subject"
    );
    build_unix_process_subject()
}

fn build_unix_process_subject() -> Subject {
    let mut details = HashMap::new();
    details.insert("pid".to_string(), OwnedValue::from(std::process::id()));
    details.insert(
        "uid".to_string(),
        OwnedValue::from(nix::unistd::geteuid().as_raw()),
    );
    if let Some(start_time) = process_start_time_ticks() {
        details.insert("start-time".to_string(), OwnedValue::from(start_time));
    } else {
        tracing::warn!("Unable to determine process start-time for polkit unix-process subject");
    }
    ("unix-process".to_string(), details)
}

fn current_session_id() -> Option<String> {
    session_id_from_env().or_else(session_id_from_proc)
}

fn session_id_from_env() -> Option<String> {
    std::env::var("XDG_SESSION_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn session_id_from_proc() -> Option<String> {
    let raw = std::fs::read_to_string("/proc/self/sessionid").ok()?;
    let session_id = raw.trim();
    if session_id.is_empty() || session_id == "0" {
        return None;
    }
    Some(session_id.to_string())
}

fn process_start_time_ticks() -> Option<u64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let (_head, tail) = stat.rsplit_once(") ")?;
    let mut fields = tail.split_whitespace();
    fields.nth(19)?.parse::<u64>().ok()
}

fn auth_max_attempts() -> usize {
    std::env::var("GARCARD_AUTH_MAX_ATTEMPTS")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_AUTH_MAX_ATTEMPTS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::polkit_helper::PromptResponse;
    use std::collections::VecDeque;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct SequencedPrompt {
        responses: VecDeque<PromptResponse>,
        success_count: usize,
        failure_messages: Vec<String>,
        error_messages: Vec<String>,
        prompt_count: usize,
    }

    impl SequencedPrompt {
        fn new(responses: Vec<PromptResponse>) -> Self {
            Self {
                responses: VecDeque::from(responses),
                success_count: 0,
                failure_messages: Vec::new(),
                error_messages: Vec::new(),
                prompt_count: 0,
            }
        }
    }

    impl PromptProvider for SequencedPrompt {
        fn prompt_secret(&mut self, _prompt: &str) -> Result<PromptResponse> {
            self.prompt_count += 1;
            Ok(self
                .responses
                .pop_front()
                .unwrap_or(PromptResponse::Canceled))
        }

        fn prompt_plain(&mut self, _prompt: &str) -> Result<PromptResponse> {
            self.prompt_secret(_prompt)
        }

        fn auth_succeeded(&mut self) -> Result<()> {
            self.success_count += 1;
            Ok(())
        }

        fn auth_failed(&mut self, message: &str) -> Result<()> {
            self.failure_messages.push(message.to_string());
            Ok(())
        }

        fn show_error(&mut self, message: &str) -> Result<()> {
            self.error_messages.push(message.to_string());
            Ok(())
        }
    }

    impl RetryPromptProvider for SequencedPrompt {}

    fn temp_socket_path() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "garcard-agent-test-{}-{}.sock",
            std::process::id(),
            nanos
        ))
    }

    fn fake_request(cookie: &str) -> AuthRequest {
        AuthRequest {
            action_id: "org.gardesk.test".to_string(),
            message: "Authenticate".to_string(),
            icon_name: "dialog-password".to_string(),
            details: HashMap::new(),
            cookie: cookie.to_string(),
            identities: Vec::new(),
        }
    }

    #[test]
    fn polkit_agent_rejects_invalid_object_path() {
        let result = PolkitAgent::new(
            PolkitBackendConfig {
                object_path: "invalid path".to_string(),
                locale: "C".to_string(),
            },
            Arc::new(AuthState::default()),
        );
        assert!(result.is_err());
    }

    #[test]
    fn subject_from_session_id_prefers_unix_session_kind() {
        let subject = subject_from_session_id(Some("1".to_string()));
        assert_eq!(subject.0.as_str(), "unix-session");
        let session_id = subject
            .1
            .get("session-id")
            .and_then(|value| <&str>::try_from(value).ok());
        assert_eq!(session_id, Some("1"));
    }

    #[test]
    fn subject_from_session_id_falls_back_to_unix_process_kind() {
        let subject = subject_from_session_id(None);
        assert_eq!(subject.0.as_str(), "unix-process");
        assert!(subject.1.contains_key("pid"));
        assert!(subject.1.contains_key("uid"));
        assert!(subject.1.contains_key("start-time"));
    }

    #[test]
    fn subject_to_polkit_subject_uses_session_resolution() {
        let resolution = SubjectResolution {
            kind: "unix-session".to_string(),
            session_id: Some("42".to_string()),
            pid: None,
            uid: None,
            start_time_ticks: None,
            has_start_time: false,
        };

        let subject = subject_to_polkit_subject(&resolution);
        assert_eq!(subject.0.as_str(), "unix-session");
        let session_id = subject
            .1
            .get("session-id")
            .and_then(|value| <&str>::try_from(value).ok());
        assert_eq!(session_id, Some("42"));
    }

    #[test]
    fn subject_to_polkit_subject_falls_back_to_unix_process_resolution() {
        let resolution = SubjectResolution {
            kind: "unix-process".to_string(),
            session_id: None,
            pid: Some(std::process::id()),
            uid: Some(nix::unistd::geteuid().as_raw()),
            start_time_ticks: process_start_time_ticks(),
            has_start_time: true,
        };

        let subject = subject_to_polkit_subject(&resolution);
        assert_eq!(subject.0.as_str(), "unix-process");
        assert!(subject.1.contains_key("pid"));
        assert!(subject.1.contains_key("uid"));
    }

    #[test]
    fn invalid_object_path_error_message_mentions_path() {
        let err = match PolkitAgent::new(
            PolkitBackendConfig {
                object_path: "invalid path".to_string(),
                locale: "C".to_string(),
            },
            Arc::new(AuthState::default()),
        ) {
            Ok(_) => panic!("invalid object path should fail"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("invalid polkit object path"));
    }

    #[test]
    fn runtime_begin_authentication_updates_state_and_counts() {
        let auth_state = Arc::new(AuthState::default());
        let runtime = Arc::new(PolkitRuntime::new_without_worker(Arc::clone(&auth_state)));

        let insert = runtime
            .begin_authentication(fake_request("cookie-1"))
            .expect("begin");
        assert_eq!(insert, QueueInsert::Activated);
        assert_eq!(auth_state.summary().state, "pending_prompt");
        assert_eq!(auth_state.summary().active_requests, 1);
        assert_eq!(auth_state.summary().queued_requests, 0);
    }

    #[test]
    fn runtime_cancel_authentication_drops_active_request() {
        let auth_state = Arc::new(AuthState::default());
        let runtime = Arc::new(PolkitRuntime::new_without_worker(Arc::clone(&auth_state)));
        runtime
            .begin_authentication(fake_request("cookie-1"))
            .expect("begin");

        let canceled = runtime.cancel_authentication("cookie-1").expect("cancel");
        assert!(canceled);
        assert_eq!(auth_state.summary().state, "canceled");
        assert_eq!(auth_state.summary().active_requests, 0);
        assert_eq!(auth_state.summary().queued_requests, 0);
    }

    #[test]
    fn runtime_cancel_authentication_removes_queued_request() {
        let auth_state = Arc::new(AuthState::default());
        let runtime = Arc::new(PolkitRuntime::new_without_worker(Arc::clone(&auth_state)));
        runtime
            .begin_authentication(fake_request("cookie-1"))
            .expect("begin first");
        runtime
            .begin_authentication(fake_request("cookie-2"))
            .expect("begin second");

        let canceled = runtime.cancel_authentication("cookie-2").expect("cancel");
        assert!(canceled);
        assert_eq!(auth_state.summary().active_requests, 1);
        assert_eq!(auth_state.summary().queued_requests, 0);
    }

    #[test]
    fn runtime_wait_for_completion_returns_recorded_outcome() {
        let auth_state = Arc::new(AuthState::default());
        let runtime = Arc::new(PolkitRuntime::new_without_worker(Arc::clone(&auth_state)));
        runtime
            .begin_authentication(fake_request("cookie-1"))
            .expect("begin");

        runtime.complete_request("cookie-1", HelperOutcome::Authorized);

        let outcome = runtime
            .wait_for_completion("cookie-1")
            .expect("wait for completion");
        assert_eq!(outcome, HelperOutcome::Authorized);
    }

    #[test]
    fn runtime_cancel_records_canceled_outcome() {
        let auth_state = Arc::new(AuthState::default());
        let runtime = Arc::new(PolkitRuntime::new_without_worker(Arc::clone(&auth_state)));
        runtime
            .begin_authentication(fake_request("cookie-1"))
            .expect("begin");

        let canceled = runtime.cancel_authentication("cookie-1").expect("cancel");
        assert!(canceled);

        let outcome = runtime
            .wait_for_completion("cookie-1")
            .expect("wait for completion");
        assert_eq!(outcome, HelperOutcome::Canceled);
    }

    #[test]
    fn runtime_wait_for_completion_clears_canceled_marker() {
        let auth_state = Arc::new(AuthState::default());
        let runtime = Arc::new(PolkitRuntime::new_without_worker(Arc::clone(&auth_state)));
        runtime.mark_canceled("cookie-1");
        runtime.record_outcome("cookie-1", HelperOutcome::Canceled);
        assert!(runtime.is_canceled("cookie-1"));

        let outcome = runtime
            .wait_for_completion("cookie-1")
            .expect("wait for completion");
        assert_eq!(outcome, HelperOutcome::Canceled);
        assert!(!runtime.is_canceled("cookie-1"));
    }

    #[test]
    fn runtime_complete_request_promotes_next_request_and_records_outcome() {
        let auth_state = Arc::new(AuthState::default());
        let runtime = Arc::new(PolkitRuntime::new_without_worker(Arc::clone(&auth_state)));
        runtime
            .begin_authentication(fake_request("cookie-1"))
            .expect("begin first");
        runtime
            .begin_authentication(fake_request("cookie-2"))
            .expect("begin second");

        runtime.complete_request("cookie-1", HelperOutcome::Denied);

        let outcome = runtime
            .wait_for_completion("cookie-1")
            .expect("wait for completion");
        assert_eq!(outcome, HelperOutcome::Denied);
        assert_eq!(auth_state.summary().state, "pending_prompt");
        assert_eq!(auth_state.summary().active_requests, 1);
        assert_eq!(auth_state.summary().queued_requests, 0);
    }

    #[test]
    fn authenticate_active_request_retries_after_failure_then_succeeds() {
        let socket_path = temp_socket_path();
        let listener = UnixListener::bind(&socket_path).expect("bind test socket");

        let server = thread::spawn(move || {
            for expected_outcome in ["failure", "success"] {
                let (mut stream, _) = listener.accept().expect("accept");
                let read_stream = stream.try_clone().expect("clone");
                let mut reader = BufReader::new(read_stream);

                let mut first_line = String::new();
                reader.read_line(&mut first_line).expect("read first line");
                let first = first_line.trim().to_string();
                if first == "operator" {
                    let mut cookie = String::new();
                    reader.read_line(&mut cookie).expect("read cookie");
                }

                stream
                    .write_all(b"PAM_PROMPT_ECHO_OFF Password:\n")
                    .expect("write prompt");
                stream.flush().expect("flush prompt");

                let mut secret = String::new();
                reader.read_line(&mut secret).expect("read secret");
                assert_eq!(secret.trim(), "correct horse");

                if expected_outcome == "failure" {
                    stream.write_all(b"FAILURE\n").expect("write failure");
                } else {
                    stream.write_all(b"SUCCESS\n").expect("write success");
                }
                stream.flush().expect("flush result");
            }
        });

        let runtime = PolkitRuntime {
            auth_state: Arc::new(AuthState::default()),
            queue: Mutex::new(AuthQueue::default()),
            outcomes: Mutex::new(HashMap::new()),
            canceled_cookies: Mutex::new(HashSet::new()),
            outcome_signal: Condvar::new(),
            helper_client: HelperSocketClient::new(&socket_path),
            processing: AtomicBool::new(false),
            worker_enabled: false,
        };
        let active = ActiveRequest {
            action_id: "org.gardesk.test".to_string(),
            message: "Authenticate".to_string(),
            icon_name: "dialog-password".to_string(),
            detail_count: 0,
            details: HashMap::new(),
            cookie: "cookie-1".to_string(),
            username: "operator".to_string(),
            identity_options: vec!["operator".to_string()],
            retention_options: vec![RetentionPolicy::OneShot],
        };
        let mut prompts = SequencedPrompt::new(vec![
            PromptResponse::Submitted("correct horse".to_string()),
            PromptResponse::Submitted("correct horse".to_string()),
        ]);

        let outcome = runtime.authenticate_active_request_with_prompts(&active, &mut prompts);
        assert_eq!(outcome, HelperOutcome::Authorized);
        assert_eq!(prompts.prompt_count, 2);
        assert_eq!(prompts.success_count, 1);
        assert_eq!(prompts.failure_messages, vec!["Authentication failed"]);

        server.join().expect("server join");
        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn authenticate_active_request_returns_timeout_when_prompt_times_out() {
        let socket_path = temp_socket_path();
        let listener = UnixListener::bind(&socket_path).expect("bind test socket");

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let read_stream = stream.try_clone().expect("clone");
            let mut reader = BufReader::new(read_stream);

            let mut first_line = String::new();
            reader.read_line(&mut first_line).expect("read first line");
            if first_line.trim() == "operator" {
                let mut cookie = String::new();
                reader.read_line(&mut cookie).expect("read cookie");
            }

            stream
                .write_all(b"PAM_PROMPT_ECHO_OFF Password:\n")
                .expect("write prompt");
            stream.flush().expect("flush prompt");
        });

        let runtime = PolkitRuntime {
            auth_state: Arc::new(AuthState::default()),
            queue: Mutex::new(AuthQueue::default()),
            outcomes: Mutex::new(HashMap::new()),
            canceled_cookies: Mutex::new(HashSet::new()),
            outcome_signal: Condvar::new(),
            helper_client: HelperSocketClient::new(&socket_path),
            processing: AtomicBool::new(false),
            worker_enabled: false,
        };
        let active = ActiveRequest {
            action_id: "org.gardesk.test".to_string(),
            message: "Authenticate".to_string(),
            icon_name: "dialog-password".to_string(),
            detail_count: 0,
            details: HashMap::new(),
            cookie: "cookie-timeout".to_string(),
            username: "operator".to_string(),
            identity_options: vec!["operator".to_string()],
            retention_options: vec![RetentionPolicy::OneShot],
        };
        let mut prompts = SequencedPrompt::new(vec![PromptResponse::TimedOut]);

        let outcome = runtime.authenticate_active_request_with_prompts(&active, &mut prompts);
        assert_eq!(outcome, HelperOutcome::Timeout);
        assert_eq!(prompts.prompt_count, 1);
        assert_eq!(prompts.success_count, 0);
        assert!(prompts.failure_messages.is_empty());

        server.join().expect("server join");
        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn authenticate_active_request_terminal_outcome_matrix() {
        struct Case {
            label: &'static str,
            responses: Vec<PromptResponse>,
            server_results: Vec<Option<&'static str>>,
            expected_outcome: HelperOutcome,
            expected_success_count: usize,
            expected_failure_count: usize,
        }

        let cases = vec![
            Case {
                label: "success",
                responses: vec![PromptResponse::Submitted("correct horse".to_string())],
                server_results: vec![Some("SUCCESS")],
                expected_outcome: HelperOutcome::Authorized,
                expected_success_count: 1,
                expected_failure_count: 0,
            },
            Case {
                label: "failure",
                responses: vec![
                    PromptResponse::Submitted("wrong horse".to_string()),
                    PromptResponse::Submitted("wrong horse".to_string()),
                    PromptResponse::Submitted("wrong horse".to_string()),
                ],
                server_results: vec![Some("FAILURE"), Some("FAILURE"), Some("FAILURE")],
                expected_outcome: HelperOutcome::Denied,
                expected_success_count: 0,
                expected_failure_count: 3,
            },
            Case {
                label: "canceled",
                responses: vec![PromptResponse::Canceled],
                server_results: vec![None],
                expected_outcome: HelperOutcome::Canceled,
                expected_success_count: 0,
                expected_failure_count: 0,
            },
            Case {
                label: "timeout",
                responses: vec![PromptResponse::TimedOut],
                server_results: vec![None],
                expected_outcome: HelperOutcome::Timeout,
                expected_success_count: 0,
                expected_failure_count: 0,
            },
        ];

        for case in cases {
            let socket_path = temp_socket_path();
            let listener = UnixListener::bind(&socket_path).expect("bind test socket");
            let server_results = case.server_results.clone();

            let server = thread::spawn(move || {
                for expected_result in server_results {
                    let (mut stream, _) = listener.accept().expect("accept");
                    let read_stream = stream.try_clone().expect("clone");
                    let mut reader = BufReader::new(read_stream);

                    let mut first_line = String::new();
                    reader.read_line(&mut first_line).expect("read first line");
                    if first_line.trim() == "operator" {
                        let mut cookie = String::new();
                        reader.read_line(&mut cookie).expect("read cookie");
                    }

                    stream
                        .write_all(b"PAM_PROMPT_ECHO_OFF Password:\n")
                        .expect("write prompt");
                    stream.flush().expect("flush prompt");

                    let mut secret = String::new();
                    let read = reader.read_line(&mut secret).expect("read secret");
                    if let Some(result) = expected_result {
                        assert!(
                            read > 0,
                            "expected submitted secret response for {}",
                            result
                        );
                        stream
                            .write_all(format!("{}\n", result).as_bytes())
                            .expect("write result");
                        stream.flush().expect("flush result");
                    }
                }
            });

            let runtime = PolkitRuntime {
                auth_state: Arc::new(AuthState::default()),
                queue: Mutex::new(AuthQueue::default()),
                outcomes: Mutex::new(HashMap::new()),
                canceled_cookies: Mutex::new(HashSet::new()),
                outcome_signal: Condvar::new(),
                helper_client: HelperSocketClient::new(&socket_path),
                processing: AtomicBool::new(false),
                worker_enabled: false,
            };
            let active = ActiveRequest {
                action_id: "org.gardesk.test".to_string(),
                message: "Authenticate".to_string(),
                icon_name: "dialog-password".to_string(),
                detail_count: 0,
                details: HashMap::new(),
                cookie: format!("cookie-{}", case.label),
                username: "operator".to_string(),
                identity_options: vec!["operator".to_string()],
                retention_options: vec![RetentionPolicy::OneShot],
            };
            let mut prompts = SequencedPrompt::new(case.responses);

            let outcome = runtime.authenticate_active_request_with_prompts(&active, &mut prompts);
            assert_eq!(outcome, case.expected_outcome, "case {}", case.label);
            assert_eq!(
                prompts.success_count, case.expected_success_count,
                "case {}",
                case.label
            );
            assert_eq!(
                prompts.failure_messages.len(),
                case.expected_failure_count,
                "case {}",
                case.label
            );

            server.join().expect("server join");
            let _ = std::fs::remove_file(&socket_path);
        }
    }

    #[test]
    fn render_prompt_context_includes_policy_details() {
        let mut details = HashMap::new();
        details.insert("vendor".to_string(), "Gardesk".to_string());
        details.insert("application_name".to_string(), "Meson".to_string());
        details.insert("program".to_string(), "/usr/bin/meson".to_string());
        details.insert(
            "polkit.exec.path".to_string(),
            "/usr/bin/pkexec".to_string(),
        );
        details.insert("command_line".to_string(), "meson install".to_string());
        details.insert(
            "polkit.retains_authorization_after_challenge".to_string(),
            "1".to_string(),
        );
        let request = ActiveRequest {
            action_id: "com.mesonbuild.install.run".to_string(),
            message: "Authentication is required to install this project".to_string(),
            icon_name: "preferences-system".to_string(),
            detail_count: details.len(),
            details,
            cookie: "cookie-ctx".to_string(),
            username: "operator".to_string(),
            identity_options: vec!["operator".to_string()],
            retention_options: vec![RetentionPolicy::OneShot, RetentionPolicy::Session],
        };

        let context = render_prompt_context(&request);
        assert!(context.contains("Authentication is required to install this project"));
        assert!(context.contains("Action: com.mesonbuild.install.run"));
        assert!(context.contains("Icon: preferences-system"));
        assert!(context.contains("Vendor: Gardesk"));
        assert!(context.contains("Application: Meson"));
        assert!(context.contains("Retention options: one-shot, keep-session"));
        assert!(context.contains("Program: /usr/bin/meson"));
        assert!(context.contains("Executable: /usr/bin/pkexec"));
        assert!(context.contains("Command: meson install"));
        assert!(context.contains("Retains authorization: 1"));
    }

    #[test]
    fn parse_identity_selection_accepts_blank_index_and_name() {
        let options = vec!["operator".to_string(), "root".to_string()];
        assert_eq!(
            parse_identity_selection("", &options, "operator"),
            Some("operator".to_string())
        );
        assert_eq!(
            parse_identity_selection("2", &options, "operator"),
            Some("root".to_string())
        );
        assert_eq!(
            parse_identity_selection("ROOT", &options, "operator"),
            Some("root".to_string())
        );
        assert_eq!(parse_identity_selection("99", &options, "operator"), None);
    }

    #[test]
    fn retention_options_from_details_supports_session_and_always() {
        let mut session_details = HashMap::new();
        session_details.insert(
            "polkit.retains_authorization_after_challenge".to_string(),
            "1".to_string(),
        );
        assert_eq!(
            retention_options_from_details(&session_details),
            vec![RetentionPolicy::OneShot, RetentionPolicy::Session]
        );

        let mut always_details = HashMap::new();
        always_details.insert("polkit.retention".to_string(), "always".to_string());
        assert_eq!(
            retention_options_from_details(&always_details),
            vec![
                RetentionPolicy::OneShot,
                RetentionPolicy::Session,
                RetentionPolicy::Always
            ]
        );
    }

    #[test]
    fn parse_retention_selection_accepts_index_and_label() {
        let options = vec![
            RetentionPolicy::OneShot,
            RetentionPolicy::Session,
            RetentionPolicy::Always,
        ];
        assert_eq!(
            parse_retention_selection("", &options),
            Some(RetentionPolicy::OneShot)
        );
        assert_eq!(
            parse_retention_selection("2", &options),
            Some(RetentionPolicy::Session)
        );
        assert_eq!(
            parse_retention_selection("keep always", &options),
            Some(RetentionPolicy::Always)
        );
        assert_eq!(parse_retention_selection("unknown", &options), None);
    }

    #[test]
    fn helper_outcome_label_maps_outcomes() {
        assert_eq!(helper_outcome_label(HelperOutcome::Authorized), "success");
        assert_eq!(helper_outcome_label(HelperOutcome::Denied), "failure");
        assert_eq!(helper_outcome_label(HelperOutcome::Canceled), "canceled");
        assert_eq!(helper_outcome_label(HelperOutcome::Timeout), "timeout");
    }

    #[test]
    fn select_retention_for_request_uses_prompted_choice() {
        let request = ActiveRequest {
            action_id: "org.gardesk.test".to_string(),
            message: "Authenticate".to_string(),
            icon_name: "dialog-password".to_string(),
            detail_count: 0,
            details: HashMap::new(),
            cookie: "cookie-retention".to_string(),
            username: "operator".to_string(),
            identity_options: vec!["operator".to_string()],
            retention_options: vec![RetentionPolicy::OneShot, RetentionPolicy::Session],
        };
        let mut prompts = SequencedPrompt::new(vec![PromptResponse::Submitted("2".to_string())]);

        let selection = select_retention_for_request(&request, &mut prompts);
        assert!(matches!(
            selection,
            RetentionSelection::Selected(RetentionPolicy::Session)
        ));
    }

    #[test]
    fn finalize_auth_attempt_records_retention_in_auth_summary() {
        let auth_state = Arc::new(AuthState::default());
        let runtime = PolkitRuntime::new_without_worker(Arc::clone(&auth_state));
        let request = ActiveRequest {
            action_id: "org.gardesk.test".to_string(),
            message: "Authenticate".to_string(),
            icon_name: "dialog-password".to_string(),
            detail_count: 0,
            details: HashMap::new(),
            cookie: "cookie-finalize".to_string(),
            username: "operator".to_string(),
            identity_options: vec!["operator".to_string()],
            retention_options: vec![RetentionPolicy::OneShot],
        };

        let outcome = runtime.finalize_auth_attempt(
            &request,
            HelperOutcome::Denied,
            Some(RetentionPolicy::OneShot),
        );
        assert_eq!(outcome, HelperOutcome::Denied);

        let summary = auth_state.summary();
        assert_eq!(summary.last_action_id.as_deref(), Some("org.gardesk.test"));
        assert_eq!(summary.last_outcome.as_deref(), Some("failure"));
        assert_eq!(summary.last_retention_policy.as_deref(), Some("one-shot"));
        assert_eq!(summary.last_retention_enforced, Some(false));
    }

    #[test]
    fn select_identity_for_request_uses_prompted_choice() {
        let request = ActiveRequest {
            action_id: "org.gardesk.test".to_string(),
            message: "Authenticate".to_string(),
            icon_name: "dialog-password".to_string(),
            detail_count: 0,
            details: HashMap::new(),
            cookie: "cookie-identity".to_string(),
            username: "operator".to_string(),
            identity_options: vec!["operator".to_string(), "root".to_string()],
            retention_options: vec![RetentionPolicy::OneShot],
        };
        let mut prompts = SequencedPrompt::new(vec![PromptResponse::Submitted("2".to_string())]);

        let selection = select_identity_for_request(&request, &mut prompts);
        assert!(matches!(
            selection,
            IdentitySelection::Selected(username) if username == "root"
        ));
        assert_eq!(prompts.prompt_count, 1);
    }

    #[test]
    fn select_identity_for_request_returns_canceled_outcome() {
        let request = ActiveRequest {
            action_id: "org.gardesk.test".to_string(),
            message: "Authenticate".to_string(),
            icon_name: "dialog-password".to_string(),
            detail_count: 0,
            details: HashMap::new(),
            cookie: "cookie-identity-cancel".to_string(),
            username: "operator".to_string(),
            identity_options: vec!["operator".to_string(), "root".to_string()],
            retention_options: vec![RetentionPolicy::OneShot],
        };
        let mut prompts = SequencedPrompt::new(vec![PromptResponse::Canceled]);

        let selection = select_identity_for_request(&request, &mut prompts);
        assert!(matches!(
            selection,
            IdentitySelection::Terminal(HelperOutcome::Canceled)
        ));
    }

    #[test]
    fn cancellation_aware_prompt_short_circuits_prompt_and_feedback() {
        let runtime = PolkitRuntime::new_without_worker(Arc::new(AuthState::default()));
        runtime.mark_canceled("cookie-1");

        let mut prompts =
            SequencedPrompt::new(vec![PromptResponse::Submitted("correct horse".to_string())]);
        {
            let mut wrapped = CancellationAwarePrompt::new(&mut prompts, &runtime, "cookie-1");
            let response = wrapped
                .prompt_secret("Password:")
                .expect("prompt response should be canceled");
            assert_eq!(response, PromptResponse::Canceled);
            wrapped
                .auth_succeeded()
                .expect("suppressed success callback");
        }

        assert_eq!(prompts.prompt_count, 0);
        assert_eq!(prompts.success_count, 0);
    }

    #[test]
    fn resolve_identity_username_uses_uid_detail() {
        let mut details = HashMap::new();
        details.insert(
            "uid".to_string(),
            OwnedValue::from(nix::unistd::geteuid().as_raw()),
        );
        let identities = vec![("unix-user".to_string(), details)];

        let resolved = resolve_identity_username(&identities);
        assert_eq!(resolved, current_username());
    }

    #[test]
    fn resolve_identity_username_prefers_current_user_name() {
        let mut first = HashMap::new();
        first.insert("uid".to_string(), OwnedValue::from(0_u32));

        let mut second = HashMap::new();
        second.insert(
            "uid".to_string(),
            OwnedValue::from(nix::unistd::geteuid().as_raw()),
        );

        let identities = vec![
            ("unix-user".to_string(), first),
            ("unix-user".to_string(), second),
        ];

        let resolved = resolve_identity_username(&identities);
        assert_eq!(resolved, current_username());
    }
}
