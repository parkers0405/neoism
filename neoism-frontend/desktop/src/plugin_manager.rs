use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use neoism_lua::{
    build_plugin_graph, discover_local_manifests, discover_plugin_specs,
    load_plugin_manifest, read_plugin_manifest, validate_manifest, CommandContribution,
    DiscoveredManifest, ExecutionScope, LazyKeyTrigger, PluginEcosystemError,
    PluginEvent, PluginGraph, PluginHost, PluginRuntime, PluginRuntimeCandidate,
    PluginSnapshot, PluginSource, PluginTrigger,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum PluginManagerError {
    #[error(transparent)]
    Ecosystem(#[from] PluginEcosystemError),
    #[error("plugin `{plugin}` failed to load: {source}")]
    Runtime {
        plugin: String,
        #[source]
        source: neoism_lua::LuaPluginError,
    },
    #[error(
        "plugin contribution conflict for {kind} `{id}` between `{first}` and `{second}`"
    )]
    Conflict {
        kind: &'static str,
        id: String,
        first: String,
        second: String,
    },
    #[error("callback `{0}` does not belong to the active plugin generation")]
    StaleCallback(String),
    #[error("invalid surface layout in plugin `{plugin}`: {message}")]
    InvalidSurfaceLayout { plugin: String, message: String },
    #[error("invalid retained UI contribution in plugin `{plugin}`: {message}")]
    InvalidContribution { plugin: String, message: String },
    #[error(transparent)]
    Acquisition(#[from] neoism_extensions::lua_plugins::AcquisitionError),
    #[error("invalid Mash Up Pack editor plugin selection: {0}")]
    MashupSelection(String),
}

#[derive(Clone, Debug)]
pub(crate) struct PluginFailure {
    pub plugin_id: String,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LuaPluginLifecycle {
    Discovered,
    Lazy,
    Loaded,
    Disabled,
    MashupExcluded,
    UpdateAvailable,
    PermissionRequired,
    Approved,
    Revoked,
    Incompatible,
    Blocked,
    Failed,
    RestoreRequired,
}

#[derive(Clone, Debug)]
pub(crate) struct LuaPluginInventoryEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub lifecycle: LuaPluginLifecycle,
    pub status_text: String,
    pub capabilities: Vec<String>,
    pub grants: Vec<String>,
    pub missing_permissions: Vec<String>,
    pub repository_url: Option<String>,
    pub requested_ref: Option<String>,
    pub installed_commit: Option<String>,
    pub root: Option<PathBuf>,
    pub mashup_controlled: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LuaPluginInventory {
    pub entries: BTreeMap<String, LuaPluginInventoryEntry>,
    pub global_errors: Vec<String>,
}

pub(crate) struct LuaPluginManager {
    config_dir: PathBuf,
    host: Arc<dyn PluginHost>,
    discovered: BTreeMap<String, DiscoveredManifest>,
    graph: PluginGraph,
    active: BTreeMap<String, PluginRuntime>,
    grants: BTreeMap<String, Vec<String>>,
    disabled: BTreeSet<String>,
    mashup_excluded: BTreeSet<String>,
    revision: u64,
    snapshot: PluginSnapshot,
    failures: BTreeMap<String, PluginFailure>,
    native_generations: BTreeMap<String, crate::native_extension::NativeGeneration>,
}

impl LuaPluginManager {
    pub fn discover(
        config_dir: impl Into<PathBuf>,
        host: Arc<dyn PluginHost>,
        policy: &neoism_backend::config::PluginPreferences,
        selection: Option<&neoism_backend::config::mashup::EditorPluginSelection>,
    ) -> Result<Self, PluginManagerError> {
        let mut manager =
            Self::discover_inactive(config_dir.into(), host, policy, selection)?;
        manager.activate_eager()?;
        manager.add_lazy_placeholders();
        Ok(manager)
    }

    pub fn discover_for_startup(
        config_dir: impl Into<PathBuf>,
        host: Arc<dyn PluginHost>,
        policy: &neoism_backend::config::PluginPreferences,
        selection: Option<&neoism_backend::config::mashup::EditorPluginSelection>,
    ) -> (Self, Option<PluginManagerError>) {
        let config_dir = config_dir.into();
        let mut manager = match Self::discover_inactive(
            config_dir.clone(),
            host.clone(),
            policy,
            selection,
        ) {
            Ok(manager) => manager,
            Err(error) => return (Self::empty(config_dir, host), Some(error)),
        };
        let error = manager.activate_eager().err();
        manager.add_lazy_placeholders();
        (manager, error)
    }

    fn discover_inactive(
        config_dir: PathBuf,
        host: Arc<dyn PluginHost>,
        policy: &neoism_backend::config::PluginPreferences,
        selection: Option<&neoism_backend::config::mashup::EditorPluginSelection>,
    ) -> Result<Self, PluginManagerError> {
        let disabled = policy.disabled.iter().cloned().collect::<BTreeSet<_>>();
        let manifests = discover_manifests(&config_dir, &policy.trusted_sources)?;
        let (manifests, mashup_excluded) =
            select_manifests(manifests, &disabled, selection)?;
        let graph = build_plugin_graph(
            &manifests
                .iter()
                .map(|plugin| plugin.manifest.clone())
                .collect::<Vec<_>>(),
        )?;
        let discovered = manifests
            .into_iter()
            .map(|plugin| (plugin.manifest.id.clone(), plugin))
            .collect();
        Ok(Self {
            config_dir,
            host,
            discovered,
            graph,
            active: BTreeMap::new(),
            grants: policy.grants.clone(),
            disabled: policy.disabled.iter().cloned().collect(),
            mashup_excluded,
            revision: 0,
            snapshot: PluginSnapshot::empty(),
            failures: BTreeMap::new(),
            native_generations: BTreeMap::new(),
        })
    }

    pub fn empty(config_dir: impl Into<PathBuf>, host: Arc<dyn PluginHost>) -> Self {
        Self {
            config_dir: config_dir.into(),
            host,
            discovered: BTreeMap::new(),
            graph: PluginGraph::default(),
            active: BTreeMap::new(),
            grants: BTreeMap::new(),
            disabled: BTreeSet::new(),
            mashup_excluded: BTreeSet::new(),
            revision: 0,
            snapshot: PluginSnapshot::empty(),
            failures: BTreeMap::new(),
            native_generations: BTreeMap::new(),
        }
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub fn snapshot(&self) -> &PluginSnapshot {
        &self.snapshot
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn failures(&self) -> impl Iterator<Item = &PluginFailure> {
        self.failures.values()
    }

    pub fn active_ids(&self) -> impl Iterator<Item = &str> {
        self.active.keys().map(String::as_str)
    }

    pub fn enabled_dependents(&self, plugin_id: &str) -> Vec<String> {
        self.discovered
            .values()
            .filter(|plugin| {
                plugin.manifest.id != plugin_id
                    && plugin
                        .manifest
                        .dependencies
                        .iter()
                        .any(|dependency| dependency == plugin_id)
            })
            .map(|plugin| plugin.manifest.id.clone())
            .collect()
    }

    pub fn status_snapshot(&self) -> serde_json::Value {
        let mut rows = self
            .discovered
            .values()
            .map(|plugin| {
                let failure = self.failures.get(&plugin.manifest.id);
                serde_json::json!({
                    "id": plugin.manifest.id,
                    "name": plugin.manifest.name,
                    "version": plugin.manifest.version,
                    "status": if failure.is_some() {
                        "failed"
                    } else if self.active.contains_key(&plugin.manifest.id) {
                        "loaded"
                    } else {
                        "lazy"
                    },
                    "capabilities": plugin.manifest.capabilities.iter().map(|capability| capability.key()).collect::<Vec<_>>(),
                    "grants": self.grants.get(&plugin.manifest.id).cloned().unwrap_or_default(),
                    "error": failure.map(|failure| failure.message.clone()),
                    "root": plugin.root,
                })
            })
            .collect::<Vec<_>>();
        rows.extend(self.disabled.iter().map(|id| {
            serde_json::json!({
                "id": id,
                "name": id,
                "status": "disabled",
                "capabilities": [],
                "grants": self.grants.get(id).cloned().unwrap_or_default(),
            })
        }));
        rows.extend(
            self.mashup_excluded
                .iter()
                .filter(|id| !self.disabled.contains(*id))
                .map(|id| {
                    serde_json::json!({
                        "id": id,
                        "name": id,
                        "status": "mashup-excluded",
                        "capabilities": [],
                        "grants": self.grants.get(id).cloned().unwrap_or_default(),
                    })
                }),
        );
        rows.sort_by(|left, right| {
            left.get("id")
                .and_then(serde_json::Value::as_str)
                .cmp(&right.get("id").and_then(serde_json::Value::as_str))
        });
        serde_json::json!({ "revision": self.revision, "plugins": rows })
    }

    pub fn inventory(
        &self,
        policy: &neoism_backend::config::PluginPreferences,
    ) -> LuaPluginInventory {
        scan_plugin_inventory(self, policy)
    }

    pub fn is_empty(&self) -> bool {
        self.discovered.is_empty()
            && self.disabled.is_empty()
            && self.mashup_excluded.is_empty()
    }

    pub fn activate_trigger(
        &mut self,
        trigger: &str,
    ) -> Result<bool, PluginManagerError> {
        let targets = self
            .graph
            .trigger_index
            .get(trigger)
            .cloned()
            .unwrap_or_default();
        self.activate_plugins(&targets)
    }

    pub fn activate_surface(
        &mut self,
        surface: &str,
    ) -> Result<bool, PluginManagerError> {
        self.activate_trigger(&format!("surface:{surface}"))
    }

    pub fn activate_command(
        &mut self,
        command: &str,
    ) -> Result<bool, PluginManagerError> {
        self.activate_trigger(&format!("command:{command}"))
    }

    pub fn activate_key(&mut self, key: &str) -> Result<bool, PluginManagerError> {
        self.activate_trigger(&format!("key:{key}"))
    }

    pub fn activate_filetype(
        &mut self,
        filetype: &str,
    ) -> Result<bool, PluginManagerError> {
        self.activate_trigger(&format!("filetype:{filetype}"))
    }

    pub fn invoke(
        &self,
        callback: &str,
        event: PluginEvent,
    ) -> Result<serde_json::Value, PluginManagerError> {
        let Some(plugin_id) = callback_owner(callback) else {
            return Err(PluginManagerError::StaleCallback(callback.to_string()));
        };
        let runtime = self
            .active
            .get(plugin_id)
            .ok_or_else(|| PluginManagerError::StaleCallback(callback.to_string()))?;
        runtime
            .invoke(callback, event)
            .map_err(|source| PluginManagerError::Runtime {
                plugin: plugin_id.to_string(),
                source,
            })
    }

    pub fn emit(&mut self, event: PluginEvent) -> Vec<PluginFailure> {
        let mut failures = Vec::new();
        let plugin_ids = self.active.keys().cloned().collect::<Vec<_>>();
        let mut changed = false;
        for plugin_id in plugin_ids {
            let Some(runtime) = self.active.get_mut(&plugin_id) else {
                continue;
            };
            let before = runtime.snapshot().clone();
            if let Err(error) = runtime.emit(event.clone()) {
                let failure = PluginFailure {
                    plugin_id: plugin_id.clone(),
                    message: error.to_string(),
                };
                self.failures.insert(plugin_id, failure.clone());
                failures.push(failure);
            }
            changed |= runtime.snapshot() != &before;
        }
        if changed {
            if let Ok(snapshot) = combined_snapshot(&self.active) {
                self.revision = self.revision.saturating_add(1);
                self.snapshot = snapshot;
                self.add_lazy_placeholders();
            }
        }
        failures
    }

    pub fn emit_to_owner(
        &mut self,
        owner: &neoism_lua::PluginOwner,
        event: PluginEvent,
    ) -> Result<bool, PluginManagerError> {
        let Some(runtime) = self.active.get_mut(&owner.plugin_id) else {
            return Err(PluginManagerError::StaleCallback(format!(
                "{}@{}",
                owner.plugin_id, owner.revision.0
            )));
        };
        if runtime.owner() != owner {
            return Err(PluginManagerError::StaleCallback(format!(
                "{}@{}",
                owner.plugin_id, owner.revision.0
            )));
        }
        let before = runtime.snapshot().clone();
        runtime
            .emit(event)
            .map_err(|source| PluginManagerError::Runtime {
                plugin: owner.plugin_id.clone(),
                source,
            })?;
        let changed = runtime.snapshot() != &before;
        if changed {
            self.snapshot = combined_snapshot(&self.active)?;
            self.revision = self.revision.wrapping_add(1);
            self.add_lazy_placeholders();
        }
        Ok(changed)
    }

    pub fn is_active_owner(&self, owner: &neoism_lua::PluginOwner) -> bool {
        self.active
            .get(&owner.plugin_id)
            .is_some_and(|runtime| runtime.owner() == owner)
    }

    pub fn active_owners(&self) -> impl Iterator<Item = &neoism_lua::PluginOwner> {
        self.active.values().map(PluginRuntime::owner)
    }

    pub fn command_contribution(
        &self,
        owner: &neoism_lua::PluginOwner,
        command_id: &str,
    ) -> Option<neoism_lua::CommandContribution> {
        self.active
            .get(&owner.plugin_id)
            .filter(|runtime| runtime.owner() == owner)
            .and_then(|runtime| {
                runtime.snapshot().commands.iter().find(|command| {
                    command.id == command_id
                        || command.aliases.iter().any(|alias| alias == command_id)
                })
            })
            .cloned()
    }

    pub fn resolve_command(
        &self,
        command_id: &str,
    ) -> Option<(neoism_lua::PluginOwner, neoism_lua::CommandContribution)> {
        self.active.values().find_map(|runtime| {
            runtime
                .snapshot()
                .commands
                .iter()
                .find(|command| {
                    command.id == command_id
                        || command.aliases.iter().any(|alias| alias == command_id)
                })
                .cloned()
                .map(|command| (runtime.owner().clone(), command))
        })
    }

    pub fn complete_command(
        &self,
        owner: &neoism_lua::PluginOwner,
        command_id: &str,
        prefix: &str,
    ) -> Result<Vec<String>, PluginManagerError> {
        let runtime = self
            .active
            .get(&owner.plugin_id)
            .filter(|runtime| runtime.owner() == owner)
            .ok_or_else(|| {
                PluginManagerError::StaleCallback(format!(
                    "{}@{}",
                    owner.plugin_id, owner.revision.0
                ))
            })?;
        runtime
            .complete_command(command_id, prefix)
            .map_err(|source| PluginManagerError::Runtime {
                plugin: owner.plugin_id.clone(),
                source,
            })
    }

    fn activate_eager(&mut self) -> Result<(), PluginManagerError> {
        let eager = self
            .discovered
            .values()
            .filter(|plugin| {
                plugin.manifest.triggers.is_empty()
                    || plugin.manifest.triggers.iter().any(|trigger| {
                        matches!(trigger.key().as_str(), "startup" | "Startup")
                    })
            })
            .map(|plugin| plugin.manifest.id.clone())
            .collect::<Vec<_>>();
        self.activate_plugins(&eager)?;
        Ok(())
    }

    fn activate_plugins(
        &mut self,
        targets: &[String],
    ) -> Result<bool, PluginManagerError> {
        if targets.is_empty() {
            return Ok(false);
        }
        let mut required = BTreeSet::new();
        for target in targets {
            self.collect_dependencies(target, &mut required);
        }
        let order = self
            .graph
            .order
            .iter()
            .filter(|id| required.contains(*id) && !self.active.contains_key(*id))
            .cloned()
            .collect::<Vec<_>>();
        if order.is_empty() {
            return Ok(false);
        }

        let candidate_revision = self.revision.saturating_add(1);
        let mut candidates = BTreeMap::<String, PluginRuntimeCandidate>::new();
        let mut native_candidates = BTreeMap::new();
        for plugin_id in order {
            let plugin = &self.discovered[&plugin_id];
            let revision = neoism_lua::plugin_content_revision(
                &plugin.root,
                plugin.manifest.editor_entrypoint(),
            )
            .map_err(|source| PluginManagerError::Runtime {
                plugin: plugin_id.clone(),
                source,
            })?;
            match PluginRuntime::build_candidate(
                &plugin.root,
                &plugin.manifest,
                revision.clone(),
                Arc::new(neoism_lua::ScopedPluginHost::new(
                    self.host.clone(),
                    neoism_lua::PluginOwner {
                        plugin_id: plugin_id.clone(),
                        revision,
                    },
                    plugin
                        .manifest
                        .capabilities
                        .iter()
                        .map(|capability| capability.key()),
                    self.grants.get(&plugin_id).cloned().unwrap_or_default(),
                )),
            ) {
                Ok(candidate) => {
                    let owner = candidate.owner().clone();
                    let package_digest =
                        neoism_extensions::lua_plugins::tree_sha256(&plugin.root)
                            .map_err(PluginManagerError::Acquisition)?;
                    let capabilities = plugin
                        .manifest
                        .capabilities
                        .iter()
                        .map(|value| value.key())
                        .collect::<BTreeSet<_>>();
                    if let Some(artifact) = &plugin.manifest.entrypoints.native {
                        if exact_artifact_approved(
                            &owner,
                            &package_digest,
                            artifact,
                            &capabilities,
                        ) {
                            let generation = crate::native_extension::start_approved(
                                owner.clone(),
                                &plugin.root,
                                &package_digest,
                                artifact,
                                &capabilities,
                                neoism_extensions::trust::ApprovalScope::User,
                            )
                            .map_err(|message| {
                                mark_artifact_failed(
                                    &owner,
                                    &package_digest,
                                    artifact,
                                    &capabilities,
                                    &message,
                                );
                                PluginManagerError::InvalidContribution {
                                    plugin: plugin_id.clone(),
                                    message,
                                }
                            })?;
                            native_candidates
                                .insert(format!("{}:native", plugin_id), generation);
                        }
                    }
                    for contribution in &candidate.snapshot().platform {
                        if let neoism_lua::PlatformContribution::TreeSitter(parser) =
                            &contribution.contribution
                        {
                            if exact_artifact_approved(
                                &owner,
                                &package_digest,
                                &parser.parser,
                                &capabilities,
                            ) {
                                let generation = crate::native_extension::validate_tree_sitter_approved(owner.clone(), &plugin.root, &package_digest, &parser.parser, &parser.language, &capabilities, neoism_extensions::trust::ApprovalScope::User)
                                    .map_err(|message| { mark_artifact_failed(&owner, &package_digest, &parser.parser, &capabilities, &message); PluginManagerError::InvalidContribution { plugin: plugin_id.clone(), message } })?;
                                native_candidates.insert(
                                    format!("{}:parser:{}", plugin_id, parser.id),
                                    generation,
                                );
                            }
                        }
                    }
                    candidates.insert(plugin_id.clone(), candidate);
                }
                Err(source) => {
                    let failure = PluginFailure {
                        plugin_id: plugin_id.clone(),
                        message: source.to_string(),
                    };
                    self.failures.insert(plugin_id.clone(), failure);
                    return Err(PluginManagerError::Runtime {
                        plugin: plugin_id,
                        source,
                    });
                }
            }
        }

        validate_candidate_snapshot(&self.active, &candidates)?;
        for (plugin_id, candidate) in candidates {
            self.native_generations
                .retain(|_, generation| generation.owner.plugin_id != plugin_id);
            self.active.insert(plugin_id.clone(), candidate.activate());
            self.failures.remove(&plugin_id);
        }
        self.native_generations.extend(native_candidates);
        self.snapshot = combined_snapshot(&self.active)?;
        self.add_lazy_placeholders();
        self.revision = candidate_revision;
        Ok(true)
    }

    fn collect_dependencies(&self, plugin_id: &str, output: &mut BTreeSet<String>) {
        if !output.insert(plugin_id.to_string()) {
            return;
        }
        let Some(plugin) = self.discovered.get(plugin_id) else {
            return;
        };
        for dependency in &plugin.manifest.dependencies {
            self.collect_dependencies(dependency, output);
        }
    }

    fn add_lazy_placeholders(&mut self) {
        for plugin in self.discovered.values() {
            if self.active.contains_key(&plugin.manifest.id) {
                continue;
            }
            for trigger in &plugin.manifest.triggers {
                let PluginTrigger::Detailed {
                    kind,
                    value: Some(value),
                    mode,
                    when,
                } = trigger
                else {
                    continue;
                };
                if kind == "key" {
                    self.snapshot.lazy_keys.push(LazyKeyTrigger {
                        key: value.clone(),
                        mode: mode.clone().unwrap_or_else(|| "global".into()),
                        when: when.clone(),
                    });
                } else if kind == "command"
                    && !self.snapshot.commands.iter().any(|item| item.id == *value)
                {
                    self.snapshot.commands.push(CommandContribution {
                        id: value.clone(),
                        callback: String::new(),
                        title: value.clone(),
                        description: format!("Load {}", plugin.manifest.name),
                        scope: ExecutionScope::Local,
                        arguments_schema: serde_json::Value::Object(Default::default()),
                        completions: Vec::new(),
                        accepts_range: false,
                        accepts_count: false,
                        accepts_bang: false,
                        aliases: Vec::new(),
                        completion_callback: None,
                        result_schema: serde_json::Value::Object(Default::default()),
                    });
                }
            }
        }
        self.snapshot.lazy_keys.sort();
        self.snapshot.lazy_keys.dedup();
    }
}

fn exact_artifact_approved(
    owner: &neoism_lua::PluginOwner,
    package_digest: &str,
    artifact: &neoism_lua::OpaqueArtifact,
    capabilities: &BTreeSet<String>,
) -> bool {
    neoism_extensions::trust::ExtensionTrustStore::managed()
        .exact(
            &owner.plugin_id,
            &owner.revision.0,
            package_digest,
            &artifact.sha256,
            artifact.abi,
            capabilities,
            &neoism_extensions::trust::ApprovalScope::User,
        )
        .ok()
        .flatten()
        .is_some_and(|approval| {
            approval.state == neoism_extensions::trust::ApprovalState::Approved
        })
}

fn mark_artifact_failed(
    owner: &neoism_lua::PluginOwner,
    package_digest: &str,
    artifact: &neoism_lua::OpaqueArtifact,
    capabilities: &BTreeSet<String>,
    message: &str,
) {
    let store = neoism_extensions::trust::ExtensionTrustStore::managed();
    if let Ok(Some(mut approval)) = store.exact(
        &owner.plugin_id,
        &owner.revision.0,
        package_digest,
        &artifact.sha256,
        artifact.abi,
        capabilities,
        &neoism_extensions::trust::ApprovalScope::User,
    ) {
        approval.decided_by = "extension-host".into();
        let _ = store.record_failure(approval, message.into());
    }
}

fn scan_plugin_inventory(
    manager: &LuaPluginManager,
    policy: &neoism_backend::config::PluginPreferences,
) -> LuaPluginInventory {
    let mut inventory = LuaPluginInventory::default();
    let store = neoism_extensions::lua_plugins::LuaPluginStore::managed();
    let lock = match store.load_lock() {
        Ok(lock) => lock,
        Err(error) => {
            inventory.global_errors.push(error.to_string());
            Default::default()
        }
    };

    for plugin in manager.discovered.values() {
        inventory.entries.insert(
            plugin.manifest.id.clone(),
            inventory_entry(
                &plugin.manifest,
                Some(plugin.root.clone()),
                if manager.active.contains_key(&plugin.manifest.id) {
                    LuaPluginLifecycle::Loaded
                } else {
                    LuaPluginLifecycle::Lazy
                },
                if manager.active.contains_key(&plugin.manifest.id) {
                    "Loaded"
                } else {
                    "Waiting for a lazy-loading trigger"
                },
                policy,
            ),
        );
    }

    let local_root = manager.config_dir.join("plugins");
    match std::fs::read_dir(&local_root) {
        Ok(entries) => {
            let mut roots = entries
                .flatten()
                .map(|entry| entry.path())
                .collect::<Vec<_>>();
            roots.sort();
            for root in roots {
                if !root.join("neoism-plugin.json").is_file() {
                    continue;
                }
                match read_plugin_manifest(&root) {
                    Ok(plugin) => {
                        let validation = validate_manifest(&plugin.manifest, &root);
                        let id = plugin.manifest.id.clone();
                        let lifecycle =
                            if policy.disabled.iter().any(|disabled| disabled == &id) {
                                LuaPluginLifecycle::Disabled
                            } else if manager.mashup_excluded.contains(&id) {
                                LuaPluginLifecycle::MashupExcluded
                            } else if validation.is_err() {
                                LuaPluginLifecycle::Incompatible
                            } else if manager.active.contains_key(&id) {
                                LuaPluginLifecycle::Loaded
                            } else {
                                LuaPluginLifecycle::Lazy
                            };
                        let status = validation
                            .err()
                            .map(|error| error.to_string())
                            .unwrap_or_else(|| lifecycle_label(lifecycle).into());
                        inventory.entries.entry(id).or_insert_with(|| {
                            inventory_entry(
                                &plugin.manifest,
                                Some(root),
                                lifecycle,
                                &status,
                                policy,
                            )
                        });
                    }
                    Err(error) => {
                        let id = root
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("invalid-local-plugin")
                            .to_string();
                        let mashup_controlled = manager.mashup_excluded.contains(&id);
                        inventory.entries.entry(id.clone()).or_insert(
                            LuaPluginInventoryEntry {
                                id: id.clone(),
                                name: id,
                                version: String::new(),
                                lifecycle: LuaPluginLifecycle::Incompatible,
                                status_text: error.to_string(),
                                capabilities: Vec::new(),
                                grants: policy
                                    .grants
                                    .get(
                                        root.file_name()
                                            .and_then(|name| name.to_str())
                                            .unwrap_or_default(),
                                    )
                                    .cloned()
                                    .unwrap_or_default(),
                                missing_permissions: Vec::new(),
                                repository_url: None,
                                requested_ref: None,
                                installed_commit: None,
                                root: Some(root),
                                mashup_controlled,
                            },
                        );
                    }
                }
            }
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            inventory
                .global_errors
                .push(format!("unable to scan {}: {error}", local_root.display()));
        }
        Err(_) => {}
    }

    match discover_plugin_specs(&manager.config_dir) {
        Ok(specs) => {
            for (_, spec) in specs {
                let existing_lock = lock.plugins.get(&spec.id);
                match &spec.source {
                    PluginSource::Git { url, rev } => {
                        let trusted = policy.trusted_sources.is_empty()
                            || policy
                                .trusted_sources
                                .iter()
                                .any(|source| url.starts_with(source));
                        let (mut lifecycle, mut status) = if !trusted {
                            (
                                LuaPluginLifecycle::Blocked,
                                format!("Git source `{url}` is not trusted"),
                            )
                        } else if !spec.enabled
                            || policy.disabled.iter().any(|id| id == &spec.id)
                        {
                            (LuaPluginLifecycle::Disabled, "Disabled".into())
                        } else if manager.mashup_excluded.contains(&spec.id) {
                            (
                                LuaPluginLifecycle::MashupExcluded,
                                "Excluded by active Mash Up Pack".into(),
                            )
                        } else if existing_lock.is_none() {
                            (
                                LuaPluginLifecycle::Discovered,
                                "Available to install".into(),
                            )
                        } else if existing_lock.is_some_and(|entry| {
                            entry.repository_url != *url
                                || rev.as_deref().is_some_and(|requested| {
                                    requested != entry.requested_ref
                                })
                        }) {
                            (
                                LuaPluginLifecycle::UpdateAvailable,
                                "Configured source differs from the installed revision"
                                    .into(),
                            )
                        } else {
                            (LuaPluginLifecycle::Lazy, "Installed".into())
                        };
                        if let Some(current) = inventory.entries.get(&spec.id) {
                            if matches!(
                                current.lifecycle,
                                LuaPluginLifecycle::Loaded
                                    | LuaPluginLifecycle::PermissionRequired
                                    | LuaPluginLifecycle::Approved
                                    | LuaPluginLifecycle::Revoked
                                    | LuaPluginLifecycle::Failed
                            ) {
                                lifecycle = current.lifecycle;
                                status = current.status_text.clone();
                            }
                        }
                        let row = inventory
                            .entries
                            .entry(spec.id.clone())
                            .or_insert_with(|| LuaPluginInventoryEntry {
                                id: spec.id.clone(),
                                name: spec.id.clone(),
                                version: existing_lock
                                    .map(|entry| entry.plugin_version.clone())
                                    .unwrap_or_default(),
                                lifecycle,
                                status_text: status.clone(),
                                capabilities: Vec::new(),
                                grants: policy
                                    .grants
                                    .get(&spec.id)
                                    .cloned()
                                    .unwrap_or_default(),
                                missing_permissions: Vec::new(),
                                repository_url: Some(url.clone()),
                                requested_ref: Some(
                                    rev.clone().unwrap_or_else(|| "HEAD".into()),
                                ),
                                installed_commit: existing_lock
                                    .map(|entry| entry.resolved_commit.clone()),
                                root: existing_lock
                                    .and_then(|entry| store.installed_path(entry).ok()),
                                mashup_controlled: manager
                                    .mashup_excluded
                                    .contains(&spec.id),
                            });
                        row.repository_url = Some(url.clone());
                        row.requested_ref =
                            Some(rev.clone().unwrap_or_else(|| "HEAD".into()));
                        row.installed_commit =
                            existing_lock.map(|entry| entry.resolved_commit.clone());
                        if !matches!(
                            row.lifecycle,
                            LuaPluginLifecycle::Loaded
                                | LuaPluginLifecycle::PermissionRequired
                                | LuaPluginLifecycle::Approved
                                | LuaPluginLifecycle::Revoked
                                | LuaPluginLifecycle::Failed
                        ) {
                            row.lifecycle = lifecycle;
                            row.status_text = status;
                        }
                    }
                    PluginSource::Registry { .. } => {
                        let mashup_controlled =
                            manager.mashup_excluded.contains(&spec.id);
                        inventory.entries.entry(spec.id.clone()).or_insert(
                            LuaPluginInventoryEntry {
                                id: spec.id.clone(),
                                name: spec.id,
                                version: String::new(),
                                lifecycle: LuaPluginLifecycle::Blocked,
                                status_text:
                                    "Registry plugin acquisition is not implemented"
                                        .into(),
                                capabilities: Vec::new(),
                                grants: Vec::new(),
                                missing_permissions: Vec::new(),
                                repository_url: None,
                                requested_ref: None,
                                installed_commit: None,
                                root: None,
                                mashup_controlled,
                            },
                        );
                    }
                    PluginSource::Local { .. } => {}
                }
            }
        }
        Err(error) => inventory.global_errors.push(error.to_string()),
    }

    for (id, entry) in &lock.plugins {
        let root = store.installed_path(entry).ok();
        let exists = root.as_ref().is_some_and(|path| path.is_dir());
        let row =
            inventory
                .entries
                .entry(id.clone())
                .or_insert(LuaPluginInventoryEntry {
                    id: id.clone(),
                    name: id.clone(),
                    version: entry.plugin_version.clone(),
                    lifecycle: if exists {
                        LuaPluginLifecycle::Lazy
                    } else {
                        LuaPluginLifecycle::RestoreRequired
                    },
                    status_text: if exists {
                        "Installed".into()
                    } else {
                        "Installed revision is missing".into()
                    },
                    capabilities: Vec::new(),
                    grants: policy.grants.get(id).cloned().unwrap_or_default(),
                    missing_permissions: Vec::new(),
                    repository_url: Some(entry.repository_url.clone()),
                    requested_ref: Some(entry.requested_ref.clone()),
                    installed_commit: Some(entry.resolved_commit.clone()),
                    root: root.clone(),
                    mashup_controlled: manager.mashup_excluded.contains(id),
                });
        row.repository_url
            .get_or_insert_with(|| entry.repository_url.clone());
        row.requested_ref
            .get_or_insert_with(|| entry.requested_ref.clone());
        row.installed_commit
            .get_or_insert_with(|| entry.resolved_commit.clone());
        if !exists {
            row.lifecycle = LuaPluginLifecycle::RestoreRequired;
            row.status_text = "Installed revision is missing".into();
        }
    }

    for owned in &manager.snapshot.platform {
        let neoism_lua::PlatformContribution::TreeSitter(parser) = &owned.contribution
        else {
            continue;
        };
        let Some(row) = inventory.entries.get_mut(&owned.owner.plugin_id) else {
            continue;
        };
        let Some(root) = row.root.as_deref() else {
            continue;
        };
        let Ok(package_digest) = neoism_extensions::lua_plugins::tree_sha256(root) else {
            continue;
        };
        let capabilities = row.capabilities.iter().cloned().collect::<BTreeSet<_>>();
        let state = neoism_extensions::trust::ExtensionTrustStore::managed()
            .exact(
                &owned.owner.plugin_id,
                &owned.owner.revision.0,
                &package_digest,
                &parser.parser.sha256,
                parser.parser.abi,
                &capabilities,
                &neoism_extensions::trust::ApprovalScope::User,
            )
            .ok()
            .flatten()
            .map(|approval| approval.state);
        match state {
            Some(neoism_extensions::trust::ApprovalState::Approved)
                if row.lifecycle == LuaPluginLifecycle::Loaded =>
            {
                row.lifecycle = LuaPluginLifecycle::Approved;
                row.status_text =
                    "Parser artifact approved for this exact revision".into();
            }
            Some(neoism_extensions::trust::ApprovalState::Revoked) => {
                row.lifecycle = LuaPluginLifecycle::Revoked;
                row.status_text = "Parser artifact approval was revoked".into();
            }
            Some(neoism_extensions::trust::ApprovalState::Failed) => {
                row.lifecycle = LuaPluginLifecycle::Failed;
                row.status_text =
                    "Parser artifact failed validation or activation".into();
            }
            None | Some(neoism_extensions::trust::ApprovalState::PermissionRequired) => {
                row.lifecycle = LuaPluginLifecycle::PermissionRequired;
                row.status_text =
                    "Parser artifact requires exact revision approval".into();
            }
            _ => {}
        }
    }
    for failure in manager.failures.values() {
        if let Some(row) = inventory.entries.get_mut(&failure.plugin_id) {
            row.lifecycle = LuaPluginLifecycle::Failed;
            row.status_text = failure.message.clone();
        }
    }
    for id in &policy.disabled {
        let row =
            inventory
                .entries
                .entry(id.clone())
                .or_insert(LuaPluginInventoryEntry {
                    id: id.clone(),
                    name: id.clone(),
                    version: String::new(),
                    lifecycle: LuaPluginLifecycle::Disabled,
                    status_text: "Disabled".into(),
                    capabilities: Vec::new(),
                    grants: policy.grants.get(id).cloned().unwrap_or_default(),
                    missing_permissions: Vec::new(),
                    repository_url: None,
                    requested_ref: None,
                    installed_commit: None,
                    root: None,
                    mashup_controlled: false,
                });
        row.lifecycle = LuaPluginLifecycle::Disabled;
        row.status_text = "Disabled".into();
        row.mashup_controlled = false;
    }
    for id in &manager.mashup_excluded {
        if policy.disabled.contains(id) {
            continue;
        }
        if let Some(row) = inventory.entries.get_mut(id) {
            row.lifecycle = LuaPluginLifecycle::MashupExcluded;
            row.status_text = "Excluded by active Mash Up Pack".into();
            row.mashup_controlled = true;
        }
    }
    for row in inventory.entries.values_mut() {
        row.capabilities.sort();
        row.capabilities.dedup();
        row.grants.sort();
        row.grants.dedup();
        row.missing_permissions = row
            .capabilities
            .iter()
            .filter(|capability| !row.grants.contains(capability))
            .cloned()
            .collect();
        if !row.missing_permissions.is_empty()
            && matches!(
                row.lifecycle,
                LuaPluginLifecycle::Loaded | LuaPluginLifecycle::Lazy
            )
        {
            row.lifecycle = LuaPluginLifecycle::PermissionRequired;
            row.status_text = format!(
                "{} permission grant(s) required",
                row.missing_permissions.len()
            );
        }
    }
    inventory
}

fn inventory_entry(
    manifest: &neoism_lua::PluginManifest,
    root: Option<PathBuf>,
    lifecycle: LuaPluginLifecycle,
    status_text: &str,
    policy: &neoism_backend::config::PluginPreferences,
) -> LuaPluginInventoryEntry {
    let (lifecycle, status_text) = trust_lifecycle(manifest, root.as_deref())
        .unwrap_or((lifecycle, status_text.into()));
    LuaPluginInventoryEntry {
        id: manifest.id.clone(),
        name: if manifest.name.is_empty() {
            manifest.id.clone()
        } else {
            manifest.name.clone()
        },
        version: manifest.version.clone(),
        lifecycle,
        status_text,
        capabilities: manifest
            .capabilities
            .iter()
            .map(|capability| capability.key())
            .collect(),
        grants: policy.grants.get(&manifest.id).cloned().unwrap_or_default(),
        missing_permissions: Vec::new(),
        repository_url: None,
        requested_ref: None,
        installed_commit: None,
        root,
        mashup_controlled: false,
    }
}

fn lifecycle_label(lifecycle: LuaPluginLifecycle) -> &'static str {
    match lifecycle {
        LuaPluginLifecycle::Discovered => "Discovered",
        LuaPluginLifecycle::Lazy => "Waiting for a lazy-loading trigger",
        LuaPluginLifecycle::Loaded => "Loaded",
        LuaPluginLifecycle::Disabled => "Disabled",
        LuaPluginLifecycle::MashupExcluded => "Excluded by active Mash Up Pack",
        LuaPluginLifecycle::UpdateAvailable => "Update available",
        LuaPluginLifecycle::PermissionRequired => "Permission required",
        LuaPluginLifecycle::Approved => "Approved",
        LuaPluginLifecycle::Revoked => "Approval revoked",
        LuaPluginLifecycle::Incompatible => "Incompatible",
        LuaPluginLifecycle::Blocked => "Blocked",
        LuaPluginLifecycle::Failed => "Failed",
        LuaPluginLifecycle::RestoreRequired => "Installed revision is missing",
    }
}

fn trust_lifecycle(
    manifest: &neoism_lua::PluginManifest,
    root: Option<&Path>,
) -> Option<(LuaPluginLifecycle, String)> {
    let artifact = manifest.entrypoints.native.as_ref()?;
    let root = root?;
    let revision =
        neoism_lua::plugin_content_revision(root, manifest.editor_entrypoint()).ok()?;
    let package_digest = neoism_extensions::lua_plugins::tree_sha256(root).ok()?;
    let capabilities = manifest
        .capabilities
        .iter()
        .map(|value| value.key())
        .collect::<BTreeSet<_>>();
    let approval = neoism_extensions::trust::ExtensionTrustStore::managed()
        .exact(
            &manifest.id,
            &revision.0,
            &package_digest,
            &artifact.sha256,
            artifact.abi,
            &capabilities,
            &neoism_extensions::trust::ApprovalScope::User,
        )
        .ok()
        .flatten();
    Some(match approval.map(|value| value.state) {
        Some(neoism_extensions::trust::ApprovalState::Approved) => (
            LuaPluginLifecycle::Approved,
            "Native artifact approved for this exact revision".into(),
        ),
        Some(neoism_extensions::trust::ApprovalState::Revoked) => (
            LuaPluginLifecycle::Revoked,
            "Native artifact approval was revoked".into(),
        ),
        Some(neoism_extensions::trust::ApprovalState::Failed) => (
            LuaPluginLifecycle::Failed,
            "Native artifact failed validation or activation".into(),
        ),
        _ => (
            LuaPluginLifecycle::PermissionRequired,
            "Native artifact requires exact revision approval".into(),
        ),
    })
}

fn discover_manifests(
    config_dir: &Path,
    trusted_sources: &[String],
) -> Result<Vec<DiscoveredManifest>, PluginManagerError> {
    let mut manifests = discover_local_manifests(config_dir)?
        .into_iter()
        .map(|manifest| (manifest.manifest.id.clone(), manifest))
        .collect::<BTreeMap<_, _>>();
    let specs = discover_plugin_specs(config_dir)?;
    let store = neoism_extensions::lua_plugins::LuaPluginStore::managed();
    let lock = store.load_lock()?;

    for (_, spec) in specs.iter().filter(|(_, spec)| spec.enabled) {
        let root = match &spec.source {
            PluginSource::Local { path } => {
                let path = PathBuf::from(path);
                if path.is_absolute() {
                    path
                } else {
                    config_dir.join(path)
                }
            }
            PluginSource::Git { url, rev } => {
                if !trusted_sources.is_empty()
                    && !trusted_sources
                        .iter()
                        .any(|trusted| url.starts_with(trusted))
                {
                    return Err(PluginManagerError::Ecosystem(
                        PluginEcosystemError::InvalidManifest {
                            id: spec.id.clone(),
                            message: format!(
                                "Git source `{url}` is not in plugins.trusted-sources"
                            ),
                        },
                    ));
                }
                let Some(entry) = lock.plugins.get(&spec.id) else {
                    continue;
                };
                if entry.repository_url != *url
                    || rev
                        .as_deref()
                        .is_some_and(|requested| requested != entry.requested_ref)
                {
                    return Err(PluginManagerError::Ecosystem(
                        PluginEcosystemError::InvalidManifest {
                            id: spec.id.clone(),
                            message: "declarative Git source does not match the installed lock entry"
                                .into(),
                        },
                    ));
                }
                store.installed_path(entry)?
            }
            PluginSource::Registry { .. } => continue,
        };
        let manifest = load_plugin_manifest(&root)?;
        if manifest.manifest.id != spec.id {
            return Err(PluginManagerError::Ecosystem(
                PluginEcosystemError::InvalidManifest {
                    id: manifest.manifest.id,
                    message: format!(
                        "spec id `{}` does not match the package manifest",
                        spec.id
                    ),
                },
            ));
        }
        if manifests.insert(spec.id.clone(), manifest).is_some() {
            return Err(PluginManagerError::Ecosystem(
                PluginEcosystemError::DuplicatePlugin(spec.id.clone()),
            ));
        }
    }

    for (id, entry) in &lock.plugins {
        if manifests.contains_key(id)
            || specs
                .iter()
                .any(|(_, spec)| spec.id == *id && !spec.enabled)
        {
            continue;
        }
        if !trusted_sources.is_empty()
            && !trusted_sources
                .iter()
                .any(|trusted| entry.repository_url.starts_with(trusted))
        {
            return Err(PluginManagerError::Ecosystem(
                PluginEcosystemError::InvalidManifest {
                    id: id.clone(),
                    message: format!(
                        "Git source `{}` is not in plugins.trusted-sources",
                        entry.repository_url
                    ),
                },
            ));
        }
        let manifest = load_plugin_manifest(&store.installed_path(entry)?)?;
        if manifest.manifest.id != *id {
            return Err(PluginManagerError::Ecosystem(
                PluginEcosystemError::InvalidManifest {
                    id: manifest.manifest.id,
                    message: format!(
                        "lockfile id `{id}` does not match the package manifest"
                    ),
                },
            ));
        }
        manifests.insert(id.clone(), manifest);
    }
    Ok(manifests.into_values().collect())
}

pub(crate) fn resolve_mashup_selection(
    config: &neoism_backend::config::Config,
) -> Result<
    Option<neoism_backend::config::mashup::EditorPluginSelection>,
    PluginManagerError,
> {
    neoism_backend::config::mashup::resolve_editor_plugin_selection(
        config.appearance.mashup_pack.as_deref(),
        &neoism_backend::config::mashup::load_mashup_packs(),
        &config.plugins.mashup_overrides,
    )
    .map_err(|error| PluginManagerError::MashupSelection(error.to_string()))
}

fn select_manifests(
    manifests: Vec<DiscoveredManifest>,
    globally_disabled: &BTreeSet<String>,
    selection: Option<&neoism_backend::config::mashup::EditorPluginSelection>,
) -> Result<(Vec<DiscoveredManifest>, BTreeSet<String>), PluginManagerError> {
    use neoism_backend::config::mashup::EditorPluginMode;

    let Some(selection) = selection else {
        return Ok((
            manifests
                .into_iter()
                .filter(|plugin| !globally_disabled.contains(&plugin.manifest.id))
                .collect(),
            BTreeSet::new(),
        ));
    };
    let by_id = manifests
        .into_iter()
        .map(|plugin| (plugin.manifest.id.clone(), plugin))
        .collect::<BTreeMap<_, _>>();
    let pack_disabled = selection.disabled.iter().cloned().collect::<BTreeSet<_>>();
    let mut keep = BTreeSet::new();
    match selection.mode {
        EditorPluginMode::Overlay => {
            keep.extend(
                by_id
                    .keys()
                    .filter(|id| {
                        !globally_disabled.contains(*id) && !pack_disabled.contains(*id)
                    })
                    .cloned(),
            );
        }
        EditorPluginMode::Only => {
            fn collect(
                id: &str,
                by_id: &BTreeMap<String, DiscoveredManifest>,
                vetoed: &BTreeSet<String>,
                keep: &mut BTreeSet<String>,
            ) {
                if vetoed.contains(id) || !keep.insert(id.to_string()) {
                    return;
                }
                let Some(plugin) = by_id.get(id) else { return };
                for dependency in &plugin.manifest.dependencies {
                    collect(dependency, by_id, vetoed, keep);
                }
            }
            let vetoed = globally_disabled
                .union(&pack_disabled)
                .cloned()
                .collect::<BTreeSet<_>>();
            for root in &selection.enabled {
                if vetoed.contains(root) {
                    continue;
                }
                if !by_id.contains_key(root) {
                    return Err(PluginManagerError::MashupSelection(format!(
                        "enabled plugin `{root}` is not installed"
                    )));
                }
                collect(root, &by_id, &vetoed, &mut keep);
            }
        }
    }
    let mashup_excluded = by_id
        .keys()
        .filter(|id| !keep.contains(*id) && !globally_disabled.contains(*id))
        .cloned()
        .collect();
    Ok((
        by_id
            .into_iter()
            .filter_map(|(id, plugin)| keep.contains(&id).then_some(plugin))
            .collect(),
        mashup_excluded,
    ))
}

pub(crate) fn overlay_snapshot(
    plugins: &PluginSnapshot,
    user: Option<&PluginSnapshot>,
) -> PluginSnapshot {
    let mut combined = plugins.clone();
    let Some(user) = user else { return combined };
    combined.api_version = user.api_version.max(combined.api_version);
    combined.config_patch = user.config_patch.clone();
    combined.styles.overlay_layer(&user.styles);
    for command in &user.commands {
        combined.commands.retain(|current| current.id != command.id);
        combined.commands.push(command.clone());
    }
    combined.keymaps.extend(user.keymaps.clone());
    combined.editor_options.extend(user.editor_options.clone());
    combined.lazy_keys.extend(user.lazy_keys.clone());
    combined.surface_layout.overlay(&user.surface_layout);
    combined.autocmds.extend(user.autocmds.clone());
    for panel in &user.panels {
        combined.panels.retain(|current| current.id != panel.id);
        combined.panels.push(panel.clone());
    }
    for contribution in &user.contributions {
        combined
            .contributions
            .retain(|current| current.id != contribution.id);
        combined.contributions.push(contribution.clone());
    }
    for view in &user.views {
        combined.views.retain(|current| current.id != view.id);
        combined.views.push(view.clone());
    }
    for contribution in &user.platform {
        combined.platform.retain(|current| {
            current.contribution.id() != contribution.contribution.id()
        });
        combined.platform.push(contribution.clone());
    }
    combined
}

fn callback_owner(callback: &str) -> Option<&str> {
    callback
        .strip_prefix("lua:")?
        .split_once('@')
        .map(|(owner, _)| owner)
}

fn validate_candidate_snapshot(
    active: &BTreeMap<String, PluginRuntime>,
    candidates: &BTreeMap<String, PluginRuntimeCandidate>,
) -> Result<(), PluginManagerError> {
    let snapshots = active
        .iter()
        .map(|(id, runtime)| (id.as_str(), runtime.snapshot()))
        .chain(
            candidates
                .iter()
                .map(|(id, candidate)| (id.as_str(), candidate.snapshot())),
        );
    validate_snapshot_ids(snapshots)
}

fn combined_snapshot(
    active: &BTreeMap<String, PluginRuntime>,
) -> Result<PluginSnapshot, PluginManagerError> {
    validate_snapshot_ids(
        active
            .iter()
            .map(|(id, runtime)| (id.as_str(), runtime.snapshot())),
    )?;
    let mut combined = PluginSnapshot::empty();
    for runtime in active.values() {
        let snapshot = runtime.snapshot();
        for (selector, style) in &snapshot.styles.0 {
            combined.styles.0.entry(selector.clone()).or_default().overlay(Some(style));
        }
        combined.commands.extend(snapshot.commands.clone());
        combined.keymaps.extend(snapshot.keymaps.clone());
        combined
            .editor_options
            .extend(snapshot.editor_options.clone());
        combined.surface_layout.overlay(&snapshot.surface_layout);
        combined.autocmds.extend(snapshot.autocmds.clone());
        combined.panels.extend(snapshot.panels.clone());
        combined
            .contributions
            .extend(snapshot.contributions.clone());
        combined.views.extend(snapshot.views.clone());
        combined.platform.extend(snapshot.platform.clone());
    }
    Ok(combined)
}

fn validate_snapshot_ids<'a>(
    snapshots: impl Iterator<Item = (&'a str, &'a PluginSnapshot)>,
) -> Result<(), PluginManagerError> {
    let mut commands = BTreeMap::<&str, &str>::new();
    let mut panels = BTreeMap::<&str, &str>::new();
    let mut contributions = BTreeMap::<&str, &str>::new();
    let mut views = BTreeMap::<&str, &str>::new();
    let mut platform = BTreeMap::<&str, &str>::new();
    for (plugin, snapshot) in snapshots {
        validate_surface_layout(plugin, snapshot)?;
        for command in &snapshot.commands {
            check_id("command", &command.id, plugin, &mut commands)?;
        }
        for panel in &snapshot.panels {
            check_id("panel", &panel.id, plugin, &mut panels)?;
        }
        for contribution in &snapshot.contributions {
            check_id(
                "UI contribution",
                &contribution.id,
                plugin,
                &mut contributions,
            )?;
        }
        for view in &snapshot.views {
            check_id("retained view", &view.id, plugin, &mut views)?;
            view.validate().map_err(|message| {
                PluginManagerError::InvalidContribution {
                    plugin: plugin.to_string(),
                    message,
                }
            })?;
        }
        for contribution in &snapshot.platform {
            check_id(
                "platform contribution",
                contribution.contribution.id(),
                plugin,
                &mut platform,
            )?;
            contribution.contribution.validate().map_err(|message| {
                PluginManagerError::InvalidContribution {
                    plugin: plugin.to_string(),
                    message,
                }
            })?;
            if contribution.owner.plugin_id != plugin {
                return Err(PluginManagerError::InvalidContribution {
                    plugin: plugin.to_string(),
                    message: "platform contribution owner does not match its package"
                        .into(),
                });
            }
        }
    }
    Ok(())
}

fn validate_surface_layout(
    plugin: &str,
    snapshot: &PluginSnapshot,
) -> Result<(), PluginManagerError> {
    snapshot.surface_layout.validate().map_err(|message| {
        PluginManagerError::InvalidSurfaceLayout {
            plugin: plugin.into(),
            message,
        }
    })
}

fn check_id<'a>(
    kind: &'static str,
    id: &'a str,
    plugin: &'a str,
    seen: &mut BTreeMap<&'a str, &'a str>,
) -> Result<(), PluginManagerError> {
    if let Some(first) = seen.insert(id, plugin) {
        return Err(PluginManagerError::Conflict {
            kind,
            id: id.to_string(),
            first: first.to_string(),
            second: plugin.to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn personal_style_overlay_is_fieldwise_and_parent_priority_is_preserved() {
        use neoism_lua::{BackgroundEffect, StylePatch};
        let mut plugins = PluginSnapshot::empty();
        plugins.styles.insert("composer.agent", StylePatch {
            background: Some("#000000".into()),
            foreground: Some("#aaaaaa".into()),
            background_effects: Some(vec![BackgroundEffect::Stars(Default::default())]),
            ..Default::default()
        });
        let mut user = PluginSnapshot::empty();
        user.styles.insert("composer", StylePatch {
            foreground: Some("#ffffff".into()),
            ..Default::default()
        });
        let inherited = overlay_snapshot(&plugins, Some(&user)).styles.resolve("composer.agent");
        assert_eq!(inherited.background.as_deref(), Some("#000000"));
        assert_eq!(inherited.foreground.as_deref(), Some("#ffffff"));
        assert_eq!(inherited.background_effects.unwrap().len(), 1);
        user.styles.insert("composer.agent", StylePatch {
            background_effects: Some(vec![]),
            ..Default::default()
        });
        let cleared = overlay_snapshot(&plugins, Some(&user)).styles.resolve("composer.agent");
        assert_eq!(cleared.background.as_deref(), Some("#000000"));
        assert_eq!(cleared.background_effects, Some(vec![]));
    }

    #[test]
    fn command_alias_and_dynamic_completion_remain_exact_owner_scoped() {
        let root = std::env::temp_dir()
            .join(format!("neoism-desktop-command-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("init.lua"), r#"
            neoism.command.register('build', function() return { ok = true } end, {
                aliases = { 'b' }, completions = { 'debug' },
                complete = function(event) return { event.payload.prefix .. '-dynamic' } end,
            })
        "#).unwrap();
        let manifest = neoism_lua::PluginManifest {
            id: "dev.neoism.desktop-command".into(),
            ..Default::default()
        };
        let runtime = neoism_lua::PluginRuntime::build_candidate(
            &root,
            &manifest,
            neoism_lua::PluginRevision("r1".into()),
            Arc::new(neoism_lua::InertHost),
        )
        .unwrap()
        .activate();
        let owner = runtime.owner().clone();
        let host = Arc::new(neoism_lua::QueuedHost::default());
        let mut manager = LuaPluginManager::empty(&root, host);
        manager.active.insert(manifest.id.clone(), runtime);

        let (resolved_owner, command) = manager.resolve_command("b").unwrap();
        assert_eq!(resolved_owner, owner);
        assert_eq!(command.id, "build");
        assert_eq!(
            manager.complete_command(&owner, "b", "d").unwrap(),
            vec!["d-dynamic", "debug"]
        );
        let stale = neoism_lua::PluginOwner {
            plugin_id: owner.plugin_id.clone(),
            revision: neoism_lua::PluginRevision("stale".into()),
        };
        assert!(manager.complete_command(&stale, "b", "d").is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn startup_retains_eager_plugin_failure_for_inventory() {
        let root = std::env::temp_dir().join(format!(
            "neoism-desktop-plugin-startup-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let plugin_root = root.join("plugins/dev.neoism.startup-failure");
        std::fs::create_dir_all(&plugin_root).unwrap();
        std::fs::write(
            plugin_root.join("neoism-plugin.json"),
            r#"{
                "id": "dev.neoism.startup-failure",
                "name": "Startup failure",
                "version": "1.0.0",
                "apiVersion": 1,
                "editor": { "entrypoint": "init.lua" },
                "triggers": []
            }"#,
        )
        .unwrap();
        std::fs::write(
            plugin_root.join("init.lua"),
            "error('expected startup failure')",
        )
        .unwrap();

        let policy = neoism_backend::config::PluginPreferences::default();
        let (manager, error) = LuaPluginManager::discover_for_startup(
            &root,
            Arc::new(neoism_lua::QueuedHost::default()),
            &policy,
            None,
        );
        assert!(error.is_some());
        assert_eq!(manager.failures().count(), 1);
        let inventory = scan_plugin_inventory(&manager, &policy);
        let row = &inventory.entries["dev.neoism.startup-failure"];
        assert_eq!(row.lifecycle, LuaPluginLifecycle::Failed);
        assert!(row.status_text.contains("expected startup failure"));
        std::fs::remove_dir_all(root).unwrap();
    }

    fn write_test_plugin(root: &Path, id: &str, dependencies: &[&str]) {
        let plugin_root = root.join("plugins").join(id);
        std::fs::create_dir_all(&plugin_root).unwrap();
        std::fs::write(
            plugin_root.join("neoism-plugin.json"),
            serde_json::json!({
                "id": id,
                "name": id,
                "version": "1.0.0",
                "apiVersion": 1,
                "editor": { "entrypoint": "init.lua" },
                "dependencies": dependencies,
                "triggers": []
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(plugin_root.join("init.lua"), "").unwrap();
    }

    #[test]
    fn only_selection_activates_roots_and_dependency_closure() {
        let root = std::env::temp_dir()
            .join(format!("neoism-desktop-mashup-only-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write_test_plugin(&root, "dev.neoism.root", &["dev.neoism.dep"]);
        write_test_plugin(&root, "dev.neoism.dep", &[]);
        write_test_plugin(&root, "dev.neoism.unrelated", &[]);
        let selection = neoism_backend::config::mashup::EditorPluginSelection {
            mode: neoism_backend::config::mashup::EditorPluginMode::Only,
            enabled: vec!["dev.neoism.root".into()],
            disabled: Vec::new(),
        };
        let manager = LuaPluginManager::discover(
            &root,
            Arc::new(neoism_lua::QueuedHost::default()),
            &Default::default(),
            Some(&selection),
        )
        .unwrap();
        assert_eq!(
            manager.active_ids().collect::<Vec<_>>(),
            ["dev.neoism.dep", "dev.neoism.root"]
        );
        assert!(manager.mashup_excluded.contains("dev.neoism.unrelated"));
        assert!(!manager
            .snapshot()
            .commands
            .iter()
            .any(|command| command.id.contains("unrelated")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn global_disabled_dependency_is_a_hard_veto_and_graph_error() {
        let root = std::env::temp_dir()
            .join(format!("neoism-desktop-mashup-veto-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write_test_plugin(&root, "dev.neoism.root", &["dev.neoism.dep"]);
        write_test_plugin(&root, "dev.neoism.dep", &[]);
        let selection = neoism_backend::config::mashup::EditorPluginSelection {
            mode: neoism_backend::config::mashup::EditorPluginMode::Only,
            enabled: vec!["dev.neoism.root".into()],
            disabled: Vec::new(),
        };
        let policy = neoism_backend::config::PluginPreferences {
            disabled: vec!["dev.neoism.dep".into()],
            ..Default::default()
        };
        let result = LuaPluginManager::discover(
            &root,
            Arc::new(neoism_lua::QueuedHost::default()),
            &policy,
            Some(&selection),
        );
        assert!(result.is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
