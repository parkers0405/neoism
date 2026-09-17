//! World-space geometry for shapes: axis-aligned bounds, hit-testing,
//! and the translate/scale primitives selection edits build on.
//!
//! All coordinates here are *world* space. Screen-space concerns
//! (handle sizes, click tolerance in pixels) are converted by the
//! caller via the [`Camera`](super::Camera) before reaching these.

use super::scene::{Scene, Shape, ShapeKind, Vec2};

/// An axis-aligned bounding box in world space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min: Vec2,
    pub max: Vec2,
}

impl Bounds {
    pub fn new(min: Vec2, max: Vec2) -> Self {
        Self {
            min: Vec2::new(min.x.min(max.x), min.y.min(max.y)),
            max: Vec2::new(min.x.max(max.x), min.y.max(max.y)),
        }
    }

    pub fn from_points(points: &[Vec2]) -> Option<Self> {
        let first = *points.first()?;
        let mut b = Bounds {
            min: first,
            max: first,
        };
        for p in &points[1..] {
            b.min.x = b.min.x.min(p.x);
            b.min.y = b.min.y.min(p.y);
            b.max.x = b.max.x.max(p.x);
            b.max.y = b.max.y.max(p.y);
        }
        Some(b)
    }

    pub fn union(self, other: Bounds) -> Bounds {
        Bounds {
            min: Vec2::new(self.min.x.min(other.min.x), self.min.y.min(other.min.y)),
            max: Vec2::new(self.max.x.max(other.max.x), self.max.y.max(other.max.y)),
        }
    }

    pub fn width(&self) -> f32 {
        self.max.x - self.min.x
    }

    pub fn height(&self) -> f32 {
        self.max.y - self.min.y
    }

    pub fn center(&self) -> Vec2 {
        Vec2::new(
            (self.min.x + self.max.x) * 0.5,
            (self.min.y + self.max.y) * 0.5,
        )
    }

    /// `[x, y, w, h]`.
    pub fn xywh(&self) -> [f32; 4] {
        [self.min.x, self.min.y, self.width(), self.height()]
    }

    pub fn contains(&self, p: Vec2, pad: f32) -> bool {
        p.x >= self.min.x - pad
            && p.x <= self.max.x + pad
            && p.y >= self.min.y - pad
            && p.y <= self.max.y + pad
    }
}

impl ShapeKind {
    /// World-space bounds. Text shares the renderer's wrapping rules, with
    /// estimated advances until the renderer supplies measured dimensions.
    pub fn bounds(&self) -> Bounds {
        match self {
            ShapeKind::Rect { x, y, w, h, .. } | ShapeKind::Ellipse { x, y, w, h } => {
                Bounds::new(Vec2::new(*x, *y), Vec2::new(x + w, y + h))
            }
            ShapeKind::Line { points }
            | ShapeKind::Polygon { points }
            | ShapeKind::Arrow { points, .. }
            | ShapeKind::Freehand { points } => {
                Bounds::from_points(points).unwrap_or(Bounds {
                    min: Vec2::ZERO,
                    max: Vec2::ZERO,
                })
            }
            ShapeKind::Text {
                x,
                y,
                content,
                size,
                width,
            } => {
                let size = super::text_layout::font_size(*size);
                let layout =
                    super::text_layout::layout_text(content, size, *width, |text| {
                        text.chars().count() as f32 * size * TEXT_ADVANCE
                    });
                Bounds::new(
                    Vec2::new(*x, *y),
                    Vec2::new(x + layout.bounds.x, y + layout.bounds.y),
                )
            }
        }
    }

    /// Translate the geometry by `d` (world space).
    pub fn translate(&mut self, d: Vec2) {
        match self {
            ShapeKind::Rect { x, y, .. }
            | ShapeKind::Ellipse { x, y, .. }
            | ShapeKind::Text { x, y, .. } => {
                *x += d.x;
                *y += d.y;
            }
            ShapeKind::Line { points }
            | ShapeKind::Polygon { points }
            | ShapeKind::Arrow { points, .. }
            | ShapeKind::Freehand { points } => {
                for p in points {
                    p.x += d.x;
                    p.y += d.y;
                }
            }
        }
    }

    /// Scale the geometry about `anchor` by `(sx, sy)`.
    pub fn scale(&mut self, anchor: Vec2, sx: f32, sy: f32) {
        let sp = |v: f32, a: f32, s: f32| a + (v - a) * s;
        match self {
            ShapeKind::Rect { x, y, w, h, .. } | ShapeKind::Ellipse { x, y, w, h } => {
                let nx = sp(*x, anchor.x, sx);
                let ny = sp(*y, anchor.y, sy);
                *w *= sx;
                *h *= sy;
                *x = nx;
                *y = ny;
                // Keep w/h positive, shifting origin if a flip occurred.
                if *w < 0.0 {
                    *x += *w;
                    *w = -*w;
                }
                if *h < 0.0 {
                    *y += *h;
                    *h = -*h;
                }
            }
            ShapeKind::Line { points }
            | ShapeKind::Polygon { points }
            | ShapeKind::Arrow { points, .. }
            | ShapeKind::Freehand { points } => {
                for p in points {
                    p.x = sp(p.x, anchor.x, sx);
                    p.y = sp(p.y, anchor.y, sy);
                }
            }
            ShapeKind::Text {
                x,
                y,
                content,
                size,
                width,
            } => {
                let old_width = width.unwrap_or_else(|| {
                    content
                        .split('\n')
                        .map(|line| {
                            line.chars().count() as f32 * size.abs() * TEXT_ADVANCE
                        })
                        .fold(size.abs() * 0.3, f32::max)
                });
                let left = sp(*x, anchor.x, sx);
                let right = sp(*x + old_width, anchor.x, sx);
                *x = left.min(right);
                *y = sp(*y, anchor.y, sy);
                *width = Some((right - left).abs().max(16.0));
            }
        }
    }
}

impl Scene {
    /// Combined world-space bounds of every shape, or `None` if empty.
    pub fn bounds(&self) -> Option<Bounds> {
        let mut acc: Option<Bounds> = None;
        for s in &self.shapes {
            let b = s.bounds();
            acc = Some(match acc {
                Some(a) => a.union(b),
                None => b,
            });
        }
        acc
    }
}

impl Shape {
    /// Whether a world-space point hits this shape within `tol` world
    /// units. Filled shapes test their interior; stroked-only shapes
    /// test proximity to their outline.
    pub fn hit(&self, p: Vec2, tol: f32) -> bool {
        let filled = self.style.fill.is_some();
        match &self.kind {
            ShapeKind::Rect { x, y, w, h, .. } => {
                let b = Bounds::new(Vec2::new(*x, *y), Vec2::new(x + w, y + h));
                if filled {
                    b.contains(p, tol)
                } else {
                    near_rect_outline(p, b, tol)
                }
            }
            ShapeKind::Ellipse { x, y, w, h } => {
                let cx = x + w * 0.5;
                let cy = y + h * 0.5;
                let rx = (w.abs() * 0.5).max(0.001);
                let ry = (h.abs() * 0.5).max(0.001);
                let nx = (p.x - cx) / rx;
                let ny = (p.y - cy) / ry;
                let d = (nx * nx + ny * ny).sqrt();
                if filled {
                    d <= 1.0 + tol / rx.min(ry)
                } else {
                    (d - 1.0).abs() <= tol / rx.min(ry)
                }
            }
            ShapeKind::Line { points }
            | ShapeKind::Arrow { points, .. }
            | ShapeKind::Freehand { points } => near_polyline(p, points, tol),
            ShapeKind::Polygon { points } => {
                if filled && point_in_polygon(p, points) {
                    true
                } else {
                    near_closed_polyline(p, points, tol)
                }
            }
            ShapeKind::Text { .. } => self.kind.bounds().contains(p, tol),
        }
    }

    pub fn bounds(&self) -> Bounds {
        self.kind.bounds()
    }

    /// Lenient hit-test for the *select* tool: clicking anywhere inside
    /// a closed shape's box selects it (even unfilled), matching what
    /// users expect from draw apps. Open shapes still use proximity.
    pub fn hit_select(&self, p: Vec2, tol: f32) -> bool {
        match &self.kind {
            ShapeKind::Rect { .. }
            | ShapeKind::Polygon { .. }
            | ShapeKind::Ellipse { .. }
            | ShapeKind::Text { .. } => self.bounds().contains(p, tol),
            _ => self.hit(p, tol),
        }
    }
}

/// Estimated per-character advance as a fraction of font size.
const TEXT_ADVANCE: f32 = 0.6;

fn near_rect_outline(p: Vec2, b: Bounds, tol: f32) -> bool {
    let corners = [
        Vec2::new(b.min.x, b.min.y),
        Vec2::new(b.max.x, b.min.y),
        Vec2::new(b.max.x, b.max.y),
        Vec2::new(b.min.x, b.max.y),
    ];
    for i in 0..4 {
        if dist_point_segment(p, corners[i], corners[(i + 1) % 4]) <= tol {
            return true;
        }
    }
    false
}

fn near_polyline(p: Vec2, points: &[Vec2], tol: f32) -> bool {
    if points.len() == 1 {
        return distance(p, points[0]) <= tol;
    }
    points
        .windows(2)
        .any(|w| dist_point_segment(p, w[0], w[1]) <= tol)
}

fn near_closed_polyline(p: Vec2, points: &[Vec2], tol: f32) -> bool {
    if near_polyline(p, points, tol) {
        return true;
    }
    if points.len() > 2 {
        dist_point_segment(p, points[points.len() - 1], points[0]) <= tol
    } else {
        false
    }
}

fn point_in_polygon(p: Vec2, points: &[Vec2]) -> bool {
    if points.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = points.len() - 1;
    for i in 0..points.len() {
        let pi = points[i];
        let pj = points[j];
        let crosses = (pi.y > p.y) != (pj.y > p.y);
        if crosses {
            let denom = pj.y - pi.y;
            if denom.abs() <= f32::EPSILON {
                j = i;
                continue;
            }
            let x = (pj.x - pi.x) * (p.y - pi.y) / denom + pi.x;
            if p.x < x {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

fn distance(a: Vec2, b: Vec2) -> f32 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    (dx * dx + dy * dy).sqrt()
}

/// Shortest distance from point `p` to segment `a`–`b`.
pub fn dist_point_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let abx = b.x - a.x;
    let aby = b.y - a.y;
    let len_sq = abx * abx + aby * aby;
    if len_sq <= f32::EPSILON {
        return distance(p, a);
    }
    let t = (((p.x - a.x) * abx + (p.y - a.y) * aby) / len_sq).clamp(0.0, 1.0);
    let proj = Vec2::new(a.x + abx * t, a.y + aby * t);
    distance(p, proj)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::neodraw::scene::{Shape, ShapeId, Style};

    fn rect_shape(fill: bool) -> Shape {
        Shape {
            id: ShapeId(1),
            kind: ShapeKind::Rect {
                x: 10.0,
                y: 10.0,
                w: 100.0,
                h: 50.0,
                corner: 0.0,
            },
            style: Style {
                fill: fill.then_some(super::super::scene::Color::WHITE),
                ..Style::default()
            },
        }
    }

    #[test]
    fn rect_bounds_and_translate() {
        let mut k = rect_shape(false).kind;
        let b = k.bounds();
        assert_eq!(b.xywh(), [10.0, 10.0, 100.0, 50.0]);
        k.translate(Vec2::new(5.0, -5.0));
        assert_eq!(k.bounds().xywh(), [15.0, 5.0, 100.0, 50.0]);
    }

    #[test]
    fn filled_rect_hits_interior_unfilled_only_outline() {
        let filled = rect_shape(true);
        let hollow = rect_shape(false);
        let center = Vec2::new(60.0, 35.0);
        assert!(filled.hit(center, 2.0), "filled hits interior");
        assert!(!hollow.hit(center, 2.0), "hollow misses interior");
        let on_edge = Vec2::new(10.0, 35.0);
        assert!(hollow.hit(on_edge, 3.0), "hollow hits its outline");
    }

    #[test]
    fn scale_about_anchor() {
        let mut k = ShapeKind::Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
            corner: 0.0,
        };
        // Scale 2x about the top-left corner anchor.
        k.scale(Vec2::ZERO, 2.0, 2.0);
        assert_eq!(k.bounds().xywh(), [0.0, 0.0, 200.0, 200.0]);
    }

    #[test]
    fn line_hit_uses_segment_distance() {
        let line = Shape {
            id: ShapeId(2),
            kind: ShapeKind::Line {
                points: vec![Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0)],
            },
            style: Style::default(),
        };
        assert!(line.hit(Vec2::new(50.0, 2.0), 3.0));
        assert!(!line.hit(Vec2::new(50.0, 20.0), 3.0));
    }

    #[test]
    fn filled_polygon_hits_interior_and_outline() {
        let poly = Shape {
            id: ShapeId(3),
            kind: ShapeKind::Polygon {
                points: vec![
                    Vec2::new(50.0, 0.0),
                    Vec2::new(100.0, 50.0),
                    Vec2::new(50.0, 100.0),
                    Vec2::new(0.0, 50.0),
                ],
            },
            style: Style {
                fill: Some(super::super::scene::Color::WHITE),
                ..Style::default()
            },
        };

        assert!(poly.hit(Vec2::new(50.0, 50.0), 2.0));
        assert!(poly.hit(Vec2::new(50.0, 2.0), 3.0));
        assert!(!poly.hit(Vec2::new(0.0, 0.0), 2.0));
    }

    #[test]
    fn point_segment_distance() {
        let d = dist_point_segment(
            Vec2::new(5.0, 5.0),
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
        );
        assert!((d - 5.0).abs() < 1e-4);
    }
}
