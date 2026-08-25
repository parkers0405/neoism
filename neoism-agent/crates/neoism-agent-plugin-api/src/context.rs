//! Capability-scoped contracts supplied by a plugin host.
//!
//! A context does not expose the host's application state. A plugin can only
//! reach a service for which the host installed a grant.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::PluginRuntimeError;

#[derive(
    Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "kebab-case")]
pub enum HostCapability {
    ConfigRead,
    ConfigWrite,
    WorkspaceRead,
    WorkspaceWrite,
    EventPublish,
    Network,
    ProcessSpawn,
    SecretRead,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PluginScope {
    Global,
    Workspace,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceIdentity {
    pub id: String,
    pub root: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeScope {
    Global,
    Workspace(WorkspaceIdentity),
}

impl RuntimeScope {
    pub fn kind(&self) -> PluginScope {
        match self {
            Self::Global => PluginScope::Global,
            Self::Workspace(_) => PluginScope::Workspace,
        }
    }
}

pub trait ConfigAccess: Send + Sync + 'static {
    fn get(&self, key: &str) -> Result<Option<Value>, PluginRuntimeError>;
    fn set(&self, key: &str, value: Value) -> Result<(), PluginRuntimeError>;
}

pub trait WorkspaceAccess: Send + Sync + 'static {
    fn read(&self, relative_path: &str) -> Result<Vec<u8>, PluginRuntimeError>;
    fn write(
        &self,
        relative_path: &str,
        contents: &[u8],
    ) -> Result<(), PluginRuntimeError>;
    fn list(&self, relative_path: &str) -> Result<Vec<String>, PluginRuntimeError>;
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginEvent {
    pub namespace: String,
    pub name: String,
    pub payload: Value,
}

pub trait EventPublisher: Send + Sync + 'static {
    fn publish(&self, event: PluginEvent) -> Result<(), PluginRuntimeError>;
}

#[derive(Clone, Default)]
pub struct CapabilityGrants {
    capabilities: BTreeSet<HostCapability>,
    config: Option<Arc<dyn ConfigAccess>>,
    workspace: Option<Arc<dyn WorkspaceAccess>>,
    events: Option<Arc<dyn EventPublisher>>,
    metadata: BTreeMap<String, Value>,
}

impl CapabilityGrants {
    pub fn allow(mut self, capability: HostCapability) -> Self {
        self.capabilities.insert(capability);
        self
    }

    pub fn config(mut self, access: Arc<dyn ConfigAccess>, writable: bool) -> Self {
        self.capabilities.insert(HostCapability::ConfigRead);
        if writable {
            self.capabilities.insert(HostCapability::ConfigWrite);
        }
        self.config = Some(access);
        self
    }

    pub fn workspace(mut self, access: Arc<dyn WorkspaceAccess>, writable: bool) -> Self {
        self.capabilities.insert(HostCapability::WorkspaceRead);
        if writable {
            self.capabilities.insert(HostCapability::WorkspaceWrite);
        }
        self.workspace = Some(access);
        self
    }

    pub fn events(mut self, publisher: Arc<dyn EventPublisher>) -> Self {
        self.capabilities.insert(HostCapability::EventPublish);
        self.events = Some(publisher);
        self
    }

    pub fn metadata(mut self, key: impl Into<String>, value: Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }
}

#[derive(Clone)]
pub struct PluginContext {
    scope: RuntimeScope,
    grants: CapabilityGrants,
}

impl PluginContext {
    pub fn new(scope: RuntimeScope, grants: CapabilityGrants) -> Self {
        Self { scope, grants }
    }

    pub fn scope(&self) -> &RuntimeScope {
        &self.scope
    }
    pub fn workspace(&self) -> Option<&WorkspaceIdentity> {
        match &self.scope {
            RuntimeScope::Workspace(workspace) => Some(workspace),
            _ => None,
        }
    }
    pub fn capabilities(&self) -> &BTreeSet<HostCapability> {
        &self.grants.capabilities
    }
    pub fn has(&self, capability: HostCapability) -> bool {
        self.grants.capabilities.contains(&capability)
    }
    pub fn require(&self, capability: HostCapability) -> Result<(), CapabilityError> {
        self.has(capability)
            .then_some(())
            .ok_or(CapabilityError::Denied(capability))
    }
    pub fn config(&self) -> Option<GrantedConfig<'_>> {
        self.grants.config.as_deref().map(|access| GrantedConfig {
            context: self,
            access,
        })
    }
    pub fn workspace_access(&self) -> Option<GrantedWorkspace<'_>> {
        self.grants
            .workspace
            .as_deref()
            .map(|access| GrantedWorkspace {
                context: self,
                access,
            })
    }
    pub fn events(&self) -> Option<&dyn EventPublisher> {
        self.grants.events.as_deref()
    }
    pub fn metadata(&self, key: &str) -> Option<&Value> {
        self.grants.metadata.get(key)
    }
}

pub struct GrantedConfig<'a> {
    context: &'a PluginContext,
    access: &'a dyn ConfigAccess,
}

impl GrantedConfig<'_> {
    pub fn get(&self, key: &str) -> Result<Option<Value>, PluginRuntimeError> {
        self.context
            .require(HostCapability::ConfigRead)
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
        self.access.get(key)
    }

    pub fn set(&self, key: &str, value: Value) -> Result<(), PluginRuntimeError> {
        self.context
            .require(HostCapability::ConfigWrite)
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
        self.access.set(key, value)
    }
}

pub struct GrantedWorkspace<'a> {
    context: &'a PluginContext,
    access: &'a dyn WorkspaceAccess,
}

impl GrantedWorkspace<'_> {
    pub fn read(&self, relative_path: &str) -> Result<Vec<u8>, PluginRuntimeError> {
        self.context
            .require(HostCapability::WorkspaceRead)
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
        self.access.read(relative_path)
    }

    pub fn write(
        &self,
        relative_path: &str,
        contents: &[u8],
    ) -> Result<(), PluginRuntimeError> {
        self.context
            .require(HostCapability::WorkspaceWrite)
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
        self.access.write(relative_path, contents)
    }

    pub fn list(&self, relative_path: &str) -> Result<Vec<String>, PluginRuntimeError> {
        self.context
            .require(HostCapability::WorkspaceRead)
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
        self.access.list(relative_path)
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CapabilityError {
    #[error("plugin was not granted host capability `{0:?}`")]
    Denied(HostCapability),
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct Config(AtomicUsize);

    impl ConfigAccess for Config {
        fn get(&self, _key: &str) -> Result<Option<Value>, PluginRuntimeError> {
            Ok(None)
        }

        fn set(&self, _key: &str, _value: Value) -> Result<(), PluginRuntimeError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn read_only_grants_cannot_reach_mutating_host_operations() {
        let access = Arc::new(Config(AtomicUsize::new(0)));
        let context = PluginContext::new(
            RuntimeScope::Global,
            CapabilityGrants::default().config(access.clone(), false),
        );

        let error = context
            .config()
            .unwrap()
            .set("key", Value::Null)
            .unwrap_err();
        assert!(error.to_string().contains("ConfigWrite"));
        assert_eq!(access.0.load(Ordering::SeqCst), 0);
    }
}
