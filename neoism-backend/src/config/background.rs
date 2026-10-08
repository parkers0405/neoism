//! Background selection, catalog discovery and durable JSONC persistence.
use super::Config;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use sugarloaf::ImageProperties;

/// Pack assets have already been resolved by the backend pack loader.
pub type ResolvedMashUpPack = super::mashup::MashupPack;

#[derive(Clone, Debug, PartialEq)]
pub enum BackgroundSelection {
    Inherit,
    None,
    Image(ImageProperties),
}

#[derive(Clone, Debug, PartialEq)]
pub struct BackgroundEntry {
    pub name: String,
    pub description: String,
    pub image: ImageProperties,
}

fn normalize_path(path: &str, base: &Path) -> PathBuf {
    let expanded = if path == "~" || path.starts_with("~/") || path.starts_with("~\\") {
        dirs::home_dir()
            .map(|home| home.join(path.get(2..).unwrap_or("")))
            .unwrap_or_else(|| base.join(path))
    } else {
        PathBuf::from(path)
    };
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        base.join(expanded)
    };
    // Canonicalization deduplicates aliases/symlinks when available, but a
    // temporarily missing explicit image must remain selectable and usable.
    std::fs::canonicalize(&absolute).unwrap_or_else(|_| {
        let mut normalized = PathBuf::new();
        for component in absolute.components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    normalized.pop();
                }
                other => normalized.push(other.as_os_str()),
            }
        }
        normalized
    })
}

fn normalize_image(image: &ImageProperties, base: &Path) -> ImageProperties {
    ImageProperties {
        path: normalize_path(&image.path, base)
            .to_string_lossy()
            .into_owned(),
        opacity: image.opacity,
    }
}

/// Disabled wins over an explicit image, which wins over the supplied pack.
/// This never loads the active pack again.
pub fn resolve_background(
    config: &Config,
    pack: Option<&ResolvedMashUpPack>,
) -> Option<ImageProperties> {
    if config.ui.window.background_image_disabled {
        return None;
    }
    config
        .ui
        .window
        .background_image
        .as_ref()
        .filter(|image| !image.path.trim().is_empty())
        .or_else(|| pack.and_then(|pack| pack.wallpaper.as_ref()))
        .map(|image| normalize_image(image, &super::config_dir_path()))
}

/// Supported formats match Sugarloaf's image decoder features.
fn supported(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "png"
                    | "jpg"
                    | "jpeg"
                    | "gif"
                    | "webp"
                    | "bmp"
                    | "ico"
                    | "pnm"
                    | "pbm"
                    | "pgm"
                    | "ppm"
                    | "pam"
            )
        })
}

fn entries_at(
    config: &Config,
    base: &Path,
    packs: &[ResolvedMashUpPack],
) -> Vec<BackgroundEntry> {
    let mut entries = BTreeMap::new();
    let mut insert = |name: String, description: String, image: ImageProperties| {
        let image = normalize_image(&image, base);
        entries.insert(
            image.path.clone(),
            BackgroundEntry {
                name,
                description,
                image,
            },
        );
    };
    if let Ok(files) = std::fs::read_dir(base.join("bg")) {
        let mut files: Vec<_> = files
            .filter_map(Result::ok)
            .map(|file| file.path())
            .filter(|path| path.is_file() && supported(path))
            .collect();
        files.sort();
        for path in files {
            insert(
                path.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                "Installed background".into(),
                ImageProperties {
                    path: path.to_string_lossy().into_owned(),
                    opacity: config
                        .ui
                        .window
                        .background_image
                        .as_ref()
                        .map_or(1.0, |image| image.opacity),
                },
            );
        }
    }
    // Stable id order makes shared-wallpaper duplicates deterministic. Pack
    // opacity beats the directory default; the explicit selection beats both.
    let mut packs: Vec<_> = packs.iter().collect();
    packs.sort_by(|a, b| a.id.cmp(&b.id));
    for pack in packs {
        if let Some(image) = &pack.wallpaper {
            insert(pack.name.clone(), pack.description.clone(), image.clone());
        }
    }
    if let Some(image) = config
        .ui
        .window
        .background_image
        .as_ref()
        .filter(|image| !image.path.trim().is_empty())
    {
        insert(
            Path::new(&image.path)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            "Configured background".into(),
            image.clone(),
        );
    }
    entries.into_values().collect()
}

/// Discover config-dir/bg, declared pack wallpapers, and the explicit image.
/// Missing directories or invalid pack manifests do not fail the catalog.
pub fn background_entries(config: &Config) -> Vec<BackgroundEntry> {
    entries_at(
        config,
        &super::config_dir_path(),
        &super::mashup::load_mashup_packs(),
    )
}

fn selection_updates(
    selection: &BackgroundSelection,
) -> std::io::Result<Vec<(&'static str, serde_json::Value)>> {
    let (disabled, image) = match selection {
        BackgroundSelection::Inherit => (false, serde_json::Value::Null),
        BackgroundSelection::None => (true, serde_json::Value::Null),
        BackgroundSelection::Image(image) => {
            if !image.opacity.is_finite() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "background opacity must be finite",
                ));
            }
            (
                false,
                serde_json::to_value(image).map_err(std::io::Error::other)?,
            )
        }
    };
    Ok(vec![
        ("ui.window.background-image-disabled", disabled.into()),
        ("ui.window.background-image", image),
    ])
}

/// Atomically write both selection fields, retaining unrelated JSONC comments.
pub fn write_background_selection(
    selection: &BackgroundSelection,
) -> std::io::Result<()> {
    super::write_settings(&selection_updates(selection)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 =
                std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "neoism-background-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn image(path: &str, opacity: f32) -> ImageProperties {
        ImageProperties {
            path: path.into(),
            opacity,
        }
    }
    fn pack(image: ImageProperties) -> ResolvedMashUpPack {
        ResolvedMashUpPack {
            id: "test".into(),
            name: "Test pack".into(),
            description: "Pack wallpaper".into(),
            theme: None,
            shader_overlay: None,
            filters: vec![],
            font_family: None,
            wallpaper: Some(image),
            look: Default::default(),
            ui: Default::default(),
            editor_plugins: None,
            dir: PathBuf::new(),
        }
    }

    #[test]
    fn precedence_and_explicit_path_normalization() {
        let mut config = Config::default();
        let pack = pack(image("pack.png", 0.3));
        assert_eq!(resolve_background(&config, None), None);
        assert_eq!(
            resolve_background(&config, Some(&pack)).unwrap().opacity,
            0.3
        );
        config.ui.window.background_image = Some(image("bg/../chosen.png", 0.6));
        let resolved = resolve_background(&config, Some(&pack)).unwrap();
        assert_eq!(
            resolved.path,
            super::super::config_dir_path()
                .join("chosen.png")
                .to_string_lossy()
        );
        assert_eq!(resolved.opacity, 0.6);
        config.ui.window.background_image_disabled = true;
        assert_eq!(resolve_background(&config, Some(&pack)), None);
        if let Some(home) = dirs::home_dir() {
            assert_eq!(
                normalize_path("~/chosen.png", &super::super::config_dir_path()),
                home.join("chosen.png")
            );
        }
    }

    #[test]
    fn blank_image_path_inherits_without_cataloging_a_directory() {
        let dir = TempDir::new();
        let mut config = Config::default();
        config.ui.window.background_image = Some(image("", 0.5));
        assert!(entries_at(&config, &dir.0, &[]).is_empty());
        assert_eq!(resolve_background(&config, None), None);
        let pack = pack(image("pack.png", 0.3));
        assert_eq!(
            resolve_background(&config, Some(&pack)).unwrap().opacity,
            0.3
        );
    }

    #[test]
    fn serialization_and_unrelated_lua_patch_preserve_images() {
        let mut config = Config::default();
        config.ui.window.background_image = Some(image("explicit.png", 0.42));
        let platform: super::super::platform::PlatformConfig =
            serde_json::from_value(json!({"window": {
                "background-image": {"path": "platform.png", "opacity": 0.7},
                "background-image-disabled": false
            }}))
            .unwrap();
        config.platform.linux = Some(platform.clone());
        let roundtrip: Config =
            serde_json::from_value(serde_json::to_value(&config).unwrap()).unwrap();
        assert_eq!(
            roundtrip.ui.window.background_image,
            config.ui.window.background_image
        );
        assert_eq!(
            roundtrip.platform.linux.as_ref().unwrap().window,
            platform.window
        );
        config
            .apply_json_patch(&json!({"ui": {"window": {"blur": true}}}))
            .unwrap();
        assert_eq!(
            config.ui.window.background_image,
            Some(image("explicit.png", 0.42))
        );
        assert_eq!(config.platform.linux.unwrap().window, platform.window);
    }

    #[test]
    fn platform_override_preserves_images_and_updates_disabled_independently() {
        let mut config = Config::default();
        let apply = |config: &mut Config, value| {
            config.overwrite_with_platform_config(serde_json::from_value(value).unwrap());
        };
        let base = Some(image("base.png", 1.0));
        config.ui.window.background_image = base.clone();
        apply(&mut config, json!({"window": {"blur": true}}));
        assert_eq!(config.ui.window.background_image, base);
        // Both omitted and explicit-null platform images mean no override,
        // regardless of whether the independent flag disables or re-enables.
        for disabled in [true, false] {
            for window in [
                json!({"background-image-disabled": disabled}),
                json!({"background-image-disabled": disabled, "background-image": null}),
            ] {
                apply(&mut config, json!({"window": window}));
                assert_eq!(config.ui.window.background_image_disabled, disabled);
                assert_eq!(config.ui.window.background_image, base);
                assert_eq!(resolve_background(&config, None).is_none(), disabled);
            }
        }
        apply(
            &mut config,
            json!({"window": {"background-image-disabled": true}}),
        );
        apply(
            &mut config,
            json!({"window": {"background-image": {"path": "override.png", "opacity": 0.5}}}),
        );
        assert!(!config.ui.window.background_image_disabled);
        assert_eq!(
            config.ui.window.background_image,
            Some(image("override.png", 0.5))
        );
        for disabled in [true, false] {
            apply(
                &mut config,
                json!({"window": {"background-image-disabled": disabled, "background-image": {"path": "flagged.png", "opacity": 0.7}}}),
            );
            assert_eq!(config.ui.window.background_image_disabled, disabled);
            assert_eq!(
                config.ui.window.background_image,
                Some(image("flagged.png", 0.7))
            );
            assert_eq!(resolve_background(&config, None).is_none(), disabled);
        }
    }

    #[test]
    fn two_field_writer_preserves_jsonc_and_all_three_states() {
        let source = "{\n // retained comment\n \"ui\": {\"window\": {\"blur\": true, // keep blur\n \"background-image\": {\"path\": \"old.png\", \"opacity\": 0.2}}},\n \"agent\": {\"model\": \"unchanged\"},\n}";
        for selection in [
            BackgroundSelection::Inherit,
            BackgroundSelection::None,
            BackgroundSelection::Image(image("new.png", 0.4)),
        ] {
            let output = super::super::settings_content_after_updates(
                source,
                &selection_updates(&selection).unwrap(),
            )
            .unwrap();
            assert!(output.contains("// retained comment"));
            assert!(output.contains("// keep blur"));
            let value: serde_json::Value =
                super::super::parse_config_content(Path::new("config.json"), &output)
                    .unwrap();
            assert_eq!(value["ui"]["window"]["blur"], true);
            assert_eq!(value["agent"]["model"], "unchanged");
            assert_eq!(
                value["ui"]["window"]["background-image-disabled"],
                matches!(selection, BackgroundSelection::None)
            );
            assert_eq!(
                value["ui"]["window"]["background-image"],
                match selection {
                    BackgroundSelection::Image(image) =>
                        serde_json::to_value(image).unwrap(),
                    _ => serde_json::Value::Null,
                }
            );
        }
        assert_eq!(
            selection_updates(&BackgroundSelection::Image(image("bad.png", f32::NAN)))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn discovery_missing_directory_sorted_dedup_and_opacity() {
        let dir = TempDir::new();
        let mut config = Config::default();
        assert!(entries_at(&config, &dir.0, &[]).is_empty());
        config.ui.window.background_image = Some(image("missing.png", 0.25));
        assert_eq!(entries_at(&config, &dir.0, &[]).len(), 1);
        std::fs::create_dir(dir.0.join("bg")).unwrap();
        for name in ["b.PNG", "a.jpg", "ignored.txt"] {
            std::fs::write(dir.0.join("bg").join(name), b"fixture").unwrap();
        }
        let declared = pack(image(&dir.0.join("bg/b.PNG").to_string_lossy(), 0.35));
        config.ui.window.background_image = Some(image("bg/../bg/b.PNG", 0.55));
        let entries = entries_at(&config, &dir.0, &[declared]);
        assert_eq!(entries.len(), 2);
        assert!(entries[0].image.path < entries[1].image.path);
        assert_eq!(entries[0].image.opacity, 0.55);
        assert_eq!(entries[1].image.opacity, 0.55);
        config.ui.window.background_image = None;
        let pack = pack(image("bg/b.PNG", 0.35));
        assert_eq!(entries_at(&config, &dir.0, &[pack])[1].image.opacity, 0.35);
    }
}
