use std::collections::BTreeMap;
use std::sync::Arc;

use neoism_agent_plugin_api::{AgentPlugin, PluginHostError, PluginManifest, PluginRegistrar};

pub const ID: &str = "dev.neoism.tools.workspace";

pub trait WorkspaceToolsHost: Send + Sync + 'static {
    fn register_tools(&self, registrar: &mut PluginRegistrar);
}

pub struct WorkspaceToolsPlugin(Arc<dyn WorkspaceToolsHost>);
impl WorkspaceToolsPlugin { pub fn new(host: Arc<dyn WorkspaceToolsHost>) -> Self { Self(host) } }

impl AgentPlugin for WorkspaceToolsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest { id: ID.into(), name: "Workspace tools".into(), version: env!("CARGO_PKG_VERSION").into(), internal: true, disableable: true, capabilities: vec!["neoism.tools.workspace".into()], requires: Vec::new(), event_namespaces: vec!["tool".into()], api_prefix: None, config: BTreeMap::new() }
    }
    fn register(&self, registrar: &mut PluginRegistrar) -> Result<(), PluginHostError> { self.0.register_tools(registrar); Ok(()) }
}