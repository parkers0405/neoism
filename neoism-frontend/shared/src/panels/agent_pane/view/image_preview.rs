use std::hash::{Hash, Hasher};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use sugarloaf::{ColorType, GraphicData, GraphicDataEntry, GraphicId, Sugarloaf};
use web_time::Instant;

use crate::panels::agent_pane::state::NeoismAgentImage;

use super::draw::{draw_rounded_rect_clipped, push_image_overlay_clipped};
use super::{ORDER_PANEL, OVERLAY_PANEL_ID};
use crate::primitives::ide_theme::IdeTheme;

const THUMB_SIZE: f32 = 64.0;

fn image_id(image: &NeoismAgentImage) -> u32 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    image.url.hash(&mut hasher);
    0xA100_0000 | (hasher.finish() as u32 & 0x00ff_ffff)
}

fn image_bytes(url: &str) -> Option<Vec<u8>> {
    let data = url.strip_prefix("data:")?;
    let (_, payload) = data.split_once(',')?;
    STANDARD.decode(payload).ok()
}

fn register_image(sugarloaf: &mut Sugarloaf, image: &NeoismAgentImage) -> Option<(u32, f32)> {
    let id = image_id(image);
    if let Some(entry) = sugarloaf.image_data.get(&id) {
        return Some((id, entry.width / entry.height.max(1.0)));
    }
    let decoded = image_rs::load_from_memory(&image_bytes(&image.url)?).ok()?.to_rgba8();
    let (width, height) = decoded.dimensions();
    sugarloaf.image_data.insert(
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

#[allow(clippy::too_many_arguments)]
pub fn render_image_strip(
    sugarloaf: &mut Sugarloaf,
    images: &[NeoismAgentImage],
    x: f32,
    y: f32,
    max_w: f32,
    theme: &IdeTheme,
    s: f32,
    clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) {
    let size = THUMB_SIZE * s;
    let gap = 10.0 * s;
    let mut thumb_x = x;
    for image in images {
        if thumb_x + size > x + max_w + 0.5 {
            break;
        }
        draw_rounded_rect_clipped(
            sugarloaf,
            [thumb_x - 2.0 * s, y - 2.0 * s, size + 4.0 * s, size + 4.0 * s],
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
                push_image_overlay_clipped(
                    sugarloaf,
                    OVERLAY_PANEL_ID,
                    id,
                    visible,
                    source,
                    8,
                    sugarloaf.scale_factor(),
                    occlusion_rects,
                );
            }
        }
        thumb_x += size + gap;
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