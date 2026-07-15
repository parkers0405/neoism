//! Desktop glue for Mash Up Packs and runtime IDE themes.
//!
//! The backend (`neoism_backend::config::mashup`) reads pack manifests
//! and theme files off disk; the shared crate owns the process-wide
//! theme registry and the picker specs. This module is the pump
//! between them: scan disk → build `IdeTheme`s → feed the registry,
//! and summarize installed packs for the picker modal.

use neoism_backend::config::mashup::{
    find_mashup_pack, load_ide_theme_specs, load_mashup_packs, LookConfig,
};
use neoism_ui::panels::command_palette::PaletteMashupEntry;
use neoism_ui::primitives::ide_theme::{
    parse_theme_hex, replace_custom_ide_themes, IdeTheme,
};
use neoism_ui::primitives::look::{
    intern_glyph, set_active_look, CheckboxLook, IconOverride, LookStyle,
    MarkdownStyle, ScrollbarStyle,
};

/// Re-scan `ide-themes/*.toml` + pack `theme.toml`s into the shared
/// theme registry. Cheap (a handful of small files), so it runs at
/// every point where fresh files could be observed: startup, config
/// reload, picker open, pack apply.
pub fn sync_custom_ide_themes() {
    let themes = load_ide_theme_specs()
        .into_iter()
        .map(|spec| {
            let (theme, warnings) =
                IdeTheme::from_overrides(&spec.extends, &spec.colors);
            for warning in warnings {
                tracing::warn!(
                    target: "neoism::mashup",
                    theme = %spec.name,
                    "{warning}"
                );
            }
            (spec.name, spec.description, theme)
        })
        .collect();
    replace_custom_ide_themes(themes);
}

/// Installed packs as picker rows; the detail line spells out which
/// slots the pack ships so the picker doubles as documentation.
pub fn mashup_palette_entries() -> Vec<PaletteMashupEntry> {
    load_mashup_packs()
        .into_iter()
        .map(|pack| {
            let mut slots = Vec::new();
            if pack.theme.is_some() {
                slots.push("theme");
            }
            if pack.shader_overlay.is_some() {
                slots.push("shader");
            }
            if pack.wallpaper.is_some() {
                slots.push("wallpaper");
            }
            if !pack.filters.is_empty() {
                slots.push("filters");
            }
            if pack.font_family.is_some() {
                slots.push("font");
            }
            let slots = if slots.is_empty() {
                "empty pack".to_string()
            } else {
                slots.join(" + ")
            };
            let detail = if pack.description.is_empty() {
                slots
            } else {
                format!("{} · {slots}", pack.description)
            };
            PaletteMashupEntry {
                id: pack.id,
                name: pack.name,
                detail,
            }
        })
        .collect()
}

/// Merge the active pack's look slots (scrollbar/markdown/icons)
/// under the user's `[look.*]` config — config wins field-by-field —
/// and publish the result to the shared `active_look` cell that draw
/// sites read. `active_pack` lets the pack-apply path publish
/// immediately instead of waiting for the config-write hot-reload.
pub fn publish_active_look(
    config_look: &LookConfig,
    active_pack: Option<&str>,
) {
    let pack_look = active_pack
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .and_then(find_mashup_pack)
        .map(|pack| pack.look)
        .unwrap_or_default();
    let merged = pack_look.merged_under(config_look);
    set_active_look(convert_look(&merged));
}

fn convert_look(look: &LookConfig) -> LookStyle {
    let color = |value: &Option<String>| -> Option<u32> {
        value.as_deref().and_then(parse_theme_hex)
    };
    let scrollbar = ScrollbarStyle {
        width: look.scrollbar.width.filter(|w| *w > 0.0),
        radius_factor: match (look.scrollbar.square, look.scrollbar.radius_factor) {
            (Some(true), _) => Some(0.0),
            (_, factor) => factor,
        },
        min_thumb: look.scrollbar.min_thumb.filter(|m| *m > 0.0),
        thumb: color(&look.scrollbar.thumb),
        thumb_drag: color(&look.scrollbar.thumb_drag),
        track: color(&look.scrollbar.track),
    };
    let markdown = MarkdownStyle {
        checkbox: look
            .markdown
            .checkbox
            .as_deref()
            .map(CheckboxLook::from_name)
            .unwrap_or_default(),
        font_family: look
            .markdown
            .font_family
            .clone()
            .filter(|family| !family.trim().is_empty()),
    };
    let wordmark_colors = look
        .wordmark
        .colors
        .iter()
        .filter_map(|value| parse_theme_hex(value))
        .collect();
    let icons = look
        .icons
        .iter()
        .map(|(key, icon)| {
            (
                key.clone(),
                IconOverride {
                    glyph: icon
                        .glyph()
                        .filter(|glyph| !glyph.is_empty())
                        .map(intern_glyph),
                    color: icon.color().and_then(parse_theme_hex).map(|c| {
                        [
                            ((c >> 16) & 0xff) as u8,
                            ((c >> 8) & 0xff) as u8,
                            (c & 0xff) as u8,
                            255,
                        ]
                    }),
                },
            )
        })
        .collect();
    LookStyle {
        scrollbar,
        markdown,
        wordmark_colors,
        icons,
    }
}

/// The nvim command that applies `name` — safe for FRESH nvim
/// instances. Builtins apply by name (the lua runtime ships their
/// palettes); custom themes must push their full palette, because a
/// by-name apply of an unknown name silently falls back to
/// pastel_dark inside `rio.theme` — that was the "editor loads black
/// on a light pack" bug. Callers must have synced the theme registry
/// (Screen::new / update_config do).
pub fn vim_theme_command(name: &str) -> String {
    let theme = IdeTheme::by_name(name);
    match theme.name {
        neoism_ui::primitives::ide_theme::IdeThemeName::Custom(_) => {
            neoism_backend::performer::nvim::vim_apply_custom_theme_command(
                theme.name.as_str(),
                &theme.lua_palette_pairs(),
            )
        }
        _ => neoism_backend::performer::nvim::vim_apply_theme_command(
            theme.name.as_str(),
        ),
    }
}

/// Seed the example packs, once each. A marker file
/// (`packs/.seeded`) records which example ids have been installed,
/// so NEW examples arrive on upgrade while a deleted or edited
/// example stays the user's decision. Migration: when the marker is
/// missing but `packs/` already exists (pre-marker installs), ids
/// whose dirs are present are assumed already-seeded.
pub fn seed_example_packs() {
    let packs_dir = neoism_backend::config::mashup::packs_dir();
    let marker_path = packs_dir.join(".seeded");
    let mut seeded: Vec<String> = std::fs::read_to_string(&marker_path)
        .map(|contents| contents.lines().map(str::to_string).collect())
        .unwrap_or_default();
    if seeded.is_empty() && packs_dir.exists() {
        seeded = EXAMPLE_PACKS
            .iter()
            .filter(|(id, _)| packs_dir.join(id).is_dir())
            .map(|(id, _)| id.to_string())
            .collect();
    }

    let mut changed = false;
    for (id, files) in EXAMPLE_PACKS {
        if seeded.iter().any(|s| s == id) {
            continue;
        }
        let dir = packs_dir.join(id);
        if let Err(err) = std::fs::create_dir_all(&dir) {
            tracing::warn!(
                target: "neoism::mashup",
                "failed to seed pack {id}: {err}"
            );
            continue;
        }
        for (file_name, contents) in *files {
            if let Err(err) = std::fs::write(dir.join(file_name), contents) {
                tracing::warn!(
                    target: "neoism::mashup",
                    "failed to seed {id}/{file_name}: {err}"
                );
            }
        }
        seeded.push(id.to_string());
        changed = true;
        tracing::info!(target: "neoism::mashup", "seeded example pack {id}");
    }
    if changed || !marker_path.exists() {
        if let Err(err) = std::fs::write(&marker_path, seeded.join("\n") + "\n") {
            tracing::warn!(
                target: "neoism::mashup",
                "failed to write pack seed marker: {err}"
            );
        }
    }
}

const EXAMPLE_PACKS: &[(&str, &[(&str, &[u8])])] = &[
    (
        "phosphor",
        &[
            (
                "pack.json",
                include_bytes!("mashup/seed/phosphor/pack.json"),
            ),
            (
                "theme.json",
                include_bytes!("mashup/seed/phosphor/theme.json"),
            ),
        ],
    ),
    (
        "retro-95",
        &[
            (
                "pack.json",
                include_bytes!("mashup/seed/retro-95/pack.json"),
            ),
            (
                "theme.json",
                include_bytes!("mashup/seed/retro-95/theme.json"),
            ),
        ],
    ),
];
