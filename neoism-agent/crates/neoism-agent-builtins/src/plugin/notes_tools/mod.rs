use std::collections::BTreeMap;
use std::sync::Arc;

use neoism_agent_plugin_api::{PluginContributions, PluginDefinition, PluginHostError, PluginManifest};

pub const ID: &str = "dev.neoism.tools.notes";

pub trait NotesToolsHost: Send + Sync + 'static {
    fn register_tools(&self, registrar: &mut PluginContributions);
}

pub struct NotesToolsPlugin(Arc<dyn NotesToolsHost>);
impl NotesToolsPlugin { pub fn new(host: Arc<dyn NotesToolsHost>) -> Self { Self(host) } }

impl PluginDefinition for NotesToolsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest { id: ID.into(), name: "Notes tools".into(), version: env!("CARGO_PKG_VERSION").into(), internal: true, disableable: true, capabilities: vec!["neoism.tools.notes".into()], requires: Vec::new(), event_namespaces: vec!["notes".into()], api_prefix: None, config: BTreeMap::new() }
    }
    fn required_capabilities(&self) -> Vec<neoism_agent_plugin_api::HostCapability> { use neoism_agent_plugin_api::HostCapability::*; vec![WorkspaceRead, WorkspaceWrite] }
    fn contributions(&self, registrar: &mut PluginContributions) -> Result<(), PluginHostError> { self.0.register_tools(registrar); Ok(()) }
}