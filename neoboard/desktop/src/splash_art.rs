//! Lowercase pixel wordmark and renderer-independent, glyph-attached fire.

use std::f32::consts::TAU;
use std::sync::OnceLock;

/// Seven rows, leftmost pixel in bit 6, in `neoboard` order.
/// `neo` is identical to Rune's original wordmark; `bard` uses faithful
/// Press Start 2P-style masks with the same two-pixel stems and x-height.
pub(super) const MASKS: [[u8; 7]; 8] = [
    [0, 0, 0b1111110, 0b1100011, 0b1100011, 0b1100011, 0b1100011],
    [0, 0, 0b0111110, 0b1100011, 0b1111111, 0b1100000, 0b0111110],
    [0, 0, 0b0111110, 0b1100011, 0b1100011, 0b1100011, 0b0111110],
    [
        0b1100000, 0b1100000, 0b1111110, 0b1100011, 0b1100011, 0b1100011, 0b1111110,
    ],
    [0, 0, 0b0111110, 0b1100011, 0b1100011, 0b1100011, 0b0111110],
    [0, 0, 0b0111110, 0b0000011, 0b0111111, 0b1100011, 0b0111111],
    [0, 0, 0b1101110, 0b1110011, 0b1100000, 0b1100000, 0b1100000],
    [
        0b0000011, 0b0000011, 0b0111111, 0b1100011, 0b1100011, 0b1100011, 0b0111111,
    ],
];

const GRID: f32 = 31.25;
const CELL: f32 = GRID / 2.0;
const ABOVE: usize = 13;
const FRAMES: usize = 32;
const FPS: f64 = 16.0;
const WIDTH: usize = 14;
// Neoism's default dark theme: ui::fire::CLASSIC.
const COLORS: [u32; 5] = [0xa8240f, 0xe0461a, 0xff8f1f, 0xffd43b, 0xfff6c2];

/// Emit flame rectangles as `(source_row, source_column, [x, y, w, h], 0xRRGGBB)`.
/// The source is the top occupied glyph cell in that column, so callers can
/// attach every flame to its animated source pixel. Coordinates are glyph-local:
/// glyph cell `(row, column)` starts at `(column * 31.25, -250 + row * 31.25)`.
/// Callers own opacity, transforms, visibility, reduced motion and frame timing.
/// Negative/non-finite time and out-of-range letters emit nothing.
pub(super) fn fire(
    letter: usize,
    time: f32,
    mut cell: impl FnMut(usize, usize, [f32; 4], u32),
) {
    if letter >= MASKS.len() || !time.is_finite() || time < 0.0 {
        return;
    }
    static FIRE: OnceLock<[PixelFire; 8]> = OnceLock::new();
    let flames = FIRE.get_or_init(|| std::array::from_fn(PixelFire::new));
    let flame = &flames[letter];
    let frame = (time as f64 * FPS).rem_euclid(FRAMES as f64) as usize;
    for (i, &heat) in flame.frames[frame].iter().enumerate() {
        if heat == 0 {
            continue;
        }
        let x = i % WIDTH;
        let column = x / 2;
        let Some(row) = flame.tops[column] else {
            continue;
        };
        let left = x as f32 * CELL;
        let top = -250.0 + (i / WIDTH) as f32 * CELL - ABOVE as f32 * CELL;
        // Match Rune's attached flame quads, overlapping fringes to avoid seams.
        cell(
            row,
            column,
            [left - 0.6, top - 0.6, CELL + 1.2, CELL + 1.2],
            COLORS[heat as usize - 1],
        );
    }
}

/// Theme/renderer-free specialization of `rune/app/src/ui/fire.rs`'s PixelFire.
/// Keeps its seeded plumes, 32-frame cycle, half-glyph-cell resolution and heat
/// ripple; omits its empty padding columns and caches once, never per frame.
struct PixelFire {
    tops: [Option<usize>; 7],
    frames: Vec<Vec<u8>>,
}

impl PixelFire {
    fn new(letter: usize) -> Self {
        let tops: [Option<usize>; 7] = std::array::from_fn(|column| {
            MASKS[letter]
                .iter()
                .position(|row| row & (1 << (6 - column)) != 0)
        });
        let height = ABOVE + tops.iter().flatten().copied().max().unwrap_or(0) * 2 + 1;
        let mut state = (letter as u32).wrapping_add(1);
        let mut random = || {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            (state >> 8) as f32 / 16777216.0
        };
        let plumes: [[f32; 4]; 4] = std::array::from_fn(|index| {
            [
                index as f32 * 4.0 + 2.0 * random(),
                1.2 + random(),
                5.0 + 5.0 * random(),
                random() * TAU,
            ]
        });
        let mut frames = Vec::with_capacity(FRAMES);
        for frame in 0..FRAMES {
            let phase = frame as f32 / FRAMES as f32 * TAU;
            let mut pixels = vec![0; WIDTH * height];
            for column in 0..WIDTH {
                let Some(top) = tops[column / 2] else {
                    continue;
                };
                let base = ABOVE + top * 2 - 1;
                let mut reach = 1.5f32;
                for &[center, radius, length, offset] in &plumes {
                    let center = center + (phase + offset).sin() * 1.1;
                    let distance = (column as f32 - center).abs() / radius;
                    let pulse = 0.6 + 0.4 * (phase * 2.0 + offset).sin();
                    reach =
                        reach.max(1.5 + length * pulse * (-distance * distance).exp());
                }
                for rise in 0..=reach.ceil() as usize {
                    if rise > base {
                        break;
                    }
                    let ripple = (rise as f32 * 0.9 + column as f32 * 0.7 - phase * 3.0)
                        .sin()
                        * 0.07;
                    let warmth = (1.0 - rise as f32 / reach)
                        * (0.88 + 0.1 * (phase + column as f32).sin())
                        + ripple;
                    if warmth > 0.02 {
                        pixels[(base - rise) * WIDTH + column] =
                            (warmth.clamp(0.0, 0.999) * 5.0) as u8 + 1;
                    }
                }
            }
            frames.push(pixels);
        }
        Self { tops, frames }
    }
}
