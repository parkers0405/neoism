//! Deterministic pixel-plasma presence avatar — the Rust port of Synapse's
//! `MemberOrb`. A hash of the seed (a peer's stable name, e.g.
//! `piss-desktop`) fixes a palette + plasma frequencies, so the same host
//! always renders the same chunky pixelated orb on every device with zero
//! storage. Rendered as a grid of small solid quads; cells outside the unit
//! circle are skipped, which gives the round pixelated silhouette for free
//! (no mask needed). Pass a wall-clock `time` in seconds to animate the
//! plasma, or a fixed value (e.g. `0.6`) for a still, still-unique frame.

use std::f32::consts::TAU;

/// One filled pixel of the avatar: a device-independent rect (relative to
/// the caller's origin, already sized to the requested box) and its RGBA.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AvatarCell {
    /// `[x, y, w, h]` in the same logical space as the requested box.
    pub rect: [f32; 4],
    /// Straight-alpha RGBA in 0..1.
    pub color: [f32; 4],
}

/// Hashed plasma parameters — the identity of one avatar. Deterministic in
/// the seed; build once and reuse across frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AvatarProfile {
    hues: [f32; 5],
    f1: f32,
    f2: f32,
    f3: f32,
    f4: f32,
    s1: f32,
    s2: f32,
    s3: f32,
    s4: f32,
    p1: f32,
    p2: f32,
    /// Chunky pixel resolution across (11–14), matching Synapse.
    grid: u32,
}

#[inline]
fn fmod(a: f32, n: f32) -> f32 {
    ((a % n) + n) % n
}

/// Shortest-path hue interpolation (cross the wheel the pretty way).
#[inline]
fn lerp_hue(a: f32, b: f32, t: f32) -> f32 {
    fmod(a + (fmod(b - a + 180.0, 360.0) - 180.0) * t, 360.0)
}

/// FNV-1a (32-bit), byte-identical to Synapse's `hashSeed` for ASCII; for
/// non-ASCII we hash UTF-16 code units so multi-byte names still match the
/// JS `charCodeAt` stream.
fn hash_seed(seed: &str) -> u32 {
    let mut h: u32 = 2_166_136_261;
    for unit in seed.encode_utf16() {
        h ^= u32::from(unit);
        h = h.wrapping_mul(16_777_619);
    }
    h
}

/// xorshift32 PRNG matching Synapse's `makeRng` (returns 0..1).
struct Rng(u32);
impl Rng {
    fn new(seed: u32) -> Self {
        Self(if seed == 0 { 1 } else { seed })
    }
    fn next(&mut self) -> f32 {
        let mut s = self.0;
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        self.0 = s;
        s as f32 / 4_294_967_296.0
    }
}

impl AvatarProfile {
    pub fn from_seed(seed: &str) -> Self {
        let h = hash_seed(if seed.is_empty() { " " } else { seed });
        let mut r = Rng::new(h);
        let base = (h % 360) as f32;
        // Vibrant palette the pixels cycle through: an analogous run plus
        // one opposite pop.
        let hues = [
            base,
            fmod(base + 26.0 + r.next() * 24.0, 360.0),
            fmod(base + 58.0 + r.next() * 30.0, 360.0),
            fmod(base - 40.0 - r.next() * 24.0, 360.0),
            fmod(base + 165.0 + r.next() * 40.0, 360.0),
        ];
        let f1 = 5.0 + r.next() * 7.0;
        let f2 = 5.0 + r.next() * 7.0;
        let f3 = 4.0 + r.next() * 6.0;
        let f4 = 6.0 + r.next() * 10.0;
        let s1 = 0.5 + r.next() * 0.9;
        let s2 = 0.5 + r.next() * 0.9;
        let s3 = 0.4 + r.next() * 0.8;
        let s4 = 0.6 + r.next() * 1.1;
        let p1 = r.next() * TAU;
        let p2 = r.next() * TAU;
        let grid = 11 + (r.next() * 4.0).floor() as u32;
        Self {
            hues,
            f1,
            f2,
            f3,
            f4,
            s1,
            s2,
            s3,
            s4,
            p1,
            p2,
            grid,
        }
    }

    /// Grid resolution (cells across / down).
    pub fn grid(&self) -> u32 {
        self.grid
    }

    /// Emit the filled cells for a `size`×`size` box whose top-left is at
    /// `(ox, oy)`, at animation time `t` seconds. Cells outside the unit
    /// circle are omitted (the round silhouette). `push` receives each
    /// `AvatarCell`; the caller draws them as solid quads.
    ///
    /// Cell boundaries are integer-snapped so the grid TILES exactly —
    /// cell `i`'s right edge is cell `i+1`'s left edge (both `round`) —
    /// with no seam and no overlap. Feed device-pixel `ox/oy/size` (the
    /// draw helper does) and every quad lands on a whole pixel: crisp,
    /// no fractional-pixel blend. This replaces the old `+1` overdraw,
    /// which smeared neighbouring cells into each other at small sizes.
    pub fn cells(&self, ox: f32, oy: f32, size: f32, t: f32, mut push: impl FnMut(AvatarCell)) {
        let grid = self.grid as f32;
        let cell = size / grid;
        for j in 0..self.grid {
            // Integer-snapped row band: this row's bottom edge IS the
            // next row's top edge, so rows tile seamlessly.
            let y0 = (oy + j as f32 * cell).round();
            let y1 = (oy + (j as f32 + 1.0) * cell).round();
            for i in 0..self.grid {
                let nx = ((i as f32 + 0.5) / grid) * 2.0 - 1.0;
                let ny = ((j as f32 + 0.5) / grid) * 2.0 - 1.0;
                let dist = (nx * nx + ny * ny).sqrt();
                // Keep the rim cells a hair past the unit circle so the
                // silhouette reads full and round at the cardinal
                // shoulders instead of being clipped a pixel shy.
                if dist > 1.02 {
                    continue; // outside the circle → transparent
                }
                let mut p = (nx * self.f1 + t * self.s1 + self.p1).sin()
                    + (ny * self.f2 - t * self.s2 + self.p2).sin()
                    + ((nx + ny) * self.f3 + t * self.s3).sin()
                    + (dist * self.f4 - t * self.s4).sin();
                p = (p + 4.0) / 8.0; // → 0..1
                let idx = p * self.hues.len() as f32;
                let lo = idx.floor() as usize;
                let hue = lerp_hue(
                    self.hues[lo % self.hues.len()],
                    self.hues[(lo + 1) % self.hues.len()],
                    idx - lo as f32,
                );
                // Rim falloff darkens the outer edge for a lit-sphere read.
                let rim = 1.0 - (((dist - 0.62) / 0.38).max(0.0)) * 0.55;
                let lightness = ((34.0 + p * 40.0) * rim) / 100.0;
                let color = hsl_to_rgba(hue, 0.88, lightness);
                // Integer-snapped column band, tiling with its neighbour.
                let x0 = (ox + i as f32 * cell).round();
                let x1 = (ox + (i as f32 + 1.0) * cell).round();
                push(AvatarCell {
                    rect: [x0, y0, x1 - x0, y1 - y0],
                    color,
                });
            }
        }
    }
}

/// Convenience: build the profile and emit cells in one call (for one-off
/// draws that don't cache the profile).
pub fn avatar_cells(
    seed: &str,
    ox: f32,
    oy: f32,
    size: f32,
    t: f32,
    push: impl FnMut(AvatarCell),
) {
    AvatarProfile::from_seed(seed).cells(ox, oy, size, t, push);
}

/// Wall-clock animation time (seconds since process start) to drive the
/// plasma. Reuses the shared process clock, which is:
///   * f32-precision safe — it counts elapsed seconds from a process
///     epoch, not the raw unix epoch (a raw epoch as f32 quantizes to
///     ~128 s and freezes any animation sampling it), and
///   * wasm-safe — `web_time` under the hood (std `Instant` panics on
///     wasm32).
/// Pass this instead of a fixed frame so the orb animates. Pair it with
/// the render loop's presence-orb redraw owner (desktop
/// `redraw_reason()` returns `Some` while any peer is present) so frames
/// keep coming while an orb is on screen — and stop, for zero cost, the
/// instant the last peer leaves.
pub fn presence_orb_now_seconds() -> f32 {
    crate::cursor_style::rainbow_now_seconds()
}

/// HSL (h in degrees, s/l in 0..1) → straight-alpha RGBA in 0..1, alpha 1.
fn hsl_to_rgba(h: f32, s: f32, l: f32) -> [f32; 4] {
    let l = l.clamp(0.0, 1.0);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = fmod(h, 360.0) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [r1 + m, g1 + m, b1 + m, 1.0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_is_deterministic_in_the_seed() {
        let a = AvatarProfile::from_seed("piss-desktop");
        let b = AvatarProfile::from_seed("piss-desktop");
        assert_eq!(a, b);
        let c = AvatarProfile::from_seed("other-host");
        assert_ne!(a, c);
        assert!((11..=14).contains(&a.grid()));
    }

    #[test]
    fn all_emitted_cells_sit_inside_the_circle_and_the_box() {
        let prof = AvatarProfile::from_seed("piss-desktop");
        let size = 40.0;
        let mut count = 0;
        let mut all_inside_box = true;
        prof.cells(0.0, 0.0, size, 0.6, |cell| {
            count += 1;
            if cell.rect[0] < 0.0 || cell.rect[1] < 0.0 {
                all_inside_box = false;
            }
            for ch in &cell.color[..3] {
                assert!((0.0..=1.0).contains(ch), "channel out of range: {ch}");
            }
        });
        assert!(all_inside_box);
        // A circle inside an NxN grid fills ~78% of the cells; assert we got
        // a plausible non-empty, non-full count.
        let grid = prof.grid();
        let total = grid * grid;
        assert!(count > (total / 2) as usize && count < total as usize);
    }

    #[test]
    fn emitted_cells_tile_on_integer_pixels_without_overlap() {
        let prof = AvatarProfile::from_seed("piss-desktop");
        let size = 42.0;
        let mut cells: Vec<[f32; 4]> = Vec::new();
        prof.cells(0.0, 0.0, size, 0.6, |c| cells.push(c.rect));
        assert!(!cells.is_empty());
        for r in &cells {
            // Every edge lands on a whole pixel — the crispness contract.
            for v in r {
                assert_eq!(*v, v.round(), "cell edge not integer-aligned: {r:?}");
            }
            // No cell collapses below a pixel, and none escapes the box.
            assert!(r[2] >= 1.0 && r[3] >= 1.0, "cell smaller than a pixel: {r:?}");
            assert!(
                r[0] >= 0.0 && r[1] >= 0.0 && r[0] + r[2] <= size && r[1] + r[3] <= size,
                "cell outside box: {r:?}"
            );
        }
        // Seamless tiling: no two cells overlap (a shared edge is not an
        // overlap — that's what makes the pixels crisp instead of blended).
        for a in 0..cells.len() {
            for b in (a + 1)..cells.len() {
                let (p, q) = (cells[a], cells[b]);
                let overlap = p[0] < q[0] + q[2]
                    && p[0] + p[2] > q[0]
                    && p[1] < q[1] + q[3]
                    && p[1] + p[3] > q[1];
                assert!(!overlap, "cells overlap: {p:?} vs {q:?}");
            }
        }
    }

    #[test]
    fn hsl_primaries_round_trip() {
        let red = hsl_to_rgba(0.0, 1.0, 0.5);
        assert!(red[0] > 0.99 && red[1] < 0.01 && red[2] < 0.01);
        let green = hsl_to_rgba(120.0, 1.0, 0.5);
        assert!(green[1] > 0.99 && green[0] < 0.01 && green[2] < 0.01);
        let blue = hsl_to_rgba(240.0, 1.0, 0.5);
        assert!(blue[2] > 0.99 && blue[0] < 0.01 && blue[1] < 0.01);
    }
}
