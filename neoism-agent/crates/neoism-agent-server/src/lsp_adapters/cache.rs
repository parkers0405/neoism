use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

use neoism_agent_core::LspConfig;
use serde_json::Value;

use super::{config::apply_config, AdapterOrigin, LanguageAdapter};

const ADAPTER_CACHE_TTL: Duration = Duration::from_millis(250);
const MAX_CACHED_ROOTS: usize = 64;

#[derive(Clone)]
struct CachedAdapters {
    loaded_at: Instant,
    snapshot_identity: String,
    capability_generation: u64,
    adapters: Vec<LanguageAdapter>,
}

#[derive(Default)]
pub(in crate::lsp) struct AdapterCache {
    entries: Mutex<HashMap<PathBuf, CachedAdapters>>,
}

pub(in crate::lsp) fn adapters_for_root_with(
    cache: &AdapterCache,
    services: &neoism_agent_service_api::AgentServices,
    root: &Path,
    generation_config: Option<&neoism_agent_core::AgentConfigDocument>,
) -> Vec<LanguageAdapter> {
    let capability_snapshot = services.language_capabilities.snapshot();
    let capability_generation = capability_snapshot.generation;
    let loaded_config = generation_config.cloned().map(Ok).unwrap_or_else(|| {
        neoism_agent_builtins::plugin::config::load(services, &root.to_string_lossy())
            .map(|(config, _)| config)
    });
    let configuration_valid = loaded_config.is_ok();
    let snapshot_identity = loaded_config
        .as_ref()
        .ok()
        .and_then(|config| serde_json::to_string(config).ok())
        .unwrap_or_default();
    if let Ok(cache) = cache.entries.lock() {
        if let Some(cached) = cache.get(root) {
            if !configuration_valid
                || (cached.loaded_at.elapsed() < ADAPTER_CACHE_TTL
                && cached.snapshot_identity == snapshot_identity
                && cached.capability_generation == capability_generation)
            {
                return cached.adapters.clone();
            }
        }
    }
    let adapters = resolve_adapters_uncached(
        services,
        root,
        &capability_snapshot,
        loaded_config.ok().as_ref(),
    );
    if let Ok(mut cache) = cache.entries.lock() {
        if cache.len() >= MAX_CACHED_ROOTS && !cache.contains_key(root) {
            if let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, cached)| cached.loaded_at)
                .map(|(root, _)| root.clone())
            {
                cache.remove(&oldest);
            }
        }
        cache.insert(
            root.to_path_buf(),
            CachedAdapters {
                loaded_at: Instant::now(),
                snapshot_identity,
                capability_generation,
                adapters: adapters.clone(),
            },
        );
    }
    adapters
}

#[cfg(test)]
pub(in crate::lsp) fn invalidate_adapter_cache(cache: &AdapterCache, root: &Path) {
    if let Ok(mut cache) = cache.entries.lock() {
        cache.remove(root);
    }
}

fn resolve_adapters_uncached(
    services: &neoism_agent_service_api::AgentServices,
    root: &Path,
    capability_snapshot: &neoism_agent_service_api::LanguageCapabilitySnapshot,
    generation_config: Option<&neoism_agent_core::AgentConfigDocument>,
) -> Vec<LanguageAdapter> {
    let mut adapters = capability_snapshot.languages
        .iter()
        .map(LanguageAdapter::from_capability)
        .collect::<Vec<_>>();
    let info = generation_config.cloned().or_else(|| {
        neoism_agent_builtins::plugin::config::load(services, &root.to_string_lossy())
            .ok()
            .map(|(config, _)| config)
    });
    let Some(info) = info else {
        return adapters;
    };
    let servers = match info.lsp {
        LspConfig::Enabled(false) => return Vec::new(),
        LspConfig::Enabled(true) => return adapters,
        LspConfig::Servers(servers) => servers,
    };

    for (id, value) in servers {
        if value.as_bool() == Some(false) {
            adapters.retain(|adapter| adapter.id != id);
            continue;
        }
        let Some(object) = value.as_object().cloned() else {
            adapters.push(invalid_adapter(
                &id,
                format!("LSP adapter `{id}` must be an object or false"),
            ));
            continue;
        };
        let referenced_adapter = object
            .get("adapter")
            .and_then(Value::as_str);
        let disabled = object.get("enabled").and_then(Value::as_bool) == Some(false);
        if disabled {
            adapters.retain(|adapter| {
                adapter.id != id && referenced_adapter != Some(adapter.id.as_str())
            });
            continue;
        }

        let base_index = adapters.iter().position(|adapter| {
            adapter.id == id || referenced_adapter == Some(adapter.id.as_str())
        });
        let mut adapter = base_index
            .map(|index| adapters[index].clone())
            .unwrap_or_else(|| LanguageAdapter::empty_configured(&id));
        if let Some(index) = base_index {
            adapters.remove(index);
        }
        adapter.id = id.clone();
        adapter.origin = AdapterOrigin::Configured;
        apply_config(&mut adapter, &object);
        adapters.push(adapter);
    }
    adapters
}

fn invalid_adapter(id: &str, error: String) -> LanguageAdapter {
    let mut adapter = LanguageAdapter::empty_configured(id);
    adapter.configuration_error = Some(error);
    adapter
}
