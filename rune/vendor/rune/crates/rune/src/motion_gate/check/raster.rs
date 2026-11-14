//! A coarse picture of the window each frame, for the region checks: the painter stream
//! (`Runtime::instances`) rasterized on the CPU into cells a few points across, or a GPU frame
//! averaged into the same cells. Each cell holds its mean colour in OKLab, over black.
//!
//! The raster is approximate where precision costs: corners are square, a glyph covers a share
//! of its quad, a shadow's blur is a three-step ramp and a radial gradient its mean colour.
//! Effects, backdrop blurs and inset shadows draw nothing. What it keeps is what a person sees
//! change: a surface's colour where it sits, through its layers' opacity.

use crate::color::{Color, over};
use crate::render::{
    Instance, KIND_DASHED, KIND_GLYPH, KIND_GRADIENT, KIND_IMAGE, KIND_LAYER, KIND_LAYER_END, KIND_RADIAL, KIND_RECT, KIND_SEGMENT, KIND_SHADOW,
    KIND_TRIANGLE,
};

/// Points per cell.
pub const CELL: f32 = 12.0;

/// One frame's cells, row-major, in OKLab scaled by [`Q`].
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    pub w: usize,
    pub h: usize,
    lab: Vec<[i16; 3]>,
}

const Q: f32 = 8192.0;

impl Grid {
    fn new(w: usize, h: usize, cells: &[[f32; 4]]) -> Grid {
        let lab = cells.iter().map(|&c| {
            let [r, g, b, _] = over(c, [0.0, 0.0, 0.0, 1.0]);
            let [l, a, bb, _] = Color { r, g, b, a: 1.0 }.to_oklab();
            [l, a, bb].map(|v| (v * Q).round().clamp(-32767.0, 32767.0) as i16)
        }).collect();
        Grid { w, h, lab }
    }

    pub fn lab(&self, i: usize) -> [f32; 3] {
        self.lab[i].map(|v| v as f32 / Q)
    }

    /// The colour of cell `i`, for evidence.
    pub fn color(&self, i: usize) -> Color {
        let [l, a, b] = self.lab(i);
        Color::from_oklab([l, a, b, 1.0])
    }

    /// Cell `i`'s rect in points.
    pub fn rect(&self, i: usize) -> [f32; 4] {
        [(i % self.w) as f32 * CELL, (i / self.w) as f32 * CELL, CELL, CELL]
    }
}

/// Cells for a window `size` points across.
pub fn dims(size: (f32, f32)) -> (usize, usize) {
    ((size.0 / CELL).ceil().max(1.0) as usize, (size.1 / CELL).ceil().max(1.0) as usize)
}

/// One shape to cover cells with: `outer` less `hole` (device px boxes `[x0, y0, x1, y1]`) at
/// `density`, in premultiplied `color`, or running to `bottom` down `outer`.
struct Draw {
    outer: [f32; 4],
    hole: Option<[f32; 4]>,
    density: f32,
    color: [f32; 4],
    bottom: Option<[f32; 4]>,
}

fn boxed(r: [f32; 4]) -> [f32; 4] {
    [r[0], r[1], r[0] + r[2], r[1] + r[3]]
}

fn meet(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])]
}

fn grow(b: [f32; 4], d: f32) -> [f32; 4] {
    [b[0] - d, b[1] - d, b[2] + d, b[3] + d]
}

fn overlap(a: [f32; 4], b: [f32; 4]) -> f32 {
    let m = meet(a, b);
    (m[2] - m[0]).max(0.0) * (m[3] - m[1]).max(0.0)
}

fn lerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

fn draws(q: &Instance, apron: bool) -> Vec<Draw> {
    let kind = q.params[1];
    let outer = boxed(q.rect);
    let solid = |color: [f32; 4], density: f32| Draw { outer, hole: None, density, color, bottom: None };
    let ring = |color: [f32; 4], width: f32, density: f32| Draw { outer, hole: Some(grow(outer, -width)), density, color, bottom: None };
    let mut out = Vec::new();
    if kind == KIND_RECT {
        out.push(solid(q.fill, 1.0));
        if q.params[0] > 0.0 { out.push(ring(q.border, q.params[0], 1.0)); }
    } else if kind == KIND_GRADIENT {
        if q.params[0] > 0.0 {
            out.push(Draw { bottom: Some(q.uv), ..ring(q.border, q.params[0], 1.0) });
        } else {
            out.push(Draw { bottom: Some(q.uv), ..solid(q.fill, 1.0) });
        }
    } else if kind == KIND_GLYPH {
        // Off the pixel grid an icon samples one transparent texel past each edge, which it
        // drops on the grid (`icon_inst`): leave that apron out, so it covers the same cells.
        let t = if apron && q.uv[2] > 2.0 && q.uv[3] > 2.0 { [q.rect[2] / q.uv[2], q.rect[3] / q.uv[3]] } else { [0.0; 2] };
        let core = [outer[0] + t[0], outer[1] + t[1], outer[2] - t[0], outer[3] - t[1]];
        out.push(Draw { outer: core, ..solid(q.fill, 0.35) });
    } else if kind == KIND_RADIAL {
        let t = (q.params[3] % 2.0 - q.params[0] % 2.0).abs().clamp(0.0, 1.0);
        out.push(solid(lerp(q.fill, q.border, 0.5), t));
    } else if kind == KIND_DASHED {
        out.push(Draw { bottom: Some(q.border), ..ring(q.fill, q.params[0], 0.5) });
    } else if kind == KIND_SEGMENT {
        let [x0, y0, x1, y1] = q.radii;
        let hw = q.params[0];
        let b = [x0.min(x1) - hw, y0.min(y1) - hw, x0.max(x1) + hw, y0.max(y1) + hw];
        let area = ((b[2] - b[0]) * (b[3] - b[1])).max(1e-3);
        let len = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
        out.push(Draw { outer: b, ..solid(q.fill, ((len + 2.0 * hw) * 2.0 * hw / area).min(1.0)) });
    } else if kind == KIND_TRIANGLE {
        let [ax, ay, bx, by] = q.radii;
        let (cx, cy) = (q.uv[0], q.uv[1]);
        let b = [ax.min(bx).min(cx), ay.min(by).min(cy), ax.max(bx).max(cx), ay.max(by).max(cy)];
        out.push(Draw { outer: b, ..solid(q.fill, 0.5) });
    } else if kind == KIND_IMAGE {
        let a = q.fill[3];
        out.push(solid([0.5 * a, 0.5 * a, 0.5 * a, a], 1.0));
    } else if kind == KIND_SHADOW {
        let sigma = q.params[2].max(0.5);
        let [u0, u1, u2, u3] = q.uv;
        // A negative `uv.z` gives the band of the shadow this instance draws; any other `uv` a
        // hole it leaves over the box casting it.
        let window = if u2 < 0.0 { [q.rect[0] + u0, q.rect[1] + u1, q.rect[0] - u2, q.rect[1] + u3] } else { [f32::NEG_INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::INFINITY] };
        let hole = (u2 > 0.0).then(|| [q.rect[0] + u0, q.rect[1] + u1, q.rect[0] + u0 + u2, q.rect[1] + u1 + u3]);
        for d in [-sigma, 0.0, sigma] {
            out.push(Draw { outer: meet(grow(outer, d), window), hole, density: 1.0 / 3.0, color: q.fill, bottom: None });
        }
    }
    out
}

/// One scope of the painter stream: its cells, opacity and clip.
struct Scope {
    cells: Vec<[f32; 4]>,
    opacity: f32,
    clip: [f32; 4],
}

/// Rasterizes `instances` (device pixels, `scale` per point) for a window `size` points across.
pub fn rasterize(instances: &[Instance], size: (f32, f32), scale: f32) -> Grid {
    rasterize_with(instances, size, scale, &[], &Default::default())
}

/// [`rasterize`], knowing which glyphs are icons drawn through their apron (`apron_key`), and
/// how far a gliding glyph's ink sits down its quad (`TextSystem::glyph_shifts`).
pub(crate) fn rasterize_with(instances: &[Instance], size: (f32, f32), scale: f32, aprons: &[[u32; 3]], shifts: &std::collections::HashMap<[u32; 3], f32>) -> Grid {
    let (w, h) = dims(size);
    let px = CELL * scale;
    let full = [f32::NEG_INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::INFINITY];
    let mut stack = vec![Scope { cells: vec![[0.0; 4]; w * h], opacity: 1.0, clip: full }];
    let cell_area = px * px;
    for q in instances {
        let kind = q.params[1];
        if kind == KIND_LAYER {
            let clip = meet(stack.last().unwrap().clip, q.clip);
            stack.push(Scope { cells: vec![[0.0; 4]; w * h], opacity: q.fill[3].clamp(0.0, 1.0), clip });
            continue;
        }
        if kind == KIND_LAYER_END {
            if stack.len() > 1 {
                let top = stack.pop().unwrap();
                let parent = &mut stack.last_mut().unwrap().cells;
                for (d, s) in parent.iter_mut().zip(&top.cells) {
                    *d = over(s.map(|v| v * top.opacity), *d);
                }
            }
            continue;
        }
        let clip = meet(stack.last().unwrap().clip, q.clip);
        let cells = &mut stack.last_mut().unwrap().cells;
        let key = (kind == KIND_GLYPH).then(|| crate::runtime::apron_key(q));
        let apron = key.is_some_and(|k| aprons.contains(&k));
        let mut q = *q;
        if let Some(dy) = key.and_then(|k| shifts.get(&k)) { q.rect[1] += dy; }
        for d in draws(&q, apron) {
            if d.color[3] <= 1e-4 || d.density <= 0.0 { continue; }
            let b = meet(d.outer, clip);
            if b[2] <= b[0] || b[3] <= b[1] { continue; }
            let (cx0, cy0) = ((b[0] / px).floor().max(0.0) as usize, (b[1] / px).floor().max(0.0) as usize);
            let (cx1, cy1) = (((b[2] / px).ceil() as usize).min(w), ((b[3] / px).ceil() as usize).min(h));
            let height = (d.outer[3] - d.outer[1]).max(1e-3);
            for cy in cy0..cy1 {
                for cx in cx0..cx1 {
                    let cell = [cx as f32 * px, cy as f32 * px, (cx + 1) as f32 * px, (cy + 1) as f32 * px];
                    let mut cover = overlap(b, cell);
                    if let Some(hole) = d.hole { cover -= overlap(meet(hole, b), cell); }
                    let k = (cover / cell_area).clamp(0.0, 1.0) * d.density;
                    if k <= 0.0 { continue; }
                    let color = match d.bottom {
                        Some(bottom) => lerp(d.color, bottom, (((cell[1] + cell[3]) / 2.0 - d.outer[1]) / height).clamp(0.0, 1.0)),
                        None => d.color,
                    };
                    let i = cy * w + cx;
                    cells[i] = over(color.map(|v| v * k), cells[i]);
                }
            }
        }
    }
    while stack.len() > 1 {
        let top = stack.pop().unwrap();
        let parent = &mut stack.last_mut().unwrap().cells;
        for (d, s) in parent.iter_mut().zip(&top.cells) { *d = over(s.map(|v| v * top.opacity), *d); }
    }
    Grid::new(w, h, &stack[0].cells)
}

/// A GPU frame `pw`×`ph` of 8-bit sRGB RGBA (straight alpha, as a readback gives it) averaged
/// into the cells of a window `size` points across.
pub fn from_pixels(rgba: &[u8], pw: u32, ph: u32, size: (f32, f32)) -> Grid {
    let (w, h) = dims(size);
    let (sx, sy) = (pw as f32 / size.0.max(1.0), ph as f32 / size.1.max(1.0));
    let decode = |v: u8| { let c = v as f32 / 255.0; if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) } };
    let lut: Vec<f32> = (0..=255u8).map(decode).collect();
    let mut cells = vec![[0.0f32; 4]; w * h];
    for cy in 0..h {
        for cx in 0..w {
            let (x0, y0) = ((cx as f32 * CELL * sx) as u32, (cy as f32 * CELL * sy) as u32);
            let (x1, y1) = ((((cx + 1) as f32 * CELL * sx) as u32).min(pw), (((cy + 1) as f32 * CELL * sy) as u32).min(ph));
            // Every other pixel each way: a cell's mean needs no more.
            let (mut sum, mut n) = ([0.0f32; 3], 0.0f32);
            for y in (y0..y1).step_by(2) {
                for x in (x0..x1).step_by(2) {
                    let p = ((y * pw + x) * 4) as usize;
                    let a = rgba[p + 3] as f32 / 255.0;
                    for c in 0..3 { sum[c] += lut[rgba[p + c] as usize] * a; }
                    n += 1.0;
                }
            }
            if n > 0.0 { cells[cy * w + cx] = [sum[0] / n, sum[1] / n, sum[2] / n, 1.0]; }
        }
    }
    Grid::new(w, h, &cells)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    fn rect(r: [f32; 4], c: Color, op: f32) -> Instance {
        Instance { rect: r, fill: c.premul(op), params: [0.0, KIND_RECT, 0.0, 0.0], clip: [-1e9, -1e9, 1e9, 1e9], ..Instance::zeroed() }
    }

    #[test]
    fn a_rect_colours_the_cells_it_covers_through_its_layer() {
        let red = Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
        let g = rasterize(&[rect([0.0, 0.0, 48.0, 24.0], red, 1.0)], (48.0, 48.0), 2.0);
        assert_eq!((g.w, g.h), (4, 4));
        assert!((g.color(0).r - 1.0).abs() < 0.02 && g.color(15).r < 0.01);
        let mut layer = Instance::zeroed();
        layer.params[1] = KIND_LAYER;
        layer.fill[3] = 0.5;
        layer.clip = [-1e9, -1e9, 1e9, 1e9];
        let mut end = Instance::zeroed();
        end.params[1] = KIND_LAYER_END;
        let half = rasterize(&[layer, rect([0.0, 0.0, 96.0, 96.0], red, 1.0), end], (48.0, 48.0), 2.0);
        assert!((half.color(0).r - 0.5).abs() < 0.02, "{:?}", half.color(0));
    }

    #[test]
    fn an_icon_covers_the_same_cells_on_the_pixel_grid_as_off_it() {
        // `icon_inst`: on the grid it loads its 16 texels directly; off it, the quad and its
        // span grow by one transparent texel each side for filtering.
        let glyph = |r: [f32; 4], span: f32| Instance {
            rect: r, fill: Color::WHITE.premul(1.0), params: [0.0, KIND_GLYPH, 0.0, 1.0], uv: [7.0, 9.0, span, span],
            clip: [-1e9, -1e9, 1e9, 1e9], ..Instance::zeroed()
        };
        let (on, off) = (glyph([48.0, 24.0, 16.0, 16.0], 0.0), glyph([47.0, 23.0, 18.0, 18.0], 18.0));
        let aprons = [crate::runtime::apron_key(&off)];
        let (a, b) = (rasterize_with(&[on], (96.0, 72.0), 1.0, &aprons, &Default::default()), rasterize_with(&[off], (96.0, 72.0), 1.0, &aprons, &Default::default()));
        let ink = |g: &Grid| (0..g.w * g.h).map(|i| g.lab(i)[0]).collect::<Vec<_>>();
        for (i, (a, b)) in ink(&a).iter().zip(ink(&b)).enumerate() {
            assert!((a - b).abs() < 0.01, "cell {i}: {a} on the grid, {b} off it");
        }
        // A zoomed text glyph's span is its own texels, with no apron: all of it is ink.
        let whole = rasterize(&[off], (96.0, 72.0), 1.0);
        assert!(ink(&whole) != ink(&b), "only an icon's apron is left out");
    }

    #[test]
    fn a_gpu_frame_averages_into_the_same_cells() {
        let mut px = vec![0u8; 24 * 24 * 4];
        for p in px.chunks_mut(4).take(24 * 12) { p.copy_from_slice(&[255, 255, 255, 255]); }
        let g = from_pixels(&px, 24, 24, (24.0, 24.0));
        assert_eq!((g.w, g.h), (2, 2));
        assert!(g.lab(0)[0] > 0.99 && g.lab(3)[0] < 0.01);
    }
}
