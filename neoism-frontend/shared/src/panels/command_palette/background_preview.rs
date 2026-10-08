//! Neutral, bounded background thumbnails. No filesystem access on wasm.
use super::actions::PaletteBackgroundEntry;
use sugarloaf::{GraphicOverlay, Sugarloaf};

pub const PANEL_ID: usize = usize::MAX - 24;
#[cfg(not(target_arch = "wasm32"))]
const IMAGE_PREFIX: u32 = 0xBC00_0000;
#[cfg(not(target_arch = "wasm32"))]
const CAPACITY: usize = 4;

#[derive(Default)]
pub struct BackgroundPreviewCache {
    #[cfg(not(target_arch = "wasm32"))]
    entries: std::collections::VecDeque<CachedImage>,
    #[cfg(not(target_arch = "wasm32"))]
    failed: Option<(String, Option<std::time::SystemTime>, u64)>,
}

#[cfg(not(target_arch = "wasm32"))]
struct CachedImage {
    path: String,
    modified: Option<std::time::SystemTime>,
    len: u64,
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    slot: u32,
    transmit_time: web_time::Instant,
}

impl BackgroundPreviewCache {
    /// Remove retained overlays even when a host skips rendering a closed palette.
    pub fn clear(sugarloaf: &mut Sugarloaf) {
        sugarloaf.clear_image_overlays_for(PANEL_ID);
    }

    #[cfg(target_arch = "wasm32")]
    fn register(
        &mut self,
        _sugarloaf: &mut Sugarloaf,
        _entry: &PaletteBackgroundEntry,
    ) -> Option<(u32, f32)> {
        None
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn register(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        entry: &PaletteBackgroundEntry,
    ) -> Option<(u32, f32)> {
        use sugarloaf::{ColorType, GraphicData, GraphicDataEntry, GraphicId};
        let path = entry.path.as_ref()?;
        let metadata = std::fs::metadata(path).ok()?;
        // Bound input and decoder allocations as well as retained thumbnail bytes.
        if metadata.len() > 32 * 1024 * 1024 {
            return None;
        }
        let modified = metadata.modified().ok();
        let cached = self.entries.iter().position(|cached| {
            cached.path == *path
                && cached.modified == modified
                && cached.len == metadata.len()
        });
        let image = if let Some(index) = cached {
            self.entries.remove(index)?
        } else {
            let identity = (path.clone(), modified, metadata.len());
            if self.failed.as_ref() == Some(&identity) {
                return None;
            }
            let Some(decoded) = decode_thumbnail(path) else {
                // Retain one failed identity so corrupt/unsupported images do not
                // decode repeatedly on every caret/animation frame. A file edit retries.
                self.failed = Some(identity);
                return None;
            };
            self.failed = None;
            let (width, height) = decoded.dimensions();
            // Show the artwork itself, independently of the applied wallpaper opacity.
            let pixels = decoded.into_raw();
            let slot = if self.entries.len() == CAPACITY {
                self.entries.pop_front()?.slot
            } else {
                (0..CAPACITY as u32)
                    .find(|slot| !self.entries.iter().any(|image| image.slot == *slot))?
            };
            CachedImage {
                path: path.clone(),
                modified,
                len: metadata.len(),
                pixels,
                width,
                height,
                slot,
                transmit_time: web_time::Instant::now(),
            }
        };
        let id = IMAGE_PREFIX | image.slot;
        // Slot reuse must replace the GPU texture (transmit_time is its revision).
        if sugarloaf
            .image_data
            .get(&id)
            .is_none_or(|entry| entry.transmit_time != image.transmit_time)
        {
            sugarloaf.image_data.insert(
                id,
                GraphicDataEntry::from_graphic_data(GraphicData {
                    id: GraphicId::new(id as u64),
                    width: image.width as usize,
                    height: image.height as usize,
                    color_type: ColorType::Rgba,
                    pixels: image.pixels.clone(),
                    is_opaque: false,
                    resize: None,
                    display_width: None,
                    display_height: None,
                    transmit_time: image.transmit_time,
                }),
            );
        }
        let aspect = image.width as f32 / image.height.max(1) as f32;
        self.entries.push_back(image);
        Some((id, aspect))
    }

    pub fn draw(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        entry: &PaletteBackgroundEntry,
        rect: [f32; 4],
        scale: f32,
    ) -> bool {
        let [x, y, w, h] = rect;
        if w <= 0.0 || h <= 0.0 {
            return false;
        }
        let Some((id, aspect)) = self.register(sugarloaf, entry) else {
            return false;
        };
        let width = w.min(h * aspect);
        let height = width / aspect;
        sugarloaf.push_image_overlay(
            PANEL_ID,
            GraphicOverlay {
                image_id: id,
                x: (x + (w - width) * 0.5) * scale,
                y: (y + (h - height) * 0.5) * scale,
                width: width * scale,
                height: height * scale,
                // Ordinary AboveText images render BEFORE the modal's opaque
                // late-overlay card. Draw after that material, before its labels.
                // The destination excludes the title/description and list bands.
                z_index: GraphicOverlay::LATE_OVERLAY_Z_INDEX,
                source_rect: GraphicOverlay::FULL_SOURCE_RECT,
            },
        );
        true
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn decode_thumbnail(path: &str) -> Option<image_rs::RgbaImage> {
    let mut reader = image_rs::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    let mut limits = image_rs::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    Some(reader.decode().ok()?.thumbnail(640, 360).to_rgba8())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    #[test]
    fn thumbnail_is_bounded_and_preserves_aspect() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wide.png");
        image_rs::RgbaImage::new(1800, 900).save(&path).unwrap();
        let image = decode_thumbnail(path.to_str().unwrap()).unwrap();
        assert_eq!(image.dimensions(), (640, 320));
        assert!(image.len() <= 640 * 360 * 4);
        std::fs::write(&path, b"not an image").unwrap();
        assert!(decode_thumbnail(path.to_str().unwrap()).is_none());
    }
}
