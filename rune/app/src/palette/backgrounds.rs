//! The Backgrounds picker's catalog (`menu-actions/backgrounds.ts` +
//! `neoism-backend/src/config/background/catalog.rs`): "Use pack/default", "None", then every
//! image in `<config>/bg`, each Mash Up Pack wallpaper and the configured image, ordered by path.

use std::collections::{BTreeMap, HashMap, VecDeque};
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

pub const MAX_PIXEL_SIZE: u8 = 32;
pub const MAX_DITHER: u8 = 100;
const EFFECT_SIDE: u32 = 1280;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Effects {
    pub blur: u8,
    pub pixel_size: u8,
    pub dither: u8,
}

impl Effects {
    pub fn from_image(image: &serde_json::Value) -> Self {
        let read = |key: &str, max: u8| image[key].as_f64().unwrap_or(0.0).clamp(0.0, max.into()).round() as u8;
        Self { blur: read("blur", 100), pixel_size: match read("pixel-size", MAX_PIXEL_SIZE) { 1 => 0, size => size }, dither: read("dither", MAX_DITHER) }
    }
}

pub type ImageKey = (PathBuf, Effects);
pub type Decoded = (ImageKey, Option<(u32, u32, Vec<u8>)>);

#[derive(Default)]
pub struct Cache {
    pub entries: HashMap<ImageKey, Option<rune::Image>>,
    pub pending_effects: usize,
    pub finished_effects: VecDeque<ImageKey>,
}

/// Work on a bounded texture so dragging an effect doesn't process full-resolution photos.
pub fn decode_effects(path: &Path, effects: Effects) -> Option<(u32, u32, Vec<u8>)> {
    use image::imageops::{self, FilterType};
    let image = image::open(path).ok()?;
    let longest = image.width().max(image.height()).max(1);
    let scale = (EFFECT_SIDE as f32 / longest as f32).min(1.0);
    let small = if scale < 1.0 { image.resize(EFFECT_SIDE, EFFECT_SIDE, FilterType::Triangle) } else { image };
    let mut pixels = if effects.blur > 0 { small.blur((f32::from(effects.blur) * scale).max(0.1)) } else { small }.into_rgba8();
    let (w, h) = pixels.dimensions();
    let size = u32::from(effects.pixel_size);
    if size > 1 {
        pixels = imageops::resize(&pixels, w.div_ceil(size), h.div_ceil(size), FilterType::Triangle);
    }
    if effects.dither > 0 {
        // Ordered Bayer thresholds stay fixed to the image, including when the window moves.
        const BAYER: [[u16; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
        let strength = u16::from(effects.dither);
        for (x, y, pixel) in pixels.enumerate_pixels_mut() {
            let threshold = BAYER[y as usize % 4][x as usize % 4];
            for channel in &mut pixel.0[..3] {
                let original = u16::from(*channel);
                let level = (original * 3 * 32 + (31 - 2 * threshold) * 255) / (255 * 32);
                *channel = ((original * (100 - strength) + level * 85 * strength + 50) / 100) as u8;
            }
        }
    }
    if size > 1 { pixels = imageops::resize(&pixels, w, h, FilterType::Nearest); }
    Some((w, h, pixels.into_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effects_make_blocks_and_ordered_colors_without_changing_flat_alpha() {
        let file = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        let image = image::RgbaImage::from_pixel(8, 8, image::Rgba([128u8, 128, 128, 195]));
        image.save(file.path()).unwrap();
        let (w, h, rgba) = decode_effects(file.path(), Effects { pixel_size: 2, dither: 100, ..Effects::default() }).unwrap();
        assert_eq!((w, h), (8, 8));
        for (y, row) in rgba.chunks_exact(8 * 4).enumerate() {
            for (x, pixel) in row.chunks_exact(4).enumerate() {
                let rgb = if (x / 2 + y / 2) % 2 == 0 { 170 } else { 85 };
                assert_eq!(pixel, &[rgb, rgb, rgb, 195]);
            }
        }
    }

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
