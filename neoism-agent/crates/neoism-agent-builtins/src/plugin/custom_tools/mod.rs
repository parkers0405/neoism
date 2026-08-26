use std::collections::BTreeMap;
use std::sync::Arc;

use neoism_agent_plugin_api::{PluginContributions, PluginDefinition, PluginHostError, PluginManifest};

pub const ID: &str = "dev.neoism.custom-tools";

pub trait CustomToolsHost: Send + Sync + 'static {
    fn register_tools(&self, registrar: &mut PluginContributions);
}

pub struct CustomToolsPlugin(Arc<dyn CustomToolsHost>);
impl CustomToolsPlugin { pub fn new(host: Arc<dyn CustomToolsHost>) -> Self { Self(host) } }

impl PluginDefinition for CustomToolsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest { id: ID.into(), name: "Workspace custom tools".into(), version: env!("CARGO_PKG_VERSION").into(), internal: true, disableable: true, capabilities: Vec::new(), requires: vec![super::workspace_tools::ID.into()], event_namespaces: Vec::new(), api_prefix: None, config: BTreeMap::new() }
    }
    fn required_capabilities(&self) -> Vec<neoism_agent_plugin_api::HostCapability> { use neoism_agent_plugin_api::HostCapability::*; vec![WorkspaceRead, WorkspaceWrite, ProcessSpawn, Network] }
    fn contributions(&self, registrar: &mut PluginContributions) -> Result<(), PluginHostError> { self.0.register_tools(registrar); Ok(()) }
}