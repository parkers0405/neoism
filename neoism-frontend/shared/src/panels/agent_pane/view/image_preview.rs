use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use sugarloaf::{ColorType, GraphicData, GraphicDataEntry, GraphicId};
use web_time::Instant;

use crate::panels::agent_pane::composer_interactions::ImagePreviewHit;
use crate::panels::agent_pane::state::NeoismAgentImage;
use crate::primitives::visible_rect_fragments;

use super::draw::{draw_rounded_rect_clipped, push_image_overlay_clipped};
use super::{ORDER_PANEL, OVERLAY_PANEL_ID};
use crate::primitives::ide_theme::IdeTheme;

const THUMB_SIZE: f32 = 64.0;

#[derive(Default)]
struct ImageIds(HashMap<[u8; 32], u32>);

impl ImageIds {
    fn get_or_allocate(&mut self, identity: [u8; 32]) -> Option<u32> {
        if let Some(id) = self.0.get(&identity) {
            return Some(*id);
        }
        // Allocate, rather than truncate a content hash. Never reuse an ID for
        // different bytes, even after a renderer has evicted its texture.
        let index = u32::try_from(self.0.len()).ok()?;
        if index > 0x00ff_ffff {
            return None;
        }
        let id = 0xA700_0000 | index;
        self.0.insert(identity, id);
        Some(id)
    }
}

fn image_id(image: &NeoismAgentImage) -> Option<u32> {
    static IDS: OnceLock<Mutex<ImageIds>> = OnceLock::new();
    image.url.renderer_id(|identity| {
        IDS.get_or_init(Default::default)
            .lock()
            .ok()?
            .get_or_allocate(identity)
    })
}

fn image_bytes(url: &str) -> Option<Vec<u8>> {
    let data = url.strip_prefix("data:")?;
    let (_, payload) = data.split_once(',')?;
    STANDARD.decode(payload).ok()
}

fn decode_image(image: &NeoismAgentImage) -> Option<image_rs::RgbaImage> {
    if image.url.decode_failed() {
        return None;
    }
    let decoded = image_bytes(&image.url)
        .and_then(|bytes| image_rs::load_from_memory(&bytes).ok())
        .map(|image| image.to_rgba8());
    if decoded.is_none() {
        image.url.mark_decode_failed();
    }
    decoded
}

/// Off-render-thread attachment decoding. A full-resolution screenshot takes
/// tens of milliseconds to base64 + PNG decode; doing that inline stalled the
/// frame in which an image first scrolled into view.
#[cfg(not(target_arch = "wasm32"))]
mod background_decode {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{channel, Sender};
    use std::sync::{Mutex, OnceLock};

    use crate::panels::agent_pane::state::NeoismAgentImage;

    enum Slot {
        Decoding,
        Ready(image_rs::RgbaImage, std::time::Instant),
    }

    /// A finished decode keeps frames coming only briefly: if its thumbnail
    /// already scrolled away, the pixels wait for the next sighting without
    /// pinning the render loop awake.
    const READY_REPAINT_WINDOW: std::time::Duration = std::time::Duration::from_millis(500);

    static SLOTS: OnceLock<Mutex<HashMap<u32, Slot>>> = OnceLock::new();
    static PENDING: AtomicUsize = AtomicUsize::new(0);

    fn slots() -> &'static Mutex<HashMap<u32, Slot>> {
        SLOTS.get_or_init(Default::default)
    }

    fn worker() -> Option<&'static Mutex<Sender<(u32, NeoismAgentImage)>>> {
        static WORKER: OnceLock<Option<Mutex<Sender<(u32, NeoismAgentImage)>>>> =
            OnceLock::new();
        WORKER
            .get_or_init(|| {
                let (sender, receiver) = channel::<(u32, NeoismAgentImage)>();
                std::thread::Builder::new()
                    .name("agent-image-decode".into())
                    .spawn(move || {
                        for (id, image) in receiver {
                            let decoded = super::decode_image(&image);
                            if let Ok(mut slots) = slots().lock() {
                                match decoded {
                                    Some(decoded) => {
                                        slots.insert(id, Slot::Ready(decoded, std::time::Instant::now()));
                                    }
                                    None => {
                                        slots.remove(&id);
                                    }
                                }
                            }
                            PENDING.fetch_sub(1, Ordering::Release);
                        }
                    })
                    .ok()?;
                Some(Mutex::new(sender))
            })
            .as_ref()
    }

    /// `Some(pixels)` once decoded; `None` while decoding or after a failure
    /// (failures set the payload's shared `decode_failed` flag).
    pub(super) fn poll(id: u32, image: &NeoismAgentImage) -> Option<image_rs::RgbaImage> {
        let mut slots = slots().lock().ok()?;
        match slots.remove(&id) {
            Some(Slot::Ready(decoded, _)) => return Some(decoded),
            Some(Slot::Decoding) => {
                slots.insert(id, Slot::Decoding);
                return None;
            }
            None => {}
        }
        let Some(worker) = worker() else {
            drop(slots);
            return super::decode_image(image);
        };
        slots.insert(id, Slot::Decoding);
        PENDING.fetch_add(1, Ordering::AcqRel);
        if worker
            .lock()
            .ok()
            .and_then(|sender| sender.send((id, image.clone())).ok())
            .is_none()
        {
            slots.remove(&id);
            PENDING.fetch_sub(1, Ordering::Release);
        }
        None
    }

    pub(super) fn pending() -> bool {
        PENDING.load(Ordering::Acquire) > 0
            || slots()
                .lock()
                .is_ok_and(|slots| {
                    slots.values().any(|slot| {
                        matches!(slot, Slot::Ready(_, at) if at.elapsed() < READY_REPAINT_WINDOW)
                    })
                })
    }
}

/// True while an attachment is decoding (or decoded but not yet uploaded):
/// the host must keep painting so the finished image appears without input.
pub fn image_decodes_pending() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        background_decode::pending()
    }
    #[cfg(target_arch = "wasm32")]
    {
        false
    }
}

/// Register the original-resolution attachment for both strips and inspection modals.
/// Cache hits inspect only the precomputed identity; no URL hashing or decoding.
/// The first sighting schedules a background decode and returns `None` (the
/// caller's placeholder frame stays visible) until the pixels are ready.
pub fn register_image(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    image: &NeoismAgentImage,
) -> Option<(u32, f32)> {
    if image.url.decode_failed() {
        return None;
    }
    let id = image_id(image)?;
    if let Some(entry) = sugarloaf.image_data_get(id) {
        return Some((id, entry.width / entry.height.max(1.0)));
    }
    #[cfg(not(target_arch = "wasm32"))]
    let decoded = background_decode::poll(id, image)?;
    #[cfg(target_arch = "wasm32")]
    let decoded = decode_image(image)?;
    let (width, height) = decoded.dimensions();
    sugarloaf.image_data_insert(
        id,
        GraphicDataEntry::from_graphic_data(GraphicData {
            id: GraphicId::new(id as u64),
            width: width as usize,
            height: height as usize,
            color_type: ColorType::Rgba,
            pixels: decoded.into_raw(),
            is_opaque: false,
            resize: None,
            display_width: None,
            display_height: None,
            transmit_time: Instant::now(),
        }),
    );
    Some((id, width as f32 / height.max(1) as f32))
}

fn image_preview_hits(
    image: &NeoismAgentImage,
    rect: [f32; 4],
    clip: [f32; 4],
    occlusions: &[[f32; 4]],
) -> Vec<ImagePreviewHit> {
    visible_rect_fragments(rect, clip, occlusions)
        .into_iter()
        .map(|rect| ImagePreviewHit {
            rect,
            image: image.clone(),
        })
        .collect()
}

fn thumbnail_is_visible(rect: [f32; 4], clip: [f32; 4], occlusions: &[[f32; 4]]) -> bool {
    !visible_rect_fragments(rect, clip, occlusions).is_empty()
}

#[allow(clippy::too_many_arguments)]
pub fn render_image_strip(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    images: &[NeoismAgentImage],
    x: f32,
    y: f32,
    max_w: f32,
    theme: &IdeTheme,
    s: f32,
    thumb_size: f32,
    clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) -> Vec<ImagePreviewHit> {
    let mut hits = Vec::new();
    let size = thumb_size * s;
    let gap = 10.0 * s;
    let mut thumb_x = x;
    for image in images {
        if thumb_x + size > x + max_w + 0.5 {
            break;
        }
        // Cull before registration: unseen attachments must not synchronously
        // decode base64 or upload the original full-resolution texture.
        if !thumbnail_is_visible([thumb_x, y, size, size], clip, occlusion_rects) {
            thumb_x += size + gap;
            continue;
        }
        draw_rounded_rect_clipped(
            sugarloaf,
            [
                thumb_x - 2.0 * s,
                y - 2.0 * s,
                size + 4.0 * s,
                size + 4.0 * s,
            ],
            theme.f32_alpha(theme.border, 0.9),
            13.0 * s,
            ORDER_PANEL + 2,
            clip,
        );
        draw_rounded_rect_clipped(
            sugarloaf,
            [thumb_x, y, size, size],
            theme.f32(theme.bg),
            11.0 * s,
            ORDER_PANEL + 3,
            clip,
        );
        if let Some((id, aspect)) = register_image(sugarloaf, image) {
            // Inspection previews must show the whole image. Fit it inside the
            // square canvas rather than cropping its source to a square.
            let fitted = if aspect >= 1.0 {
                let height = size / aspect;
                [thumb_x, y + (size - height) * 0.5, size, height]
            } else {
                let width = size * aspect;
                [thumb_x + (size - width) * 0.5, y, width, size]
            };
            if let Some((visible, source)) = clip_image(fitted, clip) {
                hits.extend(image_preview_hits(image, fitted, clip, occlusion_rects));
                let overlay_scale = sugarloaf.scale_factor();
                push_image_overlay_clipped(
                    sugarloaf,
                    OVERLAY_PANEL_ID,
                    id,
                    visible,
                    source,
                    8,
                    overlay_scale,
                    occlusion_rects,
                );
            }
        }
        thumb_x += size + gap;
    }
    hits
}

pub const COMPOSER_THUMB_SIZE: f32 = THUMB_SIZE;
pub const MESSAGE_THUMB_SIZE: f32 = 150.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unseen_thumbnails_are_culled_before_registration() {
        let canvas = [10.0, 20.0, 64.0, 64.0];
        assert!(!thumbnail_is_visible(
            canvas,
            [100.0, 0.0, 200.0, 200.0],
            &[]
        ));
        assert!(!thumbnail_is_visible(
            canvas,
            [0.0, 100.0, 200.0, 200.0],
            &[]
        ));
        let clip = [0.0, 0.0, 200.0, 200.0];
        assert!(!thumbnail_is_visible(canvas, clip, &[canvas]));
        assert!(!thumbnail_is_visible(
            canvas,
            clip,
            &[[10.0, 20.0, 32.0, 64.0], [42.0, 20.0, 32.0, 64.0],]
        ));
        assert!(thumbnail_is_visible(
            canvas,
            clip,
            &[[10.0, 20.0, 64.0, 32.0]]
        ));
    }

    #[test]
    fn partial_vertical_occlusion_preserves_clickable_lower_pixels() {
        let image = NeoismAgentImage {
            filename: "hit.png".into(),
            mime: "image/png".into(),
            url: "data:image/png;base64,AA==".into(),
        };
        let hits = image_preview_hits(
            &image,
            [0.0, 0.0, 100.0, 100.0],
            [10.0, 10.0, 80.0, 80.0],
            &[[30.0, 10.0, 40.0, 30.0]],
        );
        assert!(hits.iter().all(|hit| std::ptr::eq(
            hit.image.url.as_str().as_ptr(),
            image.url.as_str().as_ptr()
        )));
        let fragments: Vec<_> = hits.iter().map(|hit| hit.rect).collect();
        let contains = |x: f32, y: f32| {
            fragments
                .iter()
                .any(|r| x >= r[0] && x < r[0] + r[2] && y >= r[1] && y < r[1] + r[3])
        };
        assert!(
            contains(50.0, 60.0),
            "a partial-height overlay must not cut entire columns"
        );
        assert!(contains(20.0, 20.0));
        assert!(!contains(50.0, 20.0));
        assert!(!contains(5.0, 60.0));
        assert!(!contains(50.0, 95.0));
        let area: f32 = fragments.iter().map(|r| r[2] * r[3]).sum();
        assert_eq!(area, 80.0 * 80.0 - 40.0 * 30.0);
    }

    #[test]
    fn failed_decode_is_remembered_by_payload_clones() {
        for url in [
            "data:image/png;base64,???",
            "data:image/png;base64,bm90IGEgcG5n",
            "/v2/artifacts/missing/content",
        ] {
            let image = NeoismAgentImage {
                filename: "invalid.png".into(),
                mime: "image/png".into(),
                url: url.into(),
            };
            let cloned = image.clone();
            assert!(!cloned.url.decode_failed());
            assert!(decode_image(&image).is_none());
            assert!(cloned.url.decode_failed());
            assert!(decode_image(&cloned).is_none());
            // Replacing immutable bytes (e.g. artifact hydration) gets a fresh state.
            let replacement: crate::panels::agent_pane::state::AgentImagePayload =
                url.into();
            assert!(!replacement.decode_failed());
            assert_eq!(image.url.cache_identity(), replacement.cache_identity());
        }
    }

    #[test]
    fn texture_ids_do_not_truncate_content_identity() {
        let mut ids = ImageIds::default();
        let first = [0; 32];
        let mut second = first;
        second[31] = 1;
        let a = ids.get_or_allocate(first).unwrap();
        let b = ids.get_or_allocate(second).unwrap();
        assert_ne!(a, b);
        assert_eq!(ids.get_or_allocate(first), Some(a));
        assert_eq!(a & 0xff00_0000, 0xA700_0000);
    }

    #[test]
    fn attachment_decode_preserves_original_resolution_and_pixels() {
        let source =
            image_rs::RgbaImage::from_pixel(513, 257, image_rs::Rgba([12, 34, 56, 78]));
        let mut encoded = std::io::Cursor::new(Vec::new());
        source
            .write_to(&mut encoded, image_rs::ImageFormat::Png)
            .unwrap();
        let image = NeoismAgentImage {
            filename: "original.png".into(),
            mime: "image/png".into(),
            url: format!(
                "data:image/png;base64,{}",
                STANDARD.encode(encoded.into_inner())
            )
            .into(),
        };
        let decoded = decode_image(&image).unwrap();
        assert_eq!(decoded.dimensions(), (513, 257));
        assert_eq!(decoded.as_raw(), source.as_raw());
        // Even a decodable URL must not reach the decoder after a cached
        // failure; the flag is shared with clones and is checked first.
        image.url.mark_decode_failed();
        assert!(decode_image(&image.clone()).is_none());
    }
}

fn clip_image(rect: [f32; 4], clip: [f32; 4]) -> Option<([f32; 4], [f32; 4])> {
    let [x, y, w, h] = rect;
    let x1 = x.max(clip[0]);
    let y1 = y.max(clip[1]);
    let x2 = (x + w).min(clip[0] + clip[2]);
    let y2 = (y + h).min(clip[1] + clip[3]);
    if w <= 0.0 || h <= 0.0 || x2 <= x1 || y2 <= y1 {
        return None;
    }
    Some((
        [x1, y1, x2 - x1, y2 - y1],
        [(x1 - x) / w, (y1 - y) / h, (x2 - x) / w, (y2 - y) / h],
    ))
}
