// Copyright (c) 2023-present, Raphael Amorim.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Reusable rounded-panel frame.
//!
//! Most chrome panels (file_tree, command palette, finder, diagnostics)
//! paint themselves as a `surface`-colored outer rect with a `bg`-colored
//! inner rect inset by a hairline border, with edge-aware corner radii so
//! the frame can sit flush against a window edge without rounding into
//! empty space. This widget centralizes that pattern.
//!
//! The widget intentionally does NOT clip or paint row content — callers
//! still compute their own content rect (`inner_rect`) and paint inside
//! it. Pair with [`crate::primitives::edge_row_radii`] for
//! selected-row corners that meet the frame cleanly.

use sugarloaf::Sugarloaf;

/// Which of the four outer corners get rounded. The unrounded corners
/// sit flush against whatever edge they're adjacent to (window edge,
/// neighbouring panel, etc.).
///
/// Only `Top` is exercised today (file_tree). Other variants are kept
/// for the upcoming command_palette / finder / diagnostics migrations.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameCorners {
    /// All four corners rounded — used for floating overlays.
    All,
    /// Top-left + top-right only. file_tree style — the bottom edge is
    /// flush against the status bar.
    Top,
    /// Bottom-left + bottom-right only. Mirror of `Top`.
    Bottom,
    /// Top-left + bottom-left only. Used by panels flush against the
    /// right side of the window.
    Left,
    /// Top-right + bottom-right only. Mirror of `Left`.
    Right,
    /// No rounding (square frame).
    None,
}

impl FrameCorners {
    /// Per-corner radii in the `[tl, tr, br, bl]` clockwise order
    /// Sugarloaf expects.
    fn radii(self, radius: f32) -> [f32; 4] {
        match self {
            FrameCorners::All => [radius, radius, radius, radius],
            FrameCorners::Top => [radius, radius, 0.0, 0.0],
            FrameCorners::Bottom => [0.0, 0.0, radius, radius],
            FrameCorners::Left => [radius, 0.0, 0.0, radius],
            FrameCorners::Right => [0.0, radius, radius, 0.0],
            FrameCorners::None => [0.0, 0.0, 0.0, 0.0],
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FrameConfig {
    /// Outer ring color — usually `theme.surface`.
    pub outer_color: [f32; 4],
    /// Inner fill color — usually `theme.bg`.
    pub inner_color: [f32; 4],
    /// Outer corner radius (logical px). Inner radius is derived by
    /// subtracting `border_thickness` so the ring stays uniform.
    pub radius: f32,
    /// Border thickness (logical px). The inner rect is inset by this
    /// amount on all four sides.
    pub border_thickness: f32,
    /// Which outer corners to round.
    pub rounded_corners: FrameCorners,
}

/// Inner content rect for a frame at `rect` with the given `border_thickness`.
/// Use this to lay out content (rows, text, scrollbar) inside the frame.
///
/// Returns `[x, y, w, h]`. `w` and `h` are clamped at 0.
pub fn inner_rect(rect: [f32; 4], border_thickness: f32) -> [f32; 4] {
    let [x, y, w, h] = rect;
    [
        x + border_thickness,
        y + border_thickness,
        (w - border_thickness * 2.0).max(0.0),
        (h - border_thickness * 2.0).max(0.0),
    ]
}

fn inner_rect_for_corners(
    rect: [f32; 4],
    border_thickness: f32,
    rounded_corners: FrameCorners,
) -> [f32; 4] {
    let [x, y, w, h] = rect;
    match rounded_corners {
        // Top-attached panels sit flush on the status bar: no bottom
        // inset means no visible bottom stroke, while the side strokes
        // still run all the way down to the status seam.
        FrameCorners::Top => [
            x + border_thickness,
            y + border_thickness,
            (w - border_thickness * 2.0).max(0.0),
            (h - border_thickness).max(0.0),
        ],
        FrameCorners::Bottom => [
            x + border_thickness,
            y,
            (w - border_thickness * 2.0).max(0.0),
            (h - border_thickness).max(0.0),
        ],
        FrameCorners::Left => [
            x + border_thickness,
            y + border_thickness,
            (w - border_thickness).max(0.0),
            (h - border_thickness * 2.0).max(0.0),
        ],
        FrameCorners::Right => [
            x,
            y + border_thickness,
            (w - border_thickness).max(0.0),
            (h - border_thickness * 2.0).max(0.0),
        ],
        FrameCorners::All | FrameCorners::None => inner_rect(rect, border_thickness),
    }
}

/// Inner corner radius derived from outer radius and border thickness.
/// Use this when computing per-row radii via `edge_row_radii` so selected
/// rows meet the frame cleanly.
pub fn inner_radius(outer_radius: f32, border_thickness: f32) -> f32 {
    (outer_radius - border_thickness).max(0.0)
}

/// Paint only the ring, never a backing across the translucent interior.
/// Paired contours form disjoint triangles, including
/// the rounded corners; alpha is not multiplied at overlapping strip joins.
pub(crate) fn draw_background_border(
    sugarloaf: &mut Sugarloaf,
    outer: [f32; 4],
    inner: [f32; 4],
    outer_radii: [f32; 4],
    inner_radii: [f32; 4],
    color: [f32; 4],
    depth: f32,
    order: u8,
) {
    if outer[2] <= 0.0 || outer[3] <= 0.0 || inner[2] <= 0.0 || inner[3] <= 0.0 {
        return;
    }
    let outer = background_border_contour(outer, outer_radii);
    let inner = background_border_contour(inner, inner_radii);
    for i in 0..outer.len() {
        let next = (i + 1) % outer.len();
        let (a, b, c, d) = (outer[i], outer[next], inner[next], inner[i]);
        sugarloaf.triangle_ordered(a.0, a.1, b.0, b.1, c.0, c.1, depth, color, order);
        sugarloaf.triangle_ordered(a.0, a.1, c.0, c.1, d.0, d.1, depth, color, order);
    }
}

fn background_border_contour(rect: [f32; 4], radii: [f32; 4]) -> [(f32, f32); 68] {
    let [x, y, w, h] = rect;
    let mut points = [(0.0, 0.0); 68];
    for (corner, radius) in radii.into_iter().enumerate() {
        let r = radius.max(0.0).min(w * 0.5).min(h * 0.5);
        let (cx, cy) = match corner {
            0 => (x + r, y + r),
            1 => (x + w - r, y + r),
            2 => (x + w - r, y + h - r),
            _ => (x + r, y + h - r),
        };
        for step in 0..=16 {
            let angle = (std::f32::consts::PI
                + (corner as f32 + step as f32 / 16.0) * std::f32::consts::FRAC_PI_2)
                .rem_euclid(std::f32::consts::TAU);
            points[corner * 17 + step] = (cx + r * angle.cos(), cy + r * angle.sin());
        }
    }
    points
}

#[cfg(test)]
mod background_border_tests {
    use super::{background_border_contour, inner_rect_for_corners, FrameCorners};

    #[test]
    fn border_geometry_respects_each_frame_attachment() {
        let outer = [10.0, 20.0, 100.0, 80.0];
        for (corners, expected, radii) in [
            (FrameCorners::All, [12.0, 22.0, 96.0, 76.0], [16.0; 4]),
            (
                FrameCorners::Top,
                [12.0, 22.0, 96.0, 78.0],
                [16.0, 16.0, 0.0, 0.0],
            ),
            (
                FrameCorners::Bottom,
                [12.0, 20.0, 96.0, 78.0],
                [0.0, 0.0, 16.0, 16.0],
            ),
            (
                FrameCorners::Left,
                [12.0, 22.0, 98.0, 76.0],
                [16.0, 0.0, 0.0, 16.0],
            ),
            (
                FrameCorners::Right,
                [10.0, 22.0, 98.0, 76.0],
                [0.0, 16.0, 16.0, 0.0],
            ),
            (FrameCorners::None, [12.0, 22.0, 96.0, 76.0], [0.0; 4]),
        ] {
            assert_eq!(inner_rect_for_corners(outer, 2.0, corners), expected);
            assert_eq!(corners.radii(16.0), radii);
        }
    }

    #[test]
    fn transparent_ring_never_covers_the_content_center() {
        let outer = background_border_contour([0.0, 0.0, 100.0, 80.0], [18.0; 4]);
        let inner = background_border_contour([2.0, 2.0, 96.0, 76.0], [16.0; 4]);
        let cross = |a: (f32, f32), b: (f32, f32), p: (f32, f32)| {
            (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
        };
        for i in 0..outer.len() {
            let next = (i + 1) % outer.len();
            for (a, b, c) in [
                (outer[i], outer[next], inner[next]),
                (outer[i], inner[next], inner[i]),
            ] {
                let edges = [
                    cross(a, b, (50.0, 40.0)),
                    cross(b, c, (50.0, 40.0)),
                    cross(c, a, (50.0, 40.0)),
                ];
                assert!(edges.iter().any(|e| *e < 0.0) && edges.iter().any(|e| *e > 0.0));
            }
        }
    }

    #[test]
    fn top_attached_ring_keeps_square_bottom_corners_and_flush_seam() {
        let points =
            background_border_contour([2.0, 2.0, 96.0, 78.0], [16.0, 16.0, 0.0, 0.0]);
        assert_eq!(points[34], (98.0, 80.0));
        assert_eq!(points[51], (2.0, 80.0));
        assert!(points
            .iter()
            .all(|(x, y)| *x >= 2.0 && *x <= 98.0 && *y >= 2.0 && *y <= 80.0));
    }
}

/// Paint a rounded panel frame: outer ring + inner fill.
///
/// `rect` is `[x, y, w, h]` in logical pixels. `order_outer` should be
/// strictly less than `order_inner` so the inner fill paints on top of
/// the ring.
pub fn draw_frame(
    sugarloaf: &mut Sugarloaf,
    rect: [f32; 4],
    config: &FrameConfig,
    depth: f32,
    order_outer: u8,
    order_inner: u8,
) {
    let [x, y, w, h] = rect;
    let outer_radii = config.rounded_corners.radii(config.radius);
    if config.inner_color[3] < 1.0 {
        let inner =
            inner_rect_for_corners(rect, config.border_thickness, config.rounded_corners);
        let inner_radii = config.rounded_corners.radii(inner_radius(
            config.radius,
            config.border_thickness,
        ));
        draw_background_border(
            sugarloaf,
            rect,
            inner,
            outer_radii,
            inner_radii,
            config.outer_color,
            depth,
            order_outer,
        );
        sugarloaf.quad(
            None,
            inner[0],
            inner[1],
            inner[2],
            inner[3],
            config.inner_color,
            inner_radii,
            depth,
            order_inner,
        );
        return;
    }

    sugarloaf.quad(
        None,
        x,
        y,
        w,
        h,
        config.outer_color,
        outer_radii,
        depth,
        order_outer,
    );

    let inner =
        inner_rect_for_corners(rect, config.border_thickness, config.rounded_corners);
    let inner_r = inner_radius(config.radius, config.border_thickness);
    let inner_radii = config.rounded_corners.radii(inner_r);
    sugarloaf.quad(
        None,
        inner[0],
        inner[1],
        inner[2],
        inner[3],
        config.inner_color,
        inner_radii,
        depth,
        order_inner,
    );
}
