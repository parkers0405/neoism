use std::collections::BTreeMap;
use std::sync::Arc;

use neoism_agent_plugin_api::{
    AgentPlugin, PluginHostError, PluginManifest, PluginRegistrar, ProviderService,
};

pub const ID: &str = "dev.neoism.providers";

pub struct ProvidersPlugin {
    providers: Vec<(String, Arc<dyn ProviderService>)>,
}

impl ProvidersPlugin {
    pub fn new(providers: Vec<(String, Arc<dyn ProviderService>)>) -> Self {
        Self { providers }
    }
}

impl AgentPlugin for ProvidersPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: ID.into(),
            name: "Providers".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            internal: true,
            disableable: true,
            capabilities: vec!["neoism.providers".into()],
            requires: vec![super::config::ID.into()],
            event_namespaces: vec!["provider".into()],
            api_prefix: Some("/v2/providers".into()),
            config: BTreeMap::new(),
        }
    }

    fn register(&self, registrar: &mut PluginRegistrar) -> Result<(), PluginHostError> {
        for (id, provider) in &self.providers {
            registrar.provider_service_runtime(id.clone(), provider.clone());
        }
        Ok(())
    }
}