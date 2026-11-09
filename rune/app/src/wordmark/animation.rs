//! Port of `neoism-frontend/shared/src/primitives/wordmark_animation.rs` (itself a port of
//! `neoism-web/src/pixel-wordmark.ts`): the fifteen NEOISM click effects, sampled per 31.25-unit
//! cell of each letter's bitmask and emitted as quads. Timing runs on Rune's frame clock instead of
//! `Instant`, so headless `--at` renders are deterministic. The Press Start 2P glyph path the
//! original also carries (for pixel labels) is not needed here and was left out.

use std::f32::consts::{PI, TAU};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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
    started: Option<f64>,
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
    pub fn replay(&mut self, now: f64) {
        self.effect = Some(self.choose_effect());
        self.duration =
            1.7 + (self.random() as f64 / (u32::MAX as f64 + 1.0) * 0.8) as f32;
        self.started = Some(now);
    }

    pub fn is_active(&self, now: f64) -> bool {
        self.frame(now).is_some()
    }

    pub fn frame(&self, now: f64) -> Option<AnimationFrame> {
        let elapsed = (now - self.started?).max(0.0) as f32;
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
    phase: f32,
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
            phase: letter as f32,
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
            let flight = clamp((p - t.phase * 0.04) / 0.7);
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
            let lift = reveal(t.phase * 0.055 + (6 - t.row) as f32 * 0.04, 0.36);
            s.dy = (6 - t.row) as f32 * GRID * (1.0 - lift);
            s.sy = lift;
            s.opacity = lift;
        }
        Effect::Checkerboard => {
            let parity = (t.column + t.row) % 2;
            let flip = reveal(parity as f32 * 0.38 + t.phase * 0.025, 0.32);
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
    spark_at(effect, progress, letter as f32, index)
}
fn spark_at(effect: Effect, progress: f32, letter: f32, index: usize) -> Spark {
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

    /// The three shadow layers and two front-layer sparks of one letter as quads in the
    /// coordinates of `rect` (the letter's current idle slice, hover and shimmer included),
    /// with the alpha (0-1) each is drawn at. Same order and alpha as the original's
    /// `draw_letter`, which handed the same quads to Sugarloaf as triangles.
    pub fn letter_quads(&self, letter_index: usize, rect: [f32; 4], mut quad: impl FnMut([Point; 4], f32)) {
        if letter_index >= MASKS.len() || !(rect[2] > 0.0 && rect[3] > 0.0) || !self.progress.is_finite() {
            return;
        }
        let mut paint = |corners: [Point; 4], alpha: f32, layer: [f32; 3]| {
            let a = clamp(alpha) * layer[2] * clamp(self.tail_opacity);
            if a <= 0.0 {
                return;
            }
            let mapped = corners.map(|p| map_point(p, letter_index, layer, rect));
            if mapped.iter().flatten().all(|v| v.is_finite()) {
                quad(mapped, a);
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
                    paint([[x, y], [x + w, y], [x + w, y + h], [x, y + h]], s.opacity, layer);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_effect_settles_back_to_the_artwork() {
        let mut animation = WordmarkAnimation::default();
        for _ in 0..Effect::ALL.len() {
            animation.replay(10.0);
            let frame = animation.frame(10.5).expect("playing");
            let mut quads = 0;
            for letter in 0..6 { frame.letter_quads(letter, [0.0, 0.0, 100.0, 96.0], |_, _| quads += 1); }
            assert!(quads > 0, "{:?} draws", frame.effect);
            let end = animation.frame(10.0 + 2.5 + SETTLE as f64 + 0.01);
            assert!(end.is_none());
        }
    }

    #[test]
    fn bag_plays_all_fifteen_before_repeating() {
        let mut animation = WordmarkAnimation::default();
        let mut seen = Vec::new();
        for _ in 0..15 {
            animation.replay(0.0);
            seen.push(animation.frame(0.1).unwrap().effect);
        }
        for effect in Effect::ALL { assert!(seen.contains(&effect), "{effect:?} missing"); }
    }
}
