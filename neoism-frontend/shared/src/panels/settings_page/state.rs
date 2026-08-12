//! Zed-style Settings panel — data model + interaction.
//!
//! A full-screen settings surface over the unified `config.json`. Keys
//! may be flat (`minimap`) or one level deep (`cursor.blinking`); the
//! dotted form maps to a `[section]` object both when reading and
//! writing. The host feeds the raw `config.json` value plus the list of
//! installed font families, and persists any [`SettingsAction`].

use serde_json::Value;

#[derive(Debug, Clone, Copy)]
pub(crate) enum Control {
    Toggle {
        default: bool,
    },
    /// One-of-N. Clicking opens a dropdown of the options.
    Select {
        options: &'static [&'static str],
        default: &'static str,
    },
    /// A dropdown of installed system fonts (options supplied at runtime
    /// by the host). Bound to `fonts.family`.
    FontFamily,
    /// A row that runs a host GUI action instead of writing a value.
    Action {
        action: &'static str,
        button: &'static str,
    },
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SettingDef {
    pub category: Category,
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub control: Control,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    General,
    Appearance,
    Editor,
    Terminal,
    Keybinds,
    Agent,
    Developer,
}

impl Category {
    pub(crate) const ALL: [Category; 7] = [
        Category::General,
        Category::Appearance,
        Category::Editor,
        Category::Terminal,
        Category::Keybinds,
        Category::Agent,
        Category::Developer,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Category::General => "General",
            Category::Appearance => "Appearance",
            Category::Editor => "Editor",
            Category::Terminal => "Terminal",
            Category::Keybinds => "Keybinds",
            Category::Agent => "Agent",
            Category::Developer => "Developer",
        }
    }

    pub(crate) fn icon(self) -> &'static str {
        match self {
            Category::General => "\u{f013}",
            Category::Appearance => "\u{f1fc}",
            Category::Editor => "\u{f044}",
            Category::Terminal => "\u{f120}",
            Category::Keybinds => "\u{f11c}",
            Category::Agent => "\u{f544}",
            Category::Developer => "\u{f188}",
        }
    }
}

pub(crate) const SETTINGS: &[SettingDef] = &[
    // ── General ──
    tog(Category::General, "ui.confirm-before-quit", "Confirm before quit", "Ask for confirmation before closing the app.", false),
    tog(Category::General, "terminal.copy-on-select", "Copy on select", "Automatically copy selected text to the clipboard.", false),
    tog(Category::General, "terminal.hide-mouse-cursor-when-typing", "Hide mouse cursor when typing", "Hide the pointer while you type.", false),
    tog(Category::General, "terminal.use-fork", "Use fork (new terminals)", "Spawn shells with fork(); applies to newly opened terminals.", true),
    sel(Category::General, "terminal.option-as-alt", "Option as Alt (macOS)", "Treat the Option key as Alt.", &["none", "left", "right", "both"], "none"),

    // ── Appearance ──
    sel(Category::Appearance, "appearance.theme", "Theme", "The IDE theme — skins chrome, editor, and terminal.", &["pastel_dark", "nvchad_one", "tokyo_night", "catppuccin_mocha", "retro_95"], "pastel_dark"),
    font_family(Category::Appearance, "appearance.fonts.family", "Font family", "Choose an installed system font for the terminal and editor."),
    sel(Category::Appearance, "appearance.fonts.size", "Font size", "Terminal + editor font size.", &["12", "13", "14", "16", "18", "19", "20", "22"], "14"),
    sel(Category::Appearance, "appearance.fonts.weight", "Font weight", "Base text thickness. 400 is normal, 700 is bold. Bold stays bold.", &["300", "400", "500", "600", "700", "800"], "400"),
    sel(Category::Appearance, "presence.cursor-style", "Cursor style", "Solid caret, or an animated rainbow sweep.", &["solid", "rainbow"], "solid"),
    tog(Category::Appearance, "ui.status-fps", "Status bar FPS", "Show the frame-rate pill on the status bar.", true),
    sel(Category::Appearance, "appearance.line-height", "Line height", "Terminal line-height multiplier.", &["1.0", "1.1", "1.2", "1.3", "1.4", "1.5"], "1.0"),
    sel(Category::Appearance, "terminal.cursor.shape", "Cursor shape", "Block, underline, beam, or hidden caret.", &["block", "underline", "beam", "hidden"], "block"),
    tog(Category::Appearance, "terminal.cursor.blinking", "Blinking cursor", "Blink the caret.", false),
    sel(Category::Appearance, "terminal.cursor.blinking-interval", "Blink interval (ms)", "Caret blink half-period.", &["400", "530", "700", "1000"], "530"),
    tog(Category::Appearance, "appearance.fonts.hinting", "Font hinting", "Hint glyphs to the pixel grid.", true),
    tog(Category::Appearance, "appearance.fonts.use-drawable-chars", "Native box drawing", "Draw box/block glyphs natively.", true),
    sel(Category::Appearance, "ui.window.opacity", "Window opacity", "Overall window opacity.", &["0.8", "0.9", "0.95", "1.0"], "1.0"),
    tog(Category::Appearance, "ui.window.blur", "Background blur", "Blur what's behind a translucent window.", false),
    tog(Category::Appearance, "appearance.effects.trail-cursor", "Cursor trail", "Neovide-style cursor trail.", true),
    tog(Category::Appearance, "appearance.effects.custom-mouse-cursor", "Custom mouse cursor", "Render the mouse pointer ourselves.", false),

    // ── Editor ──
    tog(Category::Editor, "editor.vim-mode", "Vim mode", "Vim keybindings in the code AND markdown editors. Applies to editors you open next.", true),
    tog(Category::Editor, "editor.format-on-save", "Format on save", "Run the language server's formatter before every save.", true),
    tog(Category::Editor, "editor.minimap", "Minimap", "Show the code-editor minimap.", false),

    // ── Terminal ──
    tog(Category::Terminal, "terminal.enable-scroll-bar", "Scrollbar", "Show the terminal scrollbar.", true),
    sel(Category::Terminal, "terminal.scrollback-history-limit", "Scrollback lines", "How many lines of scrollback to keep.", &["1000", "5000", "10000", "50000", "100000"], "10000"),
    sel(Category::Terminal, "terminal.scroll.multiplier", "Scroll speed", "Scroll wheel multiplier.", &["1", "2", "3", "4", "5"], "3"),
    tog(Category::Terminal, "terminal.draw-bold-text-with-light-colors", "Bold uses bright colors", "Bold text draws in the bright ANSI palette.", false),
    tog(Category::Terminal, "ui.navigation.hide-if-single", "Hide tab bar when single", "Hide the tab strip with only one tab.", true),
    tog(Category::Terminal, "ui.navigation.use-split", "Enable splits", "Allow split panes.", true),
    tog(Category::Terminal, "ui.navigation.open-config-with-split", "Open config in a split", "Open the config file beside the current pane.", true),
    tog(Category::Terminal, "ui.navigation.current-working-directory", "New tab inherits CWD", "New tabs start in the current working directory.", true),
    tog(Category::Terminal, "terminal.keyboard.ime-cursor-positioning", "IME at cursor", "Position the IME popup at the caret.", true),
    tog(Category::Terminal, "terminal.bell.audio", "Audible bell", "Play the system sound on a terminal bell.", false),

    // ── Agent ── (only settings the neoism agent actually reads)
    action(Category::Agent, "Model & providers", "Pick the agent model, or connect a provider (API key / OAuth).", "open-model", "Choose\u{2026}"),
    sel(Category::Agent, "agent.reasoning-effort", "Reasoning effort", "How hard the model thinks.", &["low", "medium", "high", "xhigh", "max"], "medium"),
    sel(Category::Agent, "agent.text-verbosity", "Response length", "How detailed supported models make their final text.", &["low", "medium", "high"], "low"),
    tog(Category::Agent, "agent.dangerously-skip-permissions", "Skip permission prompts", "Auto-allow agent actions that would prompt. Explicit deny rules still deny.", false),

    // ── Developer ──
    sel(Category::Developer, "developer.log-level", "Log level", "Tracing verbosity (applies on next launch).", &["off", "error", "warn", "info", "debug", "trace"], "off"),
    tog(Category::Developer, "developer.enable-log-file", "Write log file", "Write logs to ~/.config/neoism/log/neoism.log.", false),
    tog(Category::Developer, "developer.enable-fps-counter", "FPS counter", "Show a developer FPS counter overlay.", false),
];

const fn tog(
    category: Category,
    key: &'static str,
    label: &'static str,
    description: &'static str,
    default: bool,
) -> SettingDef {
    SettingDef {
        category,
        key,
        label,
        description,
        control: Control::Toggle { default },
    }
}

const fn sel(
    category: Category,
    key: &'static str,
    label: &'static str,
    description: &'static str,
    options: &'static [&'static str],
    default: &'static str,
) -> SettingDef {
    SettingDef {
        category,
        key,
        label,
        description,
        control: Control::Select { options, default },
    }
}

const fn font_family(
    category: Category,
    key: &'static str,
    label: &'static str,
    description: &'static str,
) -> SettingDef {
    SettingDef {
        category,
        key,
        label,
        description,
        control: Control::FontFamily,
    }
}

const fn action(
    category: Category,
    label: &'static str,
    description: &'static str,
    action: &'static str,
    button: &'static str,
) -> SettingDef {
    SettingDef {
        category,
        key: "",
        label,
        description,
        control: Control::Action { action, button },
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
        "super",
        ",",
        "control | shift",
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
    Set { key: &'static str, value: Value },
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

pub struct NeoismSettingsPane {
    active: bool,
    values: Value,
    font_families: Vec<String>,
    category: Category,
    search: String,
    search_focused: bool,
    open_dropdown: Option<usize>,
    dropdown_scroll: usize,
    /// Filter text typed while a long (font) dropdown is open.
    dropdown_search: String,
    /// Index into `KEYBINDS` currently waiting for a captured chord.
    capturing: Option<usize>,
    scroll: f32,
    max_scroll: f32,
    pub(crate) panel_rect: [f32; 4],
    pub(crate) category_rects: Vec<([f32; 4], Category)>,
    pub(crate) control_rects: Vec<([f32; 4], usize)>,
    pub(crate) dropdown_rects: Vec<([f32; 4], usize, String)>,
    pub(crate) dropdown_search_rect: [f32; 4],
    /// Keybind row rects (click → capture a new chord) and their reset
    /// glyph rects (click → clear the override).
    pub(crate) keybind_rects: Vec<([f32; 4], usize)>,
    pub(crate) keybind_reset_rects: Vec<([f32; 4], usize)>,
    pub(crate) search_rect: [f32; 4],
    pub(crate) edit_json_rect: [f32; 4],
    pub(crate) close_rect: [f32; 4],
    pub(crate) hover_control: Option<usize>,
    pub(crate) hover_category: Option<Category>,
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
            category: Category::General,
            search: String::new(),
            search_focused: false,
            open_dropdown: None,
            dropdown_scroll: 0,
            dropdown_search: String::new(),
            capturing: None,
            scroll: 0.0,
            max_scroll: 0.0,
            panel_rect: [0.0; 4],
            category_rects: Vec::new(),
            control_rects: Vec::new(),
            dropdown_rects: Vec::new(),
            dropdown_search_rect: [0.0; 4],
            keybind_rects: Vec::new(),
            keybind_reset_rects: Vec::new(),
            search_rect: [0.0; 4],
            edit_json_rect: [0.0; 4],
            close_rect: [0.0; 4],
            hover_control: None,
            hover_category: None,
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

    pub fn open(&mut self) {
        self.active = true;
        self.search.clear();
        self.search_focused = false;
        self.open_dropdown = None;
        self.dropdown_search.clear();
        self.capturing = None;
        self.scroll = 0.0;
    }

    pub fn close(&mut self) {
        self.active = false;
        self.search_focused = false;
        self.open_dropdown = None;
        self.capturing = None;
    }

    pub fn set_values(&mut self, values: Value) {
        self.values = values;
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
        if self.open_dropdown_is_font() {
            self.dropdown_search.push(c);
            self.dropdown_scroll = 0;
        } else if self.search_focused {
            self.search.push(c);
            self.scroll = 0.0;
        }
    }

    pub fn backspace(&mut self) {
        if !self.active {
            return;
        }
        if self.open_dropdown_is_font() {
            self.dropdown_search.pop();
            self.dropdown_scroll = 0;
        } else if self.search_focused {
            self.search.pop();
        }
    }

    pub fn on_escape(&mut self) -> bool {
        if !self.active {
            return false;
        }
        if self.capturing.is_some() {
            self.capturing = None;
        } else if self.open_dropdown.is_some() {
            // Clear an in-dropdown filter first, then close the dropdown.
            if !self.dropdown_search.is_empty() {
                self.dropdown_search.clear();
                self.dropdown_scroll = 0;
            } else {
                self.open_dropdown = None;
            }
        } else if self.search_focused && !self.search.is_empty() {
            self.search.clear();
        } else if self.search_focused {
            self.search_focused = false;
        } else {
            self.close();
        }
        true
    }

    pub fn scroll_by(&mut self, delta: f32) {
        if !self.active {
            return;
        }
        // A one-notch step for the open dropdown, otherwise the content.
        if self.open_dropdown.is_some() {
            if delta > 0.0 {
                self.dropdown_scroll = self.dropdown_scroll.saturating_sub(1);
            } else {
                self.dropdown_scroll += 1;
            }
            return;
        }
        self.scroll = (self.scroll - delta).clamp(0.0, self.max_scroll);
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
    pub(crate) fn dropdown_search_query(&self) -> &str {
        &self.dropdown_search
    }
    /// True while the open dropdown is a (long) font-family list, which
    /// gets the in-dropdown search box + auto-focused typing.
    pub(crate) fn open_dropdown_is_font(&self) -> bool {
        matches!(
            self.open_dropdown.map(|i| SETTINGS[i].control),
            Some(Control::FontFamily)
        )
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
        let def = SETTINGS[idx];
        let Control::Toggle { default } = def.control else {
            return false;
        };
        self.get_value(def.key)
            .and_then(Value::as_bool)
            .unwrap_or(default)
    }
    pub(crate) fn string_at(&self, idx: usize) -> String {
        let def = SETTINGS[idx];
        let default = match def.control {
            Control::Select { default, .. } => default.to_string(),
            Control::FontFamily => String::new(),
            _ => return String::new(),
        };
        self.get_value(def.key)
            .map(json_to_option_string)
            .unwrap_or(default)
    }
    /// Options for the dropdown of a Select or FontFamily control.
    pub(crate) fn dropdown_options(&self, idx: usize) -> Vec<String> {
        if SETTINGS[idx].key == "appearance.theme" {
            return crate::primitives::ide_theme::all_ide_theme_names();
        }
        match SETTINGS[idx].control {
            Control::Select { options, .. } => {
                options.iter().map(|o| o.to_string()).collect()
            }
            Control::FontFamily => {
                let query = self.dropdown_search.trim().to_lowercase();
                if query.is_empty() {
                    self.font_families.clone()
                } else {
                    self.font_families
                        .iter()
                        .filter(|family| family.to_lowercase().contains(&query))
                        .cloned()
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
        SETTINGS
            .iter()
            .enumerate()
            .filter(|(_, def)| {
                if query.is_empty() {
                    def.category == self.category
                } else {
                    def.label.to_lowercase().contains(&query)
                        || def.key.contains(query.as_str())
                        || def.description.to_lowercase().contains(&query)
                }
            })
            .map(|(idx, _)| idx)
            .collect()
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

        if self.open_dropdown.is_some() {
            // Clicking inside the font dropdown's search box keeps it open
            // (and focused) instead of dismissing it.
            if self.open_dropdown_is_font() && point_in(self.dropdown_search_rect, x, y) {
                return swallow();
            }
            for (rect, idx, opt) in self.dropdown_rects.clone() {
                if point_in(rect, x, y) {
                    let def = SETTINGS[idx];
                    let value = option_to_json(&opt);
                    self.set_local(def.key, value.clone());
                    self.open_dropdown = None;
                    self.dropdown_search.clear();
                    return PointerOutcome {
                        consumed: true,
                        action: Some(SettingsAction::Set {
                            key: def.key,
                            value,
                        }),
                    };
                }
            }
            self.open_dropdown = None;
            self.dropdown_search.clear();
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
                return swallow();
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
                let def = SETTINGS[idx];
                match def.control {
                    Control::Toggle { default } => {
                        let next = !self
                            .get_value(def.key)
                            .and_then(Value::as_bool)
                            .unwrap_or(default);
                        self.set_local(def.key, Value::Bool(next));
                        return PointerOutcome {
                            consumed: true,
                            action: Some(SettingsAction::Set {
                                key: def.key,
                                value: Value::Bool(next),
                            }),
                        };
                    }
                    Control::Select { .. } | Control::FontFamily => {
                        self.open_dropdown = Some(idx);
                        self.dropdown_scroll = 0;
                        self.dropdown_search.clear();
                        return swallow();
                    }
                    Control::Action { action, .. } => {
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

fn json_to_option_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

fn option_to_json(opt: &str) -> Value {
    if let Ok(i) = opt.parse::<i64>() {
        return Value::Number(i.into());
    }
    if let Ok(f) = opt.parse::<f64>() {
        if let Some(n) = serde_json::Number::from_f64(f) {
            return Value::Number(n);
        }
    }
    Value::String(opt.to_string())
}

pub(crate) fn point_in(rect: [f32; 4], x: f32, y: f32) -> bool {
    x >= rect[0] && x <= rect[0] + rect[2] && y >= rect[1] && y <= rect[1] + rect[3]
}
