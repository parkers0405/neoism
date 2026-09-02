use std::collections::BTreeMap;
use std::sync::Arc;

use neoism_agent_plugin_api::{
    PluginContributions, PluginDefinition, PluginHostError, PluginManifest,
};

pub const ID: &str = "dev.neoism.tools.documentation";

pub trait DocumentationToolsHost: Send + Sync + 'static {
    fn register_tools(&self, registrar: &mut PluginContributions);
}

pub struct DocumentationToolsPlugin(Arc<dyn DocumentationToolsHost>);

impl DocumentationToolsPlugin {
    pub fn new(host: Arc<dyn DocumentationToolsHost>) -> Self {
        Self(host)
    }
}

impl PluginDefinition for DocumentationToolsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: ID.into(),
            name: "Documentation tools".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            internal: true,
            disableable: true,
            capabilities: vec!["neoism.tools.documentation".into()],
            requires: Vec::new(),
            event_namespaces: vec!["documentation".into()],
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
