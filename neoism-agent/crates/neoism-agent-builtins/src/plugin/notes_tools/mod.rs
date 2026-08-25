use std::collections::BTreeMap;
use std::sync::Arc;

use neoism_agent_plugin_api::{AgentPlugin, PluginHostError, PluginManifest, PluginRegistrar};

pub const ID: &str = "dev.neoism.tools.notes";

pub trait NotesToolsHost: Send + Sync + 'static {
    fn register_tools(&self, registrar: &mut PluginRegistrar);
}

pub struct NotesToolsPlugin(Arc<dyn NotesToolsHost>);
impl NotesToolsPlugin { pub fn new(host: Arc<dyn NotesToolsHost>) -> Self { Self(host) } }

impl AgentPlugin for NotesToolsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest { id: ID.into(), name: "Notes tools".into(), version: env!("CARGO_PKG_VERSION").into(), internal: true, disableable: true, capabilities: vec!["neoism.tools.notes".into()], requires: Vec::new(), event_namespaces: vec!["notes".into()], api_prefix: None, config: BTreeMap::new() }
    }
    fn register(&self, registrar: &mut PluginRegistrar) -> Result<(), PluginHostError> { self.0.register_tools(registrar); Ok(()) }
}