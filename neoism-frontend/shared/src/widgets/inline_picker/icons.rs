//! Presentation-only icons. Callers own loading and image registration.
use sugarloaf::text::DrawOpts;
use sugarloaf::{GraphicOverlay};

#[derive(Clone, Copy, Debug)]
pub enum InlinePickerIcon {
    Glyph(&'static str),
    /// The caller clears this panel's overlays before each render frame.
    Image {
        panel_id: usize,
        image_id: u32,
        source_rect: [f32; 4],
    },
}

/// Crop both destination and UVs so images obey the same viewport as text.
pub(crate) fn crop(
    rect: [f32; 4],
    source: [f32; 4],
    clip: [f32; 4],
) -> Option<([f32; 4], [f32; 4])> {
    let [x, y, w, h] = rect;
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let left = x.max(clip[0]);
    let top = y.max(clip[1]);
    let right = (x + w).min(clip[0] + clip[2]);
    let bottom = (y + h).min(clip[1] + clip[3]);
    if right <= left || bottom <= top {
        return None;
    }
    let [u0, v0, u1, v1] = source;
    Some((
        [left, top, right - left, bottom - top],
        [
            u0 + (left - x) / w * (u1 - u0),
            v0 + (top - y) / h * (v1 - v0),
            u0 + (right - x) / w * (u1 - u0),
            v0 + (bottom - y) / h * (v1 - v0),
        ],
    ))
}

impl InlinePickerIcon {
    pub(super) fn draw(
        self,
        sugarloaf: &mut dyn crate::primitives::paint::Paint,
        rect: [f32; 4],
        clip: [f32; 4],
        color: [u8; 4],
    ) {
        match self {
            Self::Glyph(glyph) => {
                sugarloaf.overlay_text_mut().draw(
                    rect[0],
                    rect[1],
                    glyph,
                    &DrawOpts {
                        font_size: rect[3],
                        color,
                        clip_rect: Some(clip),
                        ..DrawOpts::default()
                    },
                );
            }
            Self::Image {
                panel_id,
                image_id,
                source_rect,
            } => {
                let Some((rect, source_rect)) = crop(rect, source_rect, clip) else {
                    return;
                };
                let scale = sugarloaf.scale_factor();
                sugarloaf.push_image_overlay(
                    panel_id,
                    GraphicOverlay {
                        image_id,
                        x: rect[0] * scale,
                        y: rect[1] * scale,
                        width: rect[2] * scale,
                        height: rect[3] * scale,
                        z_index: GraphicOverlay::LATE_OVERLAY_Z_INDEX,
                        source_rect,
                    },
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cropped_atlas_icon_preserves_uvs() {
        let (rect, uv) = crop(
            [10.0, 10.0, 20.0, 20.0],
            [0.5, 0.0, 0.75, 1.0],
            [0.0, 20.0, 100.0, 100.0],
        )
        .unwrap();
        assert_eq!(rect, [10.0, 20.0, 20.0, 10.0]);
        assert_eq!(uv, [0.5, 0.5, 0.75, 1.0]);
        assert!(crop(
            [0.0, 0.0, 10.0, 10.0],
            [0.0, 0.0, 1.0, 1.0],
            [0.0, 11.0, 10.0, 10.0]
        )
        .is_none());
    }
}
