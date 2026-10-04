use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use thiserror::Error;

use crate::{PluginCapability, PluginManifest, PluginSpec, PluginTrigger, API_VERSION};

#[derive(Clone, Debug, PartialEq)]
pub struct DiscoveredManifest {
    pub root: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest: PluginManifest,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PluginDiscovery {
    pub specs: Vec<(PathBuf, PluginSpec)>,
    pub manifests: Vec<DiscoveredManifest>,
}

#[derive(Debug, Error)]
pub enum PluginEcosystemError {
    #[error("unable to read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid JSON manifest {path}: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[cfg(feature = "runtime")]
    #[error("invalid Lua plugin spec {path}: {source}")]
    LuaSpec { path: PathBuf, source: mlua::Error },
    #[error("invalid plugin manifest `{id}`: {message}")]
    InvalidManifest { id: String, message: String },
    #[error("plugin `{plugin}` depends on missing plugin `{dependency}`")]
    MissingDependency { plugin: String, dependency: String },
    #[error("plugin dependency cycle: {path}")]
    DependencyCycle { path: String },
    #[error("duplicate plugin id `{0}`")]
    DuplicatePlugin(String),
}

fn read_dir_sorted(path: &Path) -> Result<Vec<PathBuf>, PluginEcosystemError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut paths = std::fs::read_dir(path)
        .map_err(|source| PluginEcosystemError::Read {
            path: path.into(),
            source,
        })?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| PluginEcosystemError::Read {
            path: path.into(),
            source,
        })?;
    paths.sort();
    Ok(paths)
}

/// Discover local manifests in lexical plugin-id order. Directories without a
/// manifest are ignored; malformed manifests fail the complete transaction.
pub fn discover_local_manifests(
    config_dir: &Path,
) -> Result<Vec<DiscoveredManifest>, PluginEcosystemError> {
    let mut found = Vec::new();
    for root in read_dir_sorted(&config_dir.join("plugins"))? {
        if !root.is_dir() {
            continue;
        }
        if !root.join("neoism-plugin.json").is_file() {
            continue;
        }
        found.push(load_plugin_manifest(&root)?);
    }
    found.sort_by(|a, b| {
        a.manifest
            .id
            .cmp(&b.manifest.id)
            .then_with(|| a.manifest_path.cmp(&b.manifest_path))
    });
    for pair in found.windows(2) {
        if pair[0].manifest.id == pair[1].manifest.id {
            return Err(PluginEcosystemError::DuplicatePlugin(
                pair[0].manifest.id.clone(),
            ));
        }
    }
    Ok(found)
}

pub fn load_plugin_manifest(
    root: &Path,
) -> Result<DiscoveredManifest, PluginEcosystemError> {
    let discovered = read_plugin_manifest(root)?;
    validate_manifest(&discovered.manifest, root)?;
    Ok(discovered)
}

/// Parse a package manifest without applying compatibility or entrypoint
/// validation. Inventory UIs use this to retain package identity while
/// presenting validation failures as row-local lifecycle states.
pub fn read_plugin_manifest(
    root: &Path,
) -> Result<DiscoveredManifest, PluginEcosystemError> {
    let path = root.join("neoism-plugin.json");
    let source =
        std::fs::read_to_string(&path).map_err(|source| PluginEcosystemError::Read {
            path: path.clone(),
            source,
        })?;
    let manifest: PluginManifest =
        serde_json::from_str(&source).map_err(|source| PluginEcosystemError::Json {
            path: path.clone(),
            source,
        })?;
    Ok(DiscoveredManifest {
        root: root.to_path_buf(),
        manifest_path: path,
        manifest,
    })
}

#[cfg(feature = "runtime")]
pub fn discover_plugin_specs(
    config_dir: &Path,
) -> Result<Vec<(PathBuf, PluginSpec)>, PluginEcosystemError> {
    use mlua::{Lua, LuaSerdeExt, Value as LuaValue};

    let mut specs = Vec::new();
    for path in read_dir_sorted(&config_dir.join("lua/plugins"))? {
        if path.extension().and_then(|value| value.to_str()) != Some("lua") {
            continue;
        }
        let source = std::fs::read_to_string(&path).map_err(|source| {
            PluginEcosystemError::Read {
                path: path.clone(),
                source,
            }
        })?;
        let lua = Lua::new();
        lua.set_memory_limit(8 * 1024 * 1024).map_err(|source| {
            PluginEcosystemError::LuaSpec {
                path: path.clone(),
                source,
            }
        })?;
        let globals = lua.globals();
        for name in ["io", "os", "debug", "require", "dofile", "loadfile", "load"] {
            globals.set(name, LuaValue::Nil).map_err(|source| {
                PluginEcosystemError::LuaSpec {
                    path: path.clone(),
                    source,
                }
            })?;
        }
        let package: mlua::Table =
            globals
                .get("package")
                .map_err(|source| PluginEcosystemError::LuaSpec {
                    path: path.clone(),
                    source,
                })?;
        package
            .set("path", "")
            .and_then(|_| package.set("cpath", ""))
            .map_err(|source| PluginEcosystemError::LuaSpec {
                path: path.clone(),
                source,
            })?;
        package.set("loadlib", LuaValue::Nil).map_err(|source| {
            PluginEcosystemError::LuaSpec {
                path: path.clone(),
                source,
            }
        })?;
        globals.set("package", LuaValue::Nil).map_err(|source| {
            PluginEcosystemError::LuaSpec {
                path: path.clone(),
                source,
            }
        })?;
        let value: LuaValue = super::runtime::run_with_budget(
            &lua,
            std::time::Duration::from_millis(50),
            || {
                lua.load(&source)
                    .set_name(path.to_string_lossy().as_ref())
                    .eval()
            },
        )
        .map_err(|source| PluginEcosystemError::LuaSpec {
            path: path.clone(),
            source,
        })?;
        let json: serde_json::Value =
            lua.from_value(value)
                .map_err(|source| PluginEcosystemError::LuaSpec {
                    path: path.clone(),
                    source,
                })?;
        if json.is_array() {
            for spec in
                serde_json::from_value::<Vec<PluginSpec>>(json).map_err(|error| {
                    PluginEcosystemError::LuaSpec {
                        path: path.clone(),
                        source: mlua::Error::runtime(error.to_string()),
                    }
                })?
            {
                specs.push((path.clone(), spec));
            }
        } else {
            let spec = serde_json::from_value(json).map_err(|error| {
                PluginEcosystemError::LuaSpec {
                    path: path.clone(),
                    source: mlua::Error::runtime(error.to_string()),
                }
            })?;
            specs.push((path, spec));
        }
    }
    Ok(specs)
}

#[cfg(feature = "runtime")]
pub fn discover_plugins(
    config_dir: &Path,
) -> Result<PluginDiscovery, PluginEcosystemError> {
    Ok(PluginDiscovery {
        specs: discover_plugin_specs(config_dir)?,
        manifests: discover_local_manifests(config_dir)?,
    })
}

pub fn validate_manifest(
    manifest: &PluginManifest,
    root: &Path,
) -> Result<(), PluginEcosystemError> {
    let invalid = |message: String| PluginEcosystemError::InvalidManifest {
        id: manifest.id.clone(),
        message,
    };
    if !valid_reverse_dns_id(&manifest.id) {
        return Err(invalid("id must be a lowercase reverse-DNS name".into()));
    }
    if manifest.api_version != API_VERSION {
        return Err(invalid(format!(
            "API version {} is incompatible with host API {API_VERSION}",
            manifest.api_version
        )));
    }
    let editor_entrypoint = manifest.editor_entrypoint();
    let entrypoint = Path::new(editor_entrypoint);
    if editor_entrypoint.is_empty()
        || entrypoint.is_absolute()
        || entrypoint
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(invalid(
            "entrypoint must be a contained relative path".into(),
        ));
    }
    if let (Ok(root), Ok(entrypoint)) =
        (root.canonicalize(), root.join(entrypoint).canonicalize())
    {
        if !entrypoint.starts_with(root) {
            return Err(invalid(
                "entrypoint resolves outside the plugin root".into(),
            ));
        }
    }
    if root.exists() && !root.join(entrypoint).is_file() {
        return Err(invalid("entrypoint does not name a file".into()));
    }
    let allowed = ["linux", "macos", "windows"];
    let mut platforms = BTreeSet::new();
    for platform in &manifest.platforms {
        if !allowed.contains(&platform.as_str()) {
            return Err(invalid(format!("unknown platform `{platform}`")));
        }
        if !platforms.insert(platform) {
            return Err(invalid(format!("duplicate platform `{platform}`")));
        }
    }
    if !manifest.platforms.is_empty()
        && !manifest
            .platforms
            .iter()
            .any(|platform| platform == std::env::consts::OS)
    {
        return Err(invalid(format!(
            "plugin does not support platform `{}`",
            std::env::consts::OS
        )));
    }
    reject_duplicate_keys(&manifest.triggers, PluginTrigger::key, "trigger", &invalid)?;
    reject_duplicate_keys(
        &manifest.capabilities,
        PluginCapability::key,
        "capability",
        &invalid,
    )?;
    reject_duplicate_keys(
        &manifest.dependencies,
        |dependency| dependency.clone(),
        "dependency",
        &invalid,
    )?;
    for entrypoint in [
        manifest.entrypoints.ui_lua.as_deref(),
        manifest.entrypoints.editor_lua.as_deref(),
        manifest.entrypoints.agent.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        validate_contained_entrypoint(entrypoint, root).map_err(&invalid)?;
    }
    if let Some(command) = &manifest.entrypoints.subprocess {
        if command.is_empty()
            || command.len() > 256
            || command[0].trim().is_empty()
            || command.iter().any(|argument| argument.len() > 64 * 1024)
        {
            return Err(invalid(
                "subprocess entrypoint is empty or exceeds limits".into(),
            ));
        }
    }
    if let Some(native) = &manifest.entrypoints.native {
        native.validate().map_err(&invalid)?;
        if native.abi != 1 {
            return Err(invalid(format!(
                "native entrypoint ABI {} is incompatible with ABI 1",
                native.abi
            )));
        }
        if !native.platforms.is_empty()
            && !native
                .platforms
                .iter()
                .any(|platform| platform == std::env::consts::OS)
        {
            return Err(invalid(format!(
                "native entrypoint does not support platform `{}`",
                std::env::consts::OS
            )));
        }
    }
    Ok(())
}

fn validate_contained_entrypoint(entrypoint: &str, root: &Path) -> Result<(), String> {
    let path = Path::new(entrypoint);
    if entrypoint.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(format!(
            "entrypoint `{entrypoint}` is not a contained relative path"
        ));
    }
    if let (Ok(root), Ok(path)) = (root.canonicalize(), root.join(path).canonicalize()) {
        if !path.starts_with(root) {
            return Err(format!(
                "entrypoint `{entrypoint}` resolves outside the package"
            ));
        }
    }
    Ok(())
}

fn reject_duplicate_keys<T>(
    items: &[T],
    key: impl Fn(&T) -> String,
    kind: &str,
    invalid: &impl Fn(String) -> PluginEcosystemError,
) -> Result<(), PluginEcosystemError> {
    let mut seen = BTreeSet::new();
    for item in items {
        let key = key(item);
        if key.is_empty() {
            return Err(invalid(format!("{kind} must not be empty")));
        }
        if !seen.insert(key.clone()) {
            return Err(invalid(format!("duplicate {kind} `{key}`")));
        }
    }
    Ok(())
}

fn valid_reverse_dns_id(id: &str) -> bool {
    let parts = id.split('.').collect::<Vec<_>>();
    parts.len() >= 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.as_bytes()[0].is_ascii_lowercase()
                && part.bytes().all(|byte| {
                    byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                })
                && !part.ends_with('-')
        })
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PluginGraph {
    pub order: Vec<String>,
    pub trigger_index: BTreeMap<String, Vec<String>>,
    pub capability_index: BTreeMap<String, Vec<String>>,
}

pub fn build_plugin_graph(
    manifests: &[PluginManifest],
) -> Result<PluginGraph, PluginEcosystemError> {
    let by_id = manifests
        .iter()
        .map(|manifest| (manifest.id.as_str(), manifest))
        .collect::<BTreeMap<_, _>>();
    if by_id.len() != manifests.len() {
        let mut seen = BTreeSet::new();
        let id = manifests
            .iter()
            .find(|manifest| !seen.insert(&manifest.id))
            .unwrap()
            .id
            .clone();
        return Err(PluginEcosystemError::DuplicatePlugin(id));
    }
    for manifest in manifests {
        for dependency in &manifest.dependencies {
            if !by_id.contains_key(dependency.as_str()) {
                return Err(PluginEcosystemError::MissingDependency {
                    plugin: manifest.id.clone(),
                    dependency: dependency.clone(),
                });
            }
        }
    }
    let mut marks = BTreeMap::<&str, u8>::new();
    let mut stack = Vec::<&str>::new();
    let mut order = Vec::new();
    fn visit<'a>(
        id: &'a str,
        by_id: &BTreeMap<&'a str, &'a PluginManifest>,
        marks: &mut BTreeMap<&'a str, u8>,
        stack: &mut Vec<&'a str>,
        order: &mut Vec<String>,
    ) -> Result<(), PluginEcosystemError> {
        if marks.get(id) == Some(&2) {
            return Ok(());
        }
        if marks.get(id) == Some(&1) {
            let start = stack.iter().position(|item| *item == id).unwrap_or(0);
            let mut cycle = stack[start..].to_vec();
            cycle.push(id);
            return Err(PluginEcosystemError::DependencyCycle {
                path: cycle.join(" -> "),
            });
        }
        marks.insert(id, 1);
        stack.push(id);
        let mut dependencies = by_id[id]
            .dependencies
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        dependencies.sort_unstable();
        for dependency in dependencies {
            visit(dependency, by_id, marks, stack, order)?;
        }
        stack.pop();
        marks.insert(id, 2);
        order.push(id.to_owned());
        Ok(())
    }
    for id in by_id.keys().copied() {
        visit(id, &by_id, &mut marks, &mut stack, &mut order)?;
    }
    let mut graph = PluginGraph {
        order,
        ..PluginGraph::default()
    };
    for manifest in manifests {
        for trigger in &manifest.triggers {
            graph
                .trigger_index
                .entry(trigger.key())
                .or_default()
                .push(manifest.id.clone());
        }
        for capability in &manifest.capabilities {
            graph
                .capability_index
                .entry(capability.key())
                .or_default()
                .push(manifest.id.clone());
        }
    }
    for ids in graph
        .trigger_index
        .values_mut()
        .chain(graph.capability_index.values_mut())
    {
        ids.sort();
    }
    Ok(graph)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &str, dependencies: &[&str]) -> PluginManifest {
        PluginManifest {
            id: id.into(),
            dependencies: dependencies.iter().map(|value| (*value).into()).collect(),
            ..PluginManifest::default()
        }
    }

    #[test]
    fn graph_is_stable_and_reports_full_cycle() {
        let graph =
            build_plugin_graph(&[manifest("dev.z", &["dev.a"]), manifest("dev.a", &[])])
                .unwrap();
        assert_eq!(graph.order, ["dev.a", "dev.z"]);
        let error = build_plugin_graph(&[
            manifest("dev.a", &["dev.b"]),
            manifest("dev.b", &["dev.c"]),
            manifest("dev.c", &["dev.a"]),
        ])
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("dev.a -> dev.b -> dev.c -> dev.a"));
    }

    #[test]
    fn manifest_rejects_escape_and_duplicate_triggers() {
        let mut value = manifest("dev.neoism.test", &[]);
        value.entrypoint = "../init.lua".into();
        assert!(validate_manifest(&value, Path::new(".")).is_err());
        value.entrypoint = "init.lua".into();
        value.triggers = vec![
            PluginTrigger::Name("start".into()),
            PluginTrigger::Name("start".into()),
        ];
        assert!(validate_manifest(&value, Path::new(".")).is_err());
    }

    #[test]
    fn canonical_dual_target_manifest_keeps_editor_and_extension_entrypoints() {
        let manifest: PluginManifest = serde_json::from_str(r#"{
            "id":"dev.neoism.dual-target","name":"Dual target","version":"1",
            "entrypoint":"legacy.lua",
            "editor":{"entrypoint":"editor.lua"},
            "agent":{"runtime":"process","entrypoint":"agent.mjs","command":["node","agent.mjs"],"capabilities":["workspace-read"],"scope":"workspace","eventNamespaces":["session."]},
            "entrypoints":{"uiLua":"ui.lua","editorLua":"editor-tier.lua"}
        }"#).unwrap();

        assert_eq!(manifest.entrypoint, "legacy.lua");
        assert_eq!(manifest.editor_entrypoint(), "editor.lua");
        assert_eq!(manifest.entrypoints.ui_lua.as_deref(), Some("ui.lua"));
        assert_eq!(
            manifest.entrypoints.editor_lua.as_deref(),
            Some("editor-tier.lua")
        );
    }

    #[cfg(feature = "runtime")]
    #[test]
    fn discovery_is_sorted_and_specs_are_host_inert() {
        let root = std::env::temp_dir()
            .join(format!("neoism-lua-discovery-{}", std::process::id()));
        std::fs::create_dir_all(root.join("lua/plugins")).unwrap();
        std::fs::write(root.join("lua/plugins/z.lua"), "assert(os == nil); return { id = 'dev.z', source = { type = 'local', path = 'z' } }").unwrap();
        std::fs::write(
            root.join("lua/plugins/a.lua"),
            "return { id = 'dev.a', source = { type = 'local', path = 'a' } }",
        )
        .unwrap();
        for id in ["dev.z", "dev.a"] {
            let dir = root.join("plugins").join(id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("init.lua"), "").unwrap();
            std::fs::write(
                dir.join("neoism-plugin.json"),
                serde_json::to_vec(&manifest(id, &[])).unwrap(),
            )
            .unwrap();
        }
        let found = discover_plugins(&root).unwrap();
        assert_eq!(
            found
                .specs
                .iter()
                .map(|(_, spec)| spec.id.as_str())
                .collect::<Vec<_>>(),
            ["dev.a", "dev.z"]
        );
        assert_eq!(
            found
                .manifests
                .iter()
                .map(|item| item.manifest.id.as_str())
                .collect::<Vec<_>>(),
            ["dev.a", "dev.z"]
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
