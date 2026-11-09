//! The Backgrounds picker's catalog (`menu-actions/backgrounds.ts` +
//! `neoism-backend/src/config/background/catalog.rs`): "Use pack/default", "None", then every
//! image in `<config>/bg`, each Mash Up Pack wallpaper and the configured image, ordered by path.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    /// Follow the active pack (or nothing).
    Inherit,
    /// No background image.
    None,
    Image(PathBuf),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Background {
    pub choice: Choice,
    pub name: String,
    pub description: String,
}

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp"];

fn supported(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

fn expand(path: &str, base: &Path) -> PathBuf {
    let path = match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().map(|home| home.join(rest)).unwrap_or_else(|| base.join(path)),
        None => PathBuf::from(path),
    };
    let path = if path.is_absolute() { path } else { base.join(path) };
    std::fs::canonicalize(&path).unwrap_or(path)
}

/// A pack's `pack.wallpaper` (JSONC `pack.json`; `//` comments allowed).
fn pack_wallpaper(dir: &Path) -> Option<(String, String, PathBuf)> {
    let text = std::fs::read_to_string(dir.join("pack.json")).or_else(|_| std::fs::read_to_string(dir.join("pack.jsonc"))).ok()?;
    let value: serde_json::Value = serde_json::from_str(&crate::config::strip_comments(&text)).ok()?;
    let pack = value.get("pack").unwrap_or(&value);
    let wallpaper = pack["wallpaper"].as_str().filter(|w| !w.trim().is_empty())?;
    let name = pack["name"].as_str().map(str::to_owned).unwrap_or_else(|| dir.file_name().unwrap_or_default().to_string_lossy().into_owned());
    Some((name, pack["description"].as_str().unwrap_or_default().to_owned(), expand(wallpaper, dir)))
}

/// Every background under `config_dir`, plus the configured image (`configured`).
pub fn catalog(config_dir: &Path, configured: Option<&str>) -> Vec<Background> {
    let mut images: BTreeMap<PathBuf, Background> = BTreeMap::new();
    let mut insert = |path: PathBuf, name: String, description: &str| {
        images.insert(path.clone(), Background { choice: Choice::Image(path), name, description: description.to_owned() });
    };
    if let Ok(files) = std::fs::read_dir(config_dir.join("bg")) {
        for path in files.filter_map(Result::ok).map(|f| f.path()).filter(|p| p.is_file() && supported(p)) {
            let path = std::fs::canonicalize(&path).unwrap_or(path);
            let name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            insert(path, name, "Installed background");
        }
    }
    if let Ok(packs) = std::fs::read_dir(config_dir.join("packs")) {
        let mut dirs: Vec<PathBuf> = packs.filter_map(Result::ok).map(|d| d.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        for dir in dirs {
            if let Some((name, description, path)) = pack_wallpaper(&dir) { insert(path, name, &description); }
        }
    }
    if let Some(path) = configured.filter(|p| !p.trim().is_empty()) {
        let path = expand(path, config_dir);
        let name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        insert(path, name, "Configured background");
    }
    let mut entries = vec![
        Background { choice: Choice::Inherit, name: "Use pack/default".into(), description: "Use the active Mash Up Pack's background".into() },
        Background { choice: Choice::None, name: "None".into(), description: "Disable the background image without changing the active pack".into() },
    ];
    entries.extend(images.into_values());
    entries
}

/// Decode an image file to RGBA8 (Rune's own loader reads PNG only).
pub fn decode(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let image = image::open(path).ok()?.into_rgba8();
    Some((image.width(), image.height(), image.into_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_rows_then_images_by_path() {
        let dir = std::env::temp_dir().join(format!("neoism-bg-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("bg")).unwrap();
        std::fs::write(dir.join("bg/b.webp"), b"x").unwrap();
        std::fs::write(dir.join("bg/a.png"), b"x").unwrap();
        std::fs::write(dir.join("bg/notes.txt"), b"x").unwrap();
        let names: Vec<String> = catalog(&dir, None).into_iter().map(|b| b.name).collect();
        assert_eq!(names, ["Use pack/default", "None", "a", "b"]);
        std::fs::remove_dir_all(dir).ok();
    }
}
