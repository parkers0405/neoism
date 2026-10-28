//! Pure catalog used by the backend and native hosts, with caller-owned config/pack loading.
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use sugarloaf::ImageProperties;
#[derive(Clone, Debug)]
pub struct PackWallpaper {
    pub id: String,
    pub name: String,
    pub description: String,
    pub wallpaper: Option<ImageProperties>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct BackgroundEntry {
    pub name: String,
    pub description: String,
    pub image: ImageProperties,
}

pub(super) fn normalize_path(path: &str, base: &Path) -> PathBuf {
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

pub fn normalize_image(image: &ImageProperties, base: &Path) -> ImageProperties {
    ImageProperties {
        path: normalize_path(&image.path, base)
            .to_string_lossy()
            .into_owned(),
        ..image.clone()
    }
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

pub fn entries_at(
    configured: Option<&ImageProperties>,
    base: &Path,
    packs: &[PackWallpaper],
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
                    ..configured.cloned().unwrap_or_default()
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
    if let Some(image) = configured.filter(|image| !image.path.trim().is_empty()) {
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
