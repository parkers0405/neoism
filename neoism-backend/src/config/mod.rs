pub mod bell;
pub mod bindings;
pub mod colors;
pub mod defaults;
pub mod effects;
pub mod hints;
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
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::{default::Default, fs::File};
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

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct Neoism {
    #[serde(default = "default_neoism_theme")]
    pub theme: String,
    #[serde(default)]
    pub minimap: bool,
    /// Run the language server's formatter on the buffer before every
    /// save in the code editor. On by default; `[neoism]
    /// format-on-save = false` opts out.
    #[serde(default = "default_bool_true", rename = "format-on-save")]
    pub format_on_save: bool,
    /// Wave 7G: the display name other collaborators see in multiplayer
    /// presence (remote caret tags + the "who's here" roster). The
    /// `NEOISM_DISPLAY_NAME` env var overrides this; when both are
    /// unset the hostname is used. Does not affect the peer id, so
    /// presence colors stay stable when the name changes.
    #[serde(default, rename = "display-name")]
    pub display_name: Option<String>,
    /// User-picked cursor color as `#RRGGBB` (or `RRGGBB` / `#RGB`).
    /// Overrides the theme's cursor accent on every screen and is what
    /// collaborators see your caret wearing. Unset/unparseable falls
    /// back to the theme accent.
    #[serde(default, rename = "cursor-color")]
    pub cursor_color: Option<String>,
    /// Cursor preset: `"solid"` (default) paints `cursor-color` or the
    /// theme accent; `"rainbow"` animates through hues and ignores the
    /// static color. Unknown names fall back to solid.
    #[serde(default, rename = "cursor-style")]
    pub cursor_style: Option<String>,
    /// Active Mash Up Pack id (a directory under `packs/`). Applied on
    /// startup: the pack's theme wins over `theme` above, and its
    /// shader overlay / filters are re-applied. Empty/unset = no pack.
    #[serde(default, rename = "mashup-pack")]
    pub mashup_pack: Option<String>,
    /// FPS pill on the status line's right cluster — shows the frame
    /// rate the window is actually rendering at. On by default; set
    /// `status-fps = false` to hide it.
    #[serde(default = "default_bool_true", rename = "status-fps")]
    pub status_fps: bool,
    /// Vim keybindings in the code AND markdown editors. On by default;
    /// set `vim-mode = false` for plain (always-insert) editing. New
    /// editors honor this on open; toggling it re-applies on the next
    /// editor you open.
    #[serde(default = "default_bool_true", rename = "vim-mode")]
    pub vim_mode: bool,
}

impl Default for Neoism {
    fn default() -> Self {
        Self {
            theme: default_neoism_theme(),
            minimap: false,
            display_name: None,
            cursor_color: None,
            cursor_style: None,
            mashup_pack: None,
            format_on_save: true,
            status_fps: true,
            vim_mode: true,
        }
    }
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default)]
    pub cursor: CursorConfig,
    #[serde(default = "Navigation::default")]
    pub navigation: Navigation,
    #[serde(default = "Window::default")]
    pub window: Window,
    #[serde(default = "default_shell")]
    pub shell: Shell,
    #[serde(default = "Platform::default")]
    pub platform: Platform,
    #[serde(default = "default_use_fork", rename = "use-fork")]
    pub use_fork: bool,
    #[serde(default = "Keyboard::default")]
    pub keyboard: Keyboard,
    #[serde(default = "Title::default")]
    pub title: Title,
    #[serde(default = "default_working_dir", rename = "working-dir")]
    pub working_dir: Option<String>,
    #[serde(rename = "line-height", default = "default_line_height")]
    pub line_height: f32,
    /// Terminal color palette file (`themes/<name>.json`). The IDE
    /// theme is the flattened `theme` key just below (from `neoism`).
    #[serde(default = "String::default")]
    pub palette: String,
    #[serde(flatten)]
    pub neoism: Neoism,
    #[serde(default = "Scroll::default")]
    pub scroll: Scroll,
    #[serde(
        default = "Option::default",
        skip_serializing,
        rename = "adaptive-theme"
    )]
    pub adaptive_theme: Option<AdaptiveTheme>,
    #[serde(default = "SugarloafFonts::default")]
    pub fonts: SugarloafFonts,
    #[serde(default = "default_editor")]
    pub editor: Shell,
    #[serde(default = "default_margin")]
    pub margin: Margin,
    #[serde(default = "Panel::default")]
    pub panel: Panel,
    #[serde(default = "Vec::default", rename = "env-vars")]
    pub env_vars: Vec<String>,
    #[serde(default = "default_option_as_alt", rename = "option-as-alt")]
    pub option_as_alt: String,
    #[serde(default = "Colors::default", skip_serializing)]
    pub colors: Colors,
    #[serde(default = "Option::default", skip_serializing)]
    pub adaptive_colors: Option<AdaptiveColors>,
    #[serde(default = "Option::default", rename = "force-theme")]
    pub force_theme: Option<AppearanceTheme>,
    #[serde(default = "Developer::default")]
    pub developer: Developer,
    #[serde(default = "Bindings::default")]
    pub bindings: bindings::Bindings,
    #[serde(
        default = "bool::default",
        rename = "ignore-selection-foreground-color"
    )]
    pub ignore_selection_fg_color: bool,
    #[serde(default = "bool::default", rename = "confirm-before-quit")]
    pub confirm_before_quit: bool,
    #[serde(default = "bool::default", rename = "copy-on-select")]
    pub copy_on_select: bool,
    #[serde(default = "bool::default", rename = "hide-mouse-cursor-when-typing")]
    pub hide_cursor_when_typing: bool,
    #[serde(default = "Renderer::default")]
    pub renderer: Renderer,
    #[serde(default = "bool::default", rename = "draw-bold-text-with-light-colors")]
    pub draw_bold_text_with_light_colors: bool,
    #[serde(default = "Hints::default")]
    pub hints: Hints,
    #[serde(default = "Bell::default")]
    pub bell: Bell,
    /// Individual look-slot overrides (`[look.scrollbar]`,
    /// `[look.markdown]`, `[look.icons]`) — win field-by-field over
    /// the active Mash Up Pack's slots.
    #[serde(default)]
    pub look: mashup::LookConfig,
    #[serde(default = "default_bool_true", rename = "enable-scroll-bar")]
    pub enable_scroll_bar: bool,
    #[serde(
        default = "default_scrollback_history_limit",
        rename = "scrollback-history-limit"
    )]
    pub scrollback_history_limit: usize,
    #[serde(default = "effects::Effects::default")]
    pub effects: effects::Effects,
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
pub fn create_config_file(path: Option<PathBuf>) {
    let default_file_path = path.clone().unwrap_or(config_file_path());
    if default_file_path.exists() {
        tracing::info!(
            "configuration file already exists at {}",
            default_file_path.display()
        );
        return;
    }

    if path.is_none() {
        let default_dir_path = config_dir_path();
        match std::fs::create_dir_all(&default_dir_path) {
            Ok(_) => {
                tracing::info!(
                    "configuration path created {}",
                    default_dir_path.display()
                );
            }
            Err(err_message) => {
                tracing::error!("could not create config directory: {err_message}");
            }
        }
    }

    match File::create(&default_file_path) {
        Err(err_message) => {
            tracing::error!(
                "could not create config file {}: {err_message}",
                default_file_path.display()
            )
        }
        Ok(mut created_file) => {
            tracing::info!("configuration file created {}", default_file_path.display());

            if let Err(err_message) = writeln!(created_file, "{}", config_file_content())
            {
                tracing::error!(
                    "could not update config file with defaults: {err_message}"
                )
            }
        }
    }
}

pub fn write_neoism_preferences(
    theme: Option<&str>,
    minimap: Option<bool>,
    mashup_pack: Option<&str>,
) -> std::io::Result<()> {
    let mut updates: Vec<(&str, serde_json::Value)> = Vec::new();
    if let Some(theme) = theme {
        updates.push(("theme", serde_json::Value::String(theme.to_string())));
    }
    if let Some(minimap) = minimap {
        updates.push(("minimap", serde_json::Value::Bool(minimap)));
    }
    if let Some(pack) = mashup_pack {
        // Empty string persists "no pack" while keeping the key's spot
        // in the file.
        updates.push(("mashup-pack", serde_json::Value::String(pack.to_string())));
    }
    // The former `[neoism]` keys are flattened to the document root now.
    write_config_keys(None, &updates)
}

/// Persist one setting to `config.json` for the GUI settings panel. A
/// dotted `section.field` key writes into that `[section]` object; a flat
/// key writes at the document root. The fs-watcher then hot-reloads it.
pub fn write_setting(key: &str, value: serde_json::Value) -> std::io::Result<()> {
    match key.split_once('.') {
        Some((section, field)) => write_config_keys(Some(section), &[(field, value)]),
        None => write_config_keys(None, &[(key, value)]),
    }
}

/// Upsert (or clear) a `[bindings]` keybinding override for `action` in
/// config.json — backs the GUI Keybinds section. Replaces any existing
/// binding for the same action; an empty `key` removes the override so
/// the built-in default applies again. Bindings are read at launch, so a
/// change here takes effect on the next start.
pub fn write_keybind(action: &str, key: &str, with: &str) -> std::io::Result<()> {
    let config_dir = config_dir_path();
    std::fs::create_dir_all(&config_dir)?;
    let path = config_file_path();
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let cleaned = strip_trailing_commas(&strip_json_comments(&content));
    let mut root: serde_json::Value = if cleaned.trim().is_empty() {
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str(&cleaned).map_err(|err| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("failed to parse {}: {err}", path.display()),
            )
        })?
    };
    if !root.is_object() {
        root = serde_json::Value::Object(serde_json::Map::new());
    }
    let obj = root.as_object_mut().expect("root forced to object above");
    let bindings = obj
        .entry("bindings".to_string())
        .or_insert_with(|| serde_json::json!({ "keys": [] }));
    if !bindings.is_object() {
        *bindings = serde_json::json!({ "keys": [] });
    }
    let keys = bindings
        .as_object_mut()
        .expect("bindings forced to object")
        .entry("keys".to_string())
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    if !keys.is_array() {
        *keys = serde_json::Value::Array(Vec::new());
    }
    let arr = keys.as_array_mut().expect("keys forced to array");
    arr.retain(|entry| {
        entry.get("action").and_then(serde_json::Value::as_str) != Some(action)
    });
    if !key.is_empty() {
        let mut entry = serde_json::Map::new();
        entry.insert(
            "key".to_string(),
            serde_json::Value::String(key.to_string()),
        );
        if !with.is_empty() {
            entry.insert(
                "with".to_string(),
                serde_json::Value::String(with.to_string()),
            );
        }
        entry.insert(
            "action".to_string(),
            serde_json::Value::String(action.to_string()),
        );
        arr.push(serde_json::Value::Object(entry));
    }
    let mut out = serde_json::to_string_pretty(&root).map_err(|err| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string())
    })?;
    out.push('\n');
    std::fs::write(path, out)
}

/// Load the active `config.json` as a raw JSON value (all keys — terminal
/// AND agent) so the GUI settings panel can reflect current values,
/// including keys the terminal `Config` struct doesn't model.
pub fn load_config_json_value() -> serde_json::Value {
    let path = config_file_path();
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let cleaned = strip_trailing_commas(&strip_json_comments(&content));
    if cleaned.trim().is_empty() {
        return serde_json::Value::Object(serde_json::Map::new());
    }
    serde_json::from_str(&cleaned)
        .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()))
}

/// Persist `[fonts] family` — Mash Up Packs use this so their font
/// lands the same way a manual config edit would (the config watcher
/// rebuilds the font library from the write).
pub fn write_fonts_family(family: &str) -> std::io::Result<()> {
    write_config_section(
        "fonts",
        &[("family", serde_json::Value::String(family.to_string()))],
    )
}

/// Update keys inside one `[section]` of the ACTIVE `config.json`
/// (structure-preserving via serde; comments in a hand-edited JSONC
/// file are lost on programmatic writes).
fn write_config_section(
    section: &str,
    updates: &[(&str, serde_json::Value)],
) -> std::io::Result<()> {
    write_config_keys(Some(section), updates)
}

/// Merge `updates` into `config.json`: under `section` when given,
/// otherwise at the document root (used for the flattened former
/// `[neoism]` keys, which now live at top level).
fn write_config_keys(
    section: Option<&str>,
    updates: &[(&str, serde_json::Value)],
) -> std::io::Result<()> {
    if updates.is_empty() {
        return Ok(());
    }
    let config_dir = config_dir_path();
    std::fs::create_dir_all(&config_dir)?;
    let path = config_file_path();
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let cleaned = strip_trailing_commas(&strip_json_comments(&content));
    let mut root = if cleaned.trim().is_empty() {
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str::<serde_json::Value>(&cleaned).map_err(|err| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("failed to parse {}: {err}", path.display()),
            )
        })?
    };
    if !root.is_object() {
        root = serde_json::Value::Object(serde_json::Map::new());
    }
    let object = root.as_object_mut().expect("root forced to object above");
    let target = match section {
        Some(section) => {
            let entry = object
                .entry(section.to_string())
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
            if !entry.is_object() {
                *entry = serde_json::Value::Object(serde_json::Map::new());
            }
            entry.as_object_mut().expect("section forced to object")
        }
        None => object,
    };
    for (key, value) in updates {
        target.insert((*key).to_string(), value.clone());
    }
    let mut out = serde_json::to_string_pretty(&root).map_err(|err| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string())
    })?;
    out.push('\n');
    std::fs::write(path, out)
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
            match parse_config_content::<Config>(&path, &content) {
                Ok(mut decoded) => {
                    let palette = &decoded.palette;
                    if palette.is_empty() {
                        return decoded;
                    }

                    let path = config_path
                        .join("themes")
                        .join(palette)
                        .with_extension("json");
                    if let Ok(loaded_theme) = Config::load_theme(&path) {
                        decoded.colors = loaded_theme.colors;
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
                Ok(content) => match parse_config_content::<Config>(&path, &content) {
                    Ok(mut decoded) => {
                        let palette = &decoded.palette;
                        let theme_path = config_dir_path().join("themes");
                        if !palette.is_empty() {
                            let path = theme_path.join(palette).with_extension("json");
                            match Config::load_theme(&path) {
                                Ok(loaded_theme) => {
                                    decoded.colors = loaded_theme.colors;
                                }
                                Err(err_message) => {
                                    return Err(ConfigError::ErrLoadingTheme(
                                        err_message,
                                    ));
                                }
                            }
                        }

                        if let Some(adaptive_theme) = &decoded.adaptive_theme {
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
                                decoded.adaptive_colors = Some(adaptive_colors);
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
            self.shell = shell_overwrite.clone();
        }

        // Merge window fields individually
        if let Some(window_overwrite) = &platform_config.window {
            if let Some(width) = window_overwrite.width {
                self.window.width = width;
            }
            if let Some(height) = window_overwrite.height {
                self.window.height = height;
            }
            if let Some(columns) = window_overwrite.columns {
                self.window.columns = Some(columns);
            }
            if let Some(rows) = window_overwrite.rows {
                self.window.rows = Some(rows);
            }
            if let Some(mode) = window_overwrite.mode {
                self.window.mode = mode;
            }
            if let Some(opacity) = window_overwrite.opacity {
                self.window.opacity = opacity;
            }
            if let Some(blur) = window_overwrite.blur {
                self.window.blur = blur;
            }
            if let Some(bg_image) = &window_overwrite.background_image {
                self.window.background_image = Some(bg_image.clone());
            }
            if let Some(decorations) = window_overwrite.decorations {
                self.window.decorations = decorations;
            }
            if let Some(macos_unified) = window_overwrite.macos_use_unified_titlebar {
                self.window.macos_use_unified_titlebar = macos_unified;
            }
            if let Some(macos_shadow) = window_overwrite.macos_use_shadow {
                self.window.macos_use_shadow = macos_shadow;
            }
            if let Some(x) = window_overwrite.macos_traffic_light_position_x {
                self.window.macos_traffic_light_position_x = Some(x);
            }
            if let Some(y) = window_overwrite.macos_traffic_light_position_y {
                self.window.macos_traffic_light_position_y = Some(y);
            }
            if let Some(initial_title) = &window_overwrite.initial_title {
                self.window.initial_title = Some(initial_title.clone());
            }
            if let Some(win_shadow) = window_overwrite.windows_use_undecorated_shadow {
                self.window.windows_use_undecorated_shadow = Some(win_shadow);
            }
            if let Some(win_bitmap) = window_overwrite.windows_use_no_redirection_bitmap {
                self.window.windows_use_no_redirection_bitmap = Some(win_bitmap);
            }
            if let Some(win_corner) = &window_overwrite.windows_corner_preference {
                self.window.windows_corner_preference = Some(win_corner.clone());
            }
            if let Some(colorspace) = window_overwrite.colorspace {
                self.window.colorspace = colorspace;
            }
        }

        // Merge navigation fields individually
        if let Some(navigation_overwrite) = &platform_config.navigation {
            if let Some(mode) = navigation_overwrite.mode {
                self.navigation.mode = mode;
            }
            if let Some(color_automation) = &navigation_overwrite.color_automation {
                self.navigation.color_automation = color_automation.clone();
            }
            if let Some(clickable) = navigation_overwrite.clickable {
                self.navigation.clickable = clickable;
            }
            if let Some(cwd) = navigation_overwrite.current_working_directory {
                self.navigation.current_working_directory = cwd;
            }
            if let Some(use_term_title) = navigation_overwrite.use_terminal_title {
                self.navigation.use_terminal_title = use_term_title;
            }
            if let Some(hide_if_single) = navigation_overwrite.hide_if_single {
                self.navigation.hide_if_single = hide_if_single;
            }
            if let Some(use_split) = navigation_overwrite.use_split {
                self.navigation.use_split = use_split;
            }
            if let Some(open_cfg_split) = navigation_overwrite.open_config_with_split {
                self.navigation.open_config_with_split = open_cfg_split;
            }
            if let Some(unfocused_opacity) = navigation_overwrite.unfocused_split_opacity
            {
                self.navigation.unfocused_split_opacity = unfocused_opacity;
            }
            if let Some(fill) = navigation_overwrite.unfocused_split_fill {
                self.navigation.unfocused_split_fill = Some(fill);
            }
        }

        // Clamp after platform merge so both the base and any override go
        // through the same bound.
        self.navigation.unfocused_split_opacity =
            crate::config::navigation::clamp_unfocused_split_opacity(
                self.navigation.unfocused_split_opacity,
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
            self.env_vars.extend(env_vars_overwrite.clone());
        }

        // Override theme
        if let Some(theme_overwrite) = &platform_config.theme {
            self.palette = theme_overwrite.clone();
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            cursor: CursorConfig::default(),
            editor: default_editor(),
            adaptive_theme: None,
            adaptive_colors: None,
            force_theme: None,
            bindings: Bindings::default(),
            colors: Colors::default(),
            scroll: Scroll::default(),
            keyboard: Keyboard::default(),
            title: Title::default(),
            developer: Developer::default(),
            env_vars: vec![],
            fonts: SugarloafFonts::default(),
            line_height: default_line_height(),
            navigation: Navigation::default(),
            option_as_alt: default_option_as_alt(),
            palette: String::default(),
            margin: default_margin(),
            panel: Panel::default(),
            renderer: Renderer::default(),
            shell: default_shell(),
            platform: Platform::default(),
            neoism: Neoism::default(),
            use_fork: default_use_fork(),
            window: Window::default(),
            working_dir: default_working_dir(),
            ignore_selection_fg_color: false,
            confirm_before_quit: false,
            copy_on_select: false,
            hide_cursor_when_typing: false,
            draw_bold_text_with_light_colors: false,
            hints: Hints::default(),
            bell: Bell::default(),
            look: mashup::LookConfig::default(),
            enable_scroll_bar: true,
            scrollback_history_limit: default_scrollback_history_limit(),
            effects: effects::Effects::default(),
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
    use std::path::Path;

    fn parse(json: &str) -> Config {
        parse_config_content::<Config>(Path::new("config.json"), json)
            .expect("config should parse")
    }

    #[test]
    fn empty_config_uses_defaults() {
        let config = parse("{}");
        assert_eq!(config.palette, String::default());
        assert_eq!(config.cursor.shape, default_cursor());
        assert_eq!(config.fonts, SugarloafFonts::default());
        assert_eq!(config.colors, Colors::default());
        assert_eq!(config.developer, Developer::default());
        assert!(!config.renderer.disable_unfocused_render);
        // Flattened former-`[neoism]` keys default correctly at the root.
        assert_eq!(config.neoism.theme, default_neoism_theme());
        assert!(config.neoism.status_fps);
        assert!(config.neoism.format_on_save);
        assert!(!config.neoism.minimap);
    }

    #[test]
    fn jsonc_comments_and_trailing_commas_are_tolerated() {
        let config = parse(
            r#"
// line comment — legal in the unified config
{
    /* block comment */
    "line-height": 1.4,
    "palette": "lucario",
    "theme": "tokyo_night",   // IDE theme, flattened to root
    "minimap": true,
    "status-fps": false,
    "fonts": { "size": 16.0 },
    // agent-server keys co-live in the same file; the app ignores them
    "model": "anthropic/claude-opus-5",
}
"#,
        );
        assert_eq!(config.line_height, 1.4);
        assert_eq!(config.palette, "lucario");
        assert_eq!(config.neoism.theme, "tokyo_night");
        assert!(config.neoism.minimap);
        assert!(!config.neoism.status_fps);
        assert_eq!(config.fonts.size, 16.0);
    }

    #[test]
    fn neoism_keys_live_at_root_not_under_a_section() {
        // The former `[neoism]` block is flattened: its keys sit at the
        // top level. A nested `neoism` object is ignored, not honored.
        let flat = parse(r#"{ "display-name": "parker", "mashup-pack": "synth" }"#);
        assert_eq!(flat.neoism.display_name.as_deref(), Some("parker"));
        assert_eq!(flat.neoism.mashup_pack.as_deref(), Some("synth"));

        let nested = parse(r#"{ "neoism": { "display-name": "parker" } }"#);
        assert_eq!(nested.neoism.display_name, None);
    }

    #[test]
    fn theme_is_ide_and_palette_is_terminal_colors() {
        let config = parse(r#"{ "theme": "catppuccin_mocha", "palette": "lucario" }"#);
        assert_eq!(config.neoism.theme, "catppuccin_mocha"); // IDE theme
        assert_eq!(config.palette, "lucario"); // terminal color file
    }

    #[test]
    fn renderer_and_cursor_sections_parse() {
        let config = parse(
            r#"{
                "cursor": { "shape": "underline" },
                "renderer": { "backend": "Vulkan" }
            }"#,
        );
        assert_eq!(config.cursor.shape, CursorShape::Underline);
        assert_eq!(config.renderer.backend, renderer::Backend::Vulkan);
    }

    #[test]
    fn colors_parse_from_hex() {
        let config = parse(r##"{ "colors": { "foreground": "#000000" } }"##);
        assert_eq!(config.colors.foreground, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(config.colors.background, colors::defaults::background());
    }

    #[test]
    fn env_vars_and_option_as_alt_parse() {
        let config = parse(r#"{ "env-vars": ["A=5", "B=8"], "option-as-alt": "Both" }"#);
        assert_eq!(config.env_vars, [String::from("A=5"), String::from("B=8")]);
        assert_eq!(config.option_as_alt, String::from("Both"));
    }

    #[test]
    fn dropped_aliases_are_no_longer_accepted() {
        // cwd / hide-cursor-when-typing / blinking-cursor were removed;
        // they now parse as ignored unknown keys, leaving the defaults.
        let config = parse(
            r#"{ "cwd": false, "hide-cursor-when-typing": true, "blinking-cursor": true }"#,
        );
        assert!(config.navigation.current_working_directory); // default true
        assert!(!config.hide_cursor_when_typing); // default false
        assert!(!config.cursor.blinking); // default false
    }

    #[test]
    fn default_template_parses_to_defaults() {
        let config = parse(&default_config_file_content());
        assert_eq!(config.palette, String::default());
        assert_eq!(config.fonts, SugarloafFonts::default());
        assert_eq!(config.bindings, Bindings::default());
        assert!(config.neoism.status_fps);
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
