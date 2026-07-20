use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use neoism_agent_core::LspConfig;
use serde_json::Value;

use super::super::lsp_languages::LANGUAGE_SPECS;
use super::{config::apply_config, AdapterOrigin, LanguageAdapter};

const ADAPTER_CACHE_TTL: Duration = Duration::from_millis(250);
const MAX_CACHED_ROOTS: usize = 64;

#[derive(Clone)]
struct CachedAdapters {
    loaded_at: Instant,
    adapters: Vec<LanguageAdapter>,
}

static ADAPTER_CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedAdapters>>> = OnceLock::new();
pub(in crate::lsp) fn adapters_for_root(root: &Path) -> Vec<LanguageAdapter> {
    let cache = ADAPTER_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(cache) = cache.lock() {
        if let Some(cached) = cache.get(root) {
            if cached.loaded_at.elapsed() < ADAPTER_CACHE_TTL {
                return cached.adapters.clone();
            }
        }
    }
    let adapters = resolve_adapters_uncached(root);
    if let Ok(mut cache) = cache.lock() {
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
                adapters: adapters.clone(),
            },
        );
    }
    adapters
}

#[cfg(test)]
pub(in crate::lsp) fn invalidate_adapter_cache(root: &Path) {
    if let Some(cache) = ADAPTER_CACHE.get() {
        if let Ok(mut cache) = cache.lock() {
            cache.remove(root);
        }
    }
}

fn resolve_adapters_uncached(root: &Path) -> Vec<LanguageAdapter> {
    let mut adapters = LANGUAGE_SPECS
        .iter()
        .map(LanguageAdapter::from_builtin)
        .collect::<Vec<_>>();
    let Ok(loaded) = crate::config::load(&root.to_string_lossy()) else {
        return adapters;
    };
    let servers = match loaded.info.lsp {
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
            .or_else(|| object.get("adapterId"))
            .and_then(Value::as_str);
        let disabled = object.get("enabled").and_then(Value::as_bool) == Some(false)
            || object.get("disabled").and_then(Value::as_bool) == Some(true);
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
