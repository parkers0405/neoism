use std::collections::BTreeMap;
use std::sync::Arc;

use neoism_agent_plugin_api::{AgentPlugin, PluginHostError, PluginManifest, PluginRegistrar};

pub const ID: &str = "dev.neoism.custom-tools";

pub trait CustomToolsHost: Send + Sync + 'static {
    fn register_tools(&self, registrar: &mut PluginRegistrar);
}

pub struct CustomToolsPlugin(Arc<dyn CustomToolsHost>);
impl CustomToolsPlugin { pub fn new(host: Arc<dyn CustomToolsHost>) -> Self { Self(host) } }

impl AgentPlugin for CustomToolsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest { id: ID.into(), name: "Workspace custom tools".into(), version: env!("CARGO_PKG_VERSION").into(), internal: true, disableable: true, capabilities: Vec::new(), requires: vec![super::workspace_tools::ID.into()], event_namespaces: Vec::new(), api_prefix: None, config: BTreeMap::new() }
    }
    fn register(&self, registrar: &mut PluginRegistrar) -> Result<(), PluginHostError> { self.0.register_tools(registrar); Ok(()) }
}