use std::collections::BTreeMap;
use std::sync::Arc;

use neoism_agent_plugin_api::{AgentPlugin, PluginHostError, PluginManifest, PluginRegistrar};

pub const ID: &str = "dev.neoism.artifacts";

pub trait ArtifactsHost: Send + Sync + 'static {
    fn register_tools(&self, registrar: &mut PluginRegistrar);
}

pub struct ArtifactsPlugin {
    host: Arc<dyn ArtifactsHost>,
}

impl ArtifactsPlugin {
    pub fn new(host: Arc<dyn ArtifactsHost>) -> Self {
        Self { host }
    }
}

impl AgentPlugin for ArtifactsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: ID.into(),
            name: "Artifact tools".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            internal: true,
            disableable: true,
            capabilities: vec!["neoism.artifacts.tools".into()],
            requires: Vec::new(),
            event_namespaces: vec!["artifact".into()],
            api_prefix: Some("/v2/artifacts".into()),
            config: BTreeMap::new(),
        }
    }

    fn register(&self, registrar: &mut PluginRegistrar) -> Result<(), PluginHostError> {
        self.host.register_tools(registrar);
        Ok(())
    }
}