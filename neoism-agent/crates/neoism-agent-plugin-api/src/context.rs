//! Capability-scoped contracts supplied by a plugin host.
//!
//! A context does not expose the host's application state. A plugin can only
//! reach a service for which the host installed a grant.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
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
    TaskSpawn,
    SecretUse,
    SecretRead,
    PromptRead,
    MessageRead,
    ResponseTransform,
    ProviderAccess,
    PolicyInvoke,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum PluginScope {
    Global,
    User,
    Workspace,
    Session,
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
    User {
        user_id: String,
    },
    Workspace(WorkspaceIdentity),
    Session {
        workspace: WorkspaceIdentity,
        session_id: String,
    },
}

impl RuntimeScope {
    pub fn kind(&self) -> PluginScope {
        match self {
            Self::Global => PluginScope::Global,
            Self::User { .. } => PluginScope::User,
            Self::Workspace(_) => PluginScope::Workspace,
            Self::Session { .. } => PluginScope::Session,
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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BrokerRequest {
    /// Host-defined opaque operation or resource name. It is never a path,
    /// bearer token, or provider credential.
    pub operation: String,
    #[serde(default)]
    pub input: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<BrokerOwner>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrokerOwner {
    pub plugin_id: String,
    pub instance_id: String,
    pub registry_generation: u64,
    pub scope: PluginScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BrokerResponse {
    #[serde(default)]
    pub output: Value,
}

pub trait CapabilityBroker: Send + Sync + 'static {
    fn call(
        &self,
        request: BrokerRequest,
        lease: CapabilityLease,
    ) -> Result<BrokerResponse, PluginRuntimeError>;
    fn cancel(
        &self,
        opaque_id: &str,
        owner: Option<BrokerOwner>,
        lease: CapabilityLease,
    ) -> Result<(), PluginRuntimeError> {
        let _ = (opaque_id, owner, lease);
        Ok(())
    }
}

#[derive(Clone)]
pub struct CapabilityGrants {
    capabilities: BTreeSet<HostCapability>,
    config: Option<Arc<dyn ConfigAccess>>,
    workspace: Option<Arc<dyn WorkspaceAccess>>,
    events: Option<Arc<dyn EventPublisher>>,
    brokers: BTreeMap<HostCapability, Arc<dyn CapabilityBroker>>,
    metadata: BTreeMap<String, Value>,
    lease: CapabilityLease,
}

#[derive(Clone, Debug)]
pub struct CapabilityLease(Arc<AtomicBool>);

impl Default for CapabilityLease {
    fn default() -> Self {
        Self(Arc::new(AtomicBool::new(true)))
    }
}

impl CapabilityLease {
    pub fn is_active(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    pub fn revoke(&self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Default for CapabilityGrants {
    fn default() -> Self {
        Self {
            capabilities: BTreeSet::new(),
            config: None,
            workspace: None,
            events: None,
            brokers: BTreeMap::new(),
            metadata: BTreeMap::new(),
            lease: CapabilityLease::default(),
        }
    }
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

    pub fn broker(
        mut self,
        capability: HostCapability,
        broker: Arc<dyn CapabilityBroker>,
    ) -> Self {
        self.capabilities.insert(capability);
        self.brokers.insert(capability, broker);
        self
    }

    pub fn metadata(mut self, key: impl Into<String>, value: Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    fn restricted_to(&self, required: &[HostCapability]) -> Self {
        let capabilities = required
            .iter()
            .copied()
            .filter(|capability| self.capabilities.contains(capability))
            .collect();
        Self {
            capabilities,
            config: required
                .iter()
                .any(|capability| {
                    matches!(
                        capability,
                        HostCapability::ConfigRead | HostCapability::ConfigWrite
                    )
                })
                .then(|| self.config.clone())
                .flatten(),
            workspace: required
                .iter()
                .any(|capability| {
                    matches!(
                        capability,
                        HostCapability::WorkspaceRead | HostCapability::WorkspaceWrite
                    )
                })
                .then(|| self.workspace.clone())
                .flatten(),
            events: required
                .contains(&HostCapability::EventPublish)
                .then(|| self.events.clone())
                .flatten(),
            brokers: self
                .brokers
                .iter()
                .filter(|(capability, _)| required.contains(capability))
                .map(|(capability, broker)| (*capability, Arc::clone(broker)))
                .collect(),
            metadata: self.metadata.clone(),
            lease: CapabilityLease::default(),
        }
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
            RuntimeScope::Session { workspace, .. } => Some(workspace),
            RuntimeScope::Global | RuntimeScope::User { .. } => None,
        }
    }
    pub fn capabilities(&self) -> &BTreeSet<HostCapability> {
        &self.grants.capabilities
    }
    pub fn has(&self, capability: HostCapability) -> bool {
        self.grants.lease.is_active() && self.grants.capabilities.contains(&capability)
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
        self.grants
            .lease
            .is_active()
            .then(|| self.grants.events.as_deref())
            .flatten()
    }
    pub fn broker(&self, capability: HostCapability) -> Option<GrantedBroker<'_>> {
        self.grants
            .brokers
            .get(&capability)
            .map(|broker| GrantedBroker {
                context: self,
                capability,
                broker: broker.as_ref(),
            })
    }
    pub fn metadata(&self, key: &str) -> Option<&Value> {
        self.grants.metadata.get(key)
    }

    pub fn restricted_to(&self, required: &[HostCapability]) -> Self {
        Self {
            scope: self.scope.clone(),
            grants: self.grants.restricted_to(required),
        }
    }

    pub fn capability_lease(&self) -> CapabilityLease {
        self.grants.lease.clone()
    }

    pub fn capabilities_active(&self) -> bool {
        self.grants.lease.is_active()
    }

    pub fn revoke_capabilities(&self) {
        self.grants.lease.revoke();
    }

    #[doc(hidden)]
    pub fn with_host_metadata(mut self, key: impl Into<String>, value: Value) -> Self {
        self.grants.metadata.insert(key.into(), value);
        self
    }
}

pub struct GrantedBroker<'a> {
    context: &'a PluginContext,
    capability: HostCapability,
    broker: &'a dyn CapabilityBroker,
}

impl GrantedBroker<'_> {
    pub fn call(
        &self,
        request: BrokerRequest,
    ) -> Result<BrokerResponse, PluginRuntimeError> {
        self.context
            .require(self.capability)
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
        self.broker.call(request, self.context.capability_lease())
    }

    pub fn cancel(
        &self,
        opaque_id: &str,
        owner: Option<BrokerOwner>,
    ) -> Result<(), PluginRuntimeError> {
        self.context
            .require(self.capability)
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
        self.broker
            .cancel(opaque_id, owner, self.context.capability_lease())
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
            RuntimeScope::Workspace(WorkspaceIdentity {
                id: "test".into(),
                root: ".".into(),
            }),
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

    #[test]
    fn revocation_is_shared_by_generation_clones_but_not_other_generations() {
        let base = PluginContext::new(
            RuntimeScope::Workspace(WorkspaceIdentity {
                id: "test".into(),
                root: ".".into(),
            }),
            CapabilityGrants::default().allow(HostCapability::Network),
        );
        let generation = base.restricted_to(&[HostCapability::Network]);
        let callback_context = generation.clone();
        let next_generation = base.restricted_to(&[HostCapability::Network]);

        generation.revoke_capabilities();

        assert!(!callback_context.capabilities_active());
        assert!(callback_context.require(HostCapability::Network).is_err());
        assert!(next_generation.require(HostCapability::Network).is_ok());
    }
}
