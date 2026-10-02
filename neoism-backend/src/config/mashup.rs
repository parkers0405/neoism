//! Mash Up Packs: user-droppable look bundles (IDE theme + shader +
//! fonts) plus standalone runtime IDE themes.
//!
//! Disk layout under the config dir (`~/.config/neoism`):
//!
//! ```text
//! ide-themes/<name>.json          standalone runtime theme
//! packs/<id>/pack.json            pack manifest
//! packs/<id>/theme.json           optional theme the pack ships
//! packs/<id>/*.glsl               shader overlays referenced by pack.json
//! ```
//!
//! JSON files speak the same JSONC dialect as config.json (comments +
//! trailing commas).
//!
//! This module only reads and resolves files — turning specs into an
//! `IdeTheme` and applying slots lives in the frontend, which owns the
//! theme registry and the render surfaces.

use super::config_dir_path;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The user's effective appearance before the first active Mash Up Pack.
/// It remains unchanged while switching packs and is restored on deactivation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct MashupBaseline {
    pub theme: String,
    pub font_family: Option<String>,
}

/// Complete persisted appearance state produced by one pack transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppearanceTransition {
    pub mashup_pack: Option<String>,
    pub mashup_baseline: Option<MashupBaseline>,
    pub theme: String,
    pub font_family: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppearanceTransitionError {
    pub pack_id: String,
}

impl std::fmt::Display for AppearanceTransitionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Mash Up Pack not found: {}", self.pack_id)
    }
}

impl std::error::Error for AppearanceTransitionError {}

/// How a Mash Up Pack selects editor Lua plugins.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EditorPluginMode {
    /// Keep the normal globally eligible set and subtract `disabled`.
    #[default]
    Overlay,
    /// Admit only `enabled` roots and their dependency closure.
    Only,
}

/// A pack's normalized editor-only Lua plugin declaration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct EditorPluginSelection {
    pub mode: EditorPluginMode,
    pub enabled: Vec<String>,
    pub disabled: Vec<String>,
}

/// Per-pack user overrides. `None` means inherit the pack field, while
/// `Some([])` explicitly clears a list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct EditorPluginOverride {
    pub mode: Option<EditorPluginMode>,
    pub enabled: Option<Vec<String>>,
    pub disabled: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorPluginSelectionError {
    pub plugin_id: String,
}

impl std::fmt::Display for EditorPluginSelectionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "editor plugin `{}` is both enabled and disabled", self.plugin_id)
    }
}

impl std::error::Error for EditorPluginSelectionError {}

/// Look slots beyond theme/shader: scrollbars, markdown decorations,
/// icon overrides. A pack sets these from top-level sections of its
/// `pack.toml` (`[scrollbar]`, `[markdown]`, `[icons]`); the user can
/// override any slot individually from `config.toml` under `[look.*]`
/// — config wins over the active pack, field by field.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LookConfig {
    #[serde(default)]
    pub scrollbar: ScrollbarLook,
    #[serde(default)]
    pub markdown: MarkdownLook,
    #[serde(default)]
    pub wordmark: WordmarkLook,
    #[serde(default)]
    pub icons: BTreeMap<String, IconLook>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScrollbarLook {
    /// Thumb thickness in logical px (per-site default when unset).
    #[serde(default)]
    pub width: Option<f32>,
    /// Corner rounding as a fraction of width: 0.0 square … 0.5 pill.
    #[serde(default, rename = "radius-factor")]
    pub radius_factor: Option<f32>,
    /// Sugar for `radius-factor = 0` — chunky nineties bars.
    #[serde(default)]
    pub square: Option<bool>,
    #[serde(default, rename = "min-thumb")]
    pub min_thumb: Option<f32>,
    /// `#RRGGBB` colors; unset keeps each site's themed/gray default.
    #[serde(default)]
    pub thumb: Option<String>,
    #[serde(default, rename = "thumb-drag")]
    pub thumb_drag: Option<String>,
    #[serde(default)]
    pub track: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MarkdownLook {
    /// Task checkbox style: "modern" (default) or "retro95".
    #[serde(default)]
    pub checkbox: Option<String>,
    /// Font family for the markdown surface only.
    #[serde(default, rename = "font-family")]
    pub font_family: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WordmarkLook {
    /// Per-letter tint cycle for the NEOISM wordmarks (splash + agent
    /// home). One color = uniform tint; several cycle across the
    /// letters (Windows-flag style). Empty = the theme's `fg`.
    #[serde(default)]
    pub colors: Vec<String>,
}

/// One icon override: a bare glyph string, or `{ glyph, color }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IconLook {
    Glyph(String),
    Full {
        #[serde(default)]
        glyph: Option<String>,
        #[serde(default)]
        color: Option<String>,
    },
}

impl IconLook {
    pub fn glyph(&self) -> Option<&str> {
        match self {
            IconLook::Glyph(glyph) => Some(glyph),
            IconLook::Full { glyph, .. } => glyph.as_deref(),
        }
    }

    pub fn color(&self) -> Option<&str> {
        match self {
            IconLook::Glyph(_) => None,
            IconLook::Full { color, .. } => color.as_deref(),
        }
    }
}

impl LookConfig {
    /// Field-by-field merge: `overlay` wins wherever it sets a value;
    /// icon keys union with overlay priority.
    pub fn merged_under(&self, overlay: &LookConfig) -> LookConfig {
        fn pick<T: Clone>(base: &Option<T>, over: &Option<T>) -> Option<T> {
            over.clone().or_else(|| base.clone())
        }
        let mut icons = self.icons.clone();
        icons.extend(
            overlay
                .icons
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        LookConfig {
            scrollbar: ScrollbarLook {
                width: pick(&self.scrollbar.width, &overlay.scrollbar.width),
                radius_factor: pick(
                    &self.scrollbar.radius_factor,
                    &overlay.scrollbar.radius_factor,
                ),
                square: pick(&self.scrollbar.square, &overlay.scrollbar.square),
                min_thumb: pick(&self.scrollbar.min_thumb, &overlay.scrollbar.min_thumb),
                thumb: pick(&self.scrollbar.thumb, &overlay.scrollbar.thumb),
                thumb_drag: pick(
                    &self.scrollbar.thumb_drag,
                    &overlay.scrollbar.thumb_drag,
                ),
                track: pick(&self.scrollbar.track, &overlay.scrollbar.track),
            },
            markdown: MarkdownLook {
                checkbox: pick(&self.markdown.checkbox, &overlay.markdown.checkbox),
                font_family: pick(
                    &self.markdown.font_family,
                    &overlay.markdown.font_family,
                ),
            },
            wordmark: WordmarkLook {
                colors: if overlay.wordmark.colors.is_empty() {
                    self.wordmark.colors.clone()
                } else {
                    overlay.wordmark.colors.clone()
                },
            },
            icons,
        }
    }
}

/// A runtime IDE theme as read from disk: a base theme name plus
/// `key = "#hex"` overrides. The frontend converts this into an
/// `IdeTheme` and registers it.
#[derive(Debug, Clone)]
pub struct IdeThemeSpec {
    pub name: String,
    pub description: String,
    pub extends: String,
    pub colors: Vec<(String, String)>,
}

#[derive(Deserialize)]
struct IdeThemeFile {
    name: Option<String>,
    description: Option<String>,
    extends: Option<String>,
    #[serde(default)]
    colors: std::collections::BTreeMap<String, String>,
}

/// A Mash Up Pack manifest with every asset path resolved relative to
/// the pack directory. Each slot is optional — a pack sets the slots
/// it ships and leaves the rest of the user's setup alone.
#[derive(Debug, Clone)]
pub struct MashupPack {
    /// Directory name under `packs/` — the stable id persisted in
    /// `[neoism] mashup-pack`.
    pub id: String,
    pub name: String,
    pub description: String,
    /// IDE theme name this pack applies (builtin, standalone, or the
    /// pack's own `theme.toml`).
    pub theme: Option<String>,
    /// Shader overlay: `builtin:*` passed through, files resolved to
    /// absolute paths.
    pub shader_overlay: Option<String>,
    /// librashader `.slangp` filter chain (wgpu backend only).
    pub filters: Vec<String>,
    /// `[fonts] family` the pack wants active.
    pub font_family: Option<String>,
    /// Window background image (the Rio `[window] background-image`
    /// machinery): path resolved pack-relative, opacity pre-baked at
    /// upload. A user-set `[window] background-image` in config.toml
    /// wins over the pack's.
    pub wallpaper: Option<sugarloaf::ImageProperties>,
    /// Scrollbar / markdown / icon slots from the manifest's top-level
    /// `[scrollbar]` / `[markdown]` / `[icons]` sections.
    pub look: LookConfig,
    /// Editor Lua plugin policy shipped by this pack. Agent and daemon
    /// plugins are intentionally outside this selection.
    pub editor_plugins: Option<EditorPluginSelection>,
    pub dir: PathBuf,
}

#[derive(Deserialize)]
struct PackFile {
    pack: PackSection,
    #[serde(default, rename = "editor-plugins")]
    editor_plugins: Option<EditorPluginSelection>,
    #[serde(flatten)]
    look: LookConfig,
}

#[derive(Deserialize)]
struct PackSection {
    name: Option<String>,
    description: Option<String>,
    theme: Option<String>,
    #[serde(rename = "shader-overlay")]
    shader_overlay: Option<String>,
    #[serde(default)]
    filters: Vec<String>,
    #[serde(rename = "font-family")]
    font_family: Option<String>,
    wallpaper: Option<String>,
    #[serde(rename = "wallpaper-opacity")]
    wallpaper_opacity: Option<f32>,
}

pub fn ide_themes_dir() -> PathBuf {
    config_dir_path().join("ide-themes")
}

pub fn packs_dir() -> PathBuf {
    config_dir_path().join("packs")
}

/// Omarchy's stable state directory. The `theme` child is atomically
/// replaced whenever `omarchy-theme-set` runs.
#[cfg(target_os = "linux")]
pub fn omarchy_current_dir() -> Option<PathBuf> {
    let state_home = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".local/state")))?;
    Some(state_home.join("omarchy/current"))
}

#[cfg(not(target_os = "linux"))]
pub fn omarchy_current_dir() -> Option<PathBuf> {
    None
}

fn omarchy_theme_spec(path: &Path) -> Option<IdeThemeSpec> {
    let source = std::fs::read_to_string(path).ok()?;
    let colors: BTreeMap<String, String> = match toml::from_str(&source) {
        Ok(colors) => colors,
        Err(err) => {
            tracing::warn!(
                target: "neoism::mashup",
                "skipping Omarchy theme {}: {err}",
                path.display()
            );
            return None;
        }
    };

    let pick = |names: &[&str]| names.iter().find_map(|name| colors.get(*name)).cloned();
    let mappings: &[(&str, &[&str])] = &[
        ("bg", &["background"]),
        ("fg", &["bright_foreground", "foreground"]),
        ("surface", &["lighter_background", "dark_background"]),
        ("hover", &["selection", "lighter_background"]),
        ("border", &["muted"]),
        ("muted", &["dark_foreground", "muted"]),
        ("dim", &["light_foreground", "foreground"]),
        ("accent", &["accent", "blue"]),
        ("folder", &["accent", "blue"]),
        ("black", &["darker_background", "background"]),
        ("white", &["bright_foreground", "foreground"]),
        ("red", &["red"]),
        ("green", &["green"]),
        ("yellow", &["yellow"]),
        ("blue", &["blue", "accent"]),
        ("magenta", &["magenta"]),
        ("cyan", &["cyan"]),
        ("comment", &["muted", "dark_foreground"]),
        ("string", &["green"]),
        ("number", &["orange", "bright_yellow", "yellow"]),
        ("keyword", &["magenta"]),
        ("statement", &["bright_magenta", "magenta"]),
        ("func", &["bright_blue", "blue", "accent"]),
        ("type", &["yellow"]),
        ("property", &["cyan"]),
        ("constructor", &["bright_cyan", "cyan"]),
        ("special", &["bright_red", "red"]),
    ];

    Some(IdeThemeSpec {
        name: "omarchy".to_string(),
        description: "Follows the active Omarchy theme".to_string(),
        extends: "pastel_dark".to_string(),
        colors: mappings
            .iter()
            .filter_map(|(role, names)| {
                pick(names).map(|value| ((*role).to_string(), value))
            })
            .collect(),
    })
}

/// First existing spelling of a pack-relative file: `<stem>.json`
/// then `<stem>.jsonc`.
fn existing_variant(dir: &Path, stem: &str) -> Option<PathBuf> {
    ["json", "jsonc"]
        .iter()
        .map(|ext| dir.join(format!("{stem}.{ext}")))
        .find(|path| path.is_file())
}

fn parse_theme_file(path: &Path, fallback_name: &str) -> Option<IdeThemeSpec> {
    let source = std::fs::read_to_string(path).ok()?;
    let file: IdeThemeFile = match super::parse_config_content(path, &source) {
        Ok(file) => file,
        Err(err) => {
            tracing::warn!(
                target: "neoism::mashup",
                "skipping theme file {}: {err}",
                path.display()
            );
            return None;
        }
    };
    Some(IdeThemeSpec {
        name: file
            .name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| fallback_name.to_string()),
        description: file.description.unwrap_or_default(),
        extends: file
            .extends
            .filter(|base| !base.trim().is_empty())
            .unwrap_or_else(|| "pastel_dark".to_string()),
        colors: file.colors.into_iter().collect(),
    })
}

/// Every runtime theme on disk: `ide-themes/*.json` first, then each
/// pack's `theme.json` (named after the pack dir unless the file says
/// otherwise). Unreadable files are skipped with a warning so one typo
/// can't hide every other theme.
pub fn load_ide_theme_specs() -> Vec<IdeThemeSpec> {
    let mut specs: Vec<IdeThemeSpec> = Vec::new();
    let mut push = |spec: IdeThemeSpec| {
        if !specs.iter().any(|existing| existing.name == spec.name) {
            specs.push(spec);
        }
    };

    if let Ok(entries) = std::fs::read_dir(ide_themes_dir()) {
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|ext| ext == "json" || ext == "jsonc")
            })
            .collect();
        paths.sort();
        for path in paths {
            let stem = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_default();
            if let Some(spec) = parse_theme_file(&path, &stem) {
                push(spec);
            }
        }
    }

    for pack_dir in pack_dirs() {
        let Some(theme_path) = existing_variant(&pack_dir, "theme") else {
            continue;
        };
        let id = pack_dir
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        if let Some(spec) = parse_theme_file(&theme_path, &id) {
            push(spec);
        }
    }

    // Keep an explicitly installed `ide-themes/omarchy.json` authoritative,
    // otherwise expose Omarchy's active semantic palette as a native theme.
    if let Some(spec) = omarchy_current_dir()
        .map(|dir| dir.join("theme/colors.toml"))
        .and_then(|path| omarchy_theme_spec(&path))
    {
        push(spec);
    }

    specs
}

fn pack_dirs() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(packs_dir()) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && existing_variant(path, "pack").is_some())
        .collect();
    dirs.sort();
    dirs
}

fn resolve_asset(dir: &Path, value: &str) -> String {
    if value.starts_with("builtin:") || Path::new(value).is_absolute() {
        return value.to_string();
    }
    dir.join(value).to_string_lossy().to_string()
}

/// Every installed pack, sorted by id. A pack with a `theme.json` but
/// no explicit `theme` key applies its bundled theme.
pub fn load_mashup_packs() -> Vec<MashupPack> {
    pack_dirs()
        .into_iter()
        .filter_map(|dir| {
            let id = dir.file_name()?.to_string_lossy().to_string();
            let manifest = existing_variant(&dir, "pack")?;
            let source = std::fs::read_to_string(&manifest).ok()?;
            let file: PackFile = match super::parse_config_content(&manifest, &source) {
                Ok(file) => file,
                Err(err) => {
                    tracing::warn!(
                        target: "neoism::mashup",
                        "skipping pack {id}: {err}"
                    );
                    return None;
                }
            };
            let section = file.pack;
            let bundled_theme = existing_variant(&dir, "theme").map(|theme_path| {
                parse_theme_file(&theme_path, &id)
                    .map(|spec| spec.name)
                    .unwrap_or_else(|| id.clone())
            });
            Some(MashupPack {
                name: section.name.unwrap_or_else(|| id.clone()),
                description: section.description.unwrap_or_default(),
                theme: section
                    .theme
                    .filter(|name| !name.trim().is_empty())
                    .or(bundled_theme),
                shader_overlay: section
                    .shader_overlay
                    .filter(|value| !value.trim().is_empty())
                    .map(|value| resolve_asset(&dir, &value)),
                filters: section
                    .filters
                    .iter()
                    .map(|value| resolve_asset(&dir, value))
                    .collect(),
                font_family: section.font_family.filter(|value| !value.trim().is_empty()),
                wallpaper: section
                    .wallpaper
                    .filter(|value| !value.trim().is_empty())
                    .map(|value| sugarloaf::ImageProperties {
                        path: resolve_asset(&dir, &value),
                        opacity: section.wallpaper_opacity.unwrap_or(1.0).clamp(0.0, 1.0),
                    }),
                look: file.look,
                editor_plugins: file.editor_plugins,
                id,
                dir,
            })
        })
        .collect()
}

/// Look up one pack by id.
pub fn find_mashup_pack(id: &str) -> Option<MashupPack> {
    load_mashup_packs().into_iter().find(|pack| pack.id == id)
}

/// Resolve a Mash Up Pack appearance transition without reading or writing disk.
/// The original baseline is the only fallback for omitted pack slots, preventing
/// values from the previously active pack from leaking into the next one.
pub fn resolve_appearance_transition(
    active_pack: Option<&str>,
    baseline: Option<&MashupBaseline>,
    effective_theme: &str,
    effective_font_family: Option<&str>,
    requested_pack: Option<&str>,
    packs: &[MashupPack],
) -> Result<AppearanceTransition, AppearanceTransitionError> {
    let active_pack = active_pack.map(str::trim).filter(|id| !id.is_empty());
    let requested_pack = requested_pack.map(str::trim).filter(|id| !id.is_empty());

    let requested = requested_pack
        .map(|id| {
            packs
                .iter()
                .find(|pack| pack.id == id)
                .ok_or_else(|| AppearanceTransitionError { pack_id: id.to_string() })
        })
        .transpose()?;

    if active_pack.is_none() && requested.is_none() {
        return Ok(AppearanceTransition {
            mashup_pack: None,
            mashup_baseline: None,
            theme: effective_theme.to_string(),
            font_family: effective_font_family.map(str::to_string),
        });
    }

    // A legacy active pack has no trustworthy pre-pack state. Preserve the
    // current effective values on its next explicit transition rather than
    // guessing from defaults or recapturing during startup.
    let capture_effective = || MashupBaseline {
        theme: effective_theme.to_string(),
        font_family: effective_font_family.map(str::to_string),
    };
    let baseline = if active_pack.is_none() {
        // A stale baseline is never authoritative while no pack is active.
        capture_effective()
    } else {
        baseline.cloned().unwrap_or_else(capture_effective)
    };

    let Some(requested) = requested else {
        return Ok(AppearanceTransition {
            mashup_pack: None,
            mashup_baseline: None,
            theme: baseline.theme,
            font_family: baseline.font_family,
        });
    };

    Ok(AppearanceTransition {
        mashup_pack: Some(requested.id.clone()),
        mashup_baseline: Some(baseline.clone()),
        theme: requested.theme.clone().unwrap_or_else(|| baseline.theme.clone()),
        font_family: requested
            .font_family
            .clone()
            .or_else(|| baseline.font_family.clone()),
    })
}

fn normalize_plugin_ids(ids: Vec<String>) -> Vec<String> {
    let mut ids = ids
        .into_iter()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    ids
}

/// Resolve the effective editor Lua plugin selection for an active pack.
/// This is pure: callers provide the already-loaded packs and preferences.
/// No active pack, an unknown pack, or a pack with no declaration/override
/// returns `None`, preserving the global plugin policy exactly.
pub fn resolve_editor_plugin_selection(
    active_pack: Option<&str>,
    packs: &[MashupPack],
    overrides: &BTreeMap<String, EditorPluginOverride>,
) -> Result<Option<EditorPluginSelection>, EditorPluginSelectionError> {
    let Some(active_pack) = active_pack.map(str::trim).filter(|id| !id.is_empty()) else {
        return Ok(None);
    };
    let Some(pack) = packs.iter().find(|pack| pack.id == active_pack) else {
        return Ok(None);
    };
    let override_value = overrides.get(active_pack);
    if pack.editor_plugins.is_none() && override_value.is_none() {
        return Ok(None);
    }

    let mut selection = pack.editor_plugins.clone().unwrap_or_default();
    if let Some(override_value) = override_value {
        if let Some(mode) = override_value.mode {
            selection.mode = mode;
        }
        if let Some(enabled) = &override_value.enabled {
            selection.enabled = enabled.clone();
        }
        if let Some(disabled) = &override_value.disabled {
            selection.disabled = disabled.clone();
        }
    }
    selection.enabled = normalize_plugin_ids(selection.enabled);
    selection.disabled = normalize_plugin_ids(selection.disabled);
    if let Some(plugin_id) = selection
        .enabled
        .iter()
        .find(|id| selection.disabled.binary_search(id).is_ok())
    {
        return Err(EditorPluginSelectionError {
            plugin_id: plugin_id.clone(),
        });
    }
    Ok(Some(selection))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(id: &str, editor_plugins: Option<EditorPluginSelection>) -> MashupPack {
        MashupPack {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            theme: None,
            shader_overlay: None,
            filters: Vec::new(),
            font_family: None,
            wallpaper: None,
            look: LookConfig::default(),
            editor_plugins,
            dir: PathBuf::new(),
        }
    }

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("neoism-mashup-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn theme_file_parses_with_defaults() {
        let dir = scratch_dir("theme");
        let path = dir.join("phosphor.json");
        std::fs::write(
            &path,
            r##"{"description":"Green CRT","colors":{"bg":"#030f06","fg":"#33ff66"}}"##,
        )
        .unwrap();

        let spec = parse_theme_file(&path, "phosphor").unwrap();
        assert_eq!(spec.name, "phosphor");
        assert_eq!(spec.description, "Green CRT");
        assert_eq!(spec.extends, "pastel_dark");
        assert!(spec.colors.iter().any(|(k, v)| k == "bg" && v == "#030f06"));

        // Broken JSON is skipped, not fatal.
        std::fs::write(&path, "{\"colors\":").unwrap();
        assert!(parse_theme_file(&path, "phosphor").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn theme_file_parses_jsonc() {
        let dir = scratch_dir("theme-json");
        let path = dir.join("phosphor.json");
        std::fs::write(
            &path,
            r##"// CRT green
{
    "description": "Green CRT",
    "extends": "pastel_dark",
    "colors": { "bg": "#030f06", "fg": "#33ff66", },
}
"##,
        )
        .unwrap();

        let spec = parse_theme_file(&path, "phosphor").unwrap();
        assert_eq!(spec.name, "phosphor");
        assert_eq!(spec.description, "Green CRT");
        assert!(spec.colors.iter().any(|(k, v)| k == "fg" && v == "#33ff66"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn existing_variant_prefers_json_over_jsonc() {
        let dir = scratch_dir("variant");
        std::fs::write(dir.join("pack.jsonc"), "{}").unwrap();
        std::fs::write(dir.join("pack.json"), "{}").unwrap();
        assert_eq!(existing_variant(&dir, "pack"), Some(dir.join("pack.json")));
        std::fs::remove_file(dir.join("pack.json")).unwrap();
        assert_eq!(existing_variant(&dir, "pack"), Some(dir.join("pack.jsonc")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn omarchy_colors_map_to_ide_roles() {
        let dir = scratch_dir("omarchy");
        let path = dir.join("colors.toml");
        std::fs::write(
            &path,
            r##"
background = "#000000"
foreground = "#eeeeee"
bright_foreground = "#ffffff"
lighter_background = "#181818"
selection = "#242424"
muted = "#777777"
accent = "#89b4fa"
red = "#f38ba8"
green = "#a6e3a1"
yellow = "#f9e2af"
orange = "#fab387"
blue = "#89b4fa"
magenta = "#cba6f7"
cyan = "#94e2d5"
"##,
        )
        .unwrap();

        let spec = omarchy_theme_spec(&path).unwrap();
        let mapped: BTreeMap<_, _> = spec.colors.into_iter().collect();
        assert_eq!(spec.name, "omarchy");
        assert_eq!(mapped.get("bg").map(String::as_str), Some("#000000"));
        assert_eq!(mapped.get("fg").map(String::as_str), Some("#ffffff"));
        assert_eq!(mapped.get("number").map(String::as_str), Some("#fab387"));
        assert_eq!(mapped.get("accent").map(String::as_str), Some("#89b4fa"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pack_assets_resolve_relative_to_pack_dir() {
        let dir = scratch_dir("assets");
        assert_eq!(
            resolve_asset(&dir, "crawl.glsl"),
            dir.join("crawl.glsl").to_string_lossy().to_string()
        );
        assert_eq!(
            resolve_asset(&dir, "builtin:ctv_round"),
            "builtin:ctv_round"
        );
        assert_eq!(resolve_asset(&dir, "/abs/path.glsl"), "/abs/path.glsl");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn editor_plugin_declaration_parses_kebab_case() {
        let file: PackFile = super::super::parse_config_content(
            Path::new("pack.json"),
            r#"{
                "pack": {},
                "editor-plugins": {
                    "mode": "only",
                    "enabled": [" dev.example.a ", "dev.example.a"],
                    "disabled": ["dev.example.b"]
                }
            }"#,
        ).unwrap();
        let selection = file.editor_plugins.unwrap();
        assert_eq!(selection.mode, EditorPluginMode::Only);
        assert_eq!(selection.enabled.len(), 2);
    }

    #[test]
    fn resolver_preserves_no_pack_behavior_and_normalizes_lists() {
        let packs = vec![pack("focused", Some(EditorPluginSelection {
            mode: EditorPluginMode::Overlay,
            enabled: vec![" z ".into(), "".into(), "z".into()],
            disabled: vec![" b ".into(), "a".into()],
        }))];
        assert_eq!(resolve_editor_plugin_selection(None, &packs, &BTreeMap::new()).unwrap(), None);
        assert_eq!(resolve_editor_plugin_selection(Some("missing"), &packs, &BTreeMap::new()).unwrap(), None);
        let resolved = resolve_editor_plugin_selection(Some("focused"), &packs, &BTreeMap::new()).unwrap().unwrap();
        assert_eq!(resolved.enabled, ["z"]);
        assert_eq!(resolved.disabled, ["a", "b"]);
    }

    #[test]
    fn explicit_empty_override_replaces_pack_list() {
        let packs = vec![pack("focused", Some(EditorPluginSelection {
            mode: EditorPluginMode::Only,
            enabled: vec!["dev.example.a".into()],
            disabled: vec!["dev.example.b".into()],
        }))];
        let overrides = BTreeMap::from([("focused".into(), EditorPluginOverride {
            enabled: Some(Vec::new()),
            disabled: Some(Vec::new()),
            mode: None,
        })]);
        let resolved = resolve_editor_plugin_selection(Some("focused"), &packs, &overrides).unwrap().unwrap();
        assert_eq!(resolved.mode, EditorPluginMode::Only);
        assert!(resolved.enabled.is_empty());
        assert!(resolved.disabled.is_empty());
    }

    #[test]
    fn resolver_rejects_first_sorted_conflict() {
        let packs = vec![pack("bad", Some(EditorPluginSelection {
            mode: EditorPluginMode::Overlay,
            enabled: vec!["z".into(), "a".into()],
            disabled: vec!["z".into(), "a".into()],
        }))];
        let error = resolve_editor_plugin_selection(Some("bad"), &packs, &BTreeMap::new()).unwrap_err();
        assert_eq!(error.plugin_id, "a");
    }

    #[test]
    fn appearance_transition_captures_null_font_and_uses_baseline_for_omitted_slots() {
        let mut a = pack("a", None);
        a.theme = Some("alice".into());
        let mut b = pack("b", None);
        b.font_family = Some("Rabbit Mono".into());
        let packs = vec![a, b];

        let first = resolve_appearance_transition(None, None, "global", None, Some("a"), &packs).unwrap();
        assert_eq!(first.theme, "alice");
        assert_eq!(first.font_family, None);
        assert_eq!(first.mashup_baseline, Some(MashupBaseline { theme: "global".into(), font_family: None }));

        let switched = resolve_appearance_transition(
            first.mashup_pack.as_deref(),
            first.mashup_baseline.as_ref(),
            &first.theme,
            first.font_family.as_deref(),
            Some("b"),
            &packs,
        ).unwrap();
        assert_eq!(switched.theme, "global");
        assert_eq!(switched.font_family.as_deref(), Some("Rabbit Mono"));
        assert_eq!(switched.mashup_baseline, first.mashup_baseline);

        let stale = MashupBaseline { theme: "stale".into(), font_family: Some("stale-font".into()) };
        let recaptured = resolve_appearance_transition(None, Some(&stale), "current", None, Some("a"), &packs).unwrap();
        assert_eq!(recaptured.mashup_baseline, Some(MashupBaseline { theme: "current".into(), font_family: None }));
    }

    #[test]
    fn appearance_transition_deactivation_restores_and_clears() {
        let baseline = MashupBaseline { theme: "global".into(), font_family: None };
        let restored = resolve_appearance_transition(Some("a"), Some(&baseline), "alice", Some("Pack Font"), None, &[]).unwrap();
        assert_eq!(restored.mashup_pack, None);
        assert_eq!(restored.mashup_baseline, None);
        assert_eq!(restored.theme, "global");
        assert_eq!(restored.font_family, None);

        let idle = resolve_appearance_transition(None, Some(&baseline), "manual", None, None, &[]).unwrap();
        assert_eq!(idle.theme, "manual");
        assert_eq!(idle.mashup_baseline, None);
    }

    #[test]
    fn legacy_active_pack_synthesizes_current_effective_state_on_transition() {
        let mut next = pack("next", None);
        next.font_family = Some("Rabbit Mono".into());
        let transition = resolve_appearance_transition(
            Some("legacy"), None, "legacy-effective", None, Some("next"), &[next],
        ).unwrap();
        assert_eq!(transition.theme, "legacy-effective");
        assert_eq!(transition.mashup_baseline, Some(MashupBaseline {
            theme: "legacy-effective".into(), font_family: None,
        }));
    }

    #[test]
    fn appearance_transition_rejects_unknown_requested_pack() {
        assert_eq!(
            resolve_appearance_transition(None, None, "global", None, Some("missing"), &[]).unwrap_err().pack_id,
            "missing"
        );
    }
}
