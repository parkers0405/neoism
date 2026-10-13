// Temporary connections share the saved-server lookup path, but never its disk state.
use neoism_backend::server_registry::ServerRegistry as PersistentRegistry;
pub use neoism_backend::server_registry::{
    normalize_server_address, HostedServerSpec, SavedServer, ServerWorkspaceSubscription,
};
use std::{collections::HashMap, io, path::PathBuf};

struct TemporaryServer {
    server: SavedServer,
    token: Option<String>,
    hosted: bool,
}

pub struct ServerRegistry {
    saved: PersistentRegistry,
    temporary: Vec<TemporaryServer>,
    entries: Vec<SavedServer>,
    subscriptions: HashMap<(String, String), ServerWorkspaceSubscription>,
}

impl ServerRegistry {
    pub fn load(directory: PathBuf) -> io::Result<Self> {
        let saved = PersistentRegistry::load(directory)?;
        let entries = saved.servers().to_vec();
        Ok(Self {
            saved,
            temporary: Vec::new(),
            entries,
            subscriptions: HashMap::new(),
        })
    }

    fn refresh_entries(&mut self) {
        // One visible identity per endpoint. Temporary overlays never modify
        // the saved record; removing them restores its original row/credential.
        self.entries = self
            .temporary
            .iter()
            .map(|entry| entry.server.clone())
            .collect();
        self.entries.extend(
            self.saved
                .servers()
                .iter()
                .filter(|saved| {
                    !self
                        .temporary
                        .iter()
                        .any(|entry| entry.server.endpoint == saved.endpoint)
                })
                .cloned(),
        );
    }

    pub fn reload(&mut self) -> io::Result<()> {
        self.saved.reload()?;
        self.refresh_entries();
        Ok(())
    }

    pub fn servers(&self) -> &[SavedServer] {
        &self.entries
    }

    pub fn server(&self, id: &str) -> Option<&SavedServer> {
        self.temporary
            .iter()
            .find(|entry| entry.server.id == id)
            .map(|entry| &entry.server)
            .or_else(|| self.saved.servers().iter().find(|entry| entry.id == id))
    }

    pub fn is_temporary(&self, id: &str) -> bool {
        self.temporary.iter().any(|entry| entry.server.id == id)
    }

    pub fn is_temporary_host(&self, id: &str) -> bool {
        self.temporary
            .iter()
            .any(|entry| entry.server.id == id && entry.hosted)
    }

    pub fn token(&self, id: &str) -> Option<&str> {
        if let Some(entry) = self.temporary.iter().find(|entry| entry.server.id == id) {
            entry.token.as_deref()
        } else {
            self.saved.token(id)
        }
    }

    /// Only call after a successful authenticated connection. Endpoint and ID
    /// must both match; saved/master and local host credentials are immutable.
    pub(super) fn refresh_temp_credential(
        &mut self,
        id: &str,
        endpoint: &str,
        credential: Option<&str>,
    ) -> bool {
        let Some(entry) = self.temporary.iter_mut().find(|entry| {
            entry.server.id == id && entry.server.endpoint == endpoint && !entry.hosted
        }) else {
            return false;
        };
        entry.token = credential.map(str::to_owned);
        true
    }

    /// Recover an identity for an ALREADY KNOWN parked transport. This only
    /// compares its credential; callers must dial with the transport's token,
    /// never fetch a saved token from this lookup. Active transports have their
    /// own explicit active_server_id and should not use endpoint inference.
    pub(super) fn known_transport_server_id(
        &self,
        endpoint: &str,
        credential: Option<&str>,
    ) -> Option<String> {
        self.temporary
            .iter()
            .find(|entry| {
                entry.server.endpoint == endpoint && entry.token.as_deref() == credential
            })
            .map(|entry| entry.server.id.clone())
            .or_else(|| {
                self.saved
                    .servers()
                    .iter()
                    .find(|entry| {
                        entry.endpoint == endpoint
                            && self.saved.token(&entry.id) == credential
                    })
                    .map(|entry| entry.id.clone())
            })
    }

    pub fn add_temporary(
        &mut self,
        address: &str,
        name: Option<&str>,
        token: Option<&str>,
        hosted: bool,
    ) -> Result<SavedServer, String> {
        let endpoint = normalize_server_address(address)?;
        if self
            .temporary
            .iter()
            .any(|entry| entry.server.endpoint == endpoint)
        {
            return Err("That server is already in the list".into());
        }
        let server = SavedServer {
            id: format!("fast-{}", uuid::Uuid::new_v4()),
            name: name
                .filter(|name| !name.trim().is_empty())
                .map(|name| name.trim().to_string())
                .unwrap_or_else(|| {
                    url::Url::parse(&endpoint)
                        .ok()
                        .and_then(|url| url.host_str().map(str::to_string))
                        .unwrap_or_else(|| "Fast Server".into())
                }),
            endpoint,
            agent_api: false,
            directory: String::new(),
            hosted: None,
        };
        self.temporary.push(TemporaryServer {
            server: server.clone(),
            token: token.map(str::to_string),
            hosted,
        });
        self.refresh_entries();
        Ok(server)
    }

    pub fn add(
        &mut self,
        address: &str,
        name: Option<&str>,
        token: Option<&str>,
    ) -> Result<SavedServer, String> {
        let result = self.saved.add(address, name, token);
        self.refresh_entries();
        result
    }

    pub fn add_hosted(
        &mut self,
        address: &str,
        name: Option<&str>,
        token: Option<&str>,
        spec: HostedServerSpec,
    ) -> Result<SavedServer, String> {
        let result = self.saved.add_hosted(address, name, token, spec);
        self.refresh_entries();
        result
    }

    pub fn update(
        &mut self,
        id: &str,
        address: &str,
        name: Option<&str>,
        token: Option<&str>,
    ) -> Result<SavedServer, String> {
        if self.is_temporary(id) {
            return Err("Temporary servers cannot be saved or edited".into());
        }
        let result = self.saved.update(id, address, name, token);
        self.refresh_entries();
        result
    }

    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        if self.is_temporary(id) {
            self.temporary.retain(|entry| entry.server.id != id);
            self.subscriptions
                .retain(|(_, server_id), _| server_id != id);
        } else {
            self.saved.remove(id).map_err(|error| error.to_string())?;
        }
        self.refresh_entries();
        Ok(())
    }

    pub fn hosted_spec(&self, id: &str) -> Option<HostedServerSpec> {
        self.saved.hosted_spec(id)
    }

    pub fn workspace_subscription(
        &self,
        profile: &str,
        server: &str,
    ) -> ServerWorkspaceSubscription {
        if self.is_temporary(server) {
            self.subscriptions
                .get(&(profile.into(), server.into()))
                .cloned()
                .unwrap_or_default()
        } else {
            self.saved.workspace_subscription(profile, server)
        }
    }

    pub fn set_workspace_subscription(
        &mut self,
        profile: &str,
        server: &str,
        subscription: ServerWorkspaceSubscription,
    ) -> Result<(), String> {
        if self.is_temporary(server) {
            self.subscriptions
                .insert((profile.into(), server.into()), subscription);
            Ok(())
        } else {
            self.saved
                .set_workspace_subscription(profile, server, subscription)
        }
    }

    pub fn prune_workspace_subscription<'a>(
        &mut self,
        profile: &str,
        server: &str,
        live: impl IntoIterator<Item = &'a str>,
    ) -> Result<ServerWorkspaceSubscription, String> {
        if !self.is_temporary(server) {
            return self
                .saved
                .prune_workspace_subscription(profile, server, live);
        }
        let live: std::collections::HashSet<_> = live.into_iter().collect();
        let mut subscription = self.workspace_subscription(profile, server);
        subscription
            .subscribed_workspace_ids
            .retain(|id| live.contains(id.as_str()));
        if subscription
            .last_active_workspace_id
            .as_ref()
            .is_some_and(|id| !live.contains(id.as_str()))
        {
            subscription.last_active_workspace_id = None;
        }
        self.set_workspace_subscription(profile, server, subscription.clone())?;
        Ok(subscription)
    }

    pub fn remove_workspace_subscription(
        &mut self,
        profile: &str,
        server: &str,
        workspace: &str,
    ) -> Result<(), String> {
        if !self.is_temporary(server) {
            return self
                .saved
                .remove_workspace_subscription(profile, server, workspace);
        }
        let mut subscription = self.workspace_subscription(profile, server);
        subscription
            .subscribed_workspace_ids
            .retain(|id| id != workspace);
        if subscription.last_active_workspace_id.as_deref() == Some(workspace) {
            subscription.last_active_workspace_id =
                subscription.subscribed_workspace_ids.last().cloned();
        }
        self.set_workspace_subscription(profile, server, subscription)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporary_overlay_restores_saved_identity_without_changing_secrets() {
        let directory = tempfile::tempdir().unwrap();
        let mut registry = ServerRegistry::load(directory.path().into()).unwrap();
        let saved = registry
            .add("https://team.example", Some("Saved"), Some("master-secret"))
            .unwrap();
        let guest = registry
            .add_temporary(
                &saved.endpoint,
                Some("Guest"),
                Some("wrong-password"),
                false,
            )
            .unwrap();
        assert_ne!(guest.id, saved.id);
        assert_eq!(registry.servers().len(), 1);
        assert_eq!(registry.servers()[0].id, guest.id);
        assert_eq!(registry.server(&saved.id).unwrap().name, "Saved");
        assert_eq!(registry.token(&saved.id), Some("master-secret"));
        assert!(registry
            .add_temporary(&saved.endpoint, None, None, false)
            .is_err());
        registry.reload().unwrap();
        assert_eq!(registry.servers()[0].id, guest.id);
        // Failed dials don't invoke the success hook or overwrite old state.
        assert_eq!(registry.token(&guest.id), Some("wrong-password"));
        assert!(!registry.refresh_temp_credential(
            &saved.id,
            &saved.endpoint,
            Some("guest-secret")
        ));
        assert!(!registry.refresh_temp_credential(
            &guest.id,
            "wss://other.example/session",
            Some("guest-secret")
        ));
        assert!(registry.refresh_temp_credential(
            &guest.id,
            &guest.endpoint,
            Some("guest-secret")
        ));
        assert_eq!(registry.token(&guest.id), Some("guest-secret"));
        assert_eq!(
            registry.known_transport_server_id(&saved.endpoint, Some("master-secret")),
            Some(saved.id.clone())
        );
        assert_eq!(
            registry.known_transport_server_id(&guest.endpoint, Some("guest-secret")),
            Some(guest.id.clone())
        );
        assert!(registry
            .known_transport_server_id(&guest.endpoint, Some("unknown"))
            .is_none());
        let disk = ServerRegistry::load(directory.path().into()).unwrap();
        assert_eq!(disk.servers().len(), 1);
        assert_eq!(disk.servers()[0].id, saved.id);
        assert_eq!(disk.token(&saved.id), Some("master-secret"));
        assert!(disk.server(&guest.id).is_none());
        registry.remove(&guest.id).unwrap();
        assert_eq!(registry.servers()[0].id, saved.id);
        assert_eq!(registry.token(&saved.id), Some("master-secret"));
    }

    #[test]
    fn success_refresh_cannot_replace_host_credential() {
        let directory = tempfile::tempdir().unwrap();
        let mut registry = ServerRegistry::load(directory.path().into()).unwrap();
        let host = registry
            .add_temporary("localhost:23456", None, Some("host-secret"), true)
            .unwrap();
        assert!(!registry.refresh_temp_credential(
            &host.id,
            &host.endpoint,
            Some("guest-secret")
        ));
        assert_eq!(registry.token(&host.id), Some("host-secret"));
    }

    #[test]
    fn temporary_credentials_and_subscriptions_never_persist() {
        let directory = std::env::temp_dir()
            .join(format!("neoism-fast-registry-{}", uuid::Uuid::new_v4()));
        let mut registry = ServerRegistry::load(directory.clone()).unwrap();
        let saved = registry
            .add("localhost:12345", Some("Saved"), Some("saved-secret"))
            .unwrap();
        let fast = registry
            .add_temporary("localhost:23456", Some("Fast"), Some("fast-secret"), false)
            .unwrap();
        registry
            .set_workspace_subscription(
                "window-1",
                &fast.id,
                ServerWorkspaceSubscription {
                    subscribed_workspace_ids: vec!["workspace".into()],
                    last_active_workspace_id: Some("workspace".into()),
                },
            )
            .unwrap();
        registry.reload().unwrap();
        assert_eq!(registry.token(&fast.id), Some("fast-secret"));
        assert_eq!(
            registry
                .workspace_subscription("window-1", &fast.id)
                .subscribed_workspace_ids,
            vec!["workspace"]
        );
        let reloaded = ServerRegistry::load(directory.clone()).unwrap();
        assert_eq!(reloaded.servers().len(), 1);
        assert_eq!(reloaded.servers()[0].id, saved.id);
        assert!(reloaded.token(&fast.id).is_none());
        assert!(reloaded
            .workspace_subscription("window-1", &fast.id)
            .subscribed_workspace_ids
            .is_empty());
        registry.remove(&fast.id).unwrap();
        assert!(!registry.is_temporary(&fast.id));
        assert!(registry.token(&fast.id).is_none());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
