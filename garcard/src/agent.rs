use crate::state::{AuthPhase, AuthQueue, AuthState, QueueInsert};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use zbus::blocking::{Connection, Proxy};
use zbus::fdo;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

/// Backend interface for Polkit agent integration.
pub trait AuthAgentBackend {
    fn name(&self) -> &'static str;
    fn register(&mut self) -> Result<()>;
    fn unregister(&mut self) -> Result<()>;
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

#[derive(Debug)]
struct PolkitRuntime {
    auth_state: Arc<AuthState>,
    queue: Mutex<AuthQueue<AuthRequest>>,
}

impl PolkitRuntime {
    fn new(auth_state: Arc<AuthState>) -> Self {
        Self {
            auth_state,
            queue: Mutex::new(AuthQueue::default()),
        }
    }

    fn begin_authentication(&self, request: AuthRequest) -> Result<QueueInsert> {
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

        Ok(insert)
    }

    fn cancel_authentication(&self, cookie: &str) -> Result<bool> {
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

    fn reset(&self) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.clear();
        }
        self.auth_state.set_phase(AuthPhase::Idle);
        self.auth_state.sync_queue_counts(0, 0);
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
                tracing::info!(action_id = %action_id, "Started active polkit auth request");
            }
            QueueInsert::Queued { position } => {
                tracing::info!(
                    action_id = %action_id,
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

        let register_result = (|| -> Result<()> {
            let proxy = Self::proxy(&connection)?;
            let _: () = proxy.call(
                "RegisterAuthenticationAgent",
                &(
                    &self.subject,
                    self.locale.as_str(),
                    self.object_path.clone(),
                ),
            )?;
            Ok(())
        })();

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
            match Self::proxy(connection)?.call::<_, _, ()>(
                "UnregisterAuthenticationAgent",
                &(
                    &self.subject,
                    self.locale.as_str(),
                    self.object_path.clone(),
                ),
            ) {
                Ok(()) => {
                    tracing::info!(
                        backend = self.name(),
                        "Unregistered polkit authentication agent"
                    );
                }
                Err(err) => {
                    tracing::warn!(error = %err, "Failed to unregister polkit authentication agent");
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
}

fn build_subject() -> Subject {
    let mut details = HashMap::new();
    details.insert("pid".to_string(), OwnedValue::from(std::process::id()));
    details.insert(
        "uid".to_string(),
        OwnedValue::from(nix::unistd::geteuid().as_raw()),
    );
    ("unix-process".to_string(), details)
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
        assert_eq!(subject.0, "unix-process");
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
        let runtime = PolkitRuntime::new(Arc::clone(&auth_state));

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
        let runtime = PolkitRuntime::new(Arc::clone(&auth_state));
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
        let runtime = PolkitRuntime::new(Arc::clone(&auth_state));
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
}
