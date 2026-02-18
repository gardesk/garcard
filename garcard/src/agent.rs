use anyhow::Result;
use std::collections::HashMap;
use zbus::blocking::{Connection, Proxy};
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

/// Real Polkit backend scaffold based on org.freedesktop.PolicyKit1 APIs.
///
/// This registers/unregisters an authentication agent with the authority.
/// The request handling object is the next implementation phase.
pub struct PolkitAgent {
    object_path: OwnedObjectPath,
    locale: String,
    subject: Subject,
    connection: Option<Connection>,
    registered: bool,
}

impl PolkitAgent {
    pub fn new(config: PolkitBackendConfig) -> Result<Self> {
        let object_path = OwnedObjectPath::try_from(config.object_path)
            .map_err(|err| anyhow::anyhow!("invalid polkit object path: {}", err))?;

        Ok(Self {
            object_path,
            locale: config.locale,
            subject: build_subject(),
            connection: None,
            registered: false,
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
        {
            let proxy = Self::proxy(&connection)?;
            let _: () = proxy.call(
                "RegisterAuthenticationAgent",
                &(
                    &self.subject,
                    self.locale.as_str(),
                    self.object_path.clone(),
                ),
            )?;
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
        }

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

    #[test]
    fn polkit_agent_rejects_invalid_object_path() {
        let result = PolkitAgent::new(PolkitBackendConfig {
            object_path: "invalid path".to_string(),
            locale: "C".to_string(),
        });
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
        let err = match PolkitAgent::new(PolkitBackendConfig {
            object_path: "invalid path".to_string(),
            locale: "C".to_string(),
        }) {
            Ok(_) => panic!("invalid object path should fail"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("invalid polkit object path"));
    }
}
