//! NeoUI's renderer boundary. Core geometry stays in logical pixels; Sugarloaf
//! performs the device-scale conversion. No GPU resources or application state
//! are owned by NeoUI.
//!
//! This initial adapter supports non-overlapping UI rows and explicit text
//! occlusion. Sugarloaf currently submits shapes and text in separate passes, so
//! arbitrary interleaved text/shape stacking is not a supported scene contract.

use neoui::{Frame, PaintCommand, Rect, TextMeasurer, TextStyle};
use sugarloaf::{text::DrawOpts, Sugarloaf};

use crate::primitives::draw_text_with_occlusion;

pub(crate) struct TextMetrics<'a, 'surface> {
    pub sugarloaf: &'a mut Sugarloaf<'surface>,
    pub base: DrawOpts,
}

impl TextMeasurer for TextMetrics<'_, '_> {
    fn cache_key(&mut self) -> Option<neoui::MeasurementKey> {
        Some(neoui::MeasurementKey([
            self.sugarloaf.text_mut().font_cache_generation(),
            self.base.font_id.map_or(0, |id| id as u64 + 1),
            u64::from(self.base.bold) | (u64::from(self.base.italic) << 1),
            0,
        ]))
    }

    fn measure(&mut self, text: &str, style: &TextStyle) -> f32 {
        self.sugarloaf.text_mut().measure(
            text,
            &DrawOpts {
                font_size: style.size,
                ..self.base
            },
        )
    }
}

pub(crate) fn rect(rect: [f32; 4]) -> Rect {
    Rect::new(rect[0], rect[1], rect[2], rect[3])
}

fn array(rect: Rect) -> [f32; 4] {
    [rect.x, rect.y, rect.width, rect.height]
}

fn text_color(color: [f32; 4]) -> [u8; 4] {
    color.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Caller owns the surrounding late-overlay scope and any higher text layers.
/// Shapes keep their original geometry: clipping never moves rounded corners.
pub(crate) fn paint<Action>(
    sugarloaf: &mut Sugarloaf,
    frame: &Frame<Action>,
    base: DrawOpts,
    shape_order: u8,
    occlusion: &[[f32; 4]],
) {
    for command in &frame.paint {
        match command {
            PaintCommand::Quad {
                bounds,
                color,
                radius,
                clip,
            } => {
                if bounds.intersection(*clip).is_empty() {
                    continue;
                }
                sugarloaf.quad_clipped(
                    array(*bounds),
                    *color,
                    [*radius; 4],
                    0.0,
                    shape_order,
                    array(*clip),
                );
            }
            PaintCommand::Text {
                bounds,
                text,
                style,
                clip,
            } => {
                if bounds.intersection(*clip).is_empty() {
                    continue;
                }
                draw_text_with_occlusion(
                    sugarloaf,
                    bounds.x,
                    bounds.y,
                    text,
                    &DrawOpts {
                        font_size: style.size,
                        color: text_color(style.color),
                        clip_rect: Some(array(*clip)),
                        ..base
                    },
                    occlusion,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_rect_conversion_does_not_apply_device_scale() {
        let input = [1.25, 8.5, 40.0, 22.0];
        assert_eq!(array(rect(input)), input);
    }

    #[test]
    fn colors_remain_straight_alpha_and_round_trip_byte_channels() {
        for channel in 0..=255u8 {
            assert_eq!(text_color([channel as f32 / 255.0; 4]), [channel; 4]);
        }
        assert_eq!(text_color([-1.0, 2.0, 0.5, 0.25]), [0, 255, 128, 64]);
    }
}
