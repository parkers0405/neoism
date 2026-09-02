use std::collections::BTreeMap;
use std::sync::Arc;

use neoism_agent_plugin_api::{
    PluginContributions, PluginDefinition, PluginHostError, PluginManifest,
};

pub const ID: &str = "dev.neoism.tools.memory";

pub trait MemoryToolsHost: Send + Sync + 'static {
    fn register_tools(&self, registrar: &mut PluginContributions);
}

pub struct MemoryToolsPlugin(Arc<dyn MemoryToolsHost>);

impl MemoryToolsPlugin {
    pub fn new(host: Arc<dyn MemoryToolsHost>) -> Self {
        Self(host)
    }
}

impl PluginDefinition for MemoryToolsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: ID.into(),
            name: "Memory tools".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            internal: true,
            disableable: true,
            capabilities: vec!["neoism.tools.memory".into()],
            requires: Vec::new(),
            event_namespaces: vec!["memory".into()],
            api_prefix: None,
            config: BTreeMap::new(),
        }
    }

    fn contributions(
        &self,
        registrar: &mut PluginContributions,
    ) -> Result<(), PluginHostError> {
        self.0.register_tools(registrar);
        Ok(())
    }
}
