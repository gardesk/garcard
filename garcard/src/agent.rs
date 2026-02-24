use crate::polkit_helper::{DEFAULT_HELPER_SOCKET, HelperOutcome, HelperSocketClient};
use crate::prompt::CommandPrompt;
use crate::state::{AuthPhase, AuthQueue, AuthState, QueueInsert};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use zbus::blocking::{Connection, Proxy};
use zbus::fdo;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

/// Backend interface for Polkit agent integration.
pub trait AuthAgentBackend {
    fn name(&self) -> &'static str;
    fn register(&mut self) -> Result<()>;
    fn unregister(&mut self) -> Result<()>;
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
    cookie: String,
    username: String,
}

#[derive(Debug)]
struct PolkitRuntime {
    auth_state: Arc<AuthState>,
    queue: Mutex<AuthQueue<AuthRequest>>,
    helper_client: HelperSocketClient,
    processing: AtomicBool,
    worker_enabled: bool,
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
            helper_client: HelperSocketClient::new(helper_socket),
            processing: AtomicBool::new(false),
            worker_enabled,
        }
    }

    fn begin_authentication(self: &Arc<Self>, request: AuthRequest) -> Result<QueueInsert> {
        let (insert, active, queued) = {
            let mut queue = self
                .queue
                .lock()
                .map_err(|_| anyhow::anyhow!("auth request queue lock poisoned"))?;
            let insert = queue.push(request);
            let (active, queued) = queue.counts();
            (insert, active, queued)
        };

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

        Some(ActiveRequest {
            action_id: request.action_id.clone(),
            message: request.message.clone(),
            icon_name: request.icon_name.clone(),
            detail_count: request.details.len(),
            cookie: request.cookie.clone(),
            username,
        })
    }

    fn authenticate_active_request(&self, request: &ActiveRequest) -> HelperOutcome {
        let prompt_context = if request.message.is_empty() {
            request.action_id.as_str()
        } else {
            request.message.as_str()
        };

        let mut prompts = CommandPrompt::default();
        tracing::info!(
            context = %prompt_context,
            "Starting helper authentication dialog"
        );

        match self
            .helper_client
            .authenticate(&request.username, &request.cookie, &mut prompts)
        {
            Ok(outcome) => outcome,
            Err(err) => {
                tracing::warn!(
                    action_id = %request.action_id,
                    error = %err,
                    "Polkit helper authentication failed"
                );
                HelperOutcome::Denied
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

    fn reset(&self) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.clear();
        }
        self.processing.store(false, Ordering::Release);
        self.auth_state.set_phase(AuthPhase::Idle);
        self.auth_state.sync_queue_counts(0, 0);
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
    if let Ok(uid) = i32::try_from(value) {
        return u32::try_from(uid).ok();
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
        let request = AuthRequest {
            action_id: action_id.to_string(),
            message: message.to_string(),
            icon_name: icon_name.to_string(),
            details,
            cookie: cookie.to_string(),
            identities,
        };

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
                    "Queued polkit auth request"
                );
            }
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
    if let Some(session_id) = current_session_id() {
        let mut details = HashMap::new();
        let value = zbus::zvariant::Value::from(session_id.as_str());
        if let Ok(session_value) = OwnedValue::try_from(value) {
            details.insert("session-id".to_string(), session_value);
            return ("unix-session".to_string(), details);
        }
    }

    let mut details = HashMap::new();
    details.insert("pid".to_string(), OwnedValue::from(std::process::id()));
    details.insert(
        "uid".to_string(),
        OwnedValue::from(nix::unistd::geteuid().as_raw()),
    );
    if let Some(start_time) = process_start_time_ticks() {
        details.insert("start-time".to_string(), OwnedValue::from(start_time));
    } else {
        tracing::warn!("Unable to determine process start-time for polkit subject");
    }
    ("unix-process".to_string(), details)
}

fn process_start_time_ticks() -> Option<u64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let (_head, tail) = stat.rsplit_once(") ")?;
    let mut fields = tail.split_whitespace();
    fields.nth(19)?.parse::<u64>().ok()
}

fn current_session_id() -> Option<String> {
    if let Ok(raw) = std::env::var("XDG_SESSION_ID") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    let raw = std::fs::read_to_string("/proc/self/sessionid").ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "4294967295" {
        return None;
    }

    Some(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn subject_uses_unix_process_kind() {
        let subject = build_subject();
        match subject.0.as_str() {
            "unix-session" => assert!(subject.1.contains_key("session-id")),
            "unix-process" => {
                assert!(subject.1.contains_key("pid"));
                assert!(subject.1.contains_key("uid"));
                assert!(subject.1.contains_key("start-time"));
            }
            other => panic!("unexpected subject kind: {other}"),
        }
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
