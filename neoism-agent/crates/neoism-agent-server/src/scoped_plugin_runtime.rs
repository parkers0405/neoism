//! Candidate-first non-workspace plugin generations keyed by exact runtime scope.

use std::collections::BTreeMap;
use std::sync::Arc;

use neoism_agent_core::PluginConfig;
use neoism_agent_plugin_api::{
    CapabilityGrants, InstalledPlugins, PluginFactory, PluginHost, PluginRuntimeError,
    PluginScope, RegistrySnapshot, RuntimeScope,
};
use tokio::sync::Mutex;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ScopedRuntimeKey {
    pub(crate) scope: PluginScope,
    pub(crate) workspace_id: Option<String>,
    pub(crate) scope_id: Option<String>,
}

impl ScopedRuntimeKey {
    pub(crate) fn from_runtime(scope: &RuntimeScope) -> Self {
        match scope {
            RuntimeScope::Global => Self {
                scope: PluginScope::Global,
                workspace_id: None,
                scope_id: None,
            },
            RuntimeScope::User { user_id } => Self {
                scope: PluginScope::User,
                workspace_id: None,
                scope_id: Some(user_id.clone()),
            },
            RuntimeScope::Workspace(workspace) => Self {
                scope: PluginScope::Workspace,
                workspace_id: Some(workspace.id.clone()),
                scope_id: None,
            },
            RuntimeScope::Session {
                workspace,
                session_id,
            } => Self {
                scope: PluginScope::Session,
                workspace_id: Some(workspace.id.clone()),
                scope_id: Some(session_id.clone()),
            },
        }
    }
}

struct ScopedGeneration {
    host: Arc<PluginHost>,
    active: Arc<InstalledPlugins>,
}

/// Owns one independently revocable, last-known-good generation per exact
/// global/user/workspace/session key. The candidate is fully installed before
/// publication; a failed candidate never displaces the active generation.
#[derive(Default)]
pub(crate) struct ScopedPluginRuntimeRegistry {
    generations: Mutex<BTreeMap<ScopedRuntimeKey, ScopedGeneration>>,
}

impl ScopedPluginRuntimeRegistry {
    pub(crate) async fn activate_packages(
        &self,
        directory: &str,
        runtime: RuntimeScope,
        configured: &BTreeMap<String, PluginConfig>,
        grants: CapabilityGrants,
        executables: Arc<dyn neoism_agent_service_api::ExecutableService>,
    ) -> Result<Arc<RegistrySnapshot>, String> {
        let key = ScopedRuntimeKey::from_runtime(&runtime);
        let mut factories = Vec::<Box<dyn PluginFactory>>::new();
        for package in crate::plugin_package::discover(directory) {
            let package = package?;
            let Some(agent) = package.manifest.agent.as_ref() else {
                continue;
            };
            if agent.scope != key.scope {
                continue;
            }
            let Some(config) = configured.get(&package.manifest.id) else {
                continue;
            };
            if !config.enabled {
                continue;
            }
            crate::plugin_package::authorized_for(
                &package,
                key.scope,
                key.workspace_id.as_deref(),
                key.scope_id.as_deref(),
                &config.options,
            )?;
            let spec = crate::plugin_host_process::package_plugin_spec(&package, config)?;
            factories.push(Box::new(
                crate::plugin_host_process::ServePluginFactory::new(
                    spec,
                    Arc::clone(&executables),
                ),
            ));
        }
        self.activate_factories(runtime, factories, grants).await
    }

    pub(crate) async fn activate_factories(
        &self,
        runtime: RuntimeScope,
        factories: Vec<Box<dyn PluginFactory>>,
        grants: CapabilityGrants,
    ) -> Result<Arc<RegistrySnapshot>, String> {
        let key = ScopedRuntimeKey::from_runtime(&runtime);
        let mut generations = self.generations.lock().await;
        let host = generations
            .get(&key)
            .map(|generation| Arc::clone(&generation.host))
            .unwrap_or_else(|| Arc::new(PluginHost::default()));
        let candidate = Arc::new(
            host.install(
                factories,
                &[],
                neoism_agent_plugin_api::PluginContext::new(runtime, grants),
            )
            .await
            .map_err(|failure| failure.error().to_string())?,
        );
        let snapshot = candidate.snapshot();
        let previous = generations.insert(
            key,
            ScopedGeneration {
                host,
                active: candidate,
            },
        );
        drop(generations);
        if let Some(previous) = previous {
            previous.active.shutdown().await.map_err(|error| {
                format!("candidate published but prior scope cleanup failed: {error}")
            })?;
        }
        Ok(snapshot)
    }

    pub(crate) async fn snapshot(
        &self,
        runtime: &RuntimeScope,
    ) -> Option<Arc<RegistrySnapshot>> {
        self.generations
            .lock()
            .await
            .get(&ScopedRuntimeKey::from_runtime(runtime))
            .map(|generation| generation.active.snapshot())
    }

    pub(crate) async fn snapshots_for_workspace(
        &self,
        workspace_id: &str,
    ) -> Vec<Arc<RegistrySnapshot>> {
        self.generations
            .lock()
            .await
            .iter()
            .filter(|(key, _)| {
                matches!(key.scope, PluginScope::Global | PluginScope::User)
                    || key.workspace_id.as_deref() == Some(workspace_id)
            })
            .map(|(_, generation)| generation.active.snapshot())
            .collect()
    }

    pub(crate) async fn ambient_snapshots(&self) -> Vec<Arc<RegistrySnapshot>> {
        let generations = self.generations.lock().await;
        [PluginScope::User, PluginScope::Global]
            .into_iter()
            .flat_map(|scope| {
                generations
                    .iter()
                    .filter(move |(key, _)| key.scope == scope)
                    .map(|(_, generation)| generation.active.snapshot())
            })
            .collect()
    }

    pub(crate) async fn deactivate(
        &self,
        runtime: &RuntimeScope,
    ) -> Result<(), PluginRuntimeError> {
        let generation = self
            .generations
            .lock()
            .await
            .remove(&ScopedRuntimeKey::from_runtime(runtime));
        if let Some(generation) = generation {
            generation.active.shutdown().await?;
        }
        Ok(())
    }

    pub(crate) async fn close(&self) -> Result<(), PluginRuntimeError> {
        let generations = std::mem::take(&mut *self.generations.lock().await);
        let mut errors = Vec::new();
        for (key, generation) in generations {
            if let Err(error) = generation.active.shutdown().await {
                errors.push(format!("{:?}: {error}", key));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(PluginRuntimeError::new(errors.join("; ")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_agent_plugin_api::{
        PluginContributions, PluginDescriptor, PluginFuture, PluginInstance,
        PluginManifest, StaticPluginInstance, WorkspaceIdentity, PLUGIN_API_MAJOR,
    };

    struct Factory {
        id: String,
        scope: PluginScope,
        fail: bool,
    }
    impl PluginFactory for Factory {
        fn descriptor(&self) -> PluginDescriptor {
            PluginDescriptor {
                manifest: PluginManifest {
                    id: self.id.clone(),
                    name: self.id.clone(),
                    version: "1".into(),
                    internal: true,
                    disableable: true,
                    capabilities: Vec::new(),
                    requires: Vec::new(),
                    event_namespaces: Vec::new(),
                    api_prefix: None,
                    config: BTreeMap::new(),
                },
                scope: self.scope,
                required_capabilities: Vec::new(),
                plugin_api_major: PLUGIN_API_MAJOR,
            }
        }
        fn create<'a>(
            &'a self,
            _context: neoism_agent_plugin_api::PluginContext,
        ) -> PluginFuture<'a, Box<dyn PluginInstance>> {
            Box::pin(async move {
                if self.fail {
                    return Err(PluginRuntimeError::new("candidate failed"));
                }
                Ok(
                    Box::new(StaticPluginInstance::new(PluginContributions::default()))
                        as Box<dyn PluginInstance>,
                )
            })
        }
    }

    #[tokio::test]
    async fn failed_session_update_retains_old_generation_and_exact_cleanup() {
        let registry = ScopedPluginRuntimeRegistry::default();
        let runtime = RuntimeScope::Session {
            workspace: WorkspaceIdentity {
                id: "workspace".into(),
                root: ".".into(),
            },
            session_id: "session-a".into(),
        };
        let first = registry
            .activate_factories(
                runtime.clone(),
                vec![Box::new(Factory {
                    id: "dev.example.scoped".into(),
                    scope: PluginScope::Session,
                    fail: false,
                })],
                CapabilityGrants::default(),
            )
            .await
            .unwrap();
        assert!(registry
            .activate_factories(
                runtime.clone(),
                vec![Box::new(Factory {
                    id: "dev.example.scoped".into(),
                    scope: PluginScope::Session,
                    fail: true
                })],
                CapabilityGrants::default()
            )
            .await
            .is_err());
        assert_eq!(
            registry.snapshot(&runtime).await.unwrap().generation,
            first.generation
        );
        let other = RuntimeScope::Session {
            workspace: WorkspaceIdentity {
                id: "workspace".into(),
                root: ".".into(),
            },
            session_id: "session-b".into(),
        };
        registry
            .activate_factories(
                other.clone(),
                vec![Box::new(Factory {
                    id: "dev.example.other".into(),
                    scope: PluginScope::Session,
                    fail: false,
                })],
                CapabilityGrants::default(),
            )
            .await
            .unwrap();
        registry.deactivate(&runtime).await.unwrap();
        assert!(registry.snapshot(&runtime).await.is_none());
        assert!(registry.snapshot(&other).await.is_some());
    }
}
