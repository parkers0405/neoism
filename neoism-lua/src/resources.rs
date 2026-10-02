//! Language-neutral, exact-owner editor resources. These are host-side data
//! models: Lua creates mutations, while Rust relocates anchors and publishes
//! immutable snapshots for rendering.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{DocumentHandle, PluginOwner, PluginStateRequest, PluginStateScope, TextPosition};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PluginResourceId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PluginNamespaceHandle(pub PluginResourceId);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorBias {
    Before,
    After,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginAnchor {
    pub id: PluginResourceId,
    pub document: DocumentHandle,
    /// Resolved UTF-8 byte offset. A collaborative host may retain a native
    /// sticky index internally and update this projection on publication.
    pub offset: usize,
    pub bias: AnchorBias,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginDecoration {
    pub id: PluginResourceId,
    pub start: PluginResourceId,
    pub end: PluginResourceId,
    pub layer: DecorationLayer,
    #[serde(default)]
    pub class: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub severity: Option<PluginDiagnosticSeverity>,
    #[serde(default)]
    pub style: DecorationStyle,
    #[serde(default)]
    pub related_information: Vec<PluginDiagnosticRelatedInformation>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub actions: Vec<PluginDiagnosticAction>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct DecorationStyle {
    pub foreground: Option<String>,
    pub background: Option<String>,
    pub underline: Option<String>,
    pub icon: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginDiagnosticRelatedInformation {
    pub message: String,
    #[serde(default)]
    pub document: Option<DocumentHandle>,
    #[serde(default)]
    pub position: Option<TextPosition>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginDiagnosticAction {
    pub title: String,
    pub command: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecorationLayer {
    Highlight,
    GutterSign,
    VirtualText,
    VirtualLine,
    InlineWidget,
    CodeLens,
    Fold,
    Conceal,
    Diagnostic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginDiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

/// Fully resolved immutable data consumed by editor layout/rendering. It has no
/// callback or VM reference, so frame and hit-test paths cannot enter Lua.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginDecorationSnapshot {
    pub revision: u64,
    pub decorations: Vec<ResolvedPluginDecoration>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedPluginDecoration {
    pub id: PluginResourceId,
    pub owner: PluginOwner,
    pub document: DocumentHandle,
    pub start: usize,
    pub end: usize,
    pub start_position: TextPosition,
    pub end_position: TextPosition,
    pub layer: DecorationLayer,
    pub class: Option<String>,
    pub text: Option<String>,
    pub severity: Option<PluginDiagnosticSeverity>,
    pub style: DecorationStyle,
    pub resolved_style: ResolvedDecorationStyle,
    pub related_information: Vec<PluginDiagnosticRelatedInformation>,
    pub tags: Vec<String>,
    pub actions: Vec<PluginDiagnosticAction>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ResolvedDecorationStyle {
    pub foreground: Option<[u8; 4]>,
    pub background: Option<[u8; 4]>,
    pub underline: Option<[u8; 4]>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginNamespaceRequest {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginAnchorRequest {
    pub namespace: String,
    pub document: DocumentHandle,
    pub expected_revision: u64,
    pub position: TextPosition,
    pub bias: AnchorBias,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginDecorationRequest {
    pub namespace: String,
    #[serde(default)]
    pub resource: Option<String>,
    pub start: String,
    pub end: String,
    pub layer: DecorationLayer,
    #[serde(default)]
    pub class: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub severity: Option<PluginDiagnosticSeverity>,
    #[serde(default)]
    pub style: DecorationStyle,
    #[serde(default)]
    pub related_information: Vec<PluginDiagnosticRelatedInformation>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub actions: Vec<PluginDiagnosticAction>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginResourceTargetRequest {
    pub namespace: String,
    #[serde(default)]
    pub resource: Option<String>,
}

#[derive(Default)]
struct OwnerResources {
    anchors: BTreeMap<PluginResourceId, PluginAnchor>,
    decorations: BTreeMap<PluginResourceId, PluginDecoration>,
}

/// Authoritative exact-generation resource registry. Calling `remove_owner`
/// tears down every namespace, anchor and decoration for only that revision.
#[derive(Default)]
pub struct PluginResourceRegistry {
    next_id: u64,
    revision: u64,
    namespaces: BTreeMap<PluginNamespaceHandle, (PluginOwner, OwnerResources)>,
}

impl PluginResourceRegistry {
    pub fn owns_resource(&self, owner: &PluginOwner, resource: PluginResourceId) -> bool {
        self.namespaces.values().any(|(candidate, resources)| {
            candidate == owner
                && (resources.anchors.contains_key(&resource) || resources.decorations.contains_key(&resource))
        })
    }

    fn allocate(&mut self) -> PluginResourceId {
        self.next_id = self.next_id.saturating_add(1);
        PluginResourceId(self.next_id)
    }

    pub fn create_namespace(
        &mut self,
        owner: &PluginOwner,
    ) -> Result<PluginNamespaceHandle, String> {
        validate_owner(owner)?;
        let handle = PluginNamespaceHandle(self.allocate());
        self.namespaces.insert(handle, (owner.clone(), OwnerResources::default()));
        self.revision = self.revision.saturating_add(1);
        Ok(handle)
    }

    pub fn create_anchor(
        &mut self,
        owner: &PluginOwner,
        namespace: PluginNamespaceHandle,
        document: DocumentHandle,
        offset: usize,
        bias: AnchorBias,
    ) -> Result<PluginResourceId, String> {
        validate_owner(owner)?;
        self.require_namespace(owner, namespace)?;
        let id = self.allocate();
        self.namespaces.get_mut(&namespace).expect("namespace checked").1.anchors.insert(
            id,
            PluginAnchor { id, document, offset, bias },
        );
        self.revision = self.revision.saturating_add(1);
        Ok(id)
    }

    pub fn create_decoration(
        &mut self,
        owner: &PluginOwner,
        namespace: PluginNamespaceHandle,
        mut decoration: PluginDecoration,
    ) -> Result<PluginResourceId, String> {
        validate_owner(owner)?;
        let resources = &self.require_namespace(owner, namespace)?.1;
        let start = resources.anchors.get(&decoration.start)
            .ok_or_else(|| "decoration start anchor is stale or belongs to another owner".to_string())?;
        let end = resources.anchors.get(&decoration.end)
            .ok_or_else(|| "decoration end anchor is stale or belongs to another owner".to_string())?;
        if start.document != end.document {
            return Err("decoration anchors target different documents".into());
        }
        let id = self.allocate();
        decoration.id = id;
        self.namespaces.get_mut(&namespace).expect("namespace checked").1.decorations.insert(id, decoration);
        self.revision = self.revision.saturating_add(1);
        Ok(id)
    }

    pub fn set_anchor_offset(
        &mut self,
        owner: &PluginOwner,
        namespace: PluginNamespaceHandle,
        anchor: PluginResourceId,
        offset: usize,
    ) -> Result<(), String> {
        validate_owner(owner)?;
        let resources = &mut self.namespaces.get_mut(&namespace)
            .filter(|(candidate, _)| candidate == owner)
            .ok_or_else(|| "plugin namespace is stale or belongs to another owner revision".to_string())?.1;
        let anchor = resources.anchors.get_mut(&anchor)
            .ok_or_else(|| "plugin anchor is stale or belongs to another namespace".to_string())?;
        if anchor.offset != offset {
            anchor.offset = offset;
            self.revision = self.revision.saturating_add(1);
        }
        Ok(())
    }

    pub fn remove_resource(
        &mut self,
        owner: &PluginOwner,
        namespace: PluginNamespaceHandle,
        resource: PluginResourceId,
    ) -> Result<bool, String> {
        validate_owner(owner)?;
        let resources = &mut self.namespaces.get_mut(&namespace)
            .filter(|(candidate, _)| candidate == owner)
            .ok_or_else(|| "plugin namespace is stale or belongs to another owner revision".to_string())?.1;
        let mut removed = resources.decorations.remove(&resource).is_some();
        if resources.anchors.remove(&resource).is_some() {
            resources.decorations.retain(|_, decoration| {
                decoration.start != resource && decoration.end != resource
            });
            removed = true;
        }
        if removed {
            self.revision = self.revision.saturating_add(1);
        }
        Ok(removed)
    }

    pub fn clear_namespace(
        &mut self,
        owner: &PluginOwner,
        namespace: PluginNamespaceHandle,
    ) -> Result<(), String> {
        validate_owner(owner)?;
        let resources = &mut self.namespaces.get_mut(&namespace)
            .filter(|(candidate, _)| candidate == owner)
            .ok_or_else(|| "plugin namespace is stale or belongs to another owner revision".to_string())?.1;
        resources.anchors.clear();
        resources.decorations.clear();
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }

    pub fn clear_decorations(
        &mut self,
        owner: &PluginOwner,
        namespace: PluginNamespaceHandle,
        layer: Option<DecorationLayer>,
    ) -> Result<(), String> {
        validate_owner(owner)?;
        let resources = &mut self.namespaces.get_mut(&namespace)
            .filter(|(candidate, _)| candidate == owner)
            .ok_or_else(|| "plugin namespace is stale or belongs to another owner revision".to_string())?.1;
        if let Some(layer) = layer {
            resources.decorations.retain(|_, decoration| decoration.layer != layer);
        } else {
            resources.decorations.clear();
        }
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }

    pub fn remove_namespace(
        &mut self,
        owner: &PluginOwner,
        namespace: PluginNamespaceHandle,
    ) -> Result<(), String> {
        validate_owner(owner)?;
        self.require_namespace(owner, namespace)?;
        self.namespaces.remove(&namespace);
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }

    pub fn apply_text_edit(
        &mut self,
        document: &DocumentHandle,
        old_start: usize,
        old_end: usize,
        inserted_len: usize,
    ) -> Result<(), String> {
        if old_start > old_end {
            return Err("anchor relocation edit is inverted".into());
        }
        for (_, resources) in self.namespaces.values_mut() {
            for anchor in resources.anchors.values_mut().filter(|anchor| &anchor.document == document) {
                anchor.offset = relocate_offset(anchor.offset, anchor.bias, old_start, old_end, inserted_len);
            }
        }
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }

    pub fn remove_owner(&mut self, owner: &PluginOwner) -> bool {
        let old_len = self.namespaces.len();
        self.namespaces.retain(|_, (candidate, _)| candidate != owner);
        let removed = self.namespaces.len() != old_len;
        if removed {
            self.revision = self.revision.saturating_add(1);
        }
        removed
    }

    pub fn snapshot(&self) -> PluginDecorationSnapshot {
        self.snapshot_for_document(None, "")
    }

    pub fn snapshot_for_document(
        &self,
        document: Option<&DocumentHandle>,
        text: &str,
    ) -> PluginDecorationSnapshot {
        let mut decorations = Vec::new();
        for (owner, resources) in self.namespaces.values() {
            for decoration in resources.decorations.values() {
                let (Some(start), Some(end)) = (
                    resources.anchors.get(&decoration.start),
                    resources.anchors.get(&decoration.end),
                ) else { continue };
                if document.is_some_and(|document| document != &start.document) {
                    continue;
                }
                let start_offset = start.offset.min(end.offset);
                let end_offset = start.offset.max(end.offset);
                decorations.push(ResolvedPluginDecoration {
                    id: decoration.id,
                    owner: owner.clone(),
                    document: start.document.clone(),
                    start: start_offset,
                    end: end_offset,
                    start_position: position_for_offset(text, start_offset),
                    end_position: position_for_offset(text, end_offset),
                    layer: decoration.layer,
                    class: decoration.class.clone(),
                    text: decoration.text.clone(),
                    severity: decoration.severity,
                    style: decoration.style.clone(),
                    resolved_style: ResolvedDecorationStyle {
                        foreground: decoration.style.foreground.as_deref().and_then(parse_color),
                        background: decoration.style.background.as_deref().and_then(parse_color),
                        underline: decoration.style.underline.as_deref().and_then(parse_color),
                    },
                    related_information: decoration.related_information.clone(),
                    tags: decoration.tags.clone(),
                    actions: decoration.actions.clone(),
                });
            }
        }
        PluginDecorationSnapshot { revision: self.revision, decorations }
    }

    fn require_namespace(
        &self,
        owner: &PluginOwner,
        namespace: PluginNamespaceHandle,
    ) -> Result<&(PluginOwner, OwnerResources), String> {
        self.namespaces.get(&namespace)
            .filter(|(candidate, _)| candidate == owner)
            .ok_or_else(|| "plugin namespace is stale or belongs to another owner revision".into())
    }
}

fn position_for_offset(text: &str, offset: usize) -> TextPosition {
    let offset = offset.min(text.len());
    let mut line = 0u32;
    let mut line_start = 0usize;
    for (index, byte) in text.bytes().enumerate().take(offset) {
        if byte == b'\n' {
            line = line.saturating_add(1);
            line_start = index + 1;
        }
    }
    TextPosition { line, character: offset.saturating_sub(line_start) as u32 }
}

fn parse_color(value: &str) -> Option<[u8; 4]> {
    let hex = value.strip_prefix('#')?;
    let parse = |range: std::ops::Range<usize>| u8::from_str_radix(hex.get(range)?, 16).ok();
    match hex.len() {
        6 => Some([parse(0..2)?, parse(2..4)?, parse(4..6)?, 255]),
        8 => Some([parse(0..2)?, parse(2..4)?, parse(4..6)?, parse(6..8)?]),
        _ => None,
    }
}

fn validate_owner(owner: &PluginOwner) -> Result<(), String> {
    if owner.plugin_id.is_empty() || owner.revision.0.is_empty() {
        Err("plugin resource is missing an exact owner revision".into())
    } else {
        Ok(())
    }
}

fn relocate_offset(offset: usize, bias: AnchorBias, start: usize, end: usize, inserted_len: usize) -> usize {
    if offset < start {
        return offset;
    }
    if offset > end {
        return offset.saturating_sub(end - start).saturating_add(inserted_len);
    }
    if start == end {
        return match bias {
            AnchorBias::Before => start,
            AnchorBias::After => start.saturating_add(inserted_len),
        };
    }
    if offset == end {
        return start.saturating_add(inserted_len);
    }
    match bias {
        AnchorBias::Before => start,
        AnchorBias::After => start.saturating_add(inserted_len),
    }
}

/// Exact-generation state storage. Target identities remain opaque strings;
/// the host validates them before calling `set` for non-plugin scopes.
const MAX_STATE_KEY_BYTES: usize = 256;
const MAX_STATE_VALUE_BYTES: usize = 256 * 1024;
const MAX_STATE_ENTRIES_PER_OWNER: usize = 1_024;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PluginScopedStateRegistry {
    values: BTreeMap<(PluginOwner, PluginStateScope, Option<String>, String), serde_json::Value>,
    /// Durable values intentionally omit the revision so a validated new
    /// generation of the same plugin can adopt them after activation.
    persistent: BTreeMap<(String, PluginStateScope, Option<String>, String), serde_json::Value>,
    #[serde(skip)]
    staged_persistent: BTreeMap<(PluginOwner, PluginStateScope, Option<String>, String), serde_json::Value>,
    #[serde(skip)]
    staged_persistent_removals: BTreeSet<(PluginOwner, PluginStateScope, Option<String>, String)>,
    #[serde(skip)]
    active_persistent_owners: BTreeSet<PluginOwner>,
}

impl PluginScopedStateRegistry {
    pub fn fork_candidate(&self) -> Self {
        Self {
            values: BTreeMap::new(),
            persistent: self.persistent.clone(),
            staged_persistent: BTreeMap::new(),
            staged_persistent_removals: BTreeSet::new(),
            active_persistent_owners: BTreeSet::new(),
        }
    }

    pub fn activate_owner(&mut self, owner: &PluginOwner) -> bool {
        self.active_persistent_owners.insert(owner.clone());
        let staged = self.staged_persistent.iter()
            .filter(|((candidate, _, _, _), _)| candidate == owner)
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>();
        let changed = !staged.is_empty();
        let removals = self.staged_persistent_removals.iter()
            .filter(|(candidate, _, _, _)| candidate == owner)
            .cloned().collect::<Vec<_>>();
        let changed = changed || !removals.is_empty();
        for (_, scope, target, key) in removals {
            self.staged_persistent_removals.remove(&(owner.clone(), scope, target.clone(), key.clone()));
            self.persistent.remove(&(owner.plugin_id.clone(), scope, target, key));
        }
        for ((_, scope, target, key), value) in staged {
            self.staged_persistent.remove(&(owner.clone(), scope, target.clone(), key.clone()));
            self.persistent.insert((owner.plugin_id.clone(), scope, target, key), value);
        }
        changed
    }

    pub fn persistent_snapshot(&self) -> serde_json::Value {
        let entries = self.persistent.iter().map(|((plugin_id, scope, target, key), value)| {
            serde_json::json!({
                "pluginId": plugin_id,
                "scope": scope,
                "target": target,
                "key": key,
                "value": value,
            })
        }).collect::<Vec<_>>();
        serde_json::json!({ "version": 1, "entries": entries })
    }

    pub fn restore_persistent(&mut self, snapshot: &serde_json::Value) -> Result<(), String> {
        let entries = snapshot.get("entries").and_then(serde_json::Value::as_array)
            .ok_or_else(|| "persistent plugin state is missing entries".to_string())?;
        if entries.len() > 16_384 {
            return Err("persistent plugin state exceeds the global entry budget".into());
        }
        let mut restored = BTreeMap::new();
        for entry in entries {
            let plugin_id = entry.get("pluginId").and_then(serde_json::Value::as_str)
                .ok_or_else(|| "persistent plugin state has an invalid plugin id".to_string())?;
            let scope: PluginStateScope = serde_json::from_value(entry.get("scope").cloned().unwrap_or_default())
                .map_err(|error| error.to_string())?;
            if !matches!(scope, PluginStateScope::Plugin | PluginStateScope::Workspace) {
                return Err("persistent state contains a disallowed scope".into());
            }
            let target = entry.get("target").and_then(serde_json::Value::as_str).map(str::to_owned);
            if (scope == PluginStateScope::Plugin && target.is_some())
                || (scope == PluginStateScope::Workspace && target.as_deref().is_none_or(str::is_empty))
            {
                return Err("persistent state contains an invalid scope target".into());
            }
            let key = entry.get("key").and_then(serde_json::Value::as_str)
                .ok_or_else(|| "persistent plugin state has an invalid key".to_string())?;
            let value = entry.get("value").cloned().unwrap_or(serde_json::Value::Null);
            if plugin_id.is_empty() || key.is_empty() || key.len() > MAX_STATE_KEY_BYTES
                || serde_json::to_vec(&value).map_err(|error| error.to_string())?.len() > MAX_STATE_VALUE_BYTES
            {
                return Err("persistent plugin state violates size constraints".into());
            }
            restored.insert((plugin_id.to_owned(), scope, target, key.to_owned()), value);
            if restored.keys().filter(|(candidate, _, _, _)| candidate == plugin_id).count()
                > MAX_STATE_ENTRIES_PER_OWNER
            {
                return Err("persistent plugin state exceeds the per-owner entry budget".into());
            }
        }
        self.persistent = restored;
        Ok(())
    }

    pub fn set(&mut self, owner: &PluginOwner, request: PluginStateRequest) -> Result<(), String> {
        validate_owner(owner)?;
        if request.key.trim().is_empty() {
            return Err("plugin state key cannot be empty".into());
        }
        if request.key.len() > MAX_STATE_KEY_BYTES {
            return Err("plugin state key exceeds 256 bytes".into());
        }
        if serde_json::to_vec(&request.value).map_err(|error| error.to_string())?.len() > MAX_STATE_VALUE_BYTES {
            return Err("plugin state value exceeds 256 KiB".into());
        }
        if request.scope != PluginStateScope::Plugin && request.target.as_deref().is_none_or(str::is_empty) {
            return Err("scoped plugin state requires an opaque target handle".into());
        }
        if request.scope == PluginStateScope::Plugin && request.target.is_some() {
            return Err("plugin-scoped state does not accept a target handle".into());
        }
        if request.persistent && !matches!(request.scope, PluginStateScope::Plugin | PluginStateScope::Workspace) {
            return Err("persistent state is restricted to plugin and workspace scopes".into());
        }
        let owner_count = self.values.keys().filter(|(candidate, _, _, _)| candidate == owner).count()
            + self.persistent.keys().filter(|(plugin_id, _, _, _)| plugin_id == &owner.plugin_id).count()
            + self.staged_persistent.keys().filter(|(candidate, _, _, _)| candidate == owner).count();
        let exists = if request.persistent {
            self.persistent.contains_key(&(owner.plugin_id.clone(), request.scope, request.target.clone(), request.key.clone()))
        } else {
            self.values.contains_key(&(owner.clone(), request.scope, request.target.clone(), request.key.clone()))
        };
        if !exists && owner_count >= MAX_STATE_ENTRIES_PER_OWNER {
            return Err("plugin state entry budget exceeded".into());
        }
        if request.persistent {
            if self.active_persistent_owners.contains(owner) {
                self.persistent.insert((owner.plugin_id.clone(), request.scope, request.target, request.key), request.value);
            } else {
                self.staged_persistent_removals.remove(&(owner.clone(), request.scope, request.target.clone(), request.key.clone()));
                self.staged_persistent.insert((owner.clone(), request.scope, request.target, request.key), request.value);
            }
        } else {
            self.values.insert((owner.clone(), request.scope, request.target, request.key), request.value);
        }
        Ok(())
    }

    pub fn get(
        &self,
        owner: &PluginOwner,
        scope: PluginStateScope,
        target: Option<&str>,
        key: &str,
        persistent: bool,
    ) -> Option<&serde_json::Value> {
        if persistent {
            if self.staged_persistent_removals.contains(&(owner.clone(), scope, target.map(str::to_owned), key.to_owned())) {
                return None;
            }
            self.staged_persistent
                .get(&(owner.clone(), scope, target.map(str::to_owned), key.to_owned()))
                .or_else(|| self.persistent.get(&(owner.plugin_id.clone(), scope, target.map(str::to_owned), key.to_owned())))
        } else {
            self.values.get(&(owner.clone(), scope, target.map(str::to_owned), key.to_owned()))
        }
    }

    pub fn remove_owner(&mut self, owner: &PluginOwner) {
        self.values.retain(|(candidate, _, _, _), _| candidate != owner);
        self.staged_persistent.retain(|(candidate, _, _, _), _| candidate != owner);
        self.staged_persistent_removals.retain(|(candidate, _, _, _)| candidate != owner);
        self.active_persistent_owners.remove(owner);
    }

    pub fn remove(&mut self, owner: &PluginOwner, scope: PluginStateScope, target: Option<&str>, key: &str, persistent: bool) -> bool {
        if persistent {
            let staged_key = (owner.clone(), scope, target.map(str::to_owned), key.to_owned());
            let removed_staged = self.staged_persistent.remove(&staged_key).is_some();
            if self.active_persistent_owners.contains(owner) {
                self.persistent.remove(&(owner.plugin_id.clone(), scope, target.map(str::to_owned), key.to_owned())).is_some() || removed_staged
            } else {
                self.staged_persistent_removals.insert(staged_key);
                true
            }
        } else {
            self.values.remove(&(owner.clone(), scope, target.map(str::to_owned), key.to_owned())).is_some()
        }
    }

    pub fn clear_scope(&mut self, owner: &PluginOwner, scope: PluginStateScope, target: Option<&str>, persistent: bool) {
        if persistent {
            self.staged_persistent.retain(|(candidate, candidate_scope, candidate_target, _), _| {
                candidate != owner || *candidate_scope != scope || candidate_target.as_deref() != target
            });
            if self.active_persistent_owners.contains(owner) {
                self.persistent.retain(|(plugin_id, candidate_scope, candidate_target, _), _| {
                    plugin_id != &owner.plugin_id || *candidate_scope != scope || candidate_target.as_deref() != target
                });
            } else {
                self.staged_persistent_removals.extend(self.persistent.keys().filter_map(|(plugin_id, candidate_scope, candidate_target, key)| {
                    (plugin_id == &owner.plugin_id && *candidate_scope == scope && candidate_target.as_deref() == target)
                        .then(|| (owner.clone(), scope, candidate_target.clone(), key.clone()))
                }));
            }
        } else {
            self.values.retain(|(candidate, candidate_scope, candidate_target, _), _| {
                candidate != owner || *candidate_scope != scope || candidate_target.as_deref() != target
            });
        }
    }

    pub fn list(&self, owner: &PluginOwner, scope: PluginStateScope, target: Option<&str>, persistent: bool) -> BTreeMap<String, serde_json::Value> {
        if persistent {
            let mut values = self.persistent.iter().filter_map(|((plugin_id, candidate_scope, candidate_target, key), value)| {
                (plugin_id == &owner.plugin_id && *candidate_scope == scope && candidate_target.as_deref() == target)
                    .then(|| (key.clone(), value.clone()))
            }).collect::<BTreeMap<_, _>>();
            values.extend(self.staged_persistent.iter().filter_map(|((candidate, candidate_scope, candidate_target, key), value)| {
                (candidate == owner && *candidate_scope == scope && candidate_target.as_deref() == target)
                    .then(|| (key.clone(), value.clone()))
            }));
            values.retain(|key, _| !self.staged_persistent_removals.contains(&(
                owner.clone(), scope, target.map(str::to_owned), key.clone()
            )));
            values
        } else {
            self.values.iter().filter_map(|((candidate, candidate_scope, candidate_target, key), value)| {
                (candidate == owner && *candidate_scope == scope && candidate_target.as_deref() == target)
                    .then(|| (key.clone(), value.clone()))
            }).collect()
        }
    }

    pub fn clear_target(&mut self, scope: PluginStateScope, target: &str) {
        self.values.retain(|(_, candidate_scope, candidate_target, _), _| {
            *candidate_scope != scope || candidate_target.as_deref() != Some(target)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PluginRevision;

    fn owner(revision: &str) -> PluginOwner {
        PluginOwner { plugin_id: "dev.test".into(), revision: PluginRevision(revision.into()) }
    }

    #[test]
    fn anchors_relocate_and_snapshots_are_vm_free_values() {
        let mut registry = PluginResourceRegistry::default();
        let document = DocumentHandle("document:1".into());
        let namespace = registry.create_namespace(&owner("r1")).unwrap();
        let start = registry.create_anchor(&owner("r1"), namespace, document.clone(), 2, AnchorBias::Before).unwrap();
        let end = registry.create_anchor(&owner("r1"), namespace, document.clone(), 5, AnchorBias::After).unwrap();
        registry.create_decoration(&owner("r1"), namespace, PluginDecoration {
            id: PluginResourceId(0), start, end, layer: DecorationLayer::Highlight,
            class: Some("warning".into()), text: None, severity: None,
            style: DecorationStyle::default(), related_information: Vec::new(),
            tags: Vec::new(), actions: Vec::new(),
        }).unwrap();
        registry.apply_text_edit(&document, 1, 4, 6).unwrap();
        let snapshot = registry.snapshot();
        assert_eq!((snapshot.decorations[0].start, snapshot.decorations[0].end), (1, 8));
        assert!(serde_json::to_value(snapshot).unwrap().is_object());
    }

    #[test]
    fn exact_owner_teardown_does_not_touch_other_revisions() {
        let mut registry = PluginResourceRegistry::default();
        let document = DocumentHandle("document:1".into());
        let first = registry.create_namespace(&owner("r1")).unwrap();
        let second = registry.create_namespace(&owner("r2")).unwrap();
        registry.create_anchor(&owner("r1"), first, document.clone(), 0, AnchorBias::Before).unwrap();
        registry.create_anchor(&owner("r2"), second, document, 0, AnchorBias::Before).unwrap();
        assert!(registry.create_anchor(&owner("r2"), first, DocumentHandle("x".into()), 0, AnchorBias::Before).is_err());
        assert!(registry.remove_owner(&owner("r1")));
        assert!(registry.remove_owner(&owner("r2")));
        assert!(!registry.remove_owner(&owner("r1")));
        assert!(registry.create_namespace(&PluginOwner::default()).is_err());
    }

    #[test]
    fn scoped_state_is_exact_owner_and_target_isolated() {
        let mut state = PluginScopedStateRegistry::default();
        state.set(&owner("r1"), PluginStateRequest {
            scope: PluginStateScope::Document,
            target: Some("document:one".into()), persistent: false, key: "mode".into(), value: serde_json::json!(1),
        }).unwrap();
        state.set(&owner("r2"), PluginStateRequest {
            scope: PluginStateScope::Document,
            target: Some("document:one".into()), persistent: false, key: "mode".into(), value: serde_json::json!(2),
        }).unwrap();
        assert_eq!(state.get(&owner("r1"), PluginStateScope::Document, Some("document:one"), "mode", false), Some(&serde_json::json!(1)));
        state.remove_owner(&owner("r1"));
        assert!(state.get(&owner("r1"), PluginStateScope::Document, Some("document:one"), "mode", false).is_none());
        assert_eq!(state.get(&owner("r2"), PluginStateScope::Document, Some("document:one"), "mode", false), Some(&serde_json::json!(2)));
    }

    #[test]
    fn persistent_state_survives_revision_retirement_and_is_policy_bounded() {
        let mut state = PluginScopedStateRegistry::default();
        state.activate_owner(&owner("r1"));
        state.set(&owner("r1"), PluginStateRequest {
            scope: PluginStateScope::Plugin,
            target: None,
            persistent: true,
            key: "theme".into(),
            value: serde_json::json!("night"),
        }).unwrap();
        state.remove_owner(&owner("r1"));
        assert_eq!(
            state.get(&owner("r2"), PluginStateScope::Plugin, None, "theme", true),
            Some(&serde_json::json!("night")),
        );
        let snapshot = state.persistent_snapshot();
        let mut restored = PluginScopedStateRegistry::default();
        restored.restore_persistent(&snapshot).unwrap();
        assert_eq!(restored.list(&owner("r3"), PluginStateScope::Plugin, None, true)["theme"], "night");
        assert!(restored.set(&owner("r3"), PluginStateRequest {
            scope: PluginStateScope::Document,
            target: Some("opaque".into()),
            persistent: true,
            key: "bad".into(),
            value: serde_json::Value::Null,
        }).is_err());
    }

    #[test]
    fn anchor_relocation_stays_stable_under_long_edit_stream_and_reload_teardown() {
        let mut registry = PluginResourceRegistry::default();
        let first_owner = owner("reload-a");
        let namespace = registry.create_namespace(&first_owner).unwrap();
        let document = DocumentHandle("document:stress".into());
        let before = registry.create_anchor(&first_owner, namespace, document.clone(), 4, AnchorBias::Before).unwrap();
        let after = registry.create_anchor(&first_owner, namespace, document.clone(), 4, AnchorBias::After).unwrap();
        registry.create_decoration(&first_owner, namespace, PluginDecoration {
            id: PluginResourceId(0), start: before, end: after, layer: DecorationLayer::Diagnostic,
            class: None, text: Some("stress".into()), severity: Some(PluginDiagnosticSeverity::Warning),
            style: DecorationStyle::default(), related_information: Vec::new(), tags: vec!["test".into()], actions: Vec::new(),
        }).unwrap();
        for _ in 0..1_000 {
            registry.apply_text_edit(&document, 4, 4, "λ".len()).unwrap();
        }
        let snapshot = registry.snapshot_for_document(Some(&document), "xxxx");
        assert_eq!(snapshot.decorations[0].start, 4);
        assert_eq!(snapshot.decorations[0].end, 4 + 2_000);
        assert!(registry.remove_owner(&first_owner));
        assert!(registry.snapshot().decorations.is_empty());
        assert!(registry.create_anchor(&owner("reload-b"), namespace, document, 0, AnchorBias::Before).is_err());
    }
}