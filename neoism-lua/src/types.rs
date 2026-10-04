use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Identifies the exact plugin generation which owns a runtime resource.
/// Revisions are opaque: callers may use a content hash, version, or monotonic
/// generation string.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(default, rename_all = "camelCase")]
pub struct PluginOwner {
    pub plugin_id: String,
    pub revision: PluginRevision,
}

#[derive(
    Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct PluginRevision(pub String);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginStatus {
    #[default]
    Discovered,
    Disabled,
    Loading,
    Ready,
    Failed,
}

/// A source is data rather than a Lua-specific concept so another plugin host
/// can consume the same lock/spec files later.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum PluginSource {
    Local {
        path: String,
    },
    Git {
        url: String,
        #[serde(default)]
        rev: Option<String>,
    },
    Registry {
        id: String,
        #[serde(default)]
        version: Option<String>,
    },
}

impl Default for PluginSource {
    fn default() -> Self {
        Self::Local {
            path: String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PluginSpec {
    pub id: String,
    pub source: PluginSource,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub revision: PluginRevision,
    #[serde(default = "default_object")]
    pub config: Value,
}

impl Default for PluginSpec {
    fn default() -> Self {
        Self {
            id: String::new(),
            source: PluginSource::default(),
            enabled: true,
            revision: PluginRevision::default(),
            config: Value::Object(Default::default()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PluginBudgets {
    pub memory_bytes: usize,
    pub load_millis: u64,
    pub callback_millis: u64,
    pub max_event_depth: usize,
    pub max_callbacks_per_event: usize,
    pub max_queued_actions: usize,
}

impl Default for PluginBudgets {
    fn default() -> Self {
        Self {
            memory_bytes: 64 * 1024 * 1024,
            load_millis: 250,
            callback_millis: 50,
            max_event_depth: 16,
            max_callbacks_per_event: 1_024,
            max_queued_actions: 1_024,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PluginTrigger {
    Name(String),
    Detailed {
        kind: String,
        #[serde(default)]
        value: Option<String>,
        #[serde(default)]
        mode: Option<String>,
        #[serde(default)]
        when: Option<String>,
    },
}

impl PluginTrigger {
    pub fn key(&self) -> String {
        match self {
            Self::Name(name) => name.clone(),
            Self::Detailed {
                kind, value: None, ..
            } => kind.clone(),
            Self::Detailed {
                kind,
                value: Some(value),
                ..
            } => format!("{kind}:{value}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LazyKeyTrigger {
    pub key: String,
    pub mode: String,
    pub when: Option<String>,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum DockEdge {
    Top,
    Bottom,
    Left,
    Right,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum SurfaceAlign {
    Start,
    End,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct SurfacePatch {
    pub visible: Option<bool>,
    pub dock: Option<DockEdge>,
    pub thickness: Option<f32>,
    pub order: Option<i32>,
}

impl SurfacePatch {
    pub fn overlay(&mut self, patch: &Self) {
        macro_rules! replace_some {
            ($field:ident) => {
                if patch.$field.is_some() {
                    self.$field = patch.$field;
                }
            };
        }
        replace_some!(visible);
        replace_some!(dock);
        replace_some!(thickness);
        replace_some!(order);
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct SurfaceItemPatch {
    pub visible: Option<bool>,
    pub surface: Option<String>,
    pub align: Option<SurfaceAlign>,
    pub order: Option<i32>,
}

impl SurfaceItemPatch {
    pub fn overlay(&mut self, patch: &Self) {
        if patch.visible.is_some() {
            self.visible = patch.visible;
        }
        if patch.surface.is_some() {
            self.surface.clone_from(&patch.surface);
        }
        if patch.align.is_some() {
            self.align = patch.align;
        }
        if patch.order.is_some() {
            self.order = patch.order;
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct SurfaceLayoutPatch {
    pub surfaces: BTreeMap<String, SurfacePatch>,
    pub items: BTreeMap<String, SurfaceItemPatch>,
}

impl SurfaceLayoutPatch {
    pub fn overlay(&mut self, patch: &Self) {
        for (id, value) in &patch.surfaces {
            self.surfaces.entry(id.clone()).or_default().overlay(value);
        }
        for (id, value) in &patch.items {
            self.items.entry(id.clone()).or_default().overlay(value);
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        for (id, surface) in &self.surfaces {
            if id.trim().is_empty() {
                return Err("surface id cannot be empty".into());
            }
            if surface
                .thickness
                .is_some_and(|value| !value.is_finite() || value < 0.0)
            {
                return Err(format!("surface `{id}` has an invalid thickness"));
            }
            if id != "chrome.actions" {
                return Err(format!("surface `{id}` is not registered by this host"));
            }
        }
        for (id, item) in &self.items {
            if id.trim().is_empty() {
                return Err("surface item id cannot be empty".into());
            }
            if item
                .surface
                .as_deref()
                .is_some_and(|id| id.trim().is_empty())
            {
                return Err(format!("item `{id}` references an empty surface id"));
            }
            if !matches!(
                id.as_str(),
                "chrome.menu"
                    | "chrome.explorer"
                    | "chrome.notes"
                    | "chrome.new-agent"
                    | "chrome.search"
                    | "chrome.presence"
                    | "chrome.agent-details"
                    | "chrome.agent"
                    | "chrome.servers"
            ) {
                return Err(format!(
                    "surface item `{id}` is not registered by this host"
                ));
            }
            if item
                .surface
                .as_deref()
                .is_some_and(|surface| surface != "chrome.actions")
            {
                return Err(format!(
                    "item `{id}` references surface `{}` which is not registered by this host",
                    item.surface.as_deref().unwrap_or_default()
                ));
            }
        }
        Ok(())
    }
}

impl Default for LazyKeyTrigger {
    fn default() -> Self {
        Self {
            key: String::new(),
            mode: "global".into(),
            when: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PluginCapability {
    Name(String),
    Detailed {
        name: String,
        #[serde(default)]
        scope: Option<String>,
    },
}

impl PluginCapability {
    pub fn key(&self) -> String {
        match self {
            Self::Name(name) => name.clone(),
            Self::Detailed { name, scope: None } => name.clone(),
            Self::Detailed {
                name,
                scope: Some(scope),
            } => format!("{name}:{scope}"),
        }
    }
}

fn default_api_version() -> u32 {
    crate::API_VERSION
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PackageEditorEntrypoint {
    pub entrypoint: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default = "default_api_version", alias = "api_version")]
    pub api_version: u32,
    /// Legacy editor entrypoint. `editor.entrypoint` takes precedence when it
    /// is present in a canonical multi-target package manifest.
    pub entrypoint: String,
    pub editor: Option<PackageEditorEntrypoint>,
    pub platforms: Vec<String>,
    pub dependencies: Vec<String>,
    pub triggers: Vec<PluginTrigger>,
    pub capabilities: Vec<PluginCapability>,
    /// Optional multi-tier entrypoints sharing this package identity/revision.
    /// Discovery only reads these declarations; executable/native activation
    /// requires a host-owned approval record outside the package.
    pub entrypoints: crate::ExtensionEntrypoints,
}

impl Default for PluginManifest {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            version: String::new(),
            api_version: default_api_version(),
            entrypoint: "init.lua".into(),
            editor: None,
            platforms: Vec::new(),
            dependencies: Vec::new(),
            triggers: Vec::new(),
            capabilities: Vec::new(),
            entrypoints: crate::ExtensionEntrypoints::default(),
        }
    }
}

impl PluginManifest {
    pub fn editor_entrypoint(&self) -> &str {
        self.editor
            .as_ref()
            .map(|editor| editor.entrypoint.as_str())
            .unwrap_or(&self.entrypoint)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PluginSnapshot {
    pub api_version: u32,
    pub config_patch: Value,
    pub styles: StyleSheet,
    pub commands: Vec<CommandContribution>,
    pub keymaps: Vec<KeymapContribution>,
    pub editor_options: Vec<EditorOptionContribution>,
    pub lazy_keys: Vec<LazyKeyTrigger>,
    pub surface_layout: SurfaceLayoutPatch,
    pub autocmds: Vec<AutocmdContribution>,
    pub panels: Vec<PanelContribution>,
    pub contributions: Vec<UiContribution>,
    pub views: Vec<PluginViewContribution>,
    pub platform: Vec<crate::OwnedPlatformContribution>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginViewKind {
    Picker,
    Tree,
    Table,
    List,
    Form,
    Modal,
    Inspector,
    Toolbar,
    Breadcrumb,
    Detail,
    VirtualDocument,
    Scene,
    AgentTimeline,
    ApprovalCard,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginViewContribution {
    #[serde(default)]
    pub owner: PluginOwner,
    pub id: String,
    pub kind: PluginViewKind,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub nodes: Vec<PluginViewNode>,
    #[serde(default)]
    pub state: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginViewNode {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub value: Value,
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub children: Vec<PluginViewNode>,
}

impl PluginViewContribution {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty() || self.id.len() > 256 || self.title.len() > 1024 {
            return Err("plugin view identity or title exceeds limits".into());
        }
        fn walk(
            nodes: &[PluginViewNode],
            depth: usize,
            count: &mut usize,
        ) -> Result<(), String> {
            if depth > 32 {
                return Err("plugin view nesting exceeds 32 levels".into());
            }
            for node in nodes {
                *count += 1;
                if *count > 10_000
                    || node.id.is_empty()
                    || node.id.len() > 256
                    || node.kind.len() > 64
                    || node.label.len() > 16 * 1024
                {
                    return Err("plugin view node budget exceeded".into());
                }
                walk(&node.children, depth + 1, count)?;
            }
            Ok(())
        }
        let mut count = 0;
        walk(&self.nodes, 0, &mut count)?;
        if serde_json::to_vec(self)
            .map_err(|error| error.to_string())?
            .len()
            > 1024 * 1024
        {
            return Err("plugin view snapshot exceeds 1 MiB".into());
        }
        Ok(())
    }
}

impl PluginSnapshot {
    pub fn empty() -> Self {
        Self {
            api_version: crate::API_VERSION,
            config_patch: Value::Object(Default::default()),
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StyleSheet(pub BTreeMap<String, StylePatch>);

impl StyleSheet {
    pub fn insert(&mut self, selector: impl Into<String>, style: StylePatch) {
        self.0.insert(selector.into(), style);
    }

    pub fn get(&self, selector: &str) -> Option<&StylePatch> {
        self.0.get(selector)
    }

    /// Merge dotted ancestors from broad to specific. Unset properties stay
    /// unset so each Rust draw site can retain its exact current default.
    pub fn resolve(&self, selector: &str) -> StylePatch {
        let mut resolved = StylePatch::default();
        let mut end = 0;
        for (index, byte) in selector.bytes().enumerate() {
            if byte == b'.' {
                resolved.overlay(self.0.get(&selector[..index]));
                end = index + 1;
            }
        }
        if end <= selector.len() {
            resolved.overlay(self.0.get(selector));
        }
        resolved
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case", deny_unknown_fields)]
pub struct StylePatch {
    pub visible: Option<bool>,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub min_width: Option<f32>,
    pub max_width: Option<f32>,
    pub min_height: Option<f32>,
    pub max_height: Option<f32>,
    pub padding: Option<f32>,
    pub padding_x: Option<f32>,
    pub padding_y: Option<f32>,
    pub gap: Option<f32>,
    pub row_height: Option<f32>,
    pub font_family: Option<String>,
    pub font_size: Option<f32>,
    pub font_weight: Option<u16>,
    pub line_height: Option<f32>,
    pub foreground: Option<String>,
    pub background: Option<String>,
    pub border_color: Option<String>,
    pub accent: Option<String>,
    pub muted: Option<String>,
    pub border_width: Option<f32>,
    pub radius: Option<f32>,
    pub opacity: Option<f32>,
    pub order: Option<i32>,
    pub scroll_multiplier: Option<f32>,
    pub scroll_smooth: Option<bool>,
    pub animation_ms: Option<u32>,
    pub animation_easing: Option<String>,
    pub icon: Option<String>,
}

impl StylePatch {
    pub fn overlay(&mut self, other: Option<&Self>) {
        let Some(other) = other else { return };
        macro_rules! take {
            ($($field:ident),* $(,)?) => {$(
                if other.$field.is_some() {
                    self.$field = other.$field.clone();
                }
            )*};
        }
        take!(
            visible,
            width,
            height,
            min_width,
            max_width,
            min_height,
            max_height,
            padding,
            padding_x,
            padding_y,
            gap,
            row_height,
            font_family,
            font_size,
            font_weight,
            line_height,
            foreground,
            background,
            border_color,
            accent,
            muted,
            border_width,
            radius,
            opacity,
            order,
            scroll_multiplier,
            scroll_smooth,
            animation_ms,
            animation_easing,
            icon
        );
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandContribution {
    pub id: String,
    pub callback: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub scope: ExecutionScope,
    #[serde(default = "default_object")]
    pub arguments_schema: Value,
    #[serde(default)]
    pub completions: Vec<String>,
    #[serde(default)]
    pub accepts_range: bool,
    #[serde(default)]
    pub accepts_count: bool,
    #[serde(default)]
    pub accepts_bang: bool,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub completion_callback: Option<String>,
    #[serde(default = "default_object")]
    pub result_schema: Value,
}

pub fn validate_command_arguments(schema: &Value, value: &Value) -> Result<(), String> {
    let Some(schema) = schema.as_object() else {
        return Ok(());
    };
    if let Some(expected) = schema.get("type").and_then(Value::as_str) {
        let matches = match expected {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "number" => value.is_number(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            _ => true,
        };
        if !matches {
            return Err(format!("command arguments must have type `{expected}`"));
        }
    }
    if let (Some(required), Some(object)) = (
        schema.get("required").and_then(Value::as_array),
        value.as_object(),
    ) {
        for key in required.iter().filter_map(Value::as_str) {
            if !object.contains_key(key) {
                return Err(format!(
                    "command arguments are missing required key `{key}`"
                ));
            }
        }
    }
    if let (Some(properties), Some(object)) = (
        schema.get("properties").and_then(Value::as_object),
        value.as_object(),
    ) {
        for (key, property_schema) in properties {
            if let Some(value) = object.get(key) {
                validate_command_arguments(property_schema, value)?;
            }
        }
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        if !values.contains(value) {
            return Err("command argument is outside the declared enum".into());
        }
    }
    Ok(())
}

pub fn validate_command_request(
    command: &CommandContribution,
    request: &PluginCommandRequest,
) -> Result<(), String> {
    if request.range.is_some() && !command.accepts_range {
        return Err(format!("command `{}` does not accept a range", command.id));
    }
    if request.count.is_some() && !command.accepts_count {
        return Err(format!("command `{}` does not accept a count", command.id));
    }
    if request.bang && !command.accepts_bang {
        return Err(format!("command `{}` does not accept bang", command.id));
    }
    if let Some(range) = &request.range {
        if range.start_line > range.end_line {
            return Err("command range start must not exceed its end".into());
        }
    }
    validate_command_arguments(&command.arguments_schema, &request.arguments)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeymapContribution {
    pub mode: String,
    pub key: String,
    pub command: String,
    #[serde(default)]
    pub when: Option<String>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub order: u64,
    /// If a multi-key prefix fails, the final key falls through to native input.
    #[serde(default = "default_true")]
    pub fallback: bool,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum EditorOptionName {
    Wrap,
    TabWidth,
    UseTabs,
    InputMode,
}

impl EditorOptionName {
    pub fn default_value(self) -> Value {
        match self {
            Self::Wrap => Value::Bool(true),
            Self::TabWidth => Value::from(4),
            Self::UseTabs => Value::Bool(false),
            Self::InputMode => Value::String("standard".into()),
        }
    }

    pub fn validate(self, value: &Value) -> Result<(), String> {
        match self {
            Self::Wrap | Self::UseTabs if value.is_boolean() => Ok(()),
            Self::TabWidth
                if value
                    .as_u64()
                    .is_some_and(|width| (1..=16).contains(&width)) =>
            {
                Ok(())
            }
            Self::InputMode if matches!(value.as_str(), Some("standard" | "vim")) => {
                Ok(())
            }
            Self::Wrap | Self::UseTabs => Err("editor option requires a boolean".into()),
            Self::TabWidth => {
                Err("tab_width must be an integer from 1 through 16".into())
            }
            Self::InputMode => Err("input_mode must be `standard` or `vim`".into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditorOptionContribution {
    pub owner: PluginOwner,
    pub name: EditorOptionName,
    pub value: Value,
    pub scope: PluginStateScope,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub order: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutocmdContribution {
    pub event: String,
    pub callback: String,
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub once: bool,
    #[serde(default)]
    pub scope: ExecutionScope,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub nested: bool,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub order: u64,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PluginStateScope {
    Plugin,
    Document,
    Pane,
    Tab,
    Workspace,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginStateRequest {
    pub scope: PluginStateScope,
    #[serde(default)]
    pub target: Option<String>,
    /// Persistent state is permitted only for plugin and workspace scopes.
    #[serde(default)]
    pub persistent: bool,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginStateQuery {
    pub scope: PluginStateScope,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub persistent: bool,
    #[serde(default)]
    pub key: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginCommandRange {
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginCommandRequest {
    pub command: String,
    #[serde(default)]
    pub arguments: Value,
    #[serde(default)]
    pub range: Option<PluginCommandRange>,
    #[serde(default)]
    pub count: Option<u64>,
    #[serde(default)]
    pub bang: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginCommandCancelRequest {
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCommandCompletion {
    pub id: String,
    pub command: String,
    pub ok: bool,
    pub cancelled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<PluginAsyncError>,
}

impl PluginCommandCompletion {
    pub fn succeeded(id: String, command: String, result: Value) -> Self {
        Self {
            id,
            command,
            ok: true,
            cancelled: false,
            result: Some(result),
            error: None,
        }
    }

    pub fn failed(
        id: String,
        command: String,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            id,
            command,
            ok: false,
            cancelled: false,
            result: None,
            error: Some(PluginAsyncError {
                code: code.into(),
                message: message.into(),
            }),
        }
    }

    pub fn cancelled(id: String, command: String) -> Self {
        Self {
            id,
            command,
            ok: false,
            cancelled: true,
            result: None,
            error: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginAsyncError {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTimerRequest {
    pub delay_millis: u64,
    #[serde(default)]
    pub interval_millis: Option<u64>,
    pub command: String,
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginTimerCancelRequest {
    pub timer: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginJobSpawnRequest {
    pub program: String,
    #[serde(default)]
    pub arguments: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub timeout_millis: Option<u64>,
    #[serde(default)]
    pub max_output_bytes: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginJobControlRequest {
    pub job: String,
    #[serde(default)]
    pub data: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginPromptRequest {
    pub title: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub options: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginAsyncCancelRequest {
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginWatchRequest {
    pub path: String,
    #[serde(default)]
    pub recursive: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginNetworkRequest {
    pub url: String,
    #[serde(default = "default_network_method")]
    pub method: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub timeout_millis: Option<u64>,
    #[serde(default)]
    pub max_response_bytes: Option<usize>,
    #[serde(default)]
    pub credential: Option<String>,
}

fn default_network_method() -> String {
    "GET".into()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginResultEntry {
    pub document: DocumentHandle,
    pub position: TextPosition,
    pub label: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub severity: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginResultListRequest {
    #[serde(default)]
    pub list: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub entries: Vec<PluginResultEntry>,
    #[serde(default)]
    pub index: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelContribution {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub location: PanelLocation,
    #[serde(default = "default_true")]
    pub visible: bool,
    #[serde(default)]
    pub render_callback: Option<String>,
    #[serde(default)]
    pub content: Vec<UiContribution>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiContribution {
    pub id: String,
    pub slot: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub tooltip: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub style: Option<String>,
    #[serde(default)]
    pub priority: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PanelLocation {
    Left,
    Right,
    Bottom,
    Center,
    #[default]
    Overlay,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionScope {
    #[default]
    Local,
    Workspace,
    SharedBuffer,
    Presence,
}

macro_rules! opaque_handle {
    ($name:ident) => {
        #[derive(
            Clone,
            Debug,
            Default,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Serialize,
            Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
            pub fn is_empty(&self) -> bool {
                self.0.is_empty()
            }
        }
    };
}

opaque_handle!(DocumentHandle);
opaque_handle!(PaneHandle);
opaque_handle!(TabHandle);
opaque_handle!(WorkspaceHandle);
opaque_handle!(SelectionHandle);
opaque_handle!(CursorHandle);
opaque_handle!(RangeHandle);

/// Create a stable, process-local opaque resource token. Consumers must never
/// parse this value; hosts regenerate it from authoritative identities when
/// validating an action, which makes a closed or repurposed pane fail rather
/// than silently targeting the currently focused pane.
pub fn opaque_resource_handle(kind: &str, parts: &[&str]) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    crate::API_VERSION.hash(&mut hasher);
    kind.hash(&mut hasher);
    parts.hash(&mut hasher);
    format!("neoism:{kind}:{:016x}", hasher.finish())
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub struct TextPosition {
    pub line: u32,
    /// Zero-based UTF-8 byte column.
    pub character: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextRange {
    pub handle: RangeHandle,
    pub start: TextPosition,
    pub end: TextPosition,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSelection {
    pub handle: SelectionHandle,
    pub anchor: TextPosition,
    pub active: TextPosition,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentCursor {
    pub handle: CursorHandle,
    pub position: TextPosition,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentMetadata {
    pub title: String,
    /// Host-native display identity. It may be Windows/UNC/remote syntax and
    /// must not be interpreted with guest filesystem semantics.
    pub host_path: String,
    pub language: String,
    pub kind: String,
    pub remote: bool,
    pub dirty: bool,
    pub revision: u64,
    pub line_count: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSnapshot {
    pub handle: DocumentHandle,
    pub pane: PaneHandle,
    pub tab: TabHandle,
    pub workspace: WorkspaceHandle,
    pub revision: u64,
    pub text: String,
    pub cursor: DocumentCursor,
    pub selections: Vec<DocumentSelection>,
    pub metadata: DocumentMetadata,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaneSnapshot {
    pub handle: PaneHandle,
    pub document: Option<DocumentHandle>,
    pub workspace: WorkspaceHandle,
    pub rect: [f32; 4],
    pub first_visible_line: u32,
    pub visible_line_count: u32,
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub focused: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocumentTextEdit {
    pub range: TextRange,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocumentEditRequest {
    pub document: DocumentHandle,
    pub expected_revision: u64,
    pub edits: Vec<DocumentTextEdit>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocumentSelectionsRequest {
    pub document: DocumentHandle,
    pub expected_revision: u64,
    pub selections: Vec<DocumentSelection>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocumentCursorRequest {
    pub document: DocumentHandle,
    pub expected_revision: u64,
    pub position: TextPosition,
    #[serde(default)]
    pub extend_selection: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostAction {
    pub namespace: String,
    pub action: String,
    #[serde(default)]
    pub arguments: Value,
    #[serde(default)]
    pub scope: ExecutionScope,
    #[serde(default)]
    pub invocation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<PluginOwner>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LuaLspOperation {
    Hover,
    SignatureHelp,
    Definition,
    References,
    DocumentSymbols,
    WorkspaceSymbols,
    Diagnostics,
    Clients,
    CodeActions,
    Rename,
    Format,
    ApplyCodeAction,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LuaLspRequest {
    pub operation: LuaLspOperation,
    #[serde(default)]
    pub arguments: Value,
}

/// Immutable editor target captured when a structured LSP request leaves Lua.
/// Coordinates are zero-based UTF-8 byte positions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LuaLspTarget {
    pub root: String,
    pub path: String,
    pub line: u32,
    pub character: u32,
    pub buffer_revision: Option<u64>,
    pub pane_id: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LuaLspPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LuaLspRange {
    pub start: LuaLspPosition,
    pub end: LuaLspPosition,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LuaLspLocation {
    pub path: String,
    pub range: Option<LuaLspRange>,
    pub language: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LuaLspHover {
    pub path: String,
    pub contents: String,
    pub kind: Option<String>,
    pub range: Option<LuaLspRange>,
    pub language: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LuaLspParameter {
    pub label: String,
    pub documentation: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LuaLspSignature {
    pub label: String,
    pub documentation: Option<String>,
    pub parameters: Vec<LuaLspParameter>,
    pub active_parameter: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LuaLspSignatureHelp {
    pub path: String,
    pub signatures: Vec<LuaLspSignature>,
    pub active_signature: Option<u32>,
    pub active_parameter: Option<u32>,
    pub language: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LuaLspDocumentSymbol {
    pub name: String,
    pub kind: String,
    pub detail: Option<String>,
    pub path: String,
    pub range: Option<LuaLspRange>,
    pub selection_range: Option<LuaLspRange>,
    pub children: Vec<LuaLspDocumentSymbol>,
    pub language: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LuaLspWorkspaceSymbol {
    pub name: String,
    pub kind: String,
    pub path: String,
    pub line: Option<u32>,
    pub language: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LuaLspRelatedInformation {
    pub path: String,
    pub range: Option<LuaLspRange>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LuaLspDiagnostic {
    pub path: String,
    pub range: Option<LuaLspRange>,
    pub severity: String,
    pub code: Option<String>,
    pub code_description: Option<String>,
    pub source: Option<String>,
    pub message: String,
    pub tags: Vec<String>,
    pub related_information: Vec<LuaLspRelatedInformation>,
    pub data: Option<Value>,
    pub language: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LuaLspCapabilities {
    pub workspace_symbols: bool,
    pub completion: bool,
    pub hover: bool,
    pub definition: bool,
    pub references: bool,
    pub implementation: bool,
    pub call_hierarchy: bool,
    pub diagnostics: bool,
    pub document_symbols: bool,
    pub formatting: bool,
    pub code_actions: bool,
    pub rename: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LuaLspClient {
    pub id: String,
    pub name: String,
    pub status: String,
    pub language: String,
    pub command: Vec<String>,
    pub workspace_root: String,
    pub capabilities: LuaLspCapabilities,
}

/// Portable summary of one selectable code action. The opaque ID is a
/// one-shot capability bound to the exact plugin revision and source request;
/// raw server payloads never cross into Lua.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LuaLspCodeAction {
    pub id: String,
    pub request_id: String,
    pub title: String,
    pub kind: Option<String>,
    pub preferred: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LuaLspMutationOwner {
    Frontend,
    Desktop,
    Daemon,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LuaLspChangedFile {
    pub path: String,
    pub edit_count: usize,
    pub applied_by: LuaLspMutationOwner,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LuaLspMutation {
    pub title: String,
    pub changed_files: Vec<LuaLspChangedFile>,
    pub ran_command: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "items", rename_all = "snake_case")]
pub enum LuaLspOutcome {
    Hover(Vec<LuaLspHover>),
    SignatureHelp(Vec<LuaLspSignatureHelp>),
    Definition(Vec<LuaLspLocation>),
    References(Vec<LuaLspLocation>),
    DocumentSymbols(Vec<LuaLspDocumentSymbol>),
    WorkspaceSymbols(Vec<LuaLspWorkspaceSymbol>),
    Diagnostics(Vec<LuaLspDiagnostic>),
    Clients(Vec<LuaLspClient>),
    CodeActions(Vec<LuaLspCodeAction>),
    ApplyCodeAction(LuaLspMutation),
    Rename(LuaLspMutation),
    Format(LuaLspMutation),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LuaLspError {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LuaLspCompletion {
    pub id: String,
    pub operation: LuaLspOperation,
    pub target: Option<LuaLspTarget>,
    pub ok: bool,
    pub cancelled: bool,
    pub result: Option<LuaLspOutcome>,
    pub error: Option<LuaLspError>,
}

impl LuaLspCompletion {
    pub fn success(
        id: String,
        operation: LuaLspOperation,
        target: LuaLspTarget,
        result: LuaLspOutcome,
    ) -> Self {
        Self {
            id,
            operation,
            target: Some(target),
            ok: true,
            cancelled: false,
            result: Some(result),
            error: None,
        }
    }

    pub fn failed(
        id: String,
        operation: LuaLspOperation,
        target: Option<LuaLspTarget>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            id,
            operation,
            target,
            ok: false,
            cancelled: false,
            result: None,
            error: Some(LuaLspError {
                code: code.into(),
                message: message.into(),
            }),
        }
    }

    pub fn cancelled(
        id: String,
        operation: LuaLspOperation,
        target: LuaLspTarget,
    ) -> Self {
        Self {
            id,
            operation,
            target: Some(target),
            ok: false,
            cancelled: true,
            result: None,
            error: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginEvent {
    pub name: String,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub scope: ExecutionScope,
    #[serde(default)]
    pub origin: Option<String>,
}

impl PluginEvent {
    pub fn new(
        kind: crate::PluginEventKind,
        payload: Value,
        scope: ExecutionScope,
        origin: Option<String>,
    ) -> Result<Self, String> {
        let contract = crate::event_contract(kind.name())?;
        if scope != contract.scope {
            return Err(format!(
                "Lua host event `{}` requires scope `{:?}`, got `{scope:?}`",
                kind.name(),
                contract.scope
            ));
        }
        Ok(Self {
            name: kind.name().into(),
            payload,
            scope,
            origin,
        })
    }

    pub fn validate(&self) -> Result<crate::EventContract, String> {
        let contract = crate::event_contract(&self.name)?;
        if self.scope != contract.scope {
            return Err(format!(
                "Lua host event `{}` requires scope `{:?}`, got `{:?}`",
                self.name, contract.scope, self.scope
            ));
        }
        Ok(contract)
    }
}

fn default_true() -> bool {
    true
}

fn default_object() -> Value {
    Value::Object(Default::default())
}

pub trait PluginHost: Send + Sync + 'static {
    fn query(
        &self,
        namespace: &str,
        operation: &str,
        arguments: Value,
    ) -> Result<Value, String>;
    fn query_owned(
        &self,
        _owner: &PluginOwner,
        namespace: &str,
        operation: &str,
        arguments: Value,
    ) -> Result<Value, String> {
        self.query(namespace, operation, arguments)
    }
    fn dispatch(&self, action: HostAction) -> Result<Value, String>;
    fn activate_owner(&self, _owner: &PluginOwner) -> Result<(), String> {
        Ok(())
    }
    fn retire_owner(&self, _owner: &PluginOwner) {}
}

pub struct ScopedPluginHost {
    inner: std::sync::Arc<dyn PluginHost>,
    owner: PluginOwner,
    grants: BTreeSet<String>,
}

impl ScopedPluginHost {
    pub fn new(
        inner: std::sync::Arc<dyn PluginHost>,
        owner: PluginOwner,
        requested: impl IntoIterator<Item = String>,
        granted: impl IntoIterator<Item = String>,
    ) -> Self {
        let requested = requested.into_iter().collect::<BTreeSet<_>>();
        let grants = granted
            .into_iter()
            .filter(|capability| requested.contains(capability))
            .collect();
        Self {
            inner,
            owner,
            grants,
        }
    }

    fn denied(&self, namespace: &str, operation: &str) -> String {
        format!(
            "plugin `{}` is not granted capability for {namespace}.{operation}",
            self.owner.plugin_id
        )
    }
}

impl PluginHost for ScopedPluginHost {
    fn query(
        &self,
        namespace: &str,
        operation: &str,
        arguments: Value,
    ) -> Result<Value, String> {
        let contract = crate::query_contract(namespace, operation)?;
        if !self.grants.contains("*")
            && !self.grants.contains(&contract.capability)
            && !self.grants.contains(&format!("{namespace}.{operation}"))
        {
            return Err(self.denied(namespace, operation));
        }
        self.inner
            .query_owned(&self.owner, namespace, operation, arguments)
    }

    fn dispatch(&self, mut action: HostAction) -> Result<Value, String> {
        action.owner = Some(self.owner.clone());
        let contract = crate::action_contract(&action)?;
        if !self.grants.contains("*")
            && !self.grants.contains(&contract.capability)
            && !self
                .grants
                .contains(&format!("{}.{}", action.namespace, action.action))
        {
            return Err(self.denied(&action.namespace, &action.action));
        }
        self.inner.dispatch(action)
    }

    fn activate_owner(&self, owner: &PluginOwner) -> Result<(), String> {
        if owner != &self.owner {
            return Err("cannot activate state for another plugin owner".into());
        }
        self.inner.activate_owner(owner)
    }

    fn retire_owner(&self, owner: &PluginOwner) {
        if owner == &self.owner {
            self.inner.retire_owner(owner);
        }
    }
}

#[derive(Default)]
pub struct InertHost;

impl PluginHost for InertHost {
    fn query(&self, _: &str, _: &str, _: Value) -> Result<Value, String> {
        Ok(Value::Null)
    }

    fn dispatch(&self, action: HostAction) -> Result<Value, String> {
        Err(format!(
            "Neoism host action {}.{} is unavailable",
            action.namespace, action.action
        ))
    }
}

/// Frontends use this host to keep Lua off their mutable UI graph. Queries read
/// immutable snapshots and mutations enter the normal event loop as typed
/// actions, where desktop/web hosts apply local or daemon-authoritative policy.
#[derive(Default)]
pub struct QueuedHost {
    state: RwLock<BTreeMap<String, Value>>,
    actions: Mutex<Vec<HostAction>>,
    next_invocation: AtomicU64,
    scoped_state: RwLock<crate::PluginScopedStateRegistry>,
    persistent_dirty: AtomicBool,
}

impl QueuedHost {
    pub fn publish(&self, namespace: impl Into<String>, value: Value) {
        if let Ok(mut state) = self.state.write() {
            state.insert(namespace.into(), value);
        }
    }

    pub fn published_state(&self) -> BTreeMap<String, Value> {
        self.state
            .read()
            .map(|state| state.clone())
            .unwrap_or_default()
    }

    pub fn from_state(state: BTreeMap<String, Value>) -> Self {
        Self {
            state: RwLock::new(state),
            ..Self::default()
        }
    }

    /// Fork the host-visible snapshot and durable values for a transactional
    /// candidate. Ephemeral exact-revision state and queued actions are not
    /// inherited.
    pub fn fork_candidate(&self) -> Self {
        Self {
            state: RwLock::new(self.published_state()),
            scoped_state: RwLock::new(
                self.scoped_state
                    .read()
                    .map(|state| state.fork_candidate())
                    .unwrap_or_default(),
            ),
            ..Self::default()
        }
    }

    pub fn restore_persistent_state(&self, snapshot: &Value) -> Result<(), String> {
        self.scoped_state
            .write()
            .map_err(|_| "Lua scoped state lock poisoned".to_string())?
            .restore_persistent(snapshot)
    }

    pub fn persistent_state_snapshot(&self) -> Value {
        self.scoped_state
            .read()
            .map(|state| state.persistent_snapshot())
            .unwrap_or_else(|_| serde_json::json!({ "version": 1, "entries": [] }))
    }

    pub fn take_persistent_dirty(&self) -> bool {
        self.persistent_dirty.swap(false, Ordering::AcqRel)
    }

    pub fn mark_persistent_dirty(&self) {
        self.persistent_dirty.store(true, Ordering::Release);
    }

    pub fn drain_actions(&self) -> Vec<HostAction> {
        self.actions
            .lock()
            .map(|mut actions| std::mem::take(&mut *actions))
            .unwrap_or_default()
    }

    pub fn remove_owner_state(&self, owner: &PluginOwner) {
        if let Ok(mut state) = self.scoped_state.write() {
            state.remove_owner(owner);
        }
    }

    pub fn clear_target_state(&self, scope: PluginStateScope, target: &str) {
        if let Ok(mut state) = self.scoped_state.write() {
            state.clear_target(scope, target);
        }
    }
}

impl PluginHost for QueuedHost {
    fn query_owned(
        &self,
        owner: &PluginOwner,
        namespace: &str,
        operation: &str,
        arguments: Value,
    ) -> Result<Value, String> {
        if namespace != "state" {
            return self.query(namespace, operation, arguments);
        }
        crate::query_contract(namespace, operation)?;
        let query: PluginStateQuery =
            serde_json::from_value(arguments).map_err(|error| error.to_string())?;
        validate_state_scope_target(&self.state, query.scope, query.target.as_deref())?;
        if query.persistent
            && !matches!(
                query.scope,
                PluginStateScope::Plugin | PluginStateScope::Workspace
            )
        {
            return Err(
                "persistent state is restricted to plugin and workspace scopes".into(),
            );
        }
        let state = self
            .scoped_state
            .read()
            .map_err(|_| "Lua scoped state lock poisoned".to_string())?;
        match operation {
            "get" => {
                let key = query
                    .key
                    .as_deref()
                    .ok_or_else(|| "state.get requires a key".to_string())?;
                if key.is_empty() || key.len() > 256 {
                    return Err("state.get key must contain at most 256 bytes".into());
                }
                Ok(state
                    .get(
                        owner,
                        query.scope,
                        query.target.as_deref(),
                        key,
                        query.persistent,
                    )
                    .cloned()
                    .unwrap_or(Value::Null))
            }
            "list" => Ok(Value::Object(
                state
                    .list(
                        owner,
                        query.scope,
                        query.target.as_deref(),
                        query.persistent,
                    )
                    .into_iter()
                    .collect(),
            )),
            _ => Err(format!("unregistered Lua state query `{operation}`")),
        }
    }

    fn query(
        &self,
        namespace: &str,
        operation: &str,
        arguments: Value,
    ) -> Result<Value, String> {
        let contract = crate::query_contract(namespace, operation)?;
        let state = self
            .state
            .read()
            .map_err(|_| "Lua host state lock poisoned".to_string())?;
        let value = state.get(namespace).cloned().unwrap_or(Value::Null);
        if namespace == "document" {
            let handle_matches = arguments
                .get("document")
                .or_else(|| arguments.get("handle"))
                .and_then(Value::as_str)
                .is_none_or(|requested| {
                    value.get("handle").and_then(Value::as_str) == Some(requested)
                });
            if !handle_matches {
                return Err(
                    "document handle is stale or not present in this host snapshot"
                        .into(),
                );
            }
            return match contract.operation {
                crate::HostOperation::DocumentText => {
                    Ok(value.get("text").cloned().unwrap_or(Value::Null))
                }
                crate::HostOperation::DocumentLines => {
                    let text = value
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let lines = text.split('\n').map(str::to_owned).collect::<Vec<_>>();
                    let start = arguments
                        .get("startLine")
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as usize;
                    let end = arguments
                        .get("endLine")
                        .and_then(Value::as_u64)
                        .map(|v| v as usize)
                        .unwrap_or(lines.len());
                    Ok(serde_json::to_value(
                        &lines[start.min(lines.len())
                            ..end.min(lines.len()).max(start.min(lines.len()))],
                    )
                    .unwrap_or(Value::Null))
                }
                crate::HostOperation::DocumentRange => {
                    document_range_query(&value, &arguments)
                }
                crate::HostOperation::DocumentSelections => Ok(value
                    .get("selections")
                    .cloned()
                    .unwrap_or_else(|| Value::Array(Vec::new()))),
                crate::HostOperation::DocumentCursor => {
                    Ok(value.get("cursor").cloned().unwrap_or(Value::Null))
                }
                crate::HostOperation::DocumentMetadata => {
                    Ok(value.get("metadata").cloned().unwrap_or(Value::Null))
                }
                crate::HostOperation::DocumentLanguage => Ok(value
                    .pointer("/metadata/language")
                    .cloned()
                    .unwrap_or(Value::Null)),
                crate::HostOperation::DocumentDirty => Ok(value
                    .pointer("/metadata/dirty")
                    .cloned()
                    .unwrap_or(Value::Bool(false))),
                crate::HostOperation::DocumentRevision => {
                    Ok(value.get("revision").cloned().unwrap_or(Value::Null))
                }
                _ => Ok(value),
            };
        }
        if matches!(namespace, "register" | "mark" | "macro") {
            return match contract.operation {
                crate::HostOperation::RegisterGet
                | crate::HostOperation::MarkGet
                | crate::HostOperation::MacroGet => {
                    let name = arguments
                        .get("name")
                        .and_then(Value::as_str)
                        .ok_or_else(|| format!("{namespace}.get requires a name"))?;
                    Ok(value.get(name).cloned().unwrap_or(Value::Null))
                }
                _ => Ok(value),
            };
        }
        match contract.operation {
            crate::HostOperation::QueryGet
            | crate::HostOperation::QueryCurrent
            | crate::HostOperation::QueryList
            | crate::HostOperation::QuerySnapshot
            | crate::HostOperation::QueryStatus
            | crate::HostOperation::QueryDiff
            | crate::HostOperation::QuerySessions => Ok(value),
            _ => Ok(serde_json::json!({ "state": value, "arguments": arguments })),
        }
    }

    fn dispatch(&self, mut action: HostAction) -> Result<Value, String> {
        let contract = crate::action_contract(&action)?;
        let owner = crate::require_exact_owner(&action)?.clone();
        if action.invocation_id.is_none() {
            action.invocation_id = Some(format!(
                "lua-{}",
                self.next_invocation.fetch_add(1, Ordering::Relaxed)
            ));
        }
        let invocation_id = action.invocation_id.clone().unwrap_or_default();
        if matches!(
            contract.operation,
            crate::HostOperation::StateSet
                | crate::HostOperation::StateDelete
                | crate::HostOperation::StateClear
        ) {
            let request: PluginStateRequest = serde_json::from_value(action.arguments)
                .map_err(|error| error.to_string())?;
            validate_state_target(&self.state, &request)?;
            let persistent = request.persistent;
            let mut state = self
                .scoped_state
                .write()
                .map_err(|_| "Lua scoped state lock poisoned".to_string())?;
            match contract.operation {
                crate::HostOperation::StateSet => state.set(&owner, request)?,
                crate::HostOperation::StateDelete => {
                    state.remove(
                        &owner,
                        request.scope,
                        request.target.as_deref(),
                        &request.key,
                        persistent,
                    );
                }
                crate::HostOperation::StateClear => state.clear_scope(
                    &owner,
                    request.scope,
                    request.target.as_deref(),
                    persistent,
                ),
                _ => unreachable!(),
            }
            if persistent {
                self.persistent_dirty.store(true, Ordering::Release);
            }
            return Ok(serde_json::json!({ "id": invocation_id }));
        }
        let mut actions = self
            .actions
            .lock()
            .map_err(|_| "Lua action queue lock poisoned".to_string())?;
        const MAX_ACTIONS_PER_OWNER: usize = 1_024;
        const MAX_ACTIONS_GLOBAL: usize = 8_192;
        if actions.len() >= MAX_ACTIONS_GLOBAL
            || actions
                .iter()
                .filter(|queued| queued.owner.as_ref() == Some(&owner))
                .count()
                >= MAX_ACTIONS_PER_OWNER
        {
            return Err("Lua host action queue budget exceeded".into());
        }
        actions.push(action);
        Ok(serde_json::json!({ "id": invocation_id }))
    }

    fn activate_owner(&self, owner: &PluginOwner) -> Result<(), String> {
        let changed = self
            .scoped_state
            .write()
            .map_err(|_| "Lua scoped state lock poisoned".to_string())?
            .activate_owner(owner);
        if changed {
            self.persistent_dirty.store(true, Ordering::Release);
        }
        Ok(())
    }

    fn retire_owner(&self, owner: &PluginOwner) {
        self.remove_owner_state(owner);
        if let Ok(mut actions) = self.actions.lock() {
            actions.retain(|action| action.owner.as_ref() != Some(owner));
        }
    }
}

fn validate_state_target(
    published: &RwLock<BTreeMap<String, Value>>,
    request: &PluginStateRequest,
) -> Result<(), String> {
    validate_state_scope_target(published, request.scope, request.target.as_deref())
}

fn validate_state_scope_target(
    published: &RwLock<BTreeMap<String, Value>>,
    scope: PluginStateScope,
    target: Option<&str>,
) -> Result<(), String> {
    if scope == PluginStateScope::Plugin {
        return target.is_none().then_some(()).ok_or_else(|| {
            "plugin-scoped state does not accept a target handle".to_string()
        });
    }
    let target =
        target.ok_or_else(|| "scoped plugin state requires a target".to_string())?;
    let namespace = match scope {
        PluginStateScope::Document => "document",
        PluginStateScope::Pane => "pane",
        PluginStateScope::Tab => "tab",
        PluginStateScope::Workspace => "workspace",
        PluginStateScope::Plugin => unreachable!(),
    };
    let state = published
        .read()
        .map_err(|_| "Lua host state lock poisoned".to_string())?;
    state
        .get(namespace)
        .and_then(|value| value.get("handle"))
        .and_then(Value::as_str)
        .is_some_and(|handle| handle == target)
        .then_some(())
        .ok_or_else(|| format!("{namespace} state target is stale or unavailable"))
}

fn document_range_query(snapshot: &Value, arguments: &Value) -> Result<Value, String> {
    let text = snapshot
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let lines = text.split('\n').collect::<Vec<_>>();
    let range = arguments.get("range").unwrap_or(arguments);
    let point = |name: &str| -> Result<(usize, usize), String> {
        let value = range
            .get(name)
            .ok_or_else(|| format!("document range requires `{name}`"))?;
        let line = value
            .get("line")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("document range `{name}.line` is invalid"))?
            as usize;
        let col = value
            .get("character")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("document range `{name}.character` is invalid"))?
            as usize;
        let content = lines.get(line).ok_or_else(|| {
            format!("document range `{name}` line is outside the document")
        })?;
        if col > content.len() || !content.is_char_boundary(col) {
            return Err(format!(
                "document range `{name}` is not a UTF-8 byte boundary"
            ));
        }
        Ok((line, col))
    };
    let start = point("start")?;
    let end = point("end")?;
    if end < start {
        return Err("document range ends before it starts".into());
    }
    let mut output = String::new();
    for line in start.0..=end.0 {
        if line > start.0 {
            output.push('\n');
        }
        let content = lines[line];
        let from = if line == start.0 { start.1 } else { 0 };
        let to = if line == end.0 { end.1 } else { content.len() };
        output.push_str(&content[from..to]);
    }
    Ok(Value::String(output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn scoped_state_reads_writes_and_cleanup_are_exact_owner() {
        let host = Arc::new(QueuedHost::default());
        host.publish("document", serde_json::json!({ "handle": "document:one" }));
        let make = |revision: &str| {
            ScopedPluginHost::new(
                host.clone(),
                PluginOwner {
                    plugin_id: "dev.state".into(),
                    revision: PluginRevision(revision.into()),
                },
                ["state.read".into(), "state.write".into()],
                ["state.read".into(), "state.write".into()],
            )
        };
        let first = make("r1");
        let second = make("r2");
        let set = |value| HostAction {
            namespace: "state".into(),
            action: "set".into(),
            arguments: serde_json::json!({ "scope": "document", "target": "document:one", "key": "value", "value": value }),
            scope: ExecutionScope::Local,
            invocation_id: None,
            owner: None,
        };
        first.dispatch(set(1)).unwrap();
        second.dispatch(set(2)).unwrap();
        let query = serde_json::json!({ "scope": "document", "target": "document:one", "key": "value" });
        assert_eq!(
            first.query("state", "get", query.clone()).unwrap(),
            serde_json::json!(1)
        );
        assert_eq!(
            second.query("state", "get", query.clone()).unwrap(),
            serde_json::json!(2)
        );
        host.remove_owner_state(&PluginOwner {
            plugin_id: "dev.state".into(),
            revision: PluginRevision("r1".into()),
        });
        assert!(first
            .query("state", "get", query.clone())
            .unwrap()
            .is_null());
        assert_eq!(
            second.query("state", "get", query).unwrap(),
            serde_json::json!(2)
        );
        host.clear_target_state(PluginStateScope::Document, "document:one");
        assert!(second.query("state", "get", serde_json::json!({ "scope": "document", "target": "document:one", "key": "value" })).unwrap().is_null());
        assert!(second.dispatch(HostAction {
            namespace: "state".into(), action: "set".into(),
            arguments: serde_json::json!({ "scope": "document", "target": "document:stale", "key": "value", "value": 3 }),
            scope: ExecutionScope::Local, invocation_id: None, owner: None,
        }).is_err());
        assert!(second
            .query(
                "state",
                "get",
                serde_json::json!({
                    "scope": "document", "target": "document:stale", "key": "value"
                })
            )
            .is_err());
    }

    #[test]
    fn persistent_state_is_transactional_across_candidate_hosts() {
        let host = Arc::new(QueuedHost::default());
        let owner = PluginOwner {
            plugin_id: "dev.persist".into(),
            revision: PluginRevision("r1".into()),
        };
        let scoped = ScopedPluginHost::new(
            host.clone(),
            owner.clone(),
            ["state.read".into(), "state.write".into()],
            ["state.read".into(), "state.write".into()],
        );
        scoped.dispatch(HostAction {
            namespace: "state".into(), action: "set".into(),
            arguments: serde_json::json!({ "scope": "plugin", "persistent": true, "key": "value", "value": 1 }),
            scope: ExecutionScope::Local, invocation_id: None, owner: None,
        }).unwrap();
        let candidate = Arc::new(host.fork_candidate());
        let candidate_owner = PluginOwner {
            plugin_id: owner.plugin_id.clone(),
            revision: PluginRevision("r2".into()),
        };
        let candidate_scoped = ScopedPluginHost::new(
            candidate.clone(),
            candidate_owner,
            ["state.read".into(), "state.write".into()],
            ["state.read".into(), "state.write".into()],
        );
        candidate_scoped.dispatch(HostAction {
            namespace: "state".into(), action: "set".into(),
            arguments: serde_json::json!({ "scope": "plugin", "persistent": true, "key": "value", "value": 2 }),
            scope: ExecutionScope::Local, invocation_id: None, owner: None,
        }).unwrap();
        let query =
            serde_json::json!({ "scope": "plugin", "persistent": true, "key": "value" });
        assert_eq!(scoped.query("state", "get", query.clone()).unwrap(), 1);
        assert_eq!(candidate_scoped.query("state", "get", query).unwrap(), 2);
        assert!(candidate.take_persistent_dirty());
    }

    #[test]
    fn host_action_queue_has_an_exact_owner_storm_budget() {
        let host = QueuedHost::default();
        let owner = PluginOwner {
            plugin_id: "dev.queue".into(),
            revision: PluginRevision("r1".into()),
        };
        for _ in 0..1_024 {
            host.dispatch(HostAction {
                namespace: "config".into(),
                action: "set".into(),
                arguments: Value::Null,
                scope: ExecutionScope::Local,
                invocation_id: None,
                owner: Some(owner.clone()),
            })
            .unwrap();
        }
        assert!(host
            .dispatch(HostAction {
                namespace: "config".into(),
                action: "set".into(),
                arguments: Value::Null,
                scope: ExecutionScope::Local,
                invocation_id: None,
                owner: Some(owner),
            })
            .is_err());
    }

    #[test]
    fn command_request_enforces_schema_and_range_count_bang_contract() {
        let command = CommandContribution {
            id: "typed".into(),
            callback: "callback".into(),
            title: String::new(),
            description: String::new(),
            scope: ExecutionScope::Local,
            arguments_schema: serde_json::json!({
                "type": "object", "required": ["name"],
                "properties": { "name": { "type": "string" } }
            }),
            completions: Vec::new(),
            accepts_range: true,
            accepts_count: false,
            accepts_bang: false,
            aliases: vec!["t".into()],
            completion_callback: None,
            result_schema: serde_json::json!({}),
        };
        let mut request = PluginCommandRequest {
            command: "t".into(),
            arguments: serde_json::json!({ "name": "ok" }),
            range: Some(PluginCommandRange {
                start_line: 2,
                end_line: 4,
            }),
            count: None,
            bang: false,
        };
        validate_command_request(&command, &request).unwrap();
        request.count = Some(2);
        assert!(validate_command_request(&command, &request).is_err());
        request.count = None;
        request.range = Some(PluginCommandRange {
            start_line: 5,
            end_line: 1,
        });
        assert!(validate_command_request(&command, &request).is_err());
        request.range = None;
        request.arguments = serde_json::json!({});
        assert!(validate_command_request(&command, &request).is_err());
    }

    #[test]
    fn scoped_host_enforces_declared_and_persisted_grants() {
        let host = Arc::new(QueuedHost::default());
        host.publish("buffer", serde_json::json!({ "id": "one" }));
        let scoped = ScopedPluginHost::new(
            host.clone(),
            PluginOwner {
                plugin_id: "dev.neoism.test".into(),
                revision: PluginRevision("one".into()),
            },
            ["buffer.read".to_string(), "buffer.write".to_string()],
            [
                "buffer.read".to_string(),
                "buffer.write".to_string(),
                "terminal.write".to_string(),
            ],
        );

        assert_eq!(
            scoped.query("buffer", "get", Value::Null).unwrap()["id"],
            "one"
        );
        assert!(scoped
            .dispatch(HostAction {
                namespace: "buffer".into(),
                action: "edit".into(),
                arguments: Value::Null,
                scope: ExecutionScope::SharedBuffer,
                invocation_id: None,
                owner: None,
            })
            .is_ok());
        assert!(scoped
            .dispatch(HostAction {
                namespace: "terminal".into(),
                action: "send".into(),
                arguments: Value::Null,
                scope: ExecutionScope::Local,
                invocation_id: None,
                owner: None,
            })
            .is_err());
        let actions = host.drain_actions();
        assert_eq!(actions.len(), 1);
        assert_eq!(
            actions[0].owner.as_ref().unwrap().plugin_id,
            "dev.neoism.test"
        );
    }

    #[test]
    fn structured_lsp_requests_enforce_the_requested_operation_capability() {
        let host = Arc::new(QueuedHost::default());
        let owner = PluginOwner {
            plugin_id: "dev.neoism.lsp-test".into(),
            revision: PluginRevision("one".into()),
        };
        let read_only = ScopedPluginHost::new(
            host.clone(),
            owner.clone(),
            ["lsp.read".to_string(), "lsp.edit".to_string()],
            ["lsp.read".to_string()],
        );
        let request = |operation: &str| HostAction {
            namespace: "lsp".into(),
            action: "request".into(),
            arguments: serde_json::json!({ "operation": operation, "arguments": {} }),
            scope: ExecutionScope::Local,
            invocation_id: None,
            owner: None,
        };
        assert!(read_only.dispatch(request("definition")).is_ok());
        for operation in ["code_actions", "rename", "format", "apply_code_action"] {
            assert!(read_only.dispatch(request(operation)).is_err());
        }

        let editor = ScopedPluginHost::new(
            host.clone(),
            owner,
            ["lsp.edit".to_string()],
            ["lsp.edit".to_string()],
        );
        for operation in ["code_actions", "rename", "format", "apply_code_action"] {
            assert!(editor.dispatch(request(operation)).is_ok());
        }
        assert_eq!(host.drain_actions().len(), 5);
    }

    #[test]
    fn document_queries_are_handle_checked_and_utf8_strict() {
        let host = QueuedHost::default();
        host.publish(
            "document",
            serde_json::json!({
                "handle": "neoism:document:one",
                "revision": 7,
                "text": "aéz\nnext",
                "cursor": { "position": { "line": 0, "character": 3 } },
                "selections": [],
                "metadata": { "language": "rust", "dirty": true }
            }),
        );
        assert_eq!(
            host.query(
                "document",
                "range",
                serde_json::json!({
                    "document": "neoism:document:one",
                    "range": {
                        "start": { "line": 0, "character": 1 },
                        "end": { "line": 0, "character": 3 }
                    }
                })
            )
            .unwrap(),
            Value::String("é".into())
        );
        assert!(host
            .query(
                "document",
                "text",
                serde_json::json!({
                    "document": "neoism:document:stale"
                })
            )
            .is_err());
        assert!(host
            .query(
                "document",
                "range",
                serde_json::json!({
                    "document": "neoism:document:one",
                    "start": { "line": 0, "character": 2 },
                    "end": { "line": 0, "character": 3 }
                })
            )
            .is_err());
    }

    #[test]
    fn queued_mutations_reject_missing_owner_revisions() {
        let host = QueuedHost::default();
        let mut action = HostAction {
            namespace: "document".into(),
            action: "edit".into(),
            arguments: serde_json::json!({}),
            scope: ExecutionScope::SharedBuffer,
            invocation_id: None,
            owner: None,
        };
        assert!(host.dispatch(action.clone()).is_err());
        action.owner = Some(PluginOwner {
            plugin_id: "dev.neoism.owner-test".into(),
            revision: PluginRevision("candidate-9".into()),
        });
        assert!(host.dispatch(action).is_ok());
        assert_eq!(host.drain_actions().len(), 1);
    }

    #[test]
    fn structured_lsp_completion_has_a_stable_portable_envelope() {
        let value = serde_json::to_value(LuaLspCompletion::success(
            "lua-7".into(),
            LuaLspOperation::SignatureHelp,
            LuaLspTarget {
                root: "/workspace".into(),
                path: "/workspace/src/main.rs".into(),
                line: 4,
                character: 9,
                buffer_revision: Some(12),
                pane_id: 3,
            },
            LuaLspOutcome::SignatureHelp(Vec::new()),
        ))
        .unwrap();

        assert_eq!(value["id"], "lua-7");
        assert_eq!(value["operation"], "signature_help");
        assert_eq!(value["target"]["bufferRevision"], 12);
        assert_eq!(value["target"]["paneId"], 3);
        assert_eq!(value["result"]["kind"], "signature_help");
        assert_eq!(value["result"]["items"], serde_json::json!([]));
        assert_eq!(value["ok"], true);
        assert_eq!(value["cancelled"], false);
        assert!(value["error"].is_null());
    }

    #[test]
    fn structured_code_actions_expose_only_request_scoped_capabilities() {
        let value =
            serde_json::to_value(LuaLspOutcome::CodeActions(vec![LuaLspCodeAction {
                id: "action-token".into(),
                request_id: "lua-9".into(),
                title: "Import HashMap".into(),
                kind: Some("quickfix".into()),
                preferred: true,
            }]))
            .unwrap();
        let action = &value["items"][0];
        assert_eq!(action["id"], "action-token");
        assert_eq!(action["requestId"], "lua-9");
        assert!(action.get("payload").is_none());
        assert!(action.get("serverId").is_none());
        assert!(action.get("command").is_none());
        assert!(action.get("edit").is_none());
    }
}
