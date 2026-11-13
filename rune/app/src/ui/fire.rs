//! First-party pixel flames made from seeded, tapered geometric plumes.
//! Cache a PixelFire per outline, then paint on the caller's clock while visible.
use rune::Shape;
use super::theme::{Palette, rgb};
use std::f32::consts::TAU;

const ABOVE: usize = 13;
const FRAMES: usize = 32;
pub const CLASSIC: [u32; 5] = [0xa8240f, 0xe0461a, 0xff8f1f, 0xffd43b, 0xfff6c2];

pub fn colors(p: &Palette) -> [u32; 5] {
    if matches!(p.name, "neoism_dark" | "neoism_light") { return CLASSIC; }
    // The cooler tip shades into the background; the hot core picks up the theme's ink.
    let blend = |a: u32, b: u32, amount: f32| {
        [16, 8, 0].into_iter().fold(0, |color, shift| {
            let a = ((a >> shift) & 255) as f32;
            let b = ((b >> shift) & 255) as f32;
            color | (((a + (b - a) * amount).round() as u32) << shift)
        })
    };
    [blend(p.bg, p.accent, 0.4), blend(p.magenta, p.accent, 0.5), p.accent,
        blend(p.accent, p.cyan, 0.6), blend(p.cyan, p.fg, 0.7)]
}

pub struct PixelFire {
    frames: Vec<Vec<u8>>,
    width: usize,
    pub fps: f32,
}

impl PixelFire {
    /// Top occupied row per pixel column; None leaves a gap. `[Some(0); N]` is a flat edge.
    /// Outlines are bounded so decorative effects cannot allocate an unbounded simulation.
    pub fn new(profile: &[Option<usize>], seed: u32) -> Self {
        let profile = &profile[..profile.len().min(256)];
        let width = profile.len() + 2;
        let height = ABOVE + profile.iter().flatten().copied().max().unwrap_or(0).min(64) + 1;
        let mut state = seed.wrapping_add(1);
        let mut random = || {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            (state >> 8) as f32 / 16777216.0
        };
        let plumes: Vec<[f32; 4]> = (0..profile.len().div_ceil(4)).map(|index| {
            [index as f32 * 4.0 + 2.0 * random(), 1.2 + random(), 5.0 + 5.0 * random(), random() * TAU]
        }).collect();
        let mut frames = Vec::with_capacity(FRAMES);
        for frame in 0..FRAMES {
            let phase = frame as f32 / FRAMES as f32 * TAU;
            let mut pixels = vec![0u8; width * height];
            for (column, top) in profile.iter().enumerate() {
                let Some(top) = top else { continue };
                let base = ABOVE + (*top).min(64) - 1;
                let mut reach = 1.5f32;
                for &[center, radius, length, offset] in &plumes {
                    let center = center + (phase + offset).sin() * 1.1;
                    let distance = (column as f32 - center).abs() / radius;
                    let pulse = 0.6 + 0.4 * (phase * 2.0 + offset).sin();
                    reach = reach.max(1.5 + length * pulse * (-distance * distance).exp());
                }
                for rise in 0..=reach.ceil() as usize {
                    if rise > base { break; }
                    // A travelling ripple carries the brighter core upward through each tapered tongue.
                    let ripple = (rise as f32 * 0.9 + column as f32 * 0.7 - phase * 3.0).sin() * 0.07;
                    let warmth = (1.0 - rise as f32 / reach) * (0.88 + 0.1 * (phase + column as f32).sin()) + ripple;
                    if warmth <= 0.02 { continue; }
                    pixels[(base - rise) * width + column + 1] = (warmth.clamp(0.0, 0.999) * 5.0) as u8 + 1;
                }
            }
            frames.push(pixels);
        }
        Self { frames, width, fps: 16.0 }
    }

    /// Cell coordinates for hosts that attach flames to their own animated geometry.
    pub(crate) fn cells(&self, seconds: f32, colors: [u32; 5], mut cell: impl FnMut(f32, f32, u32)) {
        if !seconds.is_finite() || !self.fps.is_finite() || self.fps <= 0.0 { return; }
        let index = (seconds.max(0.0) as f64 * self.fps as f64).rem_euclid(FRAMES as f64) as usize;
        for (i, &heat) in self.frames[index].iter().enumerate() {
            if heat == 0 { continue; }
            let color = colors[heat as usize - 1];
            cell((i % self.width) as f32 - 1.0, (i / self.width) as f32 - ABOVE as f32, color);
        }
    }

    /// `origin` is the outline's top-left; cell size controls the pixel look. The caller owns
    /// fade/replay, clipping, redraw scheduling and reduced-motion policy, just like other canvas effects.
    pub fn paint(&self, shape: &mut Shape, origin: [f32; 2], cell: [f32; 2], seconds: f32, opacity: f32, colors: [u32; 5]) {
        if opacity <= 0.0 || !opacity.is_finite() || cell.iter().any(|v| !v.is_finite() || *v <= 0.0) { return; }
        self.cells(seconds, colors, |x, y, color| {
            let [x, y] = [origin[0] + x * cell[0], origin[1] + y * cell[1]];
            let [w, h] = cell;
            // Cover neighbouring polygon fringes so opaque regions do not show a hairline grid.
            let (left, top, right, bottom) = (x.round() - 0.5, y.round() - 0.5, (x + w).round() + 0.5, (y + h).round() + 0.5);
            let points = vec![[left, top], [right, top], [right, bottom], [left, bottom]];
            *shape = std::mem::take(shape).fill(points, rgb(color).alpha(opacity.clamp(0.0, 1.0)));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neoism_themes_keep_classic_fire_while_other_themes_supply_their_own_colors() {
        use crate::ui::theme::palette;
        for name in ["neoism_dark", "neoism_light", "pastel_dark"] {
            assert_eq!(colors(palette(name)), CLASSIC);
        }
        let catppuccin = palette("catppuccin_mocha");
        assert_ne!(colors(catppuccin), CLASSIC);
        assert!(colors(catppuccin).contains(&catppuccin.accent));
        assert_ne!(colors(catppuccin), colors(palette("one_light")));
    }

    #[test]
    fn flames_animate_in_discrete_cells_and_loop_without_drift() {
        let fire = PixelFire::new(&[Some(0); 12], 1);
        let sample = |time| {
            let mut cells = Vec::new();
            fire.cells(time, CLASSIC, |x, y, color| {
                assert_eq!(x.fract(), 0.0);
                assert_eq!(y.fract(), 0.0);
                assert!(CLASSIC.contains(&color));
                cells.push((x, y, color));
            });
            cells
        };
        assert!(!sample(0.5).is_empty());
        assert_ne!(sample(0.5), sample(0.75));
        assert_eq!(sample(0.5), sample(2.5));
        assert!(sample(f32::NAN).is_empty());
        let empty = PixelFire::new(&[], 0);
        empty.cells(1.0, CLASSIC, |_, _, _| panic!("an empty outline must not burn"));
    }
}