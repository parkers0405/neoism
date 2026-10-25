//! Zed-style Settings panel — data model + interaction.
//!
//! A full-screen settings surface over the unified `config.json`. The
//! daemon-provided descriptor catalog is the source of truth for nested
//! paths, controls, defaults, documentation, and host-specific choices.
//! The host feeds the raw document values and persists [`SettingsAction`].

use neoism_protocol::config::{
    ConfigCategory, ConfigConstraints, ConfigControl, ConfigDescriptor, ConfigOption,
    ConfigValueKind,
};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RowControl {
    Toggle,
    Select,
    FontFamily,
    Text,
    Keybinding,
    Action {
        action: &'static str,
        button: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SettingRow {
    pub category: Category,
    pub path: String,
    pub label: String,
    pub description: String,
    pub value_kind: ConfigValueKind,
    pub default: Value,
    pub suggestions: Vec<String>,
    pub options: Vec<ConfigOption>,
    pub constraints: ConfigConstraints,
    pub extensible: bool,
    pub control: RowControl,
}

impl SettingRow {
    fn from_descriptor(descriptor: ConfigDescriptor) -> Self {
        let mut suggestions = descriptor.static_suggestions;
        suggestions.extend(descriptor.runtime_suggestions);
        suggestions.sort_by_key(|value| value.to_lowercase());
        suggestions.dedup();
        let mut options = descriptor.options;
        for suggestion in &suggestions {
            let value = option_to_json(suggestion, descriptor.value_kind);
            if !options.iter().any(|option| option.value == value) {
                options.push(ConfigOption {
                    value,
                    label: None,
                    description: None,
                });
            }
        }
        let control = if descriptor.path == "ui.window.background-image" {
            RowControl::Action {
                action: "backgrounds:pick",
                button: "Choose\u{2026}",
            }
        } else if descriptor.path == "agent.model" {
            RowControl::Action {
                action: "open-model",
                button: "Choose\u{2026}",
            }
        } else {
            match descriptor.control {
                ConfigControl::Toggle => RowControl::Toggle,
                ConfigControl::Select => RowControl::Select,
                ConfigControl::FontFamily => RowControl::FontFamily,
                ConfigControl::Keybinding => RowControl::Keybinding,
                ConfigControl::Number if !options.is_empty() => RowControl::Select,
                ConfigControl::Text
                | ConfigControl::Number
                | ConfigControl::Color
                | ConfigControl::StringList
                | ConfigControl::Object => RowControl::Text,
            }
        };
        Self {
            category: Category::from_protocol(descriptor.category),
            path: descriptor.path,
            label: descriptor.label,
            description: descriptor.description,
            value_kind: descriptor.value_kind,
            default: descriptor.default,
            suggestions,
            options,
            constraints: descriptor.constraints,
            extensible: descriptor.extensible,
            control,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    General,
    Appearance,
    Editor,
    Terminal,
    Ui,
    Presence,
    Keybinds,
    Agent,
    Platform,
    Developer,
}

impl Category {
    pub(crate) const ALL: [Category; 10] = [
        Category::General,
        Category::Appearance,
        Category::Editor,
        Category::Terminal,
        Category::Ui,
        Category::Presence,
        Category::Keybinds,
        Category::Agent,
        Category::Platform,
        Category::Developer,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Category::General => "General",
            Category::Appearance => "Appearance",
            Category::Editor => "Editor",
            Category::Terminal => "Terminal",
            Category::Ui => "UI",
            Category::Presence => "Presence",
            Category::Keybinds => "Keybinds",
            Category::Agent => "Agent",
            Category::Platform => "Platform",
            Category::Developer => "Developer",
        }
    }

    pub(crate) fn icon(self) -> &'static str {
        match self {
            Category::General => "\u{f013}",
            Category::Appearance => "\u{f1fc}",
            Category::Editor => "\u{f044}",
            Category::Terminal => "\u{f120}",
            Category::Ui => "\u{f108}",
            Category::Presence => "\u{f0c0}",
            Category::Keybinds => "\u{f11c}",
            Category::Agent => "\u{f544}",
            Category::Platform => "\u{f17a}",
            Category::Developer => "\u{f188}",
        }
    }

    fn from_protocol(category: ConfigCategory) -> Self {
        match category {
            ConfigCategory::General => Self::General,
            ConfigCategory::Appearance => Self::Appearance,
            ConfigCategory::Editor => Self::Editor,
            ConfigCategory::Terminal => Self::Terminal,
            ConfigCategory::Ui => Self::Ui,
            ConfigCategory::Presence => Self::Presence,
            ConfigCategory::Keybinds => Self::Keybinds,
            ConfigCategory::Agent => Self::Agent,
            ConfigCategory::Platform => Self::Platform,
            ConfigCategory::Renderer | ConfigCategory::Developer => Self::Developer,
        }
    }
}

/// Which surface a shortcut applies to. Rows are shown under a header
/// for their group; `Global` shortcuts work anywhere.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyGroup {
    Global,
    Terminal,
    Editor,
    Agent,
}

impl KeyGroup {
    pub(crate) fn title(self) -> &'static str {
        // Short + uppercase: these render in the bundled Press Start 2P
        // pixel face (same as the notes / agent section headers).
        match self {
            KeyGroup::Global => "GLOBAL",
            KeyGroup::Terminal => "TERMINAL",
            KeyGroup::Editor => "EDITOR",
            KeyGroup::Agent => "AGENT",
        }
    }
}

/// A shortcut shown in the Keybinds section. Three flavors:
/// - **rebindable** (`action` set): a config action; the live binding is
///   the `bindings.keys` override, else the platform default below.
///   macOS (⌘) and Linux/Windows (Ctrl/Alt) defaults differ.
/// - **global reference** (`action` empty, `literal` empty): a hardcoded
///   app shortcut (Alt+S, Alt+E…) shown platform-aware but not rebindable.
/// - **literal reference** (`literal` set): a vim-engine key like `dd`,
///   shown verbatim, not rebindable.
#[derive(Clone, Copy)]
pub(crate) struct KeybindDef {
    pub action: &'static str,
    pub label: &'static str,
    pub group: KeyGroup,
    pub mac_mods: &'static str,
    pub mac_key: &'static str,
    pub other_mods: &'static str,
    pub other_key: &'static str,
    pub literal: &'static str,
}

/// Rebindable config action.
const fn kb(
    action: &'static str,
    label: &'static str,
    group: KeyGroup,
    mm: &'static str,
    mk: &'static str,
    om: &'static str,
    ok: &'static str,
) -> KeybindDef {
    KeybindDef {
        action,
        label,
        group,
        mac_mods: mm,
        mac_key: mk,
        other_mods: om,
        other_key: ok,
        literal: "",
    }
}

/// Global reference shortcut (hardcoded, platform-aware, not rebindable).
const fn kbg(
    label: &'static str,
    group: KeyGroup,
    mm: &'static str,
    mk: &'static str,
    om: &'static str,
    ok: &'static str,
) -> KeybindDef {
    KeybindDef {
        action: "",
        label,
        group,
        mac_mods: mm,
        mac_key: mk,
        other_mods: om,
        other_key: ok,
        literal: "",
    }
}

/// Literal reference shortcut (vim-engine key, shown verbatim).
const fn kbl(label: &'static str, group: KeyGroup, literal: &'static str) -> KeybindDef {
    KeybindDef {
        action: "",
        label,
        group,
        mac_mods: "",
        mac_key: "",
        other_mods: "",
        other_key: "",
        literal,
    }
}

/// The Keybinds section, grouped by surface. Rebindable rows use real
/// config actions (from `bindings/platform/*` + `bindings/defaults.rs`);
/// reference rows document hardcoded / vim-engine keys you can't rebind.
pub(crate) const KEYBINDS: &[KeybindDef] = &[
    // ── Global ──
    kb(
        "opencommandpalette",
        "Command palette",
        KeyGroup::Global,
        "super",
        "p",
        "alt",
        "p",
    ),
    kbg("Search files", KeyGroup::Global, "alt", "s", "alt", "s"),
    kbg("Open agent", KeyGroup::Global, "alt", "t", "alt", "t"),
    kbg("Toggle file tree", KeyGroup::Global, "alt", "e", "alt", "e"),
    kb(
        "toggleneoismnotes",
        "Toggle notes panel",
        KeyGroup::Global,
        "alt",
        "n",
        "alt",
        "n",
    ),
    kb(
        "togglegitdiffpanel",
        "Toggle Git panel",
        KeyGroup::Global,
        "alt",
        "g",
        "alt",
        "g",
    ),
    kb(
        "createtab",
        "New tab",
        KeyGroup::Global,
        "super",
        "t",
        "control | shift",
        "w",
    ),
    kb(
        "createworkspaceterminaltab",
        "New workspace tab",
        KeyGroup::Global,
        "",
        "",
        "control | shift",
        "t",
    ),
    kb(
        "createwindow",
        "New window",
        KeyGroup::Global,
        "super",
        "n",
        "control | shift",
        "n",
    ),
    kb(
        "closesplitortab",
        "Close tab / split",
        KeyGroup::Global,
        "super",
        "w",
        "",
        "",
    ),
    kb(
        "selectnexttab",
        "Next tab",
        KeyGroup::Global,
        "control",
        "tab",
        "control",
        "tab",
    ),
    kb(
        "selectprevtab",
        "Previous tab",
        KeyGroup::Global,
        "control | shift",
        "tab",
        "control | shift",
        "tab",
    ),
    kb(
        "selectnextbuffertab",
        "Next editor tab",
        KeyGroup::Global,
        "",
        "",
        "control | shift",
        "]",
    ),
    kb(
        "selectprevbuffertab",
        "Previous editor tab",
        KeyGroup::Global,
        "",
        "",
        "control | shift",
        "[",
    ),
    kb(
        "splitright",
        "Split right",
        KeyGroup::Global,
        "super",
        "d",
        "control | shift",
        "r",
    ),
    kb(
        "splitdown",
        "Split down",
        KeyGroup::Global,
        "super | shift",
        "d",
        "control | shift",
        "d",
    ),
    kb(
        "selectnextsplit",
        "Next split",
        KeyGroup::Global,
        "super",
        "]",
        "control | shift",
        "]",
    ),
    kb(
        "selectprevsplit",
        "Previous split",
        KeyGroup::Global,
        "super",
        "[",
        "control | shift",
        "[",
    ),
    kb(
        "increasefontsize",
        "Increase font size",
        KeyGroup::Global,
        "super",
        "=",
        "control",
        "=",
    ),
    kb(
        "decreasefontsize",
        "Decrease font size",
        KeyGroup::Global,
        "super",
        "-",
        "control",
        "-",
    ),
    kb(
        "resetfontsize",
        "Reset font size",
        KeyGroup::Global,
        "super",
        "0",
        "control",
        "0",
    ),
    kb(
        "openconfigeditor",
        "Open config file",
        KeyGroup::Global,
        "alt",
        ",",
        "alt",
        ",",
    ),
    kb(
        "togglefullscreen",
        "Toggle fullscreen",
        KeyGroup::Global,
        "control | super",
        "f",
        "",
        "",
    ),
    kb("quit", "Quit", KeyGroup::Global, "super", "q", "", ""),
    // ── Terminal ──
    kb(
        "copy",
        "Copy",
        KeyGroup::Terminal,
        "super",
        "c",
        "control | shift",
        "c",
    ),
    kb(
        "paste",
        "Paste",
        KeyGroup::Terminal,
        "super",
        "v",
        "control | shift",
        "v",
    ),
    kb(
        "searchforward",
        "Find in terminal",
        KeyGroup::Terminal,
        "super",
        "f",
        "control | shift",
        "f",
    ),
    kb(
        "togglevimode",
        "Toggle terminal vi-mode",
        KeyGroup::Terminal,
        "alt | shift",
        "space",
        "alt | shift",
        "space",
    ),
    kbl("Move (vi motions)", KeyGroup::Terminal, "h j k l"),
    kbl("Yank / paste", KeyGroup::Terminal, "y / p"),
    kbl("Visual / line select", KeyGroup::Terminal, "v / V"),
    kbl("Top / bottom", KeyGroup::Terminal, "gg / G"),
    kbl("Search in scrollback", KeyGroup::Terminal, "/"),
    // ── Editor (Vim) ── (reference — driven by the editor's vim engine)
    kbl("Insert before / at line start", KeyGroup::Editor, "i / I"),
    kbl("Append after / at line end", KeyGroup::Editor, "a / A"),
    kbl("Open line below / above", KeyGroup::Editor, "o / O"),
    kbl("Back to Normal mode", KeyGroup::Editor, "Esc"),
    kbl("Paste after / before", KeyGroup::Editor, "p / P"),
    kbl("Yank line", KeyGroup::Editor, "yy"),
    kbl("Delete line", KeyGroup::Editor, "dd"),
    kbl("Delete char", KeyGroup::Editor, "x"),
    kbl("Change word", KeyGroup::Editor, "cw"),
    kbl("Undo / redo", KeyGroup::Editor, "u / Ctrl+r"),
    kbl("Visual / visual line", KeyGroup::Editor, "v / V"),
    kbl("Top / bottom of file", KeyGroup::Editor, "gg / G"),
    kbl("Word forward / back", KeyGroup::Editor, "w / b"),
    kbl("Line start / end", KeyGroup::Editor, "0 / $"),
    kbl("Search", KeyGroup::Editor, "/"),
    // ── Agent ── (reference — composer input)
    kbl("Send message", KeyGroup::Agent, "Enter"),
    kbl("New line in message", KeyGroup::Agent, "Shift+Enter"),
    kbl("Stop / cancel run", KeyGroup::Agent, "Esc"),
];

/// Render a config-style `(mods, key)` combo for humans, honoring the
/// platform's modifier glyphs (⌘/⌥/⌃/⇧ on macOS, Ctrl/Alt/Shift else).
pub(crate) fn format_combo(mods: &str, key: &str) -> String {
    let mac = cfg!(target_os = "macos");
    let mut parts: Vec<String> = Vec::new();
    for m in mods.split('|').map(|s| s.trim().to_lowercase()) {
        if m.is_empty() || m == "none" {
            continue;
        }
        let glyph = match m.as_str() {
            "super" | "command" => {
                if mac {
                    "⌘"
                } else {
                    "Ctrl"
                }
            }
            "control" => {
                if mac {
                    "⌃"
                } else {
                    "Ctrl"
                }
            }
            "alt" | "option" => {
                if mac {
                    "⌥"
                } else {
                    "Alt"
                }
            }
            "shift" => {
                if mac {
                    "⇧"
                } else {
                    "Shift"
                }
            }
            _ => continue,
        };
        parts.push(glyph.to_string());
    }
    parts.push(format_key_label(key));
    parts.join(if mac { " " } else { "+" })
}

fn format_key_label(key: &str) -> String {
    match key.to_lowercase().as_str() {
        "space" => "Space".to_string(),
        "return" | "enter" => "Enter".to_string(),
        "esc" => "Esc".to_string(),
        "up" => "↑".to_string(),
        "down" => "↓".to_string(),
        "left" => "←".to_string(),
        "right" => "→".to_string(),
        other if other.chars().count() == 1 => other.to_uppercase(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}

/// A change the host must apply.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsAction {
    /// Persist `value` (bool / number / string) at `key`.
    Set { key: String, value: Value },
    /// Persist a keybinding override into `bindings.keys`. An empty `key`
    /// removes the override so the built-in default applies again.
    SetKeybind {
        action: &'static str,
        key: String,
        with: String,
    },
    /// Open the raw config.json in the editor.
    OpenConfigFile,
    /// Run a named host GUI action (e.g. open the model picker).
    RunAction(&'static str),
}

#[derive(Debug, Default, Clone)]
pub struct PointerOutcome {
    pub consumed: bool,
    pub action: Option<SettingsAction>,
}

fn swallow() -> PointerOutcome {
    PointerOutcome {
        consumed: true,
        action: None,
    }
}

/// Max option rows a dropdown shows before it scrolls (fonts can be long).
const DROPDOWN_MAX_ROWS: usize = 10;
const CUSTOM_OPTION: &str = "Custom\u{2026}";

pub struct NeoismSettingsPane {
    active: bool,
    values: Value,
    font_families: Vec<String>,
    rows: Vec<SettingRow>,
    category: Category,
    search: String,
    search_focused: bool,
    open_dropdown: Option<usize>,
    dropdown_scroll: usize,
    /// Filter text typed while a long (font) dropdown is open.
    dropdown_search: String,
    editing: Option<usize>,
    edit_buffer: neoui::input::TextInput,
    /// Index into `KEYBINDS` currently waiting for a captured chord.
    capturing: Option<usize>,
    scroll: f32,
    max_scroll: f32,
    pub(crate) panel_rect: [f32; 4],
    pub(crate) category_rects: Vec<([f32; 4], Category)>,
    pub(crate) control_rects: Vec<([f32; 4], usize)>,
    /// Declarative descriptor rows own their paint and input geometry together.
    pub(crate) row_ui: neoui::ComponentHost<String>,
    pub(super) row_component: Option<neoui::ComponentHandle<super::neoui::SettingsRows>>,
    pub(crate) row_viewport: [f32; 4],
    pub(crate) row_geometry_valid: bool,
    pub(crate) dropdown_rects: Vec<([f32; 4], usize, String)>,
    pub(crate) dropdown_search_rect: [f32; 4],
    /// Keybind row rects (click → capture a new chord) and their reset
    /// glyph rects (click → clear the override).
    pub(crate) keybind_rects: Vec<([f32; 4], usize)>,
    pub(crate) keybind_reset_rects: Vec<([f32; 4], usize)>,
    pub(crate) search_rect: [f32; 4],
    pub(crate) edit_json_rect: [f32; 4],
    pub(crate) close_rect: [f32; 4],
    pub(crate) back_rect: [f32; 4],
    pub(crate) hover_control: Option<usize>,
    pub(crate) hover_category: Option<Category>,
    safe_insets: [f32; 4],
    compact_layout: bool,
    compact_detail: bool,
    layout_initialized: bool,
}

impl Default for NeoismSettingsPane {
    fn default() -> Self {
        Self::new()
    }
}

impl NeoismSettingsPane {
    pub fn new() -> Self {
        Self {
            active: false,
            values: Value::Object(serde_json::Map::new()),
            font_families: Vec::new(),
            rows: Vec::new(),
            category: Category::General,
            search: String::new(),
            search_focused: false,
            open_dropdown: None,
            dropdown_scroll: 0,
            dropdown_search: String::new(),
            editing: None,
            edit_buffer: neoui::input::TextInput::default(),
            capturing: None,
            scroll: 0.0,
            max_scroll: 0.0,
            panel_rect: [0.0; 4],
            category_rects: Vec::new(),
            control_rects: Vec::new(),
            row_ui: neoui::ComponentHost::new(),
            row_component: None,
            row_viewport: [0.0; 4],
            row_geometry_valid: false,
            dropdown_rects: Vec::new(),
            dropdown_search_rect: [0.0; 4],
            keybind_rects: Vec::new(),
            keybind_reset_rects: Vec::new(),
            search_rect: [0.0; 4],
            edit_json_rect: [0.0; 4],
            close_rect: [0.0; 4],
            back_rect: [0.0; 4],
            hover_control: None,
            hover_category: None,
            safe_insets: [0.0; 4],
            compact_layout: false,
            compact_detail: false,
            layout_initialized: false,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn active_rect(
        &self,
        window_w: f32,
        window_h: f32,
        scale: f32,
    ) -> Option<[f32; 4]> {
        if !self.active {
            return None;
        }
        let s = if scale > 0.0 { scale } else { 1.0 };
        Some([0.0, 0.0, window_w / s, window_h / s])
    }

    /// Host-provided safe area insets in logical pixels: top/right/bottom/left.
    pub fn set_safe_area_insets(&mut self, top: f32, right: f32, bottom: f32, left: f32) {
        self.safe_insets = [top.max(0.0), right.max(0.0), bottom.max(0.0), left.max(0.0)];
    }

    pub(crate) fn safe_area_insets(&self) -> [f32; 4] {
        self.safe_insets
    }

    /// Synchronize navigation with the responsive renderer. Crossing the
    /// compact breakpoint always lands on the category root; desktop has no
    /// pushed-page state and continues to show its rail and detail together.
    pub(crate) fn set_compact_layout(&mut self, compact: bool) {
        if !self.layout_initialized || self.compact_layout != compact {
            self.clear_detail_hit_regions();
            self.compact_detail = false;
            self.scroll = 0.0;
            self.search.clear();
            self.search_focused = false;
            self.open_dropdown = None;
            self.dropdown_search.clear();
            self.editing = None;
            self.capturing = None;
        }
        self.compact_layout = compact;
        self.layout_initialized = true;
    }

    pub(crate) fn compact_root(&self) -> bool {
        self.compact_layout && !self.compact_detail
    }

    pub(crate) fn compact_detail(&self) -> bool {
        self.compact_layout && self.compact_detail
    }

    /// One painted-overlay text-entry hit test for search, dropdown search,
    /// generic values, and keybind capture controls.
    pub fn text_entry_at(&self, x: f32, y: f32) -> bool {
        if !self.active {
            return false;
        }
        if point_in(self.search_rect, x, y) || point_in(self.dropdown_search_rect, x, y) {
            return true;
        }
        self.control_rects.iter().any(|(rect, index)| {
            point_in(*rect, x, y)
                && self
                    .rows
                    .get(*index)
                    .is_some_and(|row| row.control == RowControl::Text)
        }) || self
            .keybind_rects
            .iter()
            .any(|(rect, _)| point_in(*rect, x, y))
    }

    /// The settings overlay owns the cursor, not the editor underneath it.
    pub fn pointer_kind_at(
        &self,
        x: f32,
        y: f32,
    ) -> crate::widgets::modal::ModalPointerKind {
        use crate::widgets::modal::ModalPointerKind::{Default, Pointer, Text};
        if !self.active {
            return Default;
        }
        if self.open_dropdown.is_some() {
            return if self.open_dropdown_is_searchable()
                && point_in(self.dropdown_search_rect, x, y)
            {
                Text
            } else if self
                .dropdown_rects
                .iter()
                .any(|(rect, _, _)| point_in(*rect, x, y))
            {
                Pointer
            } else {
                Default
            };
        }
        if point_in(self.close_rect, x, y)
            || (self.compact_detail() && point_in(self.back_rect, x, y))
            || (!self.compact_root() && point_in(self.edit_json_rect, x, y))
            || (!self.compact_detail()
                && self
                    .category_rects
                    .iter()
                    .any(|(rect, _)| point_in(*rect, x, y)))
        {
            return Pointer;
        }
        if self.compact_root() {
            return Default;
        }
        if point_in(self.search_rect, x, y) {
            return Text;
        }
        if self.is_keybinds()
            && self
                .keybind_rects
                .iter()
                .chain(&self.keybind_reset_rects)
                .any(|(rect, _)| point_in(*rect, x, y))
        {
            return Pointer;
        }
        if self.row_geometry_valid
            && self
                .row_ui
                .runtime()
                .frame()
                .hit_test(x, y)
                .is_some_and(|hit| hit.action.is_some())
        {
            return Pointer;
        }
        for (rect, idx) in &self.control_rects {
            if point_in(*rect, x, y) {
                return match self.rows.get(*idx) {
                    Some(row)
                        if self.editing == Some(*idx)
                            || row.control == RowControl::Text =>
                    {
                        Text
                    }
                    Some(row) if row.control != RowControl::Keybinding => Pointer,
                    _ => Default,
                };
            }
        }
        Default
    }

    fn set_row_keyboard_focus(&mut self, visible: bool) {
        if let Some(handle) = self.row_component {
            if let Err(error) = self.row_ui.send(
                handle,
                super::neoui::SettingsRowsEvent::KeyboardFocusVisible(visible),
            ) {
                tracing::error!(%error, "Settings row focus event failed");
                self.row_geometry_valid = false;
                self.row_ui.invalidate_presentation();
            }
        }
    }

    fn clear_detail_hit_regions(&mut self) {
        self.set_row_keyboard_focus(false);
        self.row_ui.runtime_mut().blur();
        self.row_ui.invalidate_presentation();
        self.row_geometry_valid = false;
        self.row_viewport = [0.0; 4];
        self.control_rects.clear();
        self.dropdown_rects.clear();
        self.dropdown_search_rect = [0.0; 4];
        self.keybind_rects.clear();
        self.keybind_reset_rects.clear();
        self.hover_control = None;
    }

    pub fn open(&mut self) {
        self.row_ui.clear();
        self.row_component = None;
        self.clear_detail_hit_regions();
        self.category_rects.clear();
        self.active = true;
        self.search.clear();
        self.search_focused = false;
        self.open_dropdown = None;
        self.dropdown_search.clear();
        self.editing = None;
        self.edit_buffer.clear();
        self.capturing = None;
        self.scroll = 0.0;
        self.compact_detail = false;
    }

    pub fn close(&mut self) {
        self.row_ui.clear();
        self.row_component = None;
        self.clear_detail_hit_regions();
        self.active = false;
        self.search_focused = false;
        self.open_dropdown = None;
        self.editing = None;
        self.capturing = None;
        self.compact_detail = false;
    }

    pub fn set_values(&mut self, values: Value) {
        self.values = values;
    }

    /// Replace all canonical metadata with host-provided descriptors.
    pub fn set_descriptors(&mut self, descriptors: Vec<ConfigDescriptor>) {
        self.rows = descriptors
            .into_iter()
            .filter(|descriptor| {
                descriptor.settings_visible
                    && (!cfg!(target_arch = "wasm32")
                        || !(descriptor.path == "ui.window.background-image"
                            || descriptor
                                .path
                                .starts_with("ui.window.background-image.")))
                    && !descriptor.path.split('.').any(|segment| segment == "*")
            })
            .map(SettingRow::from_descriptor)
            .collect();
        self.rows.sort_by(|left, right| {
            Category::ALL
                .iter()
                .position(|category| *category == left.category)
                .cmp(
                    &Category::ALL
                        .iter()
                        .position(|category| *category == right.category),
                )
                .then_with(|| left.label.to_lowercase().cmp(&right.label.to_lowercase()))
                .then_with(|| left.path.cmp(&right.path))
        });
        self.open_dropdown = None;
        self.editing = None;
        self.clear_detail_hit_regions();
        self.category_rects.clear();
        if !self.navigation_categories().contains(&self.category) {
            self.category = Category::General;
        }
    }

    pub(crate) fn navigation_categories(&self) -> Vec<Category> {
        Category::ALL
            .into_iter()
            .filter(|category| {
                *category != Category::Platform
                    || self.rows.iter().any(|row| row.category == *category)
            })
            .collect()
    }

    pub fn descriptor_count(&self) -> usize {
        self.rows.len()
    }

    /// Host provides the installed system font families (sorted).
    pub fn set_font_families(&mut self, mut families: Vec<String>) {
        families.sort_by_key(|f| f.to_lowercase());
        families.dedup();
        self.font_families = families;
    }

    pub fn input_char(&mut self, c: char) {
        if !self.active || c.is_control() {
            return;
        }
        // An open font dropdown is auto-focused: typed characters filter
        // its (long) list before reaching the top-level settings search.
        if self.editing.is_some() {
            self.edit_buffer.insert(c.encode_utf8(&mut [0; 4]));
        } else if self.open_dropdown_is_searchable() {
            self.dropdown_search.push(c);
            self.dropdown_scroll = 0;
        } else if self.search_focused {
            self.search.push(c);
            self.scroll = 0.0;
            self.clear_detail_hit_regions();
        }
    }

    pub fn backspace(&mut self) {
        if !self.active {
            return;
        }
        if self.editing.is_some() {
            self.edit_buffer.backspace(false);
        } else if self.open_dropdown_is_searchable() {
            self.dropdown_search.pop();
            self.dropdown_scroll = 0;
        } else if self.search_focused {
            self.search.pop();
            self.clear_detail_hit_regions();
        }
    }

    pub fn on_escape(&mut self) -> bool {
        if !self.active {
            return false;
        }
        if self.editing.is_some() {
            self.editing = None;
            self.edit_buffer.clear();
        } else if self.capturing.is_some() {
            self.capturing = None;
        } else if self.open_dropdown.is_some() {
            // Clear an in-dropdown filter first, then close the dropdown.
            if !self.dropdown_search.is_empty() {
                self.dropdown_search.clear();
                self.dropdown_scroll = 0;
            } else {
                self.open_dropdown = None;
            }
        } else if self.compact_detail() {
            self.compact_detail = false;
            self.scroll = 0.0;
            self.search.clear();
            self.search_focused = false;
            self.back_rect = [0.0; 4];
            self.clear_detail_hit_regions();
        } else if self.search_focused && !self.search.is_empty() {
            self.search.clear();
            self.clear_detail_hit_regions();
        } else if self.search_focused {
            self.search_focused = false;
        } else {
            self.close();
        }
        true
    }

    pub fn scroll_by(&mut self, delta: f32) -> bool {
        if !self.active {
            return false;
        }
        // A one-notch step for the open dropdown, otherwise the content.
        if self.open_dropdown.is_some() {
            if delta > 0.0 {
                self.dropdown_scroll = self.dropdown_scroll.saturating_sub(1);
            } else {
                self.dropdown_scroll += 1;
            }
            return true;
        }
        let before = self.scroll;
        self.scroll = (self.scroll - delta).clamp(0.0, self.max_scroll);
        let changed = (self.scroll - before).abs() > f32::EPSILON;
        if changed {
            // Wheel scrolling can hide the focused control. Do not leave an
            // invisible switch owning Space/Enter after the next repaint.
            self.row_ui.runtime_mut().blur();
            self.set_row_keyboard_focus(false);
            self.row_ui.invalidate_presentation();
            self.row_geometry_valid = false;
            self.control_rects.clear();
        }
        changed
    }

    // ── accessors for the renderer ──
    pub(crate) fn is_search_focused(&self) -> bool {
        self.search_focused
    }
    pub(crate) fn current_category(&self) -> Category {
        self.category
    }
    pub(crate) fn search_is_empty(&self) -> bool {
        self.search.trim().is_empty()
    }
    pub(crate) fn search_query(&self) -> &str {
        &self.search
    }
    pub(crate) fn scroll_offset(&self) -> f32 {
        self.scroll
    }
    pub(crate) fn open_dropdown(&self) -> Option<usize> {
        self.open_dropdown
    }
    pub(crate) fn dropdown_scroll(&self) -> usize {
        self.dropdown_scroll
    }
    pub(crate) fn clamp_dropdown_scroll(&mut self, max: usize) {
        self.dropdown_scroll = self.dropdown_scroll.min(max);
    }
    pub(crate) fn dropdown_search_query(&self) -> &str {
        &self.dropdown_search
    }
    /// Long choice catalogs get an auto-focused search field. Fonts always
    /// qualify; the same interaction scales to models, themes, shells, etc.
    pub(crate) fn open_dropdown_is_searchable(&self) -> bool {
        self.open_dropdown
            .and_then(|index| self.rows.get(index))
            .is_some_and(|row| {
                row.control == RowControl::FontFamily || row.options.len() > 8
            })
    }

    pub(crate) fn row(&self, index: usize) -> &SettingRow {
        &self.rows[index]
    }
    pub(crate) fn is_editing(&self, index: usize) -> bool {
        self.editing == Some(index)
    }
    pub(crate) fn edit_buffer(&self) -> &str {
        self.edit_buffer.text()
    }

    fn begin_edit(&mut self, index: usize) {
        let text = self.string_at(index);
        // Even equal text belongs to a different editing session: do not carry
        // selection, IME composition or undo history across controls.
        self.edit_buffer = neoui::input::TextInput::default();
        self.edit_buffer.set_text(text);
        self.editing = Some(index);
    }

    /// Route normalized key, clipboard and IME events only to an active value
    /// editor. The host services clipboard requests and commits on `submitted`;
    /// persistence remains exclusively in commit_edit(). Search is independent.
    pub fn handle_edit_event(
        &mut self,
        event: &neoui::input::InputEvent,
    ) -> Option<neoui::input::EditOutcome> {
        if !self.active || self.editing.is_none() {
            return None;
        }
        Some(self.edit_buffer.handle(event))
    }

    /// UTF-8 byte offsets in committed text, not glyph columns.
    pub fn edit_selection(&self) -> Option<std::ops::Range<usize>> {
        self.editing.map(|_| self.edit_buffer.selection())
    }
    pub fn edit_caret(&self) -> Option<usize> {
        self.editing.map(|_| self.edit_buffer.caret())
    }

    /// Display-space offsets include IME preedit, without committing that text
    /// into the canonical value or the editor's undo history.
    pub(crate) fn edit_presentation(
        &self,
    ) -> (
        String,
        std::ops::Range<usize>,
        usize,
        Option<std::ops::Range<usize>>,
    ) {
        let text = self.edit_buffer.display_text();
        if let Some(composition) = self.edit_buffer.composition() {
            let start = composition.replacement.start;
            let preedit = start..start + composition.text.len();
            let selected = composition
                .selection
                .clone()
                .unwrap_or(composition.text.len()..composition.text.len());
            let selection = start + selected.start..start + selected.end;
            let caret = selection.end;
            (text, selection, caret, Some(preedit))
        } else {
            (
                text,
                self.edit_buffer.selection(),
                self.edit_buffer.caret(),
                None,
            )
        }
    }

    /// Commit the active generic text/number/JSON editor.
    pub fn commit_edit(&mut self) -> Option<SettingsAction> {
        let index = self.editing?;
        let row = self.rows.get(index)?;
        let value = parse_edited_value(self.edit_buffer.text(), row.value_kind)?;
        if let Some(number) = value.as_f64() {
            if row.constraints.min.is_some_and(|min| number < min)
                || row.constraints.max.is_some_and(|max| number > max)
            {
                return None;
            }
        }
        let path = row.path.clone();
        self.set_local(&path, value.clone());
        self.editing = None;
        self.edit_buffer.clear();
        Some(SettingsAction::Set { key: path, value })
    }

    // ── Keybinds section ──
    /// The Keybinds category is showing (and not overridden by a search).
    pub(crate) fn is_keybinds(&self) -> bool {
        self.category == Category::Keybinds && self.search.trim().is_empty()
    }
    /// Index into `KEYBINDS` currently waiting for a captured chord.
    pub fn capturing(&self) -> Option<usize> {
        self.capturing
    }
    pub fn cancel_capture(&mut self) {
        self.capturing = None;
    }
    /// Finish a capture with a resolved `(key, with)` chord: returns the
    /// persist action and leaves capture mode.
    pub fn finish_capture(
        &mut self,
        key: String,
        with: String,
    ) -> Option<SettingsAction> {
        let idx = self.capturing.take()?;
        Some(SettingsAction::SetKeybind {
            action: KEYBINDS[idx].action,
            key,
            with,
        })
    }
    fn keybind_override(&self, action: &str) -> Option<(String, String)> {
        let keys = self.values.get("keybinds")?.get("keys")?.as_array()?;
        keys.iter().find_map(|entry| {
            if entry.get("action").and_then(Value::as_str) == Some(action) {
                let key = entry
                    .get("key")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let with = entry
                    .get("with")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                Some((key, with))
            } else {
                None
            }
        })
    }
    pub(crate) fn keybind_group(&self, idx: usize) -> KeyGroup {
        KEYBINDS[idx].group
    }
    /// Only rows backed by a config action can be rebound; reference rows
    /// (hardcoded / vim-engine keys) are display-only.
    pub(crate) fn keybind_is_rebindable(&self, idx: usize) -> bool {
        !KEYBINDS[idx].action.is_empty()
    }
    /// Display combo — literal reference verbatim; else the config
    /// override if set; else the platform default; else "Not bound".
    pub(crate) fn keybind_display(&self, idx: usize) -> String {
        let def = KEYBINDS[idx];
        if !def.literal.is_empty() {
            return def.literal.to_string();
        }
        if !def.action.is_empty() {
            if let Some((key, with)) = self.keybind_override(def.action) {
                return format_combo(&with, &key);
            }
        }
        let (mods, key) = if cfg!(target_os = "macos") {
            (def.mac_mods, def.mac_key)
        } else {
            (def.other_mods, def.other_key)
        };
        if mods.is_empty() && key.is_empty() {
            "Not bound".to_string()
        } else {
            format_combo(mods, key)
        }
    }
    pub(crate) fn keybind_has_override(&self, idx: usize) -> bool {
        let def = KEYBINDS[idx];
        !def.action.is_empty() && self.keybind_override(def.action).is_some()
    }
    pub(crate) fn push_keybind_rect(&mut self, rect: [f32; 4], idx: usize) {
        self.keybind_rects.push((rect, idx));
    }
    pub(crate) fn push_keybind_reset_rect(&mut self, rect: [f32; 4], idx: usize) {
        self.keybind_reset_rects.push((rect, idx));
    }
    pub(crate) fn set_content_metrics(
        &mut self,
        content_height: f32,
        viewport_height: f32,
    ) {
        self.max_scroll = (content_height - viewport_height).max(0.0);
        if self.scroll > self.max_scroll {
            self.scroll = self.max_scroll;
        }
    }
    pub(crate) fn bool_at(&self, idx: usize) -> bool {
        let row = &self.rows[idx];
        if row.control != RowControl::Toggle {
            return false;
        }
        self.get_value(&row.path)
            .and_then(Value::as_bool)
            .unwrap_or_else(|| row.default.as_bool().unwrap_or(false))
    }
    pub(crate) fn string_at(&self, idx: usize) -> String {
        let row = &self.rows[idx];
        self.get_value(&row.path)
            .map(json_to_option_string)
            .unwrap_or_else(|| {
                if row.default.is_null() {
                    String::new()
                } else {
                    json_to_option_string(&row.default)
                }
            })
    }
    /// Options for the dropdown of a Select or FontFamily control.
    pub(crate) fn dropdown_options(&self, idx: usize) -> Vec<String> {
        let row = &self.rows[idx];
        let mut options = row.options.iter().map(option_label).collect::<Vec<_>>();
        if row.path == "appearance.theme" {
            options.extend(crate::primitives::ide_theme::all_ide_theme_names());
        }
        if row.control == RowControl::FontFamily {
            options.extend(self.font_families.clone());
        }
        if row.extensible {
            options.push(CUSTOM_OPTION.to_string());
        }
        options.sort_by_key(|value| value.to_lowercase());
        options.dedup();
        match row.control {
            RowControl::Select | RowControl::FontFamily => {
                let query = self.dropdown_search.trim().to_lowercase();
                if query.is_empty() {
                    options
                } else {
                    options
                        .into_iter()
                        .filter(|option| option.to_lowercase().contains(&query))
                        .collect()
                }
            }
            _ => Vec::new(),
        }
    }
    pub(crate) fn dropdown_visible_rows(&self) -> usize {
        DROPDOWN_MAX_ROWS
    }
    pub(crate) fn push_control_rect(&mut self, rect: [f32; 4], idx: usize) {
        self.control_rects.push((rect, idx));
    }
    pub(crate) fn push_dropdown_rect(&mut self, rect: [f32; 4], idx: usize, opt: String) {
        self.dropdown_rects.push((rect, idx, opt));
    }

    /// Read a value at a golden dotted path (`appearance.fonts.family`),
    /// descending one object per segment.
    fn get_value(&self, key: &str) -> Option<&Value> {
        let mut current = &self.values;
        for segment in key.split('.') {
            current = current.get(segment)?;
        }
        Some(current)
    }

    /// Mirror a written value into the local view at its golden dotted
    /// path, creating intermediate group objects as needed.
    fn set_local(&mut self, key: &str, value: Value) {
        if !self.values.is_object() {
            self.values = Value::Object(serde_json::Map::new());
        }
        let segments: Vec<&str> = key.split('.').collect();
        let Some((leaf, parents)) = segments.split_last() else {
            return;
        };
        let mut current = self.values.as_object_mut().expect("ensured object");
        for segment in parents {
            let entry = current
                .entry((*segment).to_string())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            if !entry.is_object() {
                *entry = Value::Object(serde_json::Map::new());
            }
            current = entry.as_object_mut().expect("forced object");
        }
        current.insert((*leaf).to_string(), value);
    }

    pub(crate) fn visible_settings(&self) -> Vec<usize> {
        let query = self.search.trim().to_lowercase();
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, def)| {
                if query.is_empty() || self.compact_detail() {
                    def.category == self.category
                        && (query.is_empty()
                            || def.label.to_lowercase().contains(&query)
                            || def.path.to_lowercase().contains(query.as_str())
                            || def.description.to_lowercase().contains(&query))
                } else {
                    def.label.to_lowercase().contains(&query)
                        || def.path.to_lowercase().contains(query.as_str())
                        || def.description.to_lowercase().contains(&query)
                }
            })
            .map(|(idx, _)| idx)
            .collect()
    }

    /// Tab traversal is scoped to the declarative controls, not the editor
    /// beneath the overlay. Text editors and open dropdowns retain ownership.
    pub fn focus_next_toggle(&mut self, backwards: bool) -> bool {
        if !self.active
            || !self.row_geometry_valid
            || self.open_dropdown.is_some()
            || self.editing.is_some()
            || self.capturing.is_some()
            || self.compact_root()
        {
            return false;
        }
        let Some(key) = self
            .row_ui
            .runtime_mut()
            .focus_next(backwards)
            .map(str::to_owned)
        else {
            return false;
        };
        self.search_focused = false;
        self.set_row_keyboard_focus(true);
        if let Some(bounds) = self.row_ui.runtime().frame().bounds_for(&key) {
            let [_, top, _, height] = self.row_viewport;
            let delta = if bounds.y < top {
                bounds.y - top
            } else {
                (bounds.bottom() - (top + height)).max(0.0)
            };
            let next = (self.scroll + delta).clamp(0.0, self.max_scroll);
            if next != self.scroll {
                self.scroll = next;
                self.row_geometry_valid = false;
                self.control_rects.clear();
            }
        }
        true
    }

    pub fn activate_focused_toggle(&mut self) -> Option<SettingsAction> {
        if !self.active
            || !self.row_geometry_valid
            || self.search_focused
            || self.open_dropdown.is_some()
            || self.editing.is_some()
            || self.capturing.is_some()
            || self.compact_root()
        {
            return None;
        }
        let key = self.row_ui.runtime().focused_key()?;
        let path = self
            .row_ui
            .runtime()
            .frame()
            .hits
            .iter()
            .find(|hit| hit.key == key)?
            .action
            .clone()?;
        self.toggle_path(path)
    }

    fn toggle_path(&mut self, path: String) -> Option<SettingsAction> {
        let index = self.visible_settings().into_iter().find(|&index| {
            let row = &self.rows[index];
            row.path == path && row.control == RowControl::Toggle
        })?;
        let value = Value::Bool(!self.bool_at(index));
        self.set_local(&path, value.clone());
        Some(SettingsAction::Set { key: path, value })
    }

    pub fn pointer_move(&mut self, x: f32, y: f32) {
        if !self.active {
            return;
        }
        self.hover_control = self
            .control_rects
            .iter()
            .find(|(rect, _)| point_in(*rect, x, y))
            .map(|(_, idx)| *idx);
        self.hover_category = self
            .category_rects
            .iter()
            .find(|(rect, _)| point_in(*rect, x, y))
            .map(|(_, cat)| *cat);
    }

    pub fn pointer_down(&mut self, x: f32, y: f32) -> PointerOutcome {
        if !self.active {
            return PointerOutcome::default();
        }

        self.set_row_keyboard_focus(false);
        self.row_ui.runtime_mut().blur();
        if self.open_dropdown.is_some() {
            // Clicking inside the font dropdown's search box keeps it open
            // (and focused) instead of dismissing it.
            if self.open_dropdown_is_searchable()
                && point_in(self.dropdown_search_rect, x, y)
            {
                return swallow();
            }
            for (rect, idx, opt) in self.dropdown_rects.clone() {
                if point_in(rect, x, y) {
                    let row = &self.rows[idx];
                    if opt == CUSTOM_OPTION {
                        self.begin_edit(idx);
                        self.open_dropdown = None;
                        self.dropdown_search.clear();
                        return swallow();
                    }
                    let value = row
                        .options
                        .iter()
                        .find(|option| option_label(option) == opt)
                        .map(|option| option.value.clone())
                        .unwrap_or_else(|| option_to_json(&opt, row.value_kind));
                    let path = row.path.clone();
                    self.set_local(&path, value.clone());
                    self.open_dropdown = None;
                    self.dropdown_search.clear();
                    return PointerOutcome {
                        consumed: true,
                        action: Some(SettingsAction::Set { key: path, value }),
                    };
                }
            }
            self.open_dropdown = None;
            self.dropdown_search.clear();
            return swallow();
        }

        if self.compact_detail() && point_in(self.back_rect, x, y) {
            if self.editing.is_some() {
                self.editing = None;
                self.edit_buffer.clear();
            } else if self.capturing.is_some() {
                self.capturing = None;
            } else {
                self.compact_detail = false;
                self.scroll = 0.0;
                self.search.clear();
                self.search_focused = false;
                self.clear_detail_hit_regions();
                self.category_rects.clear();
            }
            return swallow();
        }
        if point_in(self.close_rect, x, y) {
            self.close();
            return swallow();
        }
        if point_in(self.edit_json_rect, x, y) {
            return PointerOutcome {
                consumed: true,
                action: Some(SettingsAction::OpenConfigFile),
            };
        }
        if point_in(self.search_rect, x, y) {
            self.search_focused = true;
            return swallow();
        }
        self.search_focused = false;
        for (rect, cat) in self.category_rects.clone() {
            if point_in(rect, x, y) {
                self.category = cat;
                self.scroll = 0.0;
                self.capturing = None;
                self.editing = None;
                self.clear_detail_hit_regions();
                if self.compact_root() {
                    self.compact_detail = true;
                    self.category_rects.clear();
                }
                return swallow();
            }
        }
        if self.row_geometry_valid {
            let path = self.row_ui.runtime_mut().dispatch_pointer_down(x, y);
            // Settings intentionally activates on press; it has no drag gesture.
            self.row_ui.runtime_mut().release_pointer();
            if let Some(path) = path {
                return PointerOutcome {
                    consumed: true,
                    action: self.toggle_path(path),
                };
            }
        }
        // Keybind rows (Keybinds category): a reset glyph clears the
        // override; clicking the row starts capturing a new chord.
        for (rect, idx) in self.keybind_reset_rects.clone() {
            if point_in(rect, x, y) {
                self.capturing = None;
                return PointerOutcome {
                    consumed: true,
                    action: Some(SettingsAction::SetKeybind {
                        action: KEYBINDS[idx].action,
                        key: String::new(),
                        with: String::new(),
                    }),
                };
            }
        }
        for (rect, idx) in self.keybind_rects.clone() {
            if point_in(rect, x, y) {
                self.capturing = Some(idx);
                return swallow();
            }
        }
        for (rect, idx) in self.control_rects.clone() {
            if point_in(rect, x, y) {
                let row = self.rows[idx].clone();
                match row.control {
                    // Boolean controls are dispatched from NeoUI's resolved
                    // frame, never a separately maintained control rectangle.
                    RowControl::Toggle => return swallow(),
                    RowControl::Select | RowControl::FontFamily => {
                        self.open_dropdown = Some(idx);
                        self.dropdown_scroll = 0;
                        self.dropdown_search.clear();
                        return swallow();
                    }
                    RowControl::Text => {
                        self.begin_edit(idx);
                        self.open_dropdown = None;
                        return swallow();
                    }
                    RowControl::Keybinding => return swallow(),
                    RowControl::Action { action, .. } => {
                        return PointerOutcome {
                            consumed: true,
                            action: Some(SettingsAction::RunAction(action)),
                        };
                    }
                }
            }
        }
        swallow()
    }
}

fn parse_edited_value(text: &str, kind: ConfigValueKind) -> Option<Value> {
    match kind {
        ConfigValueKind::String => Some(Value::String(text.to_string())),
        ConfigValueKind::Integer => text
            .trim()
            .parse::<i64>()
            .ok()
            .map(|value| Value::Number(value.into())),
        ConfigValueKind::Number => text
            .trim()
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number),
        ConfigValueKind::Boolean => text.trim().parse::<bool>().ok().map(Value::Bool),
        ConfigValueKind::Array | ConfigValueKind::Object => {
            serde_json::from_str(text).ok()
        }
    }
}

fn json_to_option_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

fn option_label(option: &ConfigOption) -> String {
    option
        .label
        .clone()
        .unwrap_or_else(|| json_to_option_string(&option.value))
}

fn option_to_json(opt: &str, kind: ConfigValueKind) -> Value {
    parse_edited_value(opt, kind).unwrap_or_else(|| Value::String(opt.to_string()))
}

pub(crate) fn point_in(rect: [f32; 4], x: f32, y: f32) -> bool {
    rect[2] > 0.0
        && rect[3] > 0.0
        && x >= rect[0]
        && x <= rect[0] + rect[2]
        && y >= rect[1]
        && y <= rect[1] + rect[3]
}

#[cfg(test)]
mod descriptor_tests {
    use super::*;
    use serde_json::json;

    fn descriptor(
        path: &str,
        category: ConfigCategory,
        control: ConfigControl,
        kind: ConfigValueKind,
        default: Value,
    ) -> ConfigDescriptor {
        ConfigDescriptor {
            path: path.to_string(),
            label: path.to_string(),
            description: format!("Configure {path}."),
            value_kind: kind,
            default,
            static_suggestions: vec!["static".into(), "shared".into()],
            runtime_suggestions: vec!["runtime".into(), "shared".into()],
            options: vec![],
            provider: None,
            constraints: Default::default(),
            accepted_kinds: vec![],
            extensible: true,
            category,
            control,
            settings_visible: true,
        }
    }

    fn mount_toggles(pane: &mut NeoismSettingsPane, clip: [f32; 4]) {
        let theme = crate::primitives::ide_theme::IdeTheme::default();
        let props = super::super::neoui::SettingsRowsProps {
            rows: pane
                .visible_settings()
                .into_iter()
                .filter(|&idx| pane.row(idx).control == RowControl::Toggle)
                .map(|idx| {
                    let row = pane.row(idx);
                    super::super::neoui::RowSnapshot {
                        path: row.path.clone(),
                        label: row.label.clone(),
                        detail: row.description.clone(),
                        checked: Some(pane.bool_at(idx)),
                        legacy_height: 0.0,
                    }
                })
                .collect(),
            width: 300.0,
            scale: 1.0,
            palette: super::super::neoui::RowPalette::new(&theme),
            focused_key: pane.row_ui.runtime().focused_key().map(str::to_owned),
        };
        pane.row_component = Some(
            pane.row_ui
                .set_root::<super::super::neoui::SettingsRows>("settings/rows", props)
                .unwrap(),
        );
        pane.row_ui
            .present(
                [10.0, 100.0],
                300.0,
                crate::neoui_renderer::rect(clip),
                &mut |text: &str, style: &neoui::TextStyle| {
                    text.chars().count() as f32 * style.size * 0.6
                },
            )
            .unwrap();
        pane.row_viewport = clip;
        pane.row_geometry_valid = true;
    }

    fn toggle_center(pane: &NeoismSettingsPane, path: &str) -> (f32, f32) {
        let rect = pane
            .row_ui
            .runtime()
            .frame()
            .bounds_for(&super::super::neoui::toggle_key(path))
            .unwrap();
        (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5)
    }

    fn toggle_pane() -> NeoismSettingsPane {
        let mut pane = NeoismSettingsPane::new();
        pane.set_descriptors(vec![descriptor(
            "general.toggle",
            ConfigCategory::General,
            ConfigControl::Toggle,
            ConfigValueKind::Boolean,
            json!(false),
        )]);
        pane.open();
        mount_toggles(&mut pane, [0.0, 0.0, 600.0, 1000.0]);
        pane
    }

    #[test]
    fn neoui_toggle_uses_current_model_and_emits_one_persistence_action() {
        let mut pane = toggle_pane();
        let (x, y) = toggle_center(&pane, "general.toggle");
        pane.set_values(json!({"general": {"toggle": true}}));
        let outcome = pane.pointer_down(x, y);
        assert!(outcome.consumed);
        assert!(
            matches!(outcome.action, Some(SettingsAction::Set { key, value })
            if key == "general.toggle" && value == json!(false))
        );
        assert_eq!(pane.get_value("general.toggle"), Some(&json!(false)));
        assert!(
            pane.control_rects.is_empty(),
            "no parallel boolean hit rectangles"
        );
        assert!(pane.row_ui.runtime().pressed_key().is_none());
    }

    #[test]
    fn neoui_dropdown_dismissal_never_activates_a_toggle_underneath() {
        let mut pane = toggle_pane();
        let (x, y) = toggle_center(&pane, "general.toggle");
        pane.open_dropdown = Some(0);
        assert!(pane.pointer_down(x, y).action.is_none());
        assert_eq!(pane.get_value("general.toggle"), None);
        assert!(pane.open_dropdown.is_none());
        assert!(pane.pointer_down(x, y).action.is_some());
    }

    #[test]
    fn neoui_toggle_cannot_activate_outside_viewport_or_at_half_open_edge() {
        let mut pane = toggle_pane();
        let (x, y) = toggle_center(&pane, "general.toggle");
        mount_toggles(&mut pane, [0.0, 0.0, x, 1000.0]);
        assert!(pane.pointer_down(x, y).action.is_none());
        assert!(pane.pointer_down(x - 1.0, y).action.is_some());
        assert!(pane.pointer_down(x + 5.0, y).action.is_none());
    }

    #[test]
    fn neoui_keyboard_focus_activation_and_text_entry_ownership() {
        let mut pane = toggle_pane();
        assert!(pane.activate_focused_toggle().is_none());
        assert!(pane.focus_next_toggle(false));
        assert!(pane.row_ui.runtime().focused_key().is_some());
        assert!(
            matches!(pane.activate_focused_toggle(), Some(SettingsAction::Set { value, .. })
            if value == json!(true))
        );
        pane.search_focused = true;
        assert!(pane.activate_focused_toggle().is_none());
        pane.input_char(' ');
        assert_eq!(pane.search, " ");
        assert!(pane.row_ui.runtime().frame().hits.is_empty());
    }

    #[test]
    fn neoui_search_navigation_scroll_and_close_reject_stale_geometry() {
        let mut pane = toggle_pane();
        let (x, y) = toggle_center(&pane, "general.toggle");
        pane.search_focused = true;
        pane.input_char('z');
        assert!(pane.pointer_down(x, y).action.is_none());
        pane.search.clear();
        mount_toggles(&mut pane, [0.0, 0.0, 600.0, 1000.0]);
        pane.set_content_metrics(2000.0, 300.0);
        assert!(pane.scroll_by(-40.0));
        assert!(pane.pointer_down(x, y).action.is_none());
        pane.scroll = 0.0;
        mount_toggles(&mut pane, [0.0, 0.0, 600.0, 1000.0]);
        pane.category_rects
            .push(([400.0, 0.0, 100.0, 50.0], Category::Editor));
        assert!(pane.pointer_down(410.0, 10.0).consumed);
        assert!(pane.row_ui.runtime().frame().hits.is_empty());
        pane.category = Category::General;
        mount_toggles(&mut pane, [0.0, 0.0, 600.0, 1000.0]);
        pane.close();
        assert!(pane.row_ui.runtime().frame().hits.is_empty());
        assert!(pane.pointer_down(x, y).action.is_none());
    }

    #[test]
    fn neoui_wheel_scrolling_relinquishes_invisible_keyboard_ownership() {
        let mut pane = toggle_pane();
        assert!(pane.focus_next_toggle(false));
        pane.set_content_metrics(2000.0, 100.0);
        assert!(pane.scroll_by(-400.0));
        assert!(pane.row_ui.runtime().focused_key().is_none());
        assert!(pane.row_ui.runtime().focused_key().is_none());
        mount_toggles(&mut pane, [0.0, 100.0, 600.0, 100.0]);
        assert!(pane.activate_focused_toggle().is_none());
        assert_eq!(pane.get_value("general.toggle"), None);
    }

    #[test]
    fn neoui_keyboard_focus_scrolls_offscreen_controls_before_activation() {
        let mut pane = toggle_pane();
        let mut second = pane.rows[0].clone();
        second.path = "general.second".into();
        pane.rows.push(second);
        mount_toggles(&mut pane, [0.0, 100.0, 600.0, 100.0]);
        pane.set_content_metrics(pane.row_ui.runtime().frame().bounds.height, 100.0);
        assert!(pane.focus_next_toggle(false));
        assert!(pane.focus_next_toggle(false));
        assert!(pane.scroll_offset() > 0.0);
        assert!(!pane.row_geometry_valid);
        assert!(
            pane.activate_focused_toggle().is_none(),
            "wait for a frame at the new scroll position"
        );
    }

    #[test]
    fn empty_platform_is_hidden_but_real_platform_settings_remain_accessible() {
        let mut pane = NeoismSettingsPane::new();
        assert!(!pane.navigation_categories().contains(&Category::Platform));
        let mut hidden = descriptor(
            "platform.hidden",
            ConfigCategory::Platform,
            ConfigControl::Toggle,
            ConfigValueKind::Boolean,
            json!(false),
        );
        hidden.settings_visible = false;
        pane.set_descriptors(vec![hidden]);
        assert!(!pane.navigation_categories().contains(&Category::Platform));
        pane.set_descriptors(vec![descriptor(
            "platform.option",
            ConfigCategory::Platform,
            ConfigControl::Toggle,
            ConfigValueKind::Boolean,
            json!(false),
        )]);
        assert!(pane.navigation_categories().contains(&Category::Platform));
    }

    #[test]
    fn renderer_settings_live_under_developer_and_keep_their_persistence_paths() {
        let mut pane = NeoismSettingsPane::new();
        pane.set_descriptors(vec![
            descriptor(
                "renderer.option",
                ConfigCategory::Renderer,
                ConfigControl::Toggle,
                ConfigValueKind::Boolean,
                json!(false),
            ),
            descriptor(
                "developer.option",
                ConfigCategory::Developer,
                ConfigControl::Toggle,
                ConfigValueKind::Boolean,
                json!(false),
            ),
        ]);
        pane.open();
        pane.category = Category::Developer;
        assert_eq!(pane.visible_settings().len(), 2);
        let idx = pane
            .rows
            .iter()
            .position(|row| row.path == "renderer.option")
            .unwrap();
        assert_eq!(pane.row(idx).category, Category::Developer);
        mount_toggles(&mut pane, [0.0, 0.0, 600.0, 1000.0]);
        let (x, y) = toggle_center(&pane, "renderer.option");
        let outcome = pane.pointer_down(x, y);
        assert!(
            matches!(outcome.action, Some(SettingsAction::Set { key, value }) if key == "renderer.option" && value == json!(true))
        );
        pane.search = "renderer.option".into();
        assert_eq!(pane.visible_settings(), vec![idx]);
    }

    #[test]
    fn settings_cursor_distinguishes_clickable_controls_text_and_blank_space() {
        use crate::widgets::modal::ModalPointerKind::{Default, Pointer, Text};
        let mut pane = NeoismSettingsPane::new();
        pane.set_descriptors(vec![
            descriptor(
                "general.toggle",
                ConfigCategory::General,
                ConfigControl::Toggle,
                ConfigValueKind::Boolean,
                json!(false),
            ),
            descriptor(
                "general.select",
                ConfigCategory::General,
                ConfigControl::Select,
                ConfigValueKind::String,
                json!("value"),
            ),
            descriptor(
                "general.font",
                ConfigCategory::General,
                ConfigControl::FontFamily,
                ConfigValueKind::String,
                json!(""),
            ),
            descriptor(
                "general.text",
                ConfigCategory::General,
                ConfigControl::Text,
                ConfigValueKind::String,
                json!(""),
            ),
            descriptor(
                "agent.model",
                ConfigCategory::Agent,
                ConfigControl::Text,
                ConfigValueKind::String,
                json!(""),
            ),
        ]);
        pane.open();
        pane.search_rect = [0.0, 0.0, 100.0, 20.0];
        pane.close_rect = [110.0, 0.0, 20.0, 20.0];
        pane.edit_json_rect = [140.0, 0.0, 20.0, 20.0];
        pane.category_rects
            .push(([0.0, 30.0, 100.0, 25.0], Category::Developer));
        assert_eq!(pane.pointer_kind_at(10.0, 10.0), Text);
        assert_eq!(pane.pointer_kind_at(115.0, 10.0), Pointer);
        assert_eq!(pane.pointer_kind_at(145.0, 10.0), Pointer);
        assert_eq!(pane.pointer_kind_at(10.0, 40.0), Pointer);
        assert_eq!(pane.pointer_kind_at(200.0, 40.0), Default);
        mount_toggles(&mut pane, [0.0, 0.0, 600.0, 1000.0]);
        let (x, y) = toggle_center(&pane, "general.toggle");
        assert_eq!(pane.pointer_kind_at(x, y), Pointer);
        for idx in 0..pane.rows.len() {
            if pane.rows[idx].control == RowControl::Toggle {
                continue;
            }
            pane.push_control_rect([idx as f32 * 50.0, 100.0, 40.0, 24.0], idx);
            let expected = if pane.rows[idx].control == RowControl::Text {
                Text
            } else {
                Pointer
            };
            assert_eq!(
                pane.pointer_kind_at(idx as f32 * 50.0 + 10.0, 110.0),
                expected
            );
        }
        pane.category = Category::Keybinds;
        pane.keybind_rects.push(([0.0, 150.0, 80.0, 28.0], 0));
        pane.keybind_reset_rects
            .push(([90.0, 150.0, 24.0, 24.0], 0));
        assert!(pane.text_entry_at(10.0, 160.0));
        assert_eq!(pane.pointer_kind_at(10.0, 160.0), Pointer);
        assert_eq!(pane.pointer_kind_at(100.0, 160.0), Pointer);
        pane.close();
        assert_eq!(pane.pointer_kind_at(10.0, 10.0), Default);
    }

    #[test]
    fn dropdown_cursor_owns_underlying_targets_and_custom_values_use_text() {
        use crate::widgets::modal::ModalPointerKind::{Default, Pointer, Text};
        let mut pane = NeoismSettingsPane::new();
        pane.set_descriptors(vec![descriptor(
            "appearance.font",
            ConfigCategory::Appearance,
            ConfigControl::FontFamily,
            ConfigValueKind::String,
            json!(""),
        )]);
        pane.open();
        pane.push_control_rect([0.0, 100.0, 120.0, 28.0], 0);
        pane.open_dropdown = Some(0);
        pane.dropdown_search_rect = [0.0, 140.0, 120.0, 24.0];
        pane.push_dropdown_rect([0.0, 170.0, 120.0, 28.0], 0, "Font".into());
        assert_eq!(pane.pointer_kind_at(10.0, 150.0), Text);
        assert_eq!(pane.pointer_kind_at(10.0, 180.0), Pointer);
        assert_eq!(pane.pointer_kind_at(10.0, 110.0), Default);
        pane.open_dropdown = None;
        assert_eq!(pane.pointer_kind_at(10.0, 150.0), Default);
        assert_eq!(pane.pointer_kind_at(10.0, 180.0), Default);
        pane.editing = Some(0);
        assert_eq!(pane.pointer_kind_at(10.0, 110.0), Text);
    }

    #[test]
    fn navigation_discards_previous_page_cursor_targets_before_repaint() {
        use crate::widgets::modal::ModalPointerKind::{Default, Pointer};
        let mut pane = NeoismSettingsPane::new();
        pane.open();
        pane.set_compact_layout(true);
        pane.category_rects
            .push(([0.0, 40.0, 180.0, 40.0], Category::Keybinds));
        assert_eq!(pane.pointer_kind_at(10.0, 50.0), Pointer);
        assert!(pane.pointer_down(10.0, 50.0).consumed);
        assert_eq!(pane.pointer_kind_at(10.0, 50.0), Default);
        pane.back_rect = [0.0, 0.0, 30.0, 30.0];
        pane.keybind_rects.push(([0.0, 100.0, 180.0, 28.0], 0));
        assert_eq!(pane.pointer_kind_at(10.0, 110.0), Pointer);
        assert!(pane.pointer_down(10.0, 10.0).consumed);
        assert_eq!(pane.pointer_kind_at(10.0, 110.0), Default);
    }

    #[test]
    fn descriptors_are_owned_visible_rows_and_map_every_category() {
        let mut pane = NeoismSettingsPane::new();
        let descriptors = Category::ALL
            .into_iter()
            .enumerate()
            .map(|(index, category)| {
                let protocol = match category {
                    Category::General => ConfigCategory::General,
                    Category::Appearance => ConfigCategory::Appearance,
                    Category::Editor => ConfigCategory::Editor,
                    Category::Terminal => ConfigCategory::Terminal,
                    Category::Ui => ConfigCategory::Ui,
                    Category::Presence => ConfigCategory::Presence,
                    Category::Keybinds => ConfigCategory::Keybinds,
                    Category::Agent => ConfigCategory::Agent,
                    Category::Platform => ConfigCategory::Platform,
                    Category::Developer => ConfigCategory::Developer,
                };
                descriptor(
                    &format!("group.setting-{index}"),
                    protocol,
                    ConfigControl::Text,
                    ConfigValueKind::String,
                    json!(""),
                )
            })
            .collect();
        pane.set_descriptors(descriptors);
        assert_eq!(pane.descriptor_count(), Category::ALL.len());
        for category in Category::ALL {
            pane.category = category;
            assert_eq!(pane.visible_settings().len(), 1, "missing {category:?}");
        }
    }

    #[test]
    fn completion_only_and_wildcard_descriptors_never_become_settings_rows() {
        let mut pane = NeoismSettingsPane::new();
        let visible = descriptor(
            "appearance.theme",
            ConfigCategory::Appearance,
            ConfigControl::Text,
            ConfigValueKind::String,
            json!("default"),
        );
        let mut generated = descriptor(
            "platform.windows.window.mode",
            ConfigCategory::Platform,
            ConfigControl::Text,
            ConfigValueKind::String,
            json!("Windowed"),
        );
        generated.settings_visible = false;
        let wildcard = descriptor(
            "agent.agent.*.model",
            ConfigCategory::Agent,
            ConfigControl::Text,
            ConfigValueKind::String,
            Value::Null,
        );

        pane.set_descriptors(vec![visible, generated, wildcard]);
        assert_eq!(pane.descriptor_count(), 1);
        assert_eq!(pane.rows[0].path, "appearance.theme");
        pane.search = "mode".into();
        assert!(pane.visible_settings().is_empty());
    }

    #[test]
    fn suggestions_merge_deduplicate_and_keep_special_controls() {
        let mut pane = NeoismSettingsPane::new();
        pane.set_descriptors(vec![
            descriptor(
                "appearance.fonts.family",
                ConfigCategory::Appearance,
                ConfigControl::FontFamily,
                ConfigValueKind::String,
                json!(""),
            ),
            descriptor(
                "agent.model",
                ConfigCategory::Agent,
                ConfigControl::Select,
                ConfigValueKind::String,
                Value::Null,
            ),
        ]);
        pane.set_font_families(vec!["Host Font".into(), "shared".into()]);
        let font = pane
            .rows
            .iter()
            .position(|row| row.path == "appearance.fonts.family")
            .unwrap();
        assert_eq!(
            pane.dropdown_options(font),
            vec!["Custom\u{2026}", "Host Font", "runtime", "shared", "static"]
        );
        let model = pane
            .rows
            .iter()
            .find(|row| row.path == "agent.model")
            .unwrap();
        assert!(matches!(
            model.control,
            RowControl::Action {
                action: "open-model",
                ..
            }
        ));
    }

    #[test]
    fn generic_number_editor_emits_owned_persistence_action() {
        let mut pane = NeoismSettingsPane::new();
        let mut number = descriptor(
            "terminal.scroll.multiplier",
            ConfigCategory::Terminal,
            ConfigControl::Number,
            ConfigValueKind::Number,
            json!(3.0),
        );
        number.static_suggestions.clear();
        number.runtime_suggestions.clear();
        number.options.clear();
        pane.set_descriptors(vec![number]);
        pane.open();
        pane.control_rects.push(([0.0, 0.0, 20.0, 20.0], 0));
        pane.pointer_down(5.0, 5.0);
        pane.edit_buffer.clear();
        pane.input_char('4');
        pane.input_char('.');
        pane.input_char('5');
        assert_eq!(
            pane.commit_edit(),
            Some(SettingsAction::Set {
                key: "terminal.scroll.multiplier".to_string(),
                value: json!(4.5),
            })
        );
    }

    #[test]
    fn number_presets_keep_typed_values_and_use_a_picker() {
        let mut number = descriptor(
            "appearance.fonts.size",
            ConfigCategory::Appearance,
            ConfigControl::Number,
            ConfigValueKind::Number,
            json!(14.0),
        );
        number.static_suggestions.clear();
        number.runtime_suggestions.clear();
        number.options = vec![ConfigOption {
            value: json!(16.5),
            label: Some("Large".into()),
            description: None,
        }];
        let row = SettingRow::from_descriptor(number);
        assert_eq!(row.control, RowControl::Select);
        assert_eq!(row.options[0].value, json!(16.5));
    }

    #[test]
    fn compact_category_pushes_detail_then_escape_pops_before_close() {
        let mut pane = NeoismSettingsPane::new();
        pane.open();
        pane.set_compact_layout(true);
        pane.category_rects
            .push(([10.0, 70.0, 370.0, 54.0], Category::Appearance));

        pane.pointer_down(20.0, 80.0);
        assert!(pane.compact_detail());
        assert_eq!(pane.current_category(), Category::Appearance);
        assert!(pane.is_active());

        assert!(pane.on_escape());
        assert!(pane.compact_root());
        assert!(pane.is_active());
        assert!(pane.on_escape());
        assert!(!pane.is_active());
    }

    #[test]
    fn compact_escape_resolves_detail_interactions_before_navigation() {
        let mut pane = NeoismSettingsPane::new();
        pane.open();
        pane.set_compact_layout(true);
        pane.compact_detail = true;
        pane.open_dropdown = Some(0);

        pane.on_escape();
        assert!(pane.compact_detail());
        assert!(pane.open_dropdown.is_none());
        pane.search = "font".into();
        pane.search_focused = true;
        pane.on_escape();
        assert!(pane.compact_root());
        assert!(pane.search.is_empty());
    }

    #[test]
    fn crossing_compact_breakpoint_returns_to_category_root() {
        let mut pane = NeoismSettingsPane::new();
        pane.open();
        pane.set_compact_layout(true);
        pane.compact_detail = true;
        pane.set_compact_layout(false);
        assert!(!pane.compact_detail());
        pane.set_compact_layout(true);
        assert!(pane.compact_root());
    }
}

#[cfg(test)]
mod background_action_tests {
    use super::*;

    #[test]
    fn choose_background_descriptor_uses_existing_run_action_control() {
        let descriptor: ConfigDescriptor = serde_json::from_value(serde_json::json!({
            "path": "ui.window.background-image", "label": "Backgrounds", "description": "Choose a wallpaper",
            "value_kind": "object", "default": null, "extensible": true,
            "category": "ui", "control": { "kind": "object" }, "settings_visible": true
        })).unwrap();
        let row = SettingRow::from_descriptor(descriptor);
        assert_eq!(
            row.control,
            RowControl::Action {
                action: "backgrounds:pick",
                button: "Choose\u{2026}"
            }
        );
    }
}
