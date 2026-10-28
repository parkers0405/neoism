//! Shared paint and hit geometry for the floating composer and its inset tray.

use super::INPUT_HELP_STRIP_H;

pub(super) const COMPOSER_CORNER_RADIUS: f32 = 20.0;
pub(super) const COMPOSER_TRAY_RADIUS: f32 = 16.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ComposerGeometry {
    pub card: [f32; 4],
    pub tray: Option<[f32; 4]>,
    pub card_radius: f32,
    pub tray_radius: f32,
    device_scale: f32,
}

impl ComposerGeometry {
    pub fn new(
        [x, y, w, h]: [f32; 4],
        scale: f32,
        device_scale: f32,
        footer_visible: bool,
    ) -> Self {
        let device_scale = if device_scale.is_finite() && device_scale > 0.0 {
            device_scale
        } else {
            1.0
        };
        let w = w.max(0.0);
        let h = h.max(0.0);
        // Both materials own exactly one side of this physical pixel edge.
        // Only paint is snapped; text capacity and scroll metrics stay logical.
        let seam = if h > 0.0 {
            ((y + h) * device_scale).round() / device_scale
        } else {
            y
        };
        let card = [x, y, w, (seam - y).max(0.0)];
        let inset = (24.0 * scale).min(w * 0.5).max(0.0);
        let tray = (footer_visible && w > 0.0 && h > 0.0).then(|| {
            [
                x + inset,
                seam,
                (w - 2.0 * inset).max(0.0),
                (y + h + INPUT_HELP_STRIP_H * scale - seam).max(0.0),
            ]
        });
        Self {
            card,
            tray,
            card_radius: (COMPOSER_CORNER_RADIUS * scale)
                .min(w * 0.5)
                .min(card[3] * 0.5)
                .max(0.0),
            tray_radius: tray.map_or(0.0, |rect| {
                (COMPOSER_TRAY_RADIUS * scale).min(rect[2] * 0.5).min(rect[3]).max(0.0)
            }),
            device_scale,
        }
    }

    /// Replace the card's inset bottom stroke with tray material. Starting at
    /// a physical edge above the entire stroke also avoids an antialiased gap.
    pub fn tray_join_rect(&self, border_width: f32) -> Option<[f32; 4]> {
        let tray = self.tray.filter(|rect| rect[2] > 0.0 && rect[3] > 0.0)?;
        let top = ((tray[1] - border_width.max(0.0)) * self.device_scale).floor()
            / self.device_scale;
        let top = top.max(self.card[1]);
        let height = (tray[1] - top).max(0.0);
        (height > 0.0).then_some([tray[0], top, tray[2], height])
    }

    /// Disjoint horizontal bands approximate curves at physical-pixel precision.
    /// Straight regions use a single rectangle, not a band per pixel.
    pub fn occlusion_rects(&self) -> Vec<[f32; 4]> {
        let mut rects = Vec::new();
        rounded_bands(self.card, self.card_radius, self.card_radius, self.device_scale, &mut rects);
        if let Some(tray) = self.tray {
            rounded_bands(tray, 0.0, self.tray_radius, self.device_scale, &mut rects);
        }
        rects
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        rounded_contains(self.card, self.card_radius, self.card_radius, x, y)
            || self.tray.is_some_and(|tray| rounded_contains(tray, 0.0, self.tray_radius, x, y))
    }
}

fn rounded_contains(rect: [f32; 4], top: f32, bottom: f32, px: f32, py: f32) -> bool {
    let [x, y, w, h] = rect;
    if px < x || px >= x + w || py < y || py >= y + h {
        return false;
    }
    let inset = if py < y + top {
        corner_inset(top, y + top - py)
    } else if py > y + h - bottom {
        corner_inset(bottom, py - (y + h - bottom))
    } else {
        0.0
    };
    px >= x + inset && px < x + w - inset
}

fn corner_inset(radius: f32, distance: f32) -> f32 {
    radius - (radius * radius - distance * distance).max(0.0).sqrt()
}

fn rounded_bands(
    [x, y, w, h]: [f32; 4],
    top: f32,
    bottom: f32,
    device_scale: f32,
    rects: &mut Vec<[f32; 4]>,
) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    // The large center rejects most covered primitives before corner bands.
    let middle_h = (h - top - bottom).max(0.0);
    if middle_h > 0.0 {
        rects.push([x, y + top, w, middle_h]);
    }
    // Bound pathological zoom factors without affecting normal device scales.
    for (radius, start, upper) in [(top, y, true), (bottom, y + h - bottom, false)] {
        if radius <= 0.0 {
            continue;
        }
        let count = (radius * device_scale).ceil().clamp(1.0, 256.0) as usize;
        for row in 0..count {
            let y0 = start + radius * row as f32 / count as f32;
            let y1 = start + radius * (row + 1) as f32 / count as f32;
            let midpoint = (y0 + y1) * 0.5;
            let distance = if upper { start + radius - midpoint } else { midpoint - start };
            let inset = corner_inset(radius, distance);
            rects.push([x + inset, y0, (w - 2.0 * inset).max(0.0), y1 - y0]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn in_rect([x, y, w, h]: [f32; 4], px: f32, py: f32) -> bool {
        px >= x && px < x + w && py >= y && py < y + h
    }

    #[test]
    fn main_card_and_tray_share_one_snapped_edge_without_alpha_overlap() {
        for device in [1.0, 1.25, 1.5, 2.0, 3.0] {
            for scale in [0.8, 1.0, 1.2, 1.5] {
                for y in [17.13, 42.37, 200.61] {
                    let original = [30.3, y, 500.0, 98.0 * scale];
                    let geometry = ComposerGeometry::new(original, scale, device, true);
                    let tray = geometry.tray.unwrap();
                    let seam = geometry.card[1] + geometry.card[3];
                    assert!((seam - tray[1]).abs() < 0.0001);
                    assert!((seam * device - (seam * device).round()).abs() < 0.0001);
                    assert!((tray[1] + tray[3] - (y + original[3] + INPUT_HELP_STRIP_H * scale)).abs() < 0.0001);
                    let masks = geometry.occlusion_rects();
                    for py in [seam - 0.25 / device, seam + 0.25 / device] {
                        assert_eq!(masks.iter().filter(|rect| in_rect(**rect, 200.0, py)).count(), 1);
                    }
                }
            }
        }
    }

    #[test]
    fn tray_join_replaces_the_entire_bottom_stroke_with_single_owned_material() {
        for device in [1.0, 1.25, 1.5, 2.0, 3.0] {
            for scale in [0.8, 1.0, 1.2, 1.5] {
                for y in [17.13, 42.37, 200.61] {
                    let geometry = ComposerGeometry::new(
                        [30.3, y, 500.0, 98.0 * scale], scale, device, true,
                    );
                    let tray = geometry.tray.unwrap();
                    for border in [1.0, 1.125 * scale, 2.25 * scale] {
                        let join = geometry.tray_join_rect(border).unwrap();
                        assert_eq!(join[0], tray[0]);
                        assert_eq!(join[2], tray[2]);
                        assert!(join[1] <= tray[1] - border + 0.0001);
                        assert!((join[1] * device - (join[1] * device).round()).abs() < 0.0001);
                        assert!((join[1] + join[3] - tray[1]).abs() < 0.0001);
                        for py in [
                            join[1] - 0.25 / device,
                            join[1] + 0.25 / device,
                            tray[1] - 0.25 / device,
                            tray[1] + 0.25 / device,
                        ] {
                            let in_join = in_rect(join, 200.0, py);
                            let main_material = in_rect(geometry.card, 200.0, py) && !in_join;
                            let tray_material = in_join || in_rect(tray, 200.0, py);
                            assert_eq!(usize::from(main_material) + usize::from(tray_material), 1);
                        }
                    }
                }
            }
        }
        let hidden = ComposerGeometry::new([0.0, 0.0, 400.0, 98.0], 1.0, 1.0, false);
        assert!(hidden.tray_join_rect(1.125).is_none());
        let narrow = ComposerGeometry::new([0.0, 0.0, 30.0, 98.0], 1.0, 1.0, true);
        assert!(narrow.tray_join_rect(1.125).is_none());
    }

    #[test]
    fn exposed_card_corners_and_tray_sides_are_not_masked() {
        let geometry = ComposerGeometry::new([10.0, 20.0, 400.0, 98.0], 1.0, 2.0, true);
        let tray = geometry.tray.unwrap();
        let tray_bottom = tray[1] + tray[3];
        let masks = geometry.occlusion_rects();
        for (x, y) in [(10.5, 20.5), (12.0, 130.0), (408.0, 130.0), (34.5, tray_bottom - 0.5)] {
            assert!(!geometry.contains(x, y));
            assert!(!masks.iter().any(|rect| in_rect(*rect, x, y)));
        }
        for (x, y) in [(210.0, 21.0), (12.0, 70.0), (210.0, 117.5), (210.0, 119.0), (210.0, tray_bottom - 1.0)] {
            assert!(geometry.contains(x, y));
            assert_eq!(masks.iter().filter(|rect| in_rect(**rect, x, y)).count(), 1);
        }
    }

    #[test]
    fn hidden_tray_and_empty_composer_do_not_create_phantom_masks() {
        let geometry = ComposerGeometry::new([5.0, 12.0, 300.0, 98.0], 1.0, 1.0, false);
        assert!(geometry.tray.is_none());
        assert!(!geometry.contains(100.0, 111.0));
        for rect in [[0.0, 0.0, 0.0, 100.0], [0.0, 0.0, 100.0, 0.0]] {
            assert!(ComposerGeometry::new(rect, 1.0, 1.0, true).occlusion_rects().is_empty());
        }
    }
}
