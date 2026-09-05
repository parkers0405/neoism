use std::collections::BTreeMap;
use std::sync::Arc;

use neoism_agent_plugin_api::{
    PluginContributions, PluginDefinition, PluginHostError, PluginManifest,
};

pub const ID: &str = "dev.neoism.tools.workspace";

pub trait WorkspaceToolsHost: Send + Sync + 'static {
    fn register_tools(&self, registrar: &mut PluginContributions);
}

pub struct WorkspaceToolsPlugin(Arc<dyn WorkspaceToolsHost>);
impl WorkspaceToolsPlugin {
    pub fn new(host: Arc<dyn WorkspaceToolsHost>) -> Self {
        Self(host)
    }
}

impl PluginDefinition for WorkspaceToolsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: ID.into(),
            name: "Workspace tools".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            internal: true,
            disableable: true,
            capabilities: vec!["neoism.tools.workspace".into()],
            requires: Vec::new(),
            event_namespaces: vec!["tool".into()],
            api_prefix: None,
            config: BTreeMap::new(),
        }
    }
    fn required_capabilities(&self) -> Vec<neoism_agent_plugin_api::HostCapability> {
        use neoism_agent_plugin_api::HostCapability::*;
        vec![
            WorkspaceRead,
            WorkspaceWrite,
            ProcessSpawn,
            EventPublish,
            Network,
        ]
    }
    fn contributions(
        &self,
        registrar: &mut PluginContributions,
    ) -> Result<(), PluginHostError> {
        self.0.register_tools(registrar);
        Ok(())
    }
}
