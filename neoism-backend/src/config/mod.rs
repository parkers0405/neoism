pub mod bell;
pub mod bindings;
pub mod colors;
pub mod defaults;
pub mod effects;
pub mod hints;
pub mod intelligence;
mod jsonc_edit;
pub mod keyboard;
pub mod layout;
pub mod mashup;
pub mod navigation;
pub mod platform;
pub mod renderer;
pub mod theme;
pub mod title;
pub mod window;

use crate::config::bell::Bell;
use crate::config::bindings::Bindings;
use crate::config::defaults::*;
use crate::config::hints::Hints;
use crate::config::keyboard::Keyboard;
use crate::config::layout::{Margin, Panel};
use crate::config::navigation::Navigation;
use crate::config::platform::{Platform, PlatformConfig};
use crate::config::renderer::Renderer;
use crate::config::title::Title;
use crate::config::window::Window;
use colors::Colors;
use neoism_terminal_core::ansi::CursorShape;
use serde::{Deserialize, Serialize};
use std::default::Default;
use std::io::{Error, ErrorKind, Write};
use std::path::Path;
use std::path::PathBuf;
use sugarloaf::font::fonts::SugarloafFonts;
use theme::{AdaptiveColors, AdaptiveTheme, AppearanceTheme, Theme};
use tracing::warn;

#[derive(Clone, Debug)]
pub enum ConfigError {
    ErrLoadingConfig(String),
    ErrLoadingTheme(String),
    PathNotFound,
}

#[derive(Default, Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct Shell {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct Scroll {
    #[serde(default = "default_scroll_multiplier")]
    pub multiplier: f64,
    #[serde(default = "default_scroll_divider")]
    pub divider: f64,
}

fn default_scroll_multiplier() -> f64 {
    3.0
}

fn default_scroll_divider() -> f64 {
    1.0
}

impl Default for Scroll {
    fn default() -> Scroll {
        Scroll {
            multiplier: default_scroll_multiplier(),
            divider: default_scroll_divider(),
        }
    }
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct Developer {
    #[serde(default = "bool::default", rename = "enable-fps-counter")]
    pub enable_fps_counter: bool,
    #[serde(default = "default_log_level", rename = "log-level")]
    pub log_level: String,
    #[serde(rename = "enable-log-file", default)]
    pub enable_log_file: bool,
}

/// `[appearance]` — theming + typography, cross-cutting the terminal and
/// the editor. The IDE theme, the terminal color `palette`, fonts,
/// Mash Up Pack, and GPU effects all live here.
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct Appearance {
    #[serde(default = "default_neoism_theme")]
    pub theme: String,
    /// Terminal color palette file (`themes/<name>.json`). Distinct from
    /// the IDE `theme` above.
    #[serde(default = "String::default")]
    pub palette: String,
    #[serde(default = "SugarloafFonts::default")]
    pub fonts: SugarloafFonts,
    #[serde(rename = "line-height", default = "default_line_height")]
    pub line_height: f32,
    /// Active Mash Up Pack id (a directory under `packs/`). Applied on
    /// startup: the pack's theme wins over `theme` above, and its
    /// shader overlay / filters are re-applied. Empty/unset = no pack.
    #[serde(default, rename = "mashup-pack")]
    pub mashup_pack: Option<String>,
    /// Individual look-slot overrides (`[appearance.look.scrollbar]`,
    /// `…markdown]`, `…icons]`) — win field-by-field over the active
    /// Mash Up Pack's slots.
    #[serde(default)]
    pub look: mashup::LookConfig,
    #[serde(default = "effects::Effects::default")]
    pub effects: effects::Effects,
    #[serde(default = "Option::default", rename = "force-theme")]
    pub force_theme: Option<AppearanceTheme>,
    /// Runtime-derived terminal colors loaded from `palette` — never
    /// written back to the file.
    #[serde(default = "Colors::default", skip_serializing)]
    pub colors: Colors,
    #[serde(
        default = "Option::default",
        skip_serializing,
        rename = "adaptive-theme"
    )]
    pub adaptive_theme: Option<AdaptiveTheme>,
    #[serde(default = "Option::default", skip_serializing)]
    pub adaptive_colors: Option<AdaptiveColors>,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: default_neoism_theme(),
            palette: String::default(),
            fonts: SugarloafFonts::default(),
            line_height: default_line_height(),
            mashup_pack: None,
            look: mashup::LookConfig::default(),
            effects: effects::Effects::default(),
            force_theme: None,
            colors: Colors::default(),
            adaptive_theme: None,
            adaptive_colors: None,
        }
    }
}

/// `[editor]` — the built-in code + markdown editor.
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct EditorConfig {
    /// Vim keybindings in the code AND markdown editors. On by default;
    /// set `vim-mode = false` for plain (always-insert) editing. New
    /// editors honor this on open; toggling it re-applies on the next
    /// editor you open.
    #[serde(default = "default_bool_true", rename = "vim-mode")]
    pub vim_mode: bool,
    /// Run the language server's formatter on the buffer before every
    /// save in the code editor. On by default.
    #[serde(default = "default_bool_true", rename = "format-on-save")]
    pub format_on_save: bool,
    #[serde(default)]
    pub minimap: bool,
    #[serde(default)]
    pub markdown: MarkdownEditorConfig,
    /// External editor command shelled out to for "open in editor" (Rio
    /// heritage). Nested here as `[editor] external` so the `editor`
    /// group name stays free.
    #[serde(default = "default_editor")]
    pub external: Shell,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            vim_mode: true,
            format_on_save: true,
            minimap: false,
            markdown: MarkdownEditorConfig::default(),
            external: default_editor(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct MarkdownEditorConfig {
    /// Show spelling underlines and spelling actions in Markdown surfaces.
    #[serde(default = "default_bool_true")]
    pub spellcheck: bool,
}

impl Default for MarkdownEditorConfig {
    fn default() -> Self {
        Self { spellcheck: true }
    }
}

/// `[terminal]` — the terminal emulator (Rio heritage).
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct TerminalConfig {
    #[serde(default = "default_shell")]
    pub shell: Shell,
    #[serde(default)]
    pub cursor: CursorConfig,
    #[serde(default = "Scroll::default")]
    pub scroll: Scroll,
    #[serde(default = "Keyboard::default")]
    pub keyboard: Keyboard,
    #[serde(default = "default_use_fork", rename = "use-fork")]
    pub use_fork: bool,
    #[serde(default = "default_working_dir", rename = "working-dir")]
    pub working_dir: Option<String>,
    #[serde(default = "Vec::default", rename = "env-vars")]
    pub env_vars: Vec<String>,
    #[serde(default = "default_option_as_alt", rename = "option-as-alt")]
    pub option_as_alt: String,
    #[serde(default = "bool::default", rename = "copy-on-select")]
    pub copy_on_select: bool,
    #[serde(default = "bool::default", rename = "hide-mouse-cursor-when-typing")]
    pub hide_cursor_when_typing: bool,
    #[serde(default = "bool::default", rename = "draw-bold-text-with-light-colors")]
    pub draw_bold_text_with_light_colors: bool,
    #[serde(
        default = "bool::default",
        rename = "ignore-selection-foreground-color"
    )]
    pub ignore_selection_fg_color: bool,
    #[serde(default = "Hints::default")]
    pub hints: Hints,
    #[serde(default = "Bell::default")]
    pub bell: Bell,
    #[serde(default = "default_bool_true", rename = "enable-scroll-bar")]
    pub enable_scroll_bar: bool,
    #[serde(
        default = "default_scrollback_history_limit",
        rename = "scrollback-history-limit"
    )]
    pub scrollback_history_limit: usize,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            shell: default_shell(),
            cursor: CursorConfig::default(),
            scroll: Scroll::default(),
            keyboard: Keyboard::default(),
            use_fork: default_use_fork(),
            working_dir: default_working_dir(),
            env_vars: vec![],
            option_as_alt: default_option_as_alt(),
            copy_on_select: false,
            hide_cursor_when_typing: false,
            draw_bold_text_with_light_colors: false,
            ignore_selection_fg_color: false,
            hints: Hints::default(),
            bell: Bell::default(),
            enable_scroll_bar: true,
            scrollback_history_limit: default_scrollback_history_limit(),
        }
    }
}

/// `[ui]` — the window chrome: OS window, tab navigation, title, side
/// panels, and status line.
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct UiConfig {
    #[serde(default = "Window::default")]
    pub window: Window,
    #[serde(default = "Navigation::default")]
    pub navigation: Navigation,
    #[serde(default = "Title::default")]
    pub title: Title,
    #[serde(default = "Panel::default")]
    pub panel: Panel,
    #[serde(default = "default_margin")]
    pub margin: Margin,
    /// FPS pill on the status line's right cluster. On by default.
    #[serde(default = "default_bool_true", rename = "status-fps")]
    pub status_fps: bool,
    #[serde(default = "bool::default", rename = "confirm-before-quit")]
    pub confirm_before_quit: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            window: Window::default(),
            navigation: Navigation::default(),
            title: Title::default(),
            panel: Panel::default(),
            margin: default_margin(),
            status_fps: true,
            confirm_before_quit: false,
        }
    }
}

/// `[presence]` — how you appear to collaborators in multiplayer.
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Default)]
pub struct Presence {
    /// The display name other collaborators see (remote caret tags +
    /// the "who's here" roster). `NEOISM_DISPLAY_NAME` overrides; when
    /// both are unset the hostname is used.
    #[serde(default, rename = "display-name")]
    pub display_name: Option<String>,
    /// User-picked cursor color as `#RRGGBB` (or `RRGGBB` / `#RGB`).
    /// Overrides the theme's cursor accent. Unset/unparseable falls back
    /// to the theme accent.
    #[serde(default, rename = "cursor-color")]
    pub cursor_color: Option<String>,
    /// Cursor preset: `"solid"` (default) paints `cursor-color` or the
    /// theme accent; `"rainbow"` animates through hues.
    #[serde(default, rename = "cursor-style")]
    pub cursor_style: Option<String>,
}

impl Default for Developer {
    fn default() -> Developer {
        Developer {
            log_level: default_log_level(),
            enable_log_file: false,
            enable_fps_counter: false,
        }
    }
}

/// The golden grouped `config.json`. Every domain is its own block —
/// `appearance`, `editor`, `terminal`, `ui`, `presence`, `keybinds` —
/// plus the standalone `platform`, `renderer`, and `developer` domains.
/// The agent reads its own `agent` block from the same file (ignored
/// here as an unknown key).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default)]
    pub appearance: Appearance,
    #[serde(default)]
    pub editor: EditorConfig,
    #[serde(default)]
    pub terminal: TerminalConfig,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub presence: Presence,
    #[serde(default = "Bindings::default")]
    pub keybinds: bindings::Bindings,
    #[serde(default = "Platform::default")]
    pub platform: Platform,
    #[serde(default = "Renderer::default")]
    pub renderer: Renderer,
    #[serde(default = "Developer::default")]
    pub developer: Developer,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CursorConfig {
    #[serde(default = "default_cursor")]
    pub shape: CursorShape,
    #[serde(default = "bool::default")]
    pub blinking: bool,
    #[serde(default = "default_cursor_interval", rename = "blinking-interval")]
    pub blinking_interval: u64,
}

#[cfg(target_os = "macos")]
#[inline]
pub fn config_dir_path() -> PathBuf {
    std::env::var("NEOISM_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or(dirs::home_dir().unwrap().join(".config").join("neoism"))
}

#[cfg(target_os = "windows")]
#[inline]
pub fn config_dir_path() -> PathBuf {
    std::env::var("NEOISM_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or(
            dirs::home_dir()
                .unwrap()
                .join("AppData")
                .join("Local")
                .join("neoism"),
        )
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
#[inline]
pub fn config_dir_path() -> PathBuf {
    std::env::var("NEOISM_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or(
            std::env::var("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or(dirs::home_dir().unwrap().join(".config"))
                .join("neoism"),
        )
}

/// Canonical (primary) config file: `config.json`. Supports `//` and
/// `/* */` comments plus trailing commas (JSONC) — see
/// [`parse_config_content`].
#[inline]
pub fn json_config_file_path() -> PathBuf {
    config_dir_path().join("config.json")
}

/// Active config file: always `config.json` (JSONC). Neoism is
/// JSON-only — there is no legacy `config.toml` fallback.
#[inline]
pub fn config_file_path() -> PathBuf {
    json_config_file_path()
}

/// Parse config file content as JSONC (comments + trailing commas
/// tolerated). Shared with the mashup-pack loader so pack.json /
/// theme.json speak the same dialect as config.json.
pub(crate) fn parse_config_content<T: serde::de::DeserializeOwned>(
    _path: &Path,
    content: &str,
) -> Result<T, String> {
    let cleaned = strip_trailing_commas(&strip_json_comments(content));
    let cleaned = if cleaned.trim().is_empty() {
        "{}".to_string()
    } else {
        cleaned
    };
    serde_json::from_str::<T>(&cleaned).map_err(|err| err.to_string())
}

/// Parse `config.json` content into the grouped [`Config`] struct
/// (`appearance`, `editor`, `terminal`, `ui`, `presence`, `keybinds`).
/// Comments + trailing commas tolerated (JSONC).
fn deserialize_config(content: &str) -> Result<Config, String> {
    parse_config_content::<Config>(Path::new("config.json"), content)
}

/// Strip `//` line and `/* */` block comments outside strings so a
/// hand-commented `config.json` stays loadable (JSONC). Mirrors the
/// agent server's parser so both readers accept the same file.
fn strip_json_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if in_string {
            escaped = ch == '\\' && !escaped;
            if ch == '"' && !escaped {
                in_string = false;
            }
            if ch != '\\' {
                escaped = false;
            }
            out.push(ch);
            continue;
        }
        if ch == '"' {
            in_string = true;
            out.push(ch);
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'/') {
            let _ = chars.next();
            for next in chars.by_ref() {
                if next == '\n' {
                    out.push('\n');
                    break;
                }
            }
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'*') {
            let _ = chars.next();
            let mut previous = '\0';
            for next in chars.by_ref() {
                if previous == '*' && next == '/' {
                    break;
                }
                previous = next;
            }
            continue;
        }
        out.push(ch);
    }
    out
}

/// Drop trailing commas before `}` / `]` outside strings (the other
/// half of JSONC tolerance).
fn strip_trailing_commas(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if in_string {
            escaped = ch == '\\' && !escaped;
            if ch == '"' && !escaped {
                in_string = false;
            }
            if ch != '\\' {
                escaped = false;
            }
            out.push(ch);
            continue;
        }
        if ch == '"' {
            in_string = true;
            out.push(ch);
            continue;
        }
        if ch == ',' {
            let closes_next = chars
                .clone()
                .find(|next| !next.is_whitespace())
                .is_some_and(|next| matches!(next, '}' | ']'));
            if closes_next {
                continue;
            }
        }
        out.push(ch);
    }
    out
}

#[inline]
pub fn config_file_content() -> String {
    default_config_file_content()
}

#[inline]
pub fn create_config_file(path: Option<PathBuf>) -> std::io::Result<PathBuf> {
    let default_file_path = path.unwrap_or_else(config_file_path);
    if default_file_path.exists() {
        tracing::info!(
            "configuration file already exists at {}",
            default_file_path.display()
        );
        return Ok(default_file_path);
    }

    let parent = default_file_path.parent().ok_or_else(|| {
        Error::new(
            ErrorKind::InvalidInput,
            "config path has no parent directory",
        )
    })?;
    std::fs::create_dir_all(parent)?;
    let mut created_file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&default_file_path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            return Ok(default_file_path)
        }
        Err(error) => return Err(error),
    };
    writeln!(created_file, "{}", config_file_content())?;
    created_file.sync_all()?;
    tracing::info!("configuration file created {}", default_file_path.display());
    Ok(default_file_path)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigDocumentSnapshot {
    pub content: String,
    pub display_path: String,
    pub revision: String,
    pub writable: bool,
}

/// Read the raw active JSONC document. This does not create a missing file.
pub fn read_config_document() -> std::io::Result<ConfigDocumentSnapshot> {
    read_config_document_at(&config_file_path())
}

/// Ensure the canonical JSONC file exists, then return its raw document.
pub fn ensure_config_document() -> std::io::Result<ConfigDocumentSnapshot> {
    let path = create_config_file(None)?;
    read_config_document_at(&path)
}

fn read_config_document_at(path: &Path) -> std::io::Result<ConfigDocumentSnapshot> {
    let content = std::fs::read_to_string(path)?;
    let writable = std::fs::metadata(path)
        .map(|metadata| !metadata.permissions().readonly())
        .unwrap_or(false);
    Ok(ConfigDocumentSnapshot {
        revision: config_revision(&content),
        content,
        display_path: path.display().to_string(),
        writable,
    })
}

/// Validate JSONC syntax and the grouped application fields. Unknown agent
/// fields remain valid because the application config intentionally ignores them.
pub fn validate_config_document(content: &str) -> Result<(), String> {
    let cleaned = strip_trailing_commas(&strip_json_comments(content));
    let value: serde_json::Value = serde_json::from_str(if cleaned.trim().is_empty() {
        "{}"
    } else {
        &cleaned
    })
    .map_err(|error| error.to_string())?;
    if !value.is_object() {
        return Err("config root must be a JSON object".into());
    }
    deserialize_config(content)?;
    if let Some(agent) = value.get("agent") {
        serde_json::from_value::<neoism_agent_core::api::AgentConfigDocument>(
            agent.clone(),
        )
        .map_err(|error| format!("agent config: {error}"))?;
    }
    Ok(())
}

/// Save a complete JSONC document if `expected_revision` still names the file
/// currently on disk. The replacement is validated and atomically renamed.
pub fn save_config_document(
    content: &str,
    expected_revision: &str,
) -> std::io::Result<ConfigDocumentSnapshot> {
    let path = config_file_path();
    save_config_document_at(&path, content, expected_revision)
}

fn save_config_document_at(
    path: &Path,
    content: &str,
    expected_revision: &str,
) -> std::io::Result<ConfigDocumentSnapshot> {
    let current = std::fs::read_to_string(&path)?;
    let actual_revision = config_revision(&current);
    if actual_revision != expected_revision {
        return Err(Error::new(
            ErrorKind::AlreadyExists,
            format!("config revision conflict: expected {expected_revision}, current {actual_revision}"),
        ));
    }
    validate_config_document(content)
        .map_err(|message| Error::new(ErrorKind::InvalidData, message))?;
    let parent = path.parent().ok_or_else(|| {
        Error::new(ErrorKind::InvalidInput, "config path has no parent")
    })?;
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".config.json.{}.tmp", std::process::id()));
    let result: std::io::Result<()> = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, &path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result?;
    read_config_document_at(&path)
}

fn config_revision(content: &str) -> String {
    // Stable FNV-1a; the revision is an opaque concurrency token, not a security hash.
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in content.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub fn write_neoism_preferences(
    theme: Option<&str>,
    minimap: Option<bool>,
    mashup_pack: Option<&str>,
) -> std::io::Result<()> {
    let mut updates: Vec<(&str, serde_json::Value)> = Vec::new();
    if let Some(theme) = theme {
        updates.push((
            "appearance.theme",
            serde_json::Value::String(theme.to_string()),
        ));
    }
    if let Some(minimap) = minimap {
        updates.push(("editor.minimap", serde_json::Value::Bool(minimap)));
    }
    if let Some(pack) = mashup_pack {
        // Empty string persists "no pack" while keeping the key's spot
        // in the file.
        updates.push((
            "appearance.mashup-pack",
            serde_json::Value::String(pack.to_string()),
        ));
    }
    write_settings(&updates)
}

/// Persist one setting to `config.json` for the GUI settings panel. The
/// dotted `key` is the golden grouped path (`appearance.fonts.family`,
/// `ui.window.opacity`, `editor.vim-mode`) and each segment nests an
/// object. The fs-watcher then hot-reloads it.
pub fn write_setting(key: &str, value: serde_json::Value) -> std::io::Result<()> {
    write_settings(&[(key, value)])
}

/// Persist one setting only when the user has not configured it yet.
///
/// The check and write share one document edit, so first-run UI choices
/// cannot overwrite a preference that was already present. Empty strings
/// and `null` count as unset; a non-object parent is left untouched.
pub fn write_setting_if_absent(
    key: &str,
    value: serde_json::Value,
) -> std::io::Result<bool> {
    let root = load_config_json_value();
    let mut current = &root;
    for segment in key.split('.') {
        let Some(next) = current.get(segment) else {
            write_setting(key, value)?;
            return Ok(true);
        };
        current = next;
    }
    if current.is_null()
        || current
            .as_str()
            .is_some_and(|value| value.trim().is_empty())
    {
        write_setting(key, value)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Persist several golden-path settings with targeted JSONC edits.
fn write_settings(updates: &[(&str, serde_json::Value)]) -> std::io::Result<()> {
    if updates.is_empty() {
        return Ok(());
    }
    let path = create_config_file(None)?;
    let mut content = std::fs::read_to_string(&path)?;
    for (key, value) in updates {
        content = jsonc_edit::set_path(
            &content,
            &key.split('.').collect::<Vec<_>>(),
            value.clone(),
        )?;
    }
    std::fs::write(path, content)?;
    invalidate_config_json_cache();
    Ok(())
}

/// Upsert (or clear) a `keybinds.keys` binding override for `action` in
/// config.json — backs the GUI Keybinds section. Replaces any existing
/// binding for the same action; an empty `key` removes the override so
/// the built-in default applies again. Bindings are read at launch, so a
/// change here takes effect on the next start.
pub fn write_keybind(action: &str, key: &str, with: &str) -> std::io::Result<()> {
    let path = create_config_file(None)?;
    let content = std::fs::read_to_string(&path)?;
    let output = jsonc_edit::edit_keybind(&content, action, key, with)?;
    if output != content {
        std::fs::write(path, output)?;
        invalidate_config_json_cache();
    }
    Ok(())
}

/// Load the active `config.json` as a raw JSON value (all keys — terminal
/// AND agent) so the GUI settings panel can reflect current values,
/// including keys the terminal `Config` struct doesn't model.
pub fn load_config_json_value() -> serde_json::Value {
    let path = config_file_path();
    let fingerprint = std::fs::metadata(&path)
        .ok()
        .map(|metadata| (metadata.len(), metadata.modified().ok()));
    if let Some(value) = config_json_cache().lock().ok().and_then(|cache| {
        cache
            .as_ref()
            .filter(|entry| entry.fingerprint == fingerprint)
            .map(|entry| entry.value.clone())
    }) {
        return value;
    }
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let cleaned = strip_trailing_commas(&strip_json_comments(&content));
    let value = if cleaned.trim().is_empty() {
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str(&cleaned)
            .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()))
    };
    if let Ok(mut cache) = config_json_cache().lock() {
        *cache = Some(ConfigJsonCache {
            fingerprint,
            value: value.clone(),
        });
    }
    value
}

#[derive(Clone)]
struct ConfigJsonCache {
    fingerprint: Option<(u64, Option<std::time::SystemTime>)>,
    value: serde_json::Value,
}

fn config_json_cache() -> &'static std::sync::Mutex<Option<ConfigJsonCache>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<Option<ConfigJsonCache>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(None))
}

fn invalidate_config_json_cache() {
    if let Ok(mut cache) = config_json_cache().lock() {
        *cache = None;
    }
}

/// Persist `appearance.fonts.family` — Mash Up Packs use this so their
/// font lands the same way a manual config edit would (the config watcher
/// rebuilds the font library from the write).
pub fn write_fonts_family(family: &str) -> std::io::Result<()> {
    write_setting(
        "appearance.fonts.family",
        serde_json::Value::String(family.to_string()),
    )
}

impl Config {
    fn load_theme(path: &PathBuf) -> Result<Theme, String> {
        if path.exists() {
            let content = std::fs::read_to_string(path).unwrap();
            parse_config_content::<Theme>(path, &content)
                .map_err(|err_message| format!("error parsing: {err_message:?}"))
        } else {
            Err(String::from("filepath does not exist"))
        }
    }

    pub fn load() -> Self {
        let config_path = config_dir_path();
        let path = config_file_path();
        if path.exists() {
            let content = std::fs::read_to_string(&path).unwrap();
            match deserialize_config(&content) {
                Ok(mut decoded) => {
                    let palette = &decoded.appearance.palette;
                    if palette.is_empty() {
                        return decoded;
                    }

                    let path = config_path
                        .join("themes")
                        .join(palette)
                        .with_extension("json");
                    if let Ok(loaded_theme) = Config::load_theme(&path) {
                        decoded.appearance.colors = loaded_theme.colors;
                    } else {
                        warn!("failed to load palette: {}", palette);
                    }

                    decoded
                }
                Err(err_message) => {
                    warn!("failure to parse config file, falling back to default...\n{err_message:?}");
                    Config::default()
                }
            }
        } else {
            Config::default()
        }
    }

    pub fn try_load() -> Result<Self, ConfigError> {
        let path = config_file_path();
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(content) => match deserialize_config(&content) {
                    Ok(mut decoded) => {
                        let palette = &decoded.appearance.palette;
                        let theme_path = config_dir_path().join("themes");
                        if !palette.is_empty() {
                            let path = theme_path.join(palette).with_extension("json");
                            match Config::load_theme(&path) {
                                Ok(loaded_theme) => {
                                    decoded.appearance.colors = loaded_theme.colors;
                                }
                                Err(err_message) => {
                                    return Err(ConfigError::ErrLoadingTheme(
                                        err_message,
                                    ));
                                }
                            }
                        }

                        if let Some(adaptive_theme) = &decoded.appearance.adaptive_theme {
                            let mut adaptive_colors = AdaptiveColors {
                                dark: None,
                                light: None,
                            };

                            let light_theme = &adaptive_theme.light;
                            let path =
                                theme_path.join(light_theme).with_extension("json");
                            match Config::load_theme(&path) {
                                Ok(light_loaded_theme) => {
                                    adaptive_colors.light =
                                        Some(light_loaded_theme.colors)
                                }
                                Err(err_message) => {
                                    warn!("failed to load light theme: {}", light_theme);
                                    return Err(ConfigError::ErrLoadingTheme(
                                        err_message,
                                    ));
                                }
                            }

                            let dark_theme = &adaptive_theme.dark;
                            let path = theme_path.join(dark_theme).with_extension("json");
                            match Config::load_theme(&path) {
                                Ok(dark_loaded_theme) => {
                                    adaptive_colors.dark = Some(dark_loaded_theme.colors)
                                }
                                Err(err_message) => {
                                    warn!("failed to load dark theme: {}", dark_theme);
                                    return Err(ConfigError::ErrLoadingTheme(
                                        err_message,
                                    ));
                                }
                            }

                            if adaptive_colors.light.is_some()
                                && adaptive_colors.dark.is_some()
                            {
                                decoded.appearance.adaptive_colors =
                                    Some(adaptive_colors);
                            }
                        }

                        Ok(decoded)
                    }
                    Err(err_message) => {
                        Err(ConfigError::ErrLoadingConfig(err_message.to_string()))
                    }
                },
                Err(err_message) => {
                    Err(ConfigError::ErrLoadingConfig(err_message.to_string()))
                }
            }
        } else {
            Err(ConfigError::PathNotFound)
        }
    }

    pub fn overwrite_based_on_platform(&mut self) {
        #[cfg(windows)]
        if let Some(windows) = &self.platform.windows {
            self.overwrite_with_platform_config(windows.clone());
        }

        #[cfg(target_os = "linux")]
        if let Some(linux) = &self.platform.linux {
            self.overwrite_with_platform_config(linux.clone());
        }

        #[cfg(target_os = "macos")]
        if let Some(macos) = &self.platform.macos {
            self.overwrite_with_platform_config(macos.clone());
        }
    }

    fn overwrite_with_platform_config(&mut self, platform_config: PlatformConfig) {
        // Replace shell entirely if specified
        if let Some(shell_overwrite) = &platform_config.shell {
            self.terminal.shell = shell_overwrite.clone();
        }

        // Merge window fields individually
        if let Some(window_overwrite) = &platform_config.window {
            if let Some(width) = window_overwrite.width {
                self.ui.window.width = width;
            }
            if let Some(height) = window_overwrite.height {
                self.ui.window.height = height;
            }
            if let Some(columns) = window_overwrite.columns {
                self.ui.window.columns = Some(columns);
            }
            if let Some(rows) = window_overwrite.rows {
                self.ui.window.rows = Some(rows);
            }
            if let Some(mode) = window_overwrite.mode {
                self.ui.window.mode = mode;
            }
            if let Some(opacity) = window_overwrite.opacity {
                self.ui.window.opacity = opacity;
            }
            if let Some(blur) = window_overwrite.blur {
                self.ui.window.blur = blur;
            }
            if let Some(bg_image) = &window_overwrite.background_image {
                self.ui.window.background_image = Some(bg_image.clone());
            }
            if let Some(decorations) = window_overwrite.decorations {
                self.ui.window.decorations = decorations;
            }
            if let Some(macos_unified) = window_overwrite.macos_use_unified_titlebar {
                self.ui.window.macos_use_unified_titlebar = macos_unified;
            }
            if let Some(macos_shadow) = window_overwrite.macos_use_shadow {
                self.ui.window.macos_use_shadow = macos_shadow;
            }
            if let Some(x) = window_overwrite.macos_traffic_light_position_x {
                self.ui.window.macos_traffic_light_position_x = Some(x);
            }
            if let Some(y) = window_overwrite.macos_traffic_light_position_y {
                self.ui.window.macos_traffic_light_position_y = Some(y);
            }
            if let Some(initial_title) = &window_overwrite.initial_title {
                self.ui.window.initial_title = Some(initial_title.clone());
            }
            if let Some(win_shadow) = window_overwrite.windows_use_undecorated_shadow {
                self.ui.window.windows_use_undecorated_shadow = Some(win_shadow);
            }
            if let Some(win_bitmap) = window_overwrite.windows_use_no_redirection_bitmap {
                self.ui.window.windows_use_no_redirection_bitmap = Some(win_bitmap);
            }
            if let Some(win_corner) = &window_overwrite.windows_corner_preference {
                self.ui.window.windows_corner_preference = Some(win_corner.clone());
            }
            if let Some(colorspace) = window_overwrite.colorspace {
                self.ui.window.colorspace = colorspace;
            }
        }

        // Merge navigation fields individually
        if let Some(navigation_overwrite) = &platform_config.navigation {
            if let Some(mode) = navigation_overwrite.mode {
                self.ui.navigation.mode = mode;
            }
            if let Some(color_automation) = &navigation_overwrite.color_automation {
                self.ui.navigation.color_automation = color_automation.clone();
            }
            if let Some(clickable) = navigation_overwrite.clickable {
                self.ui.navigation.clickable = clickable;
            }
            if let Some(cwd) = navigation_overwrite.current_working_directory {
                self.ui.navigation.current_working_directory = cwd;
            }
            if let Some(use_term_title) = navigation_overwrite.use_terminal_title {
                self.ui.navigation.use_terminal_title = use_term_title;
            }
            if let Some(hide_if_single) = navigation_overwrite.hide_if_single {
                self.ui.navigation.hide_if_single = hide_if_single;
            }
            if let Some(use_split) = navigation_overwrite.use_split {
                self.ui.navigation.use_split = use_split;
            }
            if let Some(open_cfg_split) = navigation_overwrite.open_config_with_split {
                self.ui.navigation.open_config_with_split = open_cfg_split;
            }
            if let Some(unfocused_opacity) = navigation_overwrite.unfocused_split_opacity
            {
                self.ui.navigation.unfocused_split_opacity = unfocused_opacity;
            }
            if let Some(fill) = navigation_overwrite.unfocused_split_fill {
                self.ui.navigation.unfocused_split_fill = Some(fill);
            }
        }

        // Clamp after platform merge so both the base and any override go
        // through the same bound.
        self.ui.navigation.unfocused_split_opacity =
            crate::config::navigation::clamp_unfocused_split_opacity(
                self.ui.navigation.unfocused_split_opacity,
            );

        // Merge renderer fields individually
        if let Some(renderer_overwrite) = &platform_config.renderer {
            if let Some(backend) = &renderer_overwrite.backend {
                self.renderer.backend = backend.clone();
            }
            if let Some(disable_unfocused) = renderer_overwrite.disable_unfocused_render {
                self.renderer.disable_unfocused_render = disable_unfocused;
            }
            if let Some(disable_occluded) = renderer_overwrite.disable_occluded_render {
                self.renderer.disable_occluded_render = disable_occluded;
            }
            #[cfg(feature = "wgpu")]
            if let Some(filters) = &renderer_overwrite.filters {
                self.renderer.filters = filters.clone();
            }
            if let Some(strategy) = &renderer_overwrite.strategy {
                self.renderer.strategy = strategy.clone();
            }
        }

        // Append platform-specific env vars to the global ones
        if let Some(env_vars_overwrite) = &platform_config.env_vars {
            self.terminal.env_vars.extend(env_vars_overwrite.clone());
        }

        // Override theme
        if let Some(theme_overwrite) = &platform_config.theme {
            self.appearance.palette = theme_overwrite.clone();
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            appearance: Appearance::default(),
            editor: EditorConfig::default(),
            terminal: TerminalConfig::default(),
            ui: UiConfig::default(),
            presence: Presence::default(),
            keybinds: Bindings::default(),
            platform: Platform::default(),
            renderer: Renderer::default(),
            developer: Developer::default(),
        }
    }
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            shape: default_cursor(),
            blinking: false,
            blinking_interval: default_cursor_interval(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Config {
        deserialize_config(json).expect("config should parse")
    }

    #[test]
    fn empty_config_uses_defaults() {
        let config = parse("{}");
        assert_eq!(config.appearance.palette, String::default());
        assert_eq!(config.terminal.cursor.shape, default_cursor());
        assert_eq!(config.appearance.fonts, SugarloafFonts::default());
        assert_eq!(config.appearance.colors, Colors::default());
        assert_eq!(config.developer, Developer::default());
        assert!(!config.renderer.disable_unfocused_render);
        assert_eq!(config.appearance.theme, default_neoism_theme());
        assert!(config.ui.status_fps);
        assert!(config.editor.format_on_save);
        assert!(!config.editor.minimap);
        assert!(config.editor.markdown.spellcheck);
    }

    #[test]
    fn jsonc_comments_and_trailing_commas_are_tolerated() {
        let config = parse(
            r#"
// line comment — legal in the unified config
{
    /* block comment */
    "appearance": {
        "line-height": 1.4,
        "palette": "lucario",
        "theme": "tokyo_night",   // IDE theme
        "fonts": { "size": 16.0 },
    },
    "editor": { "minimap": true, "markdown": { "spellcheck": false } },
    "ui": { "status-fps": false },
    // agent-server keys co-live in the same file; the app ignores them
    "agent": { "model": "anthropic/claude-opus-5" },
}
"#,
        );
        assert_eq!(config.appearance.line_height, 1.4);
        assert_eq!(config.appearance.palette, "lucario");
        assert_eq!(config.appearance.theme, "tokyo_night");
        assert!(config.editor.minimap);
        assert!(!config.editor.markdown.spellcheck);
        assert!(!config.ui.status_fps);
        assert_eq!(config.appearance.fonts.size, 16.0);
    }

    #[test]
    fn keys_are_grouped_by_domain() {
        // Golden standard: each setting lives inside its domain block.
        let config = parse(
            r#"{
                "presence": { "display-name": "parker" },
                "appearance": { "mashup-pack": "synth" }
            }"#,
        );
        assert_eq!(config.presence.display_name.as_deref(), Some("parker"));
        assert_eq!(config.appearance.mashup_pack.as_deref(), Some("synth"));

        // A key placed at the wrong level (bare root) is ignored, not honored.
        let misplaced = parse(r#"{ "display-name": "parker" }"#);
        assert_eq!(misplaced.presence.display_name, None);
    }

    #[test]
    fn theme_is_ide_and_palette_is_terminal_colors() {
        let config = parse(
            r#"{ "appearance": { "theme": "catppuccin_mocha", "palette": "lucario" } }"#,
        );
        assert_eq!(config.appearance.theme, "catppuccin_mocha"); // IDE theme
        assert_eq!(config.appearance.palette, "lucario"); // terminal color file
    }

    #[test]
    fn renderer_and_cursor_sections_parse() {
        let config = parse(
            r#"{
                "terminal": { "cursor": { "shape": "underline" } },
                "renderer": { "backend": "Vulkan" }
            }"#,
        );
        assert_eq!(config.terminal.cursor.shape, CursorShape::Underline);
        assert_eq!(config.renderer.backend, renderer::Backend::Vulkan);
    }

    #[test]
    fn colors_parse_from_hex() {
        let config =
            parse(r##"{ "appearance": { "colors": { "foreground": "#000000" } } }"##);
        assert_eq!(config.appearance.colors.foreground, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(
            config.appearance.colors.background,
            colors::defaults::background()
        );
    }

    #[test]
    fn env_vars_and_option_as_alt_parse() {
        let config = parse(
            r#"{ "terminal": { "env-vars": ["A=5", "B=8"], "option-as-alt": "Both" } }"#,
        );
        assert_eq!(
            config.terminal.env_vars,
            [String::from("A=5"), String::from("B=8")]
        );
        assert_eq!(config.terminal.option_as_alt, String::from("Both"));
    }

    #[test]
    fn unknown_keys_are_ignored() {
        // Misspelled / dropped keys parse as ignored, leaving defaults.
        let config =
            parse(r#"{ "terminal": { "cwd": false, "hide-cursor-when-typing": true } }"#);
        assert!(config.ui.navigation.current_working_directory); // default true
        assert!(!config.terminal.hide_cursor_when_typing); // real key is hide-mouse-…
        assert!(!config.terminal.cursor.blinking); // default false
    }

    #[test]
    fn create_config_file_creates_custom_parent_and_never_truncates() {
        let root = std::env::temp_dir().join(format!(
            "neoism-config-create-{}-{}",
            std::process::id(),
            config_revision(module_path!())
        ));
        let path = root.join("nested/config.json");
        assert_eq!(create_config_file(Some(path.clone())).unwrap(), path);
        std::fs::write(&path, "// user content\n{}\n").unwrap();
        create_config_file(Some(path.clone())).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "// user content\n{}\n"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn raw_document_validation_accepts_jsonc_and_rejects_bad_grouped_values() {
        assert!(validate_config_document(
            "// hello\n{ \"editor\": { \"vim-mode\": true, }, }"
        )
        .is_ok());
        assert!(
            validate_config_document("{ \"editor\": { \"vim-mode\": \"yes\" } }")
                .is_err()
        );
        assert!(validate_config_document(
            "{ \"agent\": { \"enabledProviders\": \"all\" } }"
        )
        .is_err());
        assert!(validate_config_document("[]").is_err());
    }

    #[test]
    fn raw_document_save_checks_revision_and_preserves_jsonc() {
        let root = std::env::temp_dir().join(format!(
            "neoism-config-save-{}-{}",
            std::process::id(),
            config_revision("raw-document-save")
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("config.json");
        let original = "// original\n{ \"editor\": { \"vim-mode\": true } }\n";
        std::fs::write(&path, original).unwrap();
        let revision = config_revision(original);
        let replacement =
            "// replacement survives\n{ \"editor\": { \"vim-mode\": false, }, }\n";
        let saved = save_config_document_at(&path, replacement, &revision).unwrap();
        assert_eq!(saved.content, replacement);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), replacement);
        let conflict = save_config_document_at(&path, "{}", &revision).unwrap_err();
        assert_eq!(conflict.kind(), ErrorKind::AlreadyExists);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn default_template_parses_to_defaults() {
        let config = parse(&default_config_file_content());
        assert_eq!(config.appearance.palette, String::default());
        assert_eq!(config.appearance.fonts, SugarloafFonts::default());
        assert_eq!(config.keybinds, Bindings::default());
        assert!(config.ui.status_fps);
    }

    #[test]
    fn write_merge_handles_root_and_section_keys() {
        // Exercise the JSON merge primitive used by write_config_keys at
        // both the root (flattened neoism theme) and a named section.
        let content =
            "// note\n{ \"fonts\": { \"size\": 14.0 }, \"mcp\": { \"x\": {} } }";
        let cleaned = strip_trailing_commas(&strip_json_comments(content));
        let mut root: serde_json::Value = serde_json::from_str(&cleaned).unwrap();
        let object = root.as_object_mut().unwrap();
        object.insert("theme".into(), serde_json::Value::String("phosphor".into()));
        let fonts = object
            .entry("fonts".to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        fonts
            .as_object_mut()
            .unwrap()
            .insert("size".into(), serde_json::json!(16.0));
        let out = serde_json::to_string_pretty(&root).unwrap();
        let reparsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(reparsed["theme"], "phosphor");
        assert_eq!(reparsed["fonts"]["size"], 16.0);
        assert!(reparsed["mcp"]["x"].is_object());
    }

    #[test]
    fn strip_helpers_leave_strings_untouched() {
        let input = r#"{ "a": "http://not/a//comment", "b": "star /* keep */" }"#;
        let cleaned = strip_trailing_commas(&strip_json_comments(input));
        let parsed: serde_json::Value = serde_json::from_str(&cleaned).unwrap();
        assert_eq!(parsed["a"], "http://not/a//comment");
        assert_eq!(parsed["b"], "star /* keep */");
    }
}
