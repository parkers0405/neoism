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
use neoism_backend::sugarloaf::font::SugarloafFonts;
use neoism_ui::panels::command_palette::PaletteMashupEntry;
use neoism_ui::primitives::ide_theme::{
    parse_theme_hex, replace_custom_ide_themes, IdeTheme,
};
use neoism_ui::primitives::look::{
    intern_glyph, set_active_look, CheckboxLook, IconOverride, LookStyle, MarkdownStyle,
    ScrollbarStyle,
};

/// Re-scan `ide-themes/*.json`, pack themes, and the active Omarchy
/// palette into the shared
/// theme registry. Cheap (a handful of small files), so it runs at
/// every point where fresh files could be observed: startup, config
/// reload, picker open, pack apply.
pub fn sync_custom_ide_themes() {
    let themes = load_ide_theme_specs()
        .into_iter()
        .map(|spec| {
            let (theme, warnings) = IdeTheme::from_overrides(&spec.extends, &spec.colors);
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
                id: Some(pack.id),
                name: pack.name,
                detail,
                theme: pack.theme,
                shader_overlay: pack.shader_overlay,
                font_family: pack.font_family,
            }
        })
        .collect()
}

/// Fold a markdown look family into the font-library spec so
/// `font_id_for_family` can resolve it without changing the primary
/// terminal/code cascade.
pub fn fonts_with_markdown_family(
    mut fonts: SugarloafFonts,
    markdown_family: Option<&str>,
) -> SugarloafFonts {
    let Some(family) = markdown_family
        .map(str::trim)
        .filter(|name| !name.is_empty())
    else {
        return fonts;
    };
    if fonts
        .family
        .as_deref()
        .is_some_and(|primary| primary.eq_ignore_ascii_case(family))
    {
        return fonts;
    }
    let extras = fonts.extra_families.get_or_insert_with(Vec::new);
    if !extras
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(family))
    {
        extras.push(family.to_string());
    }
    fonts
}

/// Merge the active pack's look slots (scrollbar/markdown/icons)
/// under the user's `[look.*]` config — config wins field-by-field —
/// and publish the result to the shared `active_look` cell that draw
/// sites read. Startup and ordinary watcher reloads resolve by id; the
/// application-owned pack transaction uses `publish_resolved_look`.
pub fn publish_active_look(config_look: &LookConfig, active_pack: Option<&str>) {
    let pack = active_pack
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .and_then(find_mashup_pack);
    publish_resolved_look(config_look, pack.as_ref());
}

/// Publish look slots from an already-resolved pack so a transaction cannot
/// observe a different manifest between plugin validation and visual commit.
pub fn publish_resolved_look(
    config_look: &LookConfig,
    pack: Option<&neoism_backend::config::mashup::MashupPack>,
) {
    let pack_look = pack.map(|pack| &pack.look).cloned().unwrap_or_default();
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

/// Seed the example packs, once each. A marker file
/// (`packs/.seeded`) records which example ids have been installed,
/// so NEW examples arrive on upgrade while a deleted or edited
/// example stays the user's decision. Migration: when the marker is
/// missing but `packs/` already exists (pre-marker installs), ids
/// whose dirs are present are assumed already-seeded.
pub fn seed_example_packs() {
    let packs_dir = neoism_backend::config::mashup::packs_dir();
    if let Err(error) = migrate_lucid_blocks_manifest_at(&packs_dir) {
        tracing::warn!(target: "neoism::mashup", %error, "failed to migrate unmodified Lucid Blocks manifest");
    }
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

fn migrate_lucid_blocks_manifest_at(
    packs_dir: &std::path::Path,
) -> std::io::Result<bool> {
    let path = packs_dir.join("lucid-blocks/pack.json");
    let Ok(installed) = std::fs::read(&path) else {
        return Ok(false);
    };
    if installed.as_slice() != LUCID_BLOCKS_V1_MANIFEST {
        return Ok(false);
    }

    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&temporary);
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        std::io::Write::write_all(&mut file, LUCID_BLOCKS_PLUGIN_FILES[0].1)?;
        file.sync_all()?;
        std::fs::rename(&temporary, &path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map(|()| true)
}

/// Install Neoism's first-party Lucid Rabbit editor package once, independently
/// from pack and welcome-document seeds. Existing package directories are
/// treated as user-owned and are never modified.
pub fn seed_first_party_plugins() {
    if let Err(error) =
        seed_first_party_plugins_at(&neoism_backend::config::config_dir_path())
    {
        tracing::warn!(target: "neoism::mashup", %error, "failed to seed first-party editor plugin");
    }
}

fn seed_first_party_plugins_at(config_dir: &std::path::Path) -> std::io::Result<()> {
    const SEED_ID: &str = "io.neoism.lucid-birds@1";
    const PLUGIN_ID: &str = "io.neoism.lucid-birds";
    let plugins_dir = config_dir.join("plugins");
    std::fs::create_dir_all(&plugins_dir)?;
    let marker = plugins_dir.join(".neoism-first-party-seeds");
    let mut seeded = std::fs::read_to_string(&marker).unwrap_or_default();
    if seeded.lines().any(|line| line == SEED_ID) {
        return Ok(());
    }

    let destination = plugins_dir.join(PLUGIN_ID);
    if !destination.exists() {
        let staging =
            plugins_dir.join(format!(".{PLUGIN_ID}.{}.seed", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir(&staging)?;
        let install = (|| {
            for (name, bytes) in LUCID_RABBIT_PLUGIN_FILES {
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(staging.join(name))?;
                std::io::Write::write_all(&mut file, bytes)?;
                file.sync_all()?;
            }
            std::fs::rename(&staging, &destination)
        })();
        if install.is_err() {
            let _ = std::fs::remove_dir_all(&staging);
            install?;
        }
    }

    if !seeded.is_empty() && !seeded.ends_with('\n') {
        seeded.push('\n');
    }
    seeded.push_str(SEED_ID);
    seeded.push('\n');
    std::fs::write(marker, seeded)
}

const EXAMPLE_PACKS: &[(&str, &[(&str, &[u8])])] = &[
    ("lucid-blocks", LUCID_BLOCKS_PLUGIN_FILES),
    (
        "neon-unit-01",
        &[
            (
                "pack.json",
                include_bytes!("mashup/seed/neon-unit-01/pack.json"),
            ),
            (
                "theme.json",
                include_bytes!("mashup/seed/neon-unit-01/theme.json"),
            ),
            (
                "unit-sync.glsl",
                include_bytes!("mashup/seed/neon-unit-01/unit-sync.glsl"),
            ),
        ],
    ),
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

const LUCID_BLOCKS_PLUGIN_FILES: &[(&str, &[u8])] = &[
    (
        "pack.json",
        include_bytes!("mashup/seed/lucid-blocks/pack.json"),
    ),
    (
        "theme.json",
        include_bytes!("mashup/seed/lucid-blocks/theme.json"),
    ),
    (
        "looking-glass.glsl",
        include_bytes!("mashup/seed/lucid-blocks/looking-glass.glsl"),
    ),
];

const LUCID_BLOCKS_V1_MANIFEST: &[u8] =
    include_bytes!("mashup/seed/lucid-blocks/pack-v1.jsonc");

const LUCID_RABBIT_PLUGIN_FILES: &[(&str, &[u8])] = &[
    (
        "neoism-plugin.json",
        include_bytes!("mashup/plugin-seed/lucid-rabbit/neoism-plugin.json"),
    ),
    (
        "init.lua",
        include_bytes!("mashup/plugin-seed/lucid-rabbit/init.lua"),
    ),
];

#[cfg(test)]
mod seed_tests {
    use super::{
        migrate_lucid_blocks_manifest_at, seed_first_party_plugins_at,
        LUCID_BLOCKS_V1_MANIFEST,
    };

    #[test]
    fn lucid_blocks_migration_only_replaces_the_exact_first_manifest() {
        let root = std::env::temp_dir()
            .join(format!("neoism-lucid-pack-migrate-{}", std::process::id()));
        let pack = root.join("lucid-blocks/pack.json");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(pack.parent().unwrap()).unwrap();
        std::fs::write(&pack, LUCID_BLOCKS_V1_MANIFEST).unwrap();
        assert!(migrate_lucid_blocks_manifest_at(&root).unwrap());
        assert!(std::fs::read_to_string(&pack)
            .unwrap()
            .contains("io.neoism.lucid-birds"));

        std::fs::write(&pack, "// user edit\n{}").unwrap();
        assert!(!migrate_lucid_blocks_manifest_at(&root).unwrap());
        assert_eq!(std::fs::read_to_string(&pack).unwrap(), "// user edit\n{}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn first_party_plugin_seed_never_overwrites_existing_package() {
        let root = std::env::temp_dir()
            .join(format!("neoism-lucid-seed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let plugin = root.join("plugins/io.neoism.lucid-birds");
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::write(plugin.join("init.lua"), "-- user edit").unwrap();
        seed_first_party_plugins_at(&root).unwrap();
        assert_eq!(
            std::fs::read_to_string(plugin.join("init.lua")).unwrap(),
            "-- user edit"
        );
        assert!(
            std::fs::read_to_string(root.join("plugins/.neoism-first-party-seeds"))
                .unwrap()
                .contains("io.neoism.lucid-birds@1")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn bundled_lucid_rabbit_manifest_uses_current_exact_capabilities() {
        let manifest: neoism_lua::PluginManifest = serde_json::from_slice(
            include_bytes!("mashup/plugin-seed/lucid-rabbit/neoism-plugin.json"),
        )
        .unwrap();
        assert_eq!(manifest.id, "io.neoism.lucid-birds");
        assert_eq!(
            manifest
                .capabilities
                .iter()
                .map(neoism_lua::PluginCapability::key)
                .collect::<Vec<_>>(),
            ["config.current", "effect.emit"]
        );
    }

    #[test]
    fn freshly_seeded_lucid_rabbit_source_builds_as_a_sandboxed_candidate() {
        let root = std::env::temp_dir()
            .join(format!("neoism-lucid-source-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        seed_first_party_plugins_at(&root).unwrap();
        let plugin_root = root.join("plugins/io.neoism.lucid-birds");
        let discovered = neoism_lua::load_plugin_manifest(&plugin_root).unwrap();
        let host = std::sync::Arc::new(neoism_lua::QueuedHost::default());
        host.publish(
            "config",
            serde_json::json!({
                "appearance": { "mashup-pack": "lucid-blocks" }
            }),
        );
        let candidate = neoism_lua::PluginRuntime::build_candidate(
            &plugin_root,
            &discovered.manifest,
            neoism_lua::PluginRevision("seed-test".into()),
            host.clone(),
        )
        .unwrap();
        let mut runtime = candidate.activate();
        assert!(runtime.snapshot().panels.is_empty());
        assert_eq!(runtime.snapshot().autocmds.len(), 1);
        runtime
            .emit(
                neoism_lua::PluginEvent::new(
                    neoism_lua::PluginEventKind::AgentChanged,
                    serde_json::json!({ "composerRevision": 7, "composerLength": 23 }),
                    neoism_lua::ExecutionScope::Local,
                    Some("seed-test".into()),
                )
                .unwrap(),
            )
            .unwrap();
        let actions = host.drain_actions();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].namespace, "effect");
        assert_eq!(actions[0].action, "emit");
        assert_eq!(actions[0].arguments["kind"], "particles");
        assert_eq!(actions[0].arguments["seed"], 7);
        assert_eq!(
            actions[0].arguments["polygons"].as_array().unwrap().len(),
            7
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn lucid_rabbit_registers_nothing_outside_its_pack() {
        let root = std::env::temp_dir()
            .join(format!("neoism-lucid-inert-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        seed_first_party_plugins_at(&root).unwrap();
        let plugin_root = root.join("plugins/io.neoism.lucid-birds");
        let discovered = neoism_lua::load_plugin_manifest(&plugin_root).unwrap();
        let host = std::sync::Arc::new(neoism_lua::QueuedHost::default());
        host.publish("config", serde_json::json!({ "appearance": {} }));
        let candidate = neoism_lua::PluginRuntime::build_candidate(
            &plugin_root,
            &discovered.manifest,
            neoism_lua::PluginRevision("inert-test".into()),
            host,
        )
        .unwrap();
        assert!(candidate.snapshot().panels.is_empty());
        assert!(candidate.snapshot().autocmds.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }
}
