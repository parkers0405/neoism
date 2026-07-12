//! Mash Up Packs: user-droppable look bundles (IDE theme + shader +
//! fonts) plus standalone runtime IDE themes.
//!
//! Disk layout under the config dir (`~/.config/neoism`):
//!
//! ```text
//! ide-themes/<name>.toml          standalone runtime theme
//! packs/<id>/pack.toml            pack manifest
//! packs/<id>/theme.toml           optional theme the pack ships
//! packs/<id>/*.glsl               shader overlays referenced by pack.toml
//! ```
//!
//! This module only reads and resolves files — turning specs into an
//! `IdeTheme` and applying slots lives in the frontend, which owns the
//! theme registry and the render surfaces.

use super::config_dir_path;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
    pub dir: PathBuf,
}

#[derive(Deserialize)]
struct PackFile {
    pack: PackSection,
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

fn parse_theme_file(path: &Path, fallback_name: &str) -> Option<IdeThemeSpec> {
    let source = std::fs::read_to_string(path).ok()?;
    let file: IdeThemeFile = match toml::from_str(&source) {
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

/// Every runtime theme on disk: `ide-themes/*.toml` first, then each
/// pack's `theme.toml` (named after the pack dir unless the file says
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
            .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
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
        let theme_path = pack_dir.join("theme.toml");
        if !theme_path.is_file() {
            continue;
        }
        let id = pack_dir
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        if let Some(spec) = parse_theme_file(&theme_path, &id) {
            push(spec);
        }
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
        .filter(|path| path.is_dir() && path.join("pack.toml").is_file())
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

/// Every installed pack, sorted by id. A pack with a `theme.toml` but
/// no explicit `theme = "..."` key applies its bundled theme.
pub fn load_mashup_packs() -> Vec<MashupPack> {
    pack_dirs()
        .into_iter()
        .filter_map(|dir| {
            let id = dir.file_name()?.to_string_lossy().to_string();
            let source = std::fs::read_to_string(dir.join("pack.toml")).ok()?;
            let file: PackFile = match toml::from_str(&source) {
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
            let bundled_theme = dir.join("theme.toml").is_file().then(|| {
                parse_theme_file(&dir.join("theme.toml"), &id)
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
                font_family: section
                    .font_family
                    .filter(|value| !value.trim().is_empty()),
                wallpaper: section
                    .wallpaper
                    .filter(|value| !value.trim().is_empty())
                    .map(|value| sugarloaf::ImageProperties {
                        path: resolve_asset(&dir, &value),
                        opacity: section
                            .wallpaper_opacity
                            .unwrap_or(1.0)
                            .clamp(0.0, 1.0),
                    }),
                look: file.look,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "neoism-mashup-test-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn theme_file_parses_with_defaults() {
        let dir = scratch_dir("theme");
        let path = dir.join("phosphor.toml");
        std::fs::write(
            &path,
            "description = \"Green CRT\"\n[colors]\nbg = \"#030f06\"\nfg = \"#33ff66\"\n",
        )
        .unwrap();

        let spec = parse_theme_file(&path, "phosphor").unwrap();
        assert_eq!(spec.name, "phosphor");
        assert_eq!(spec.description, "Green CRT");
        assert_eq!(spec.extends, "pastel_dark");
        assert!(spec
            .colors
            .iter()
            .any(|(k, v)| k == "bg" && v == "#030f06"));

        // Broken TOML is skipped, not fatal.
        std::fs::write(&path, "[colors\nbg = ").unwrap();
        assert!(parse_theme_file(&path, "phosphor").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pack_assets_resolve_relative_to_pack_dir() {
        let dir = scratch_dir("assets");
        assert_eq!(
            resolve_asset(&dir, "crawl.glsl"),
            dir.join("crawl.glsl").to_string_lossy().to_string()
        );
        assert_eq!(resolve_asset(&dir, "builtin:ctv_round"), "builtin:ctv_round");
        assert_eq!(resolve_asset(&dir, "/abs/path.glsl"), "/abs/path.glsl");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
