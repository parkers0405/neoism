//! Dependency-free vector port of `neoism-web/src/pixel-wordmark.ts`.
//!
//! Hosts own redraw scheduling, visibility, and the unchanged idle artwork.
//! Rectangles (including clip/occlusions) are `[x, y, width, height]` in logical
//! pixels. Pass each *current idle image slice's* rect, including its
//! hover/shimmer transform, not a tight glyph bounding box.
//! During `show_original()` draw idle first, then this fading vector overlay.

use std::f32::consts::{PI, TAU};
use std::sync::atomic::{AtomicU32, Ordering};
use sugarloaf::Sugarloaf;
use web_time::{Instant, SystemTime, UNIX_EPOCH};

const GRID: f32 = 31.25;
const SETTLE: f32 = 0.2;
/// Seven rows, leftmost cell in bit 6. Manually sampled at cell centres from
/// the original SVG paths (nonzero winding, including E/O counters).
const MASKS: [[u8; 7]; 6] = [
    [0, 0, 0b1111110, 0b1100011, 0b1100011, 0b1100011, 0b1100011],
    [0, 0, 0b0111110, 0b1100011, 0b1111111, 0b1100000, 0b0111110],
    [0, 0, 0b0111110, 0b1100011, 0b1100011, 0b1100011, 0b0111110],
    [
        0b0001100, 0, 0b0011100, 0b0001100, 0b0001100, 0b0001100, 0b0111111,
    ],
    [0, 0, 0b0111110, 0b1100000, 0b0111110, 0b0000011, 0b1111110],
    [0, 0, 0b1111110, 0b1011011, 0b1011011, 0b1011011, 0b1011011],
];
// Preserve the SVG's serialized foreground origins rather than rounding the
// approximately 249.99706-unit advance or assuming 250-unit letter strips.
const LETTER_X: [f64; 6] = [
    0.00003335,
    249.997092,
    499.99415,
    749.991186,
    999.988267,
    1249.985348,
];
const PNG_WIDTH: f64 = 1196.0;
const PNG_HEIGHT: f64 = 193.0;
const SVG_TO_PNG: f64 = 0.8;
const PNG_CROP_Y: f64 = 482.0;
const SVG_BASELINE: f64 = 874.498729;
const LAYERS: [[f32; 3]; 3] = [
    [25.742966, -20.846183, 0.302],
    [12.8715, -10.423127, 0.502],
    [0.0, 0.0, 1.0],
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Scan,
    Scatter,
    Rain,
    Beams,
    Decrypt,
    Crumble,
    Slice,
    Wipe,
    Waves,
    Fireworks,
    Burn,
    Vhs,
    Rise,
    Checkerboard,
    Spiral,
}
impl Effect {
    pub const ALL: [Self; 15] = [
        Self::Scan,
        Self::Scatter,
        Self::Rain,
        Self::Beams,
        Self::Decrypt,
        Self::Crumble,
        Self::Slice,
        Self::Wipe,
        Self::Waves,
        Self::Fireworks,
        Self::Burn,
        Self::Vhs,
        Self::Rise,
        Self::Checkerboard,
        Self::Spiral,
    ];
}

/// Owned snapshot; never borrows the animation state or its clock.
#[derive(Clone, Copy, Debug)]
pub struct AnimationFrame {
    pub effect: Effect,
    /// Clamped main-animation progress, 1 throughout the settle tail.
    pub progress: f32,
    /// 1 during the main animation, fading from 1 to 0 during the tail.
    pub tail_opacity: f32,
    tail: bool,
}

#[derive(Clone, Debug, Default)]
pub struct WordmarkAnimation {
    started: Option<Instant>,
    duration: f32,
    effect: Option<Effect>,
    bag: Vec<Effect>,
    previous: Option<Effect>,
    rng: u32,
}

static SEQUENCE: AtomicU32 = AtomicU32::new(1);
impl WordmarkAnimation {
    fn random(&mut self) -> u32 {
        if self.rng == 0 {
            let time = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default();
            self.rng = (time.as_secs() as u32)
                ^ time.subsec_nanos()
                ^ SEQUENCE.fetch_add(0x9e3779b9, Ordering::Relaxed);
            if self.rng == 0 {
                self.rng = 0xa341316c;
            }
        }
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng
    }

    fn choose_effect(&mut self) -> Effect {
        if self.bag.is_empty() {
            self.bag.extend(Effect::ALL);
            for i in (1..self.bag.len()).rev() {
                let j = self.random() as usize % (i + 1);
                self.bag.swap(i, j);
            }
            if self.bag.last().copied() == self.previous {
                self.bag.swap(0, Effect::ALL.len() - 1);
            }
        }
        let effect = self.bag.pop().expect("refilled effect bag");
        self.previous = Some(effect);
        effect
    }

    /// Restart immediately, choosing the next effect even if already playing.
    pub fn replay(&mut self) {
        self.effect = Some(self.choose_effect());
        self.duration =
            1.7 + (self.random() as f64 / (u32::MAX as f64 + 1.0) * 0.8) as f32;
        self.started = Some(Instant::now());
    }

    /// Stop without resetting the bag (including its boundary-repeat guard).
    pub fn cancel(&mut self) {
        self.started = None;
    }

    pub fn is_active(&self) -> bool {
        self.frame().is_some()
    }

    pub fn frame(&self) -> Option<AnimationFrame> {
        self.frame_at(Instant::now())
    }

    fn frame_at(&self, now: Instant) -> Option<AnimationFrame> {
        let elapsed = now.saturating_duration_since(self.started?).as_secs_f32();
        if elapsed >= self.duration + SETTLE {
            return None;
        }
        Some(AnimationFrame {
            effect: self.effect?,
            progress: clamp(elapsed / self.duration),
            tail_opacity: if elapsed < self.duration {
                1.0
            } else {
                clamp(1.0 - (elapsed - self.duration) / SETTLE)
            },
            tail: elapsed >= self.duration,
        })
    }
}

fn clamp(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}
fn ease(x: f32) -> f32 {
    1.0 - (1.0 - clamp(x)).powi(3)
}

#[derive(Clone, Copy, Debug)]
struct Tile {
    x: f32,
    y: f32,
    across: f32,
    row: usize,
    column: usize,
    letter: usize,
    seed: f32,
}
impl Tile {
    fn new(letter: usize, row: usize, column: usize) -> Self {
        Self {
            x: column as f32 * GRID,
            y: -250.0 + row as f32 * GRID,
            across: (letter as f32 + column as f32 / 7.0) / 6.0,
            row,
            column,
            letter,
            seed: ((row * 37 + column * 19 + letter * 13) % 101) as f32 / 101.0,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
struct Sample {
    dx: f32,
    dy: f32,
    sx: f32,
    sy: f32,
    rotation: f32,
    opacity: f32,
}
impl Default for Sample {
    fn default() -> Self {
        Self {
            dx: 0.0,
            dy: 0.0,
            sx: 1.0,
            sy: 1.0,
            rotation: 0.0,
            opacity: 1.0,
        }
    }
}

/// Pure website sampling. Stable seed intentionally excludes the shadow layer.
fn sample(effect: Effect, progress: f32, t: Tile) -> Sample {
    let p = clamp(progress);
    // All website effects have settled by 1. Avoid residual trig roundoff at
    // exact multiples of PI so the final vector geometry is exactly identity.
    if p == 1.0 {
        return Sample::default();
    }
    let beat = (p * 18.0).floor() as usize;
    let reveal = |arrival: f32, span: f32| ease((p - arrival) / span);
    let mut s = Sample::default();
    match effect {
        Effect::Scan => {
            let built = reveal(t.across * 0.72, 0.22);
            s.sx = 0.15 + built * 0.85;
            s.sy = s.sx;
            s.opacity = built;
        }
        Effect::Scatter => {
            let d = ease(p / 0.22) * (1.0 - reveal(0.28 + t.seed * 0.1, 0.52));
            s.dx = (t.seed * TAU).cos() * 22.0 * d;
            s.dy = (t.seed * TAU).sin() * 22.0 * d;
            s.sx = 1.0 - d * 0.5;
            s.sy = s.sx;
            s.opacity = 1.0 - d * 0.35;
        }
        Effect::Rain => {
            let fall = reveal(t.seed * 0.35 + t.across * 0.25, 0.35);
            s.dy = -GRID * 1.5 * (1.0 - fall);
            s.opacity = fall;
            s.sx = 0.6 + fall * 0.4;
            s.sy = s.sx;
        }
        Effect::Beams => {
            let built = reveal(((t.column * 3 + t.letter) % 7) as f32 / 7.0 * 0.55, 0.35);
            s.sx = built.powi(2);
            s.sy = 1.0 + (built * PI).sin() * 2.0;
            s.opacity = clamp(built * 3.0);
        }
        Effect::Decrypt => {
            let lock = reveal(t.seed * 0.5, 0.35);
            let noise = (t.column * 17 + t.row * 23 + beat * 11) % 49;
            s.dx = ((noise % 7) as f32 - t.column as f32) * GRID * (1.0 - lock);
            s.dy = ((noise / 7) as f32 - t.row as f32) * GRID * (1.0 - lock);
            s.sx = lock + (1.0 - lock) * (0.25 + (noise % 3) as f32 * 0.2);
            s.sy = s.sx;
            s.opacity = 0.35 + lock * 0.65;
        }
        Effect::Crumble => {
            let collapse = ease((p - t.seed * 0.13) / 0.22);
            let rebuild = reveal(0.4 + (6 - t.row) as f32 * 0.035, 0.32);
            let rubble = collapse * (1.0 - rebuild);
            s.dy = (6 - t.row) as f32 * GRID * rubble;
            s.dx = (t.seed - 0.5) * GRID * rubble;
            s.rotation = (t.seed - 0.5) * 150.0 * rubble;
            s.sx = 1.0 - rubble * 0.7;
            s.sy = s.sx;
            s.opacity = 1.0 - rubble * 0.3;
        }
        Effect::Slice => {
            let join = reveal(t.row as f32 * 0.065, 0.38);
            s.dx = if t.row % 2 == 1 { 1.0 } else { -1.0 } * 7.0 * GRID * (1.0 - join);
            s.sy = 0.25 + join * 0.75;
            s.opacity = clamp(join * 4.0);
        }
        Effect::Wipe => {
            s.sx = clamp((p * 1.2 - t.across) * 42.0);
            s.dx = -GRID * (1.0 - s.sx) / 2.0;
        }
        Effect::Waves => {
            let crest = p * PI * 5.0 - t.across * PI * 4.0;
            let envelope = (1.0 - p).powi(2);
            s.dy = crest.sin() * GRID * 1.4 * envelope;
            s.sy = 1.0 + crest.cos() * 0.45 * envelope;
            s.opacity = reveal(t.across * 0.25, 0.25);
        }
        Effect::Fireworks => {
            let flight = clamp((p - t.letter as f32 * 0.04) / 0.7);
            let launch = ease(flight / 0.4);
            let travel = launch + (flight * PI).sin() * 0.4;
            s.dx = (3.0 * GRID - t.x) * (1.0 - travel);
            s.dy = (-140.0 - t.y) * (1.0 - travel) - (flight * PI).sin() * GRID;
            s.sx = launch.max(0.08);
            s.sy = s.sx;
            s.rotation = (1.0 - flight) * (t.seed - 0.5) * 180.0;
            s.opacity = clamp(flight * 6.0);
        }
        Effect::Burn => {
            let scorch = reveal(t.row as f32 * 0.035, 0.18);
            let regrow = reveal(0.42 + (6 - t.row) as f32 * 0.035, 0.3);
            let ash = scorch * (1.0 - regrow);
            s.sx = 1.0 - ash * 0.85;
            s.sy = 1.0 - ash * 0.5;
            s.dy = -ash * GRID * (0.4 + t.seed);
            s.opacity = 1.0 - ash * 0.9;
        }
        Effect::Vhs => {
            let envelope = (1.0 - p).powi(2);
            let tracking = (t.row as f32 * 4.0 + beat as f32 * 2.0).sin();
            // JS Math.round ties toward +infinity (Rust round differs for negatives).
            s.dx = (tracking * 2.0 + 0.5).floor() * GRID * envelope;
            s.sy = 1.0 - if t.row % 2 == 1 { 0.65 } else { 0.15 } * envelope;
            s.opacity = 1.0 - if (beat + t.row) % 3 == 0 { 0.55 } else { 0.1 } * envelope;
        }
        Effect::Rise => {
            let lift = reveal(t.letter as f32 * 0.055 + (6 - t.row) as f32 * 0.04, 0.36);
            s.dy = (6 - t.row) as f32 * GRID * (1.0 - lift);
            s.sy = lift;
            s.opacity = lift;
        }
        Effect::Checkerboard => {
            let parity = (t.column + t.row) % 2;
            let flip = reveal(parity as f32 * 0.38 + t.letter as f32 * 0.025, 0.32);
            s.sx = ((1.0 - flip) * PI / 2.0).cos().abs();
            s.sy = 0.2 + flip * 0.8;
            s.rotation = (1.0 - flip) * if parity == 1 { 90.0 } else { -90.0 };
            s.opacity = flip;
        }
        Effect::Spiral => {
            let assemble = reveal(t.seed * 0.18, 0.65);
            let angle = (1.0 - assemble) * TAU;
            let x = t.x - 3.0 * GRID;
            let y = t.y + 140.0;
            let radius = 0.25 + assemble * 0.75;
            s.dx = (angle.cos() * x - angle.sin() * y) * radius - x;
            s.dy = (angle.sin() * x + angle.cos() * y) * radius - y;
            s.sx = 0.15 + assemble * 0.85;
            s.sy = s.sx;
            s.rotation = (1.0 - assemble) * 270.0;
            s.opacity = clamp(assemble * 3.0);
        }
    }
    s
}

#[derive(Clone, Copy, Debug)]
struct Spark {
    rect: [f32; 4],
    opacity: f32,
}
fn spark(effect: Effect, progress: f32, letter: usize, index: usize) -> Spark {
    let p = clamp(progress);
    let seed = index as f32 * 0.43 + letter as f32 * 0.07;
    let mut s = Spark {
        rect: [0.0, 0.0, GRID / 5.0, GRID / 5.0],
        opacity: 0.0,
    };
    if p == 1.0 {
        return s;
    }
    match effect {
        Effect::Scan => {
            let sweep = p / 0.94 * 6.0 - letter as f32;
            if (0.0..=1.0).contains(&sweep) {
                s.opacity = (1.0 - sweep) * 0.85;
            }
            s.rect[0] = clamp(sweep) * 7.0 * GRID;
            s.rect[1] = -200.0 + (p * 19.0 + seed * 12.0).sin() * 45.0;
        }
        Effect::Beams => {
            let head = clamp((p - letter as f32 * 0.035) / 0.72);
            s.rect = [
                (seed * 7.0).floor() * GRID + GRID / 2.0,
                -250.0 + head * 7.0 * GRID,
                GRID / 8.0,
                GRID * 2.0,
            ];
            s.opacity = (head * PI).sin().max(0.0) * 0.8;
        }
        Effect::Fireworks => {
            let flight = clamp((p - letter as f32 * 0.04) / 0.7);
            let angle = seed * PI * 6.0;
            s.rect[0] = 3.0 * GRID + angle.cos() * flight * 4.0 * GRID;
            s.rect[1] = -140.0 + angle.sin() * flight * 3.0 * GRID;
            s.opacity = (flight * PI).sin().max(0.0) * (1.0 - flight);
        }
        Effect::Burn => {
            s.rect[0] = seed * 7.0 * GRID;
            s.rect[1] = -60.0 - p * 6.0 * GRID;
            s.opacity = (p * PI).sin().powi(2) * 0.8;
        }
        _ => {}
    }
    s
}

type Point = [f32; 2];

/// The PNG was rasterized at 0.8 pixels/SVG unit and integer-trimmed:
/// x=[0,1196), y=[482,675). Top alpha at x=700 is 6/255 and bottom
/// alpha at x=10 is 153/255, confirming fractional SVG edge coverage.
/// Thus its SVG-space bounds are [0,602.5,1495,241.25], NOT the tight
/// [0,603.652546,1494.478,239.596183] geometric bounds. Retain this padding.
fn map_point(local: Point, letter: usize, layer: [f32; 3], rect: [f32; 4]) -> Point {
    // Use f64 until the final renderer coordinate: subtracting a late letter's
    // strip origin in f32 needlessly loses precision near the glyph edges.
    let global_x = LETTER_X[letter] + layer[0] as f64 + local[0] as f64;
    let global_y = SVG_BASELINE + layer[1] as f64 + local[1] as f64;
    let strip_width = PNG_WIDTH / 6.0;
    let local_x = (global_x * SVG_TO_PNG - letter as f64 * strip_width) / strip_width;
    let local_y = (global_y * SVG_TO_PNG - PNG_CROP_Y) / PNG_HEIGHT;
    [
        (rect[0] as f64 + local_x * rect[2] as f64) as f32,
        (rect[1] as f64 + local_y * rect[3] as f64) as f32,
    ]
}

fn tile_corners(t: Tile, s: Sample) -> [Point; 4] {
    let cx = t.x + GRID / 2.0 + s.dx;
    let cy = t.y + GRID / 2.0 + s.dy;
    let (sin, cos) = s.rotation.to_radians().sin_cos();
    let mut corners = [[0.0; 2]; 4];
    for (out, (x, y)) in
        corners
            .iter_mut()
            .zip([(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)])
    {
        let x = x * GRID * s.sx;
        let y = y * GRID * s.sy;
        *out = [cx + cos * x - sin * y, cy + sin * x + cos * y];
    }
    corners
}

impl AnimationFrame {
    pub fn show_original(&self) -> bool {
        self.tail
    }

    /// Draw 3 shadow layers and 2 front-layer sparks for one letter in logical pixels.
    /// Animated cells may travel outside their idle slice; only `clip` and the
    /// occlusions constrain them (not the letter rect).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_letter(
        &self,
        sugarloaf: &mut Sugarloaf,
        letter_index: usize,
        rect: [f32; 4],
        color: [f32; 4],
        clip: [f32; 4],
        occlusions: &[[f32; 4]],
        depth: f32,
        order: u8,
    ) {
        if letter_index >= MASKS.len()
            || !valid_rect(rect)
            || !valid_rect(clip)
            || !color.iter().all(|v| v.is_finite())
            || !depth.is_finite()
            || !self.progress.is_finite()
            || !self.tail_opacity.is_finite()
        {
            return;
        }
        let mut paint = |corners: [Point; 4], alpha: f32, layer: [f32; 3]| {
            let mut rgba = color;
            rgba[3] =
                clamp(color[3]) * clamp(alpha) * layer[2] * clamp(self.tail_opacity);
            if rgba[3] <= 0.0 {
                return;
            }
            let mapped = corners.map(|p| map_point(p, letter_index, layer, rect));
            if !mapped.iter().flatten().all(|v| v.is_finite()) {
                return;
            }
            for poly in visible_polygons(&mapped, clip, occlusions) {
                for i in 1..poly.len().saturating_sub(1) {
                    let (a, b, c) = (poly[0], poly[i], poly[i + 1]);
                    let area =
                        (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
                    if area.is_finite() && area.abs() > 0.000001 {
                        sugarloaf.triangle_ordered(
                            a[0], a[1], b[0], b[1], c[0], c[1], depth, rgba, order,
                        );
                    }
                }
            }
        };
        for (layer_index, layer) in LAYERS.into_iter().enumerate() {
            for (row, mask) in MASKS[letter_index].iter().enumerate() {
                for column in 0..7 {
                    if mask & (1 << (6 - column)) == 0 {
                        continue;
                    }
                    let tile = Tile::new(letter_index, row, column);
                    let s = sample(self.effect, self.progress, tile);
                    paint(tile_corners(tile, s), s.opacity, layer);
                }
            }
            if layer_index == 2 {
                for index in 0..2 {
                    let s = spark(self.effect, self.progress, letter_index, index);
                    let [x, y, w, h] = s.rect;
                    paint(
                        [[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
                        s.opacity,
                        layer,
                    );
                }
            }
        }
    }
}

fn valid_rect(r: [f32; 4]) -> bool {
    r.iter().all(|v| v.is_finite())
        && r[2] > 0.0
        && r[3] > 0.0
        && (r[0] + r[2]).is_finite()
        && (r[1] + r[3]).is_finite()
}

/// Sutherland-Hodgman half-plane clipping; convex input stays convex.
fn half_plane(poly: &[Point], axis: usize, edge: f32, greater: bool) -> Vec<Point> {
    let mut out = Vec::with_capacity(poly.len() + 1);
    let Some(&last) = poly.last() else {
        return out;
    };
    let inside = |p: Point| {
        if greater {
            p[axis] >= edge
        } else {
            p[axis] <= edge
        }
    };
    let mut prev = last;
    for &curr in poly {
        if inside(prev) != inside(curr) {
            // f64 prevents overflow on otherwise finite extreme input geometry.
            let t = (edge as f64 - prev[axis] as f64)
                / (curr[axis] as f64 - prev[axis] as f64);
            let mut cross = [0.0; 2];
            for i in 0..2 {
                cross[i] =
                    (prev[i] as f64 + t * (curr[i] as f64 - prev[i] as f64)) as f32;
            }
            cross[axis] = edge;
            out.push(cross);
        }
        if inside(curr) {
            out.push(curr);
        }
        prev = curr;
    }
    out
}

fn visible_polygons(
    corners: &[Point; 4],
    clip: [f32; 4],
    occlusions: &[[f32; 4]],
) -> Vec<Vec<Point>> {
    let mut poly = corners.to_vec();
    for (axis, edge, greater) in [
        (0, clip[0], true),
        (0, clip[0] + clip[2], false),
        (1, clip[1], true),
        (1, clip[1] + clip[3], false),
    ] {
        poly = half_plane(&poly, axis, edge, greater);
    }
    if poly.len() < 3 {
        return Vec::new();
    }
    let mut pieces = vec![poly];
    for &r in occlusions {
        if !valid_rect(r) {
            continue;
        }
        let mut next = Vec::new();
        for mut remainder in pieces {
            // Emit disjoint outside strips; carry only the inside remainder
            // to the next edge. Discard the final (occluded) intersection.
            for (axis, edge, greater) in [
                (0, r[0], true),
                (0, r[0] + r[2], false),
                (1, r[1], true),
                (1, r[1] + r[3], false),
            ] {
                let outside = half_plane(&remainder, axis, edge, !greater);
                if outside.len() >= 3 {
                    next.push(outside);
                }
                remainder = half_plane(&remainder, axis, edge, greater);
                if remainder.len() < 3 {
                    break;
                }
            }
        }
        pieces = next;
        if pieces.is_empty() {
            break;
        }
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;
    use web_time::Duration;
    fn close(a: f32, b: f32) {
        assert!((a - b).abs() < 0.0001, "{a} != {b}");
    }

    #[test]
    fn permutations_and_boundary_guard() {
        let mut a = WordmarkAnimation {
            rng: 12345,
            ..Default::default()
        };
        let mut previous = None;
        for _ in 0..100 {
            let mut seen = Vec::new();
            for _ in 0..15 {
                let effect = a.choose_effect();
                assert_ne!(Some(effect), previous);
                assert!(!seen.contains(&effect));
                seen.push(effect);
                previous = Some(effect);
            }
            assert_eq!(seen.len(), Effect::ALL.len());
        }
    }

    #[test]
    fn restart_cancel_and_bounded_lifetime() {
        let mut a = WordmarkAnimation::default();
        assert!(!a.is_active());
        a.replay();
        let first = a.effect;
        assert!((1.7..=2.5).contains(&a.duration));
        let start = a.started.unwrap();
        let frame = a.frame_at(start).unwrap();
        assert_eq!(frame.progress, 0.0);
        assert_eq!(frame.tail_opacity, 1.0);
        assert!(!frame.show_original());
        let tail = a
            .frame_at(start + Duration::from_secs_f32(a.duration + 0.1))
            .unwrap();
        assert!(tail.show_original());
        close(tail.progress, 1.0);
        close(tail.tail_opacity, 0.5);
        assert!(a
            .frame_at(start + Duration::from_secs_f32(a.duration + 0.201))
            .is_none());
        a.replay();
        assert_ne!(a.effect, first);
        assert!(a.is_active());
        a.cancel();
        assert!(a.frame().is_none());
        assert_eq!(a.bag.len(), 13);
        a.replay();
        assert_eq!(a.bag.len(), 12);
    }

    #[test]
    fn exact_masks_and_finite_effects() {
        let counts: Vec<u32> = MASKS
            .iter()
            .map(|m| m.iter().map(|r| r.count_ones()).sum())
            .collect();
        assert_eq!(counts, [22, 23, 22, 17, 20, 26]);
        assert_eq!(counts.iter().sum::<u32>() * 3, 390);
        assert_eq!(MASKS.len() * 2, 12);
        for effect in Effect::ALL {
            for letter in 0..6 {
                for row in 0..7 {
                    for col in 0..7 {
                        let tile = Tile::new(letter, row, col);
                        assert_eq!(sample(effect, 1.0, tile), Sample::default());
                        for step in 0..=100 {
                            let s = sample(effect, step as f32 / 100.0, tile);
                            assert!([s.dx, s.dy, s.sx, s.sy, s.rotation, s.opacity]
                                .iter()
                                .all(|v| v.is_finite()));
                            assert!((0.0..=1.0).contains(&s.opacity));
                            assert!(tile_corners(tile, s)
                                .iter()
                                .flatten()
                                .all(|v| v.is_finite()));
                        }
                    }
                }
                for i in 0..2 {
                    assert_eq!(spark(effect, 1.0, letter, i).opacity, 0.0);
                    for step in 0..=100 {
                        let s = spark(effect, step as f32 / 100.0, letter, i);
                        assert!(s.rect.iter().all(|v| v.is_finite()));
                        assert!((0.0..=1.0).contains(&s.opacity));
                    }
                }
            }
        }
    }

    #[test]
    fn website_sample_parity_points() {
        // Independent hand-computed website values, not calls to the port's ease.
        let t = Tile::new(0, 0, 0); // seed/across = 0
        let s = sample(Effect::Scan, 0.11, t);
        close(s.opacity, 0.875);
        close(s.sx, 0.89375);
        let s = sample(Effect::Scatter, 0.22, t);
        close(s.dx, 22.0);
        close(s.dy, 0.0);
        close(s.sx, 0.5);
        close(s.opacity, 0.65);
        let s = sample(Effect::Rain, 0.175, t);
        close(s.dy, -5.859375);
        close(s.sx, 0.95);
        close(s.opacity, 0.875);
        let s = sample(Effect::Decrypt, 0.0, Tile::new(0, 2, 0));
        close(s.dx, 125.0);
        close(s.dy, 125.0);
        close(s.sx, 0.45);
        close(s.opacity, 0.35);
        let s = sample(Effect::Slice, 0.0, Tile::new(0, 2, 0));
        close(s.dx, -218.75);
        close(s.sy, 0.25);
        close(s.opacity, 0.0);
        let s = sample(Effect::Wipe, 0.0, t);
        close(s.dx, -15.625);
        close(s.sx, 0.0);
        close(s.opacity, 1.0);
        let s = sample(Effect::Vhs, 0.0, t);
        close(s.dx, 0.0);
        close(s.sy, 0.85);
        close(s.opacity, 0.45);
        let s = spark(Effect::Beams, 0.36, 0, 0);
        close(s.rect[0], 15.625);
        close(s.rect[1], -140.625);
        close(s.rect[2], 3.90625);
        close(s.rect[3], 62.5);
        close(s.opacity, 0.8);
        let s = spark(Effect::Burn, 0.5, 0, 0);
        close(s.rect[1], -153.75);
        close(s.opacity, 0.8);
    }

    fn area(poly: &[Point]) -> f32 {
        (0..poly.len())
            .map(|i| {
                let a = poly[i];
                let b = poly[(i + 1) % poly.len()];
                a[0] * b[1] - a[1] * b[0]
            })
            .sum::<f32>()
            .abs()
            / 2.0
    }

    #[test]
    fn polygon_clip_and_overlapping_occlusion_union() {
        let corners = [[-5.0, -5.0], [15.0, -5.0], [15.0, 15.0], [-5.0, 15.0]];
        let pieces = visible_polygons(
            &corners,
            [0.0, 0.0, 10.0, 10.0],
            &[[2.0, 2.0, 4.0, 4.0], [4.0, 4.0, 4.0, 4.0]],
        );
        close(pieces.iter().map(|p| area(p)).sum(), 72.0);
        for p in pieces {
            assert!(p.iter().flatten().all(|v| v.is_finite()));
            assert!(p
                .iter()
                .all(|v| v[0] >= 0.0 && v[0] <= 10.0 && v[1] >= 0.0 && v[1] <= 10.0));
        }
        // Diamond clipped by a half-width overlay: keep the actual triangle,
        // not an axis-aligned bounding box or an all-or-nothing tile skip.
        let diamond = [[5.0, 0.0], [10.0, 5.0], [5.0, 10.0], [0.0, 5.0]];
        let pieces =
            visible_polygons(&diamond, [0.0, 0.0, 10.0, 10.0], &[[5.0, 0.0, 5.0, 10.0]]);
        close(pieces.iter().map(|p| area(p)).sum(), 25.0);
    }

    #[test]
    fn all_settled_tiles_reconstruct_original_composite_pixels() {
        // Independent source-SVG coordinates and exact raster/crop dimensions.
        let origins = [
            0.00003335_f64,
            249.997092,
            499.99415,
            749.991186,
            999.988267,
            1249.985348,
        ];
        let offsets = [
            (25.742966_f64, -20.846183_f64),
            (12.8715, -10.423127),
            (0.0, 0.0),
        ];
        let strip_width = 1196.0_f64 / 6.0;
        for effect in Effect::ALL {
            for letter in 0..6 {
                // Adjacent idle strips in the original composite's pixel space.
                let rect = [
                    (letter as f64 * strip_width) as f32,
                    0.0,
                    strip_width as f32,
                    193.0,
                ];
                for (layer_index, &(ox, oy)) in offsets.iter().enumerate() {
                    for row in 0..7 {
                        for column in 0..7 {
                            if MASKS[letter][row] & (1 << (6 - column)) == 0 {
                                continue;
                            }
                            let tile = Tile::new(letter, row, column);
                            let corners = tile_corners(tile, sample(effect, 1.0, tile));
                            for local in corners {
                                let actual =
                                    map_point(local, letter, LAYERS[layer_index], rect);
                                let expected_x =
                                    (origins[letter] + ox + local[0] as f64) * 0.8;
                                let expected_y =
                                    (874.498729 + oy + local[1] as f64) * 0.8 - 482.0;
                                // Renderer coordinates and slice rects are f32;
                                // tolerate at most a fraction of one pixel ULP.
                                assert!((actual[0] as f64 - expected_x).abs() < 0.0002);
                                assert!((actual[1] as f64 - expected_y).abs() < 0.0001);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn mapping_uses_equal_png_strips_and_current_idle_rect() {
        let strip_width = 1196.0_f64 / 6.0;
        for letter in 0..6 {
            // A global PNG strip boundary maps to its own rect edge, NOT
            // to the nearest multiple of the SVG letter advance.
            let boundary_local_x = letter as f64 * strip_width / 0.8 - LETTER_X[letter];
            let rect = [37.0, -12.0, 303.0, 81.0];
            let left = map_point(
                [boundary_local_x as f32, 482.0 / 0.8 - 874.498729],
                letter,
                LAYERS[2],
                rect,
            );
            close(left[0], rect[0]);
            close(left[1], rect[1]);
            let right = map_point(
                [
                    (boundary_local_x + strip_width / 0.8) as f32,
                    675.0 / 0.8 - 874.498729,
                ],
                letter,
                LAYERS[2],
                rect,
            );
            close(right[0], rect[0] + rect[2]);
            close(right[1], rect[1] + rect[3]);
        }
    }

    #[test]
    fn png_coordinate_mapping_retains_trim_padding() {
        // Invert the equal-width slice mapping to recover original PNG pixels.
        let rect = [0.0, 0.0, 1196.0 / 6.0, 193.0];
        let top_dot = map_point([93.75, -250.0], 3, LAYERS[0], rect);
        close(top_dot[1], 0.92204);
        let bottom = map_point([0.0, -31.25], 0, LAYERS[2], rect);
        close(bottom[1], 192.599);
        let last = map_point([218.75, -31.25], 5, LAYERS[0], rect);
        close(last[0] + 5.0 * rect[2], 1195.5826);
    }
}
