use std::collections::BTreeMap;
use std::sync::Arc;

use neoism_agent_plugin_api::{PluginContributions, PluginDefinition, PluginHostError, PluginManifest};

pub const ID: &str = "dev.neoism.interactions";

pub trait InteractionsHost: Send + Sync + 'static {
    fn register_tools(&self, registrar: &mut PluginContributions);
}

pub struct InteractionsPlugin {
    host: Arc<dyn InteractionsHost>,
}

impl InteractionsPlugin {
    pub fn new(host: Arc<dyn InteractionsHost>) -> Self {
        Self { host }
    }
}

impl PluginDefinition for InteractionsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: ID.into(),
            name: "Interaction tools".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            internal: true,
            disableable: true,
            capabilities: vec!["neoism.interactions.tools".into()],
            requires: Vec::new(),
            event_namespaces: vec!["interaction".into()],
            api_prefix: Some("/v2/interactions".into()),
            config: BTreeMap::new(),
        }
    }

    fn required_capabilities(&self) -> Vec<neoism_agent_plugin_api::HostCapability> { vec![neoism_agent_plugin_api::HostCapability::EventPublish] }
    fn contributions(&self, registrar: &mut PluginContributions) -> Result<(), PluginHostError> {
        self.host.register_tools(registrar);
        Ok(())
    }
}