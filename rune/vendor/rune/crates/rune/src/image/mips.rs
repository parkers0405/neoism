//! Linear-light mip chains, built whole on a cache worker or a slice at a time where the
//! same thread also draws frames (the browser's main thread), so it can stop between slices.
use super::{Decoded, ImageError, Mip};

pub(super) fn mip_chain(width: u32, height: u32, bytes: &[u8]) -> Result<Decoded, ImageError> {
    let mut steps = MipSteps::new(width, height, bytes)?;
    while !steps.step(usize::MAX) {}
    Ok(steps.finish())
}

/// A mip chain under construction: mip 0 at once, then the smaller levels a slice of rows
/// at a time.
pub(super) struct MipSteps<'a> {
    /// The straight-alpha source, which mip 1 filters.
    source: &'a [u8],
    mips: Vec<Mip>,
    /// The level being built, and the row it continues from.
    next: Vec<u8>,
    row: u32,
}

impl<'a> MipSteps<'a> {
    pub(super) fn new(width: u32, height: u32, bytes: &'a [u8]) -> Result<Self, ImageError> {
        if width == 0 || height == 0 || width > 8192 || height > 8192
            || bytes.len() != width as usize * height as usize * 4
        { return Err(ImageError("RGBA image dimensions or byte length are invalid".into())); }
        let srgb = &*SRGB;
        // Mip 0 is the source with RGB premultiplied in linear light; an opaque pixel is unchanged.
        let mut top = bytes.to_vec();
        for p in top.as_chunks_mut::<4>().0 {
            if p[3] == 255 { continue; }
            let a = p[3] as f32 / 255.0;
            for c in 0..3 { p[c] = srgb.encode(srgb.decode[p[c] as usize] * a, p[3]); }
        }
        Ok(MipSteps { source: bytes, mips: vec![Mip { width, height, pixels: top }], next: Vec::new(), row: 0 })
    }

    /// Build at most about `budget` more pixels (whole rows, at least one); true once the
    /// chain reaches 1×1.
    pub(super) fn step(&mut self, mut budget: usize) -> bool {
        let srgb = &*SRGB;
        loop {
            let last = self.mips.last().expect("mip 0 is built first");
            let (w, h) = (last.width, last.height);
            if w == 1 && h == 1 { return true; }
            if budget == 0 { return false; }
            let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
            if self.row == 0 { self.next.reserve_exact(nw as usize * nh as usize * 4); }
            let rows = (budget / nw as usize).clamp(1, (nh - self.row) as usize) as u32;
            let range = self.row..self.row + rows;
            // Mip 1 filters the straight source, so a translucent edge is premultiplied once,
            // not quantized first. Each later mip filters the bytes of the one above it.
            if self.mips.len() == 1 {
                downsample_rows(w, h, self.source, range, &mut self.next, |p| {
                    let a = p[3] as f32 / 255.0;
                    [srgb.decode[p[0] as usize] * a, srgb.decode[p[1] as usize] * a, srgb.decode[p[2] as usize] * a, a]
                });
            } else {
                downsample_rows(w, h, &last.pixels, range, &mut self.next, |p| {
                    [srgb.decode[p[0] as usize], srgb.decode[p[1] as usize], srgb.decode[p[2] as usize], p[3] as f32 / 255.0]
                });
            }
            self.row += rows;
            budget = budget.saturating_sub(rows as usize * nw as usize);
            if self.row == nh {
                self.mips.push(Mip { width: nw, height: nh, pixels: std::mem::take(&mut self.next) });
                self.row = 0;
            }
        }
    }

    pub(super) fn finish(self) -> Decoded { Decoded { mips: self.mips, motion: None } }
}

/// Rows `rows` of `w`×`h` RGBA8 `src` halved by area, appended to `out`, summing `linear`
/// (premultiplied linear RGBA) of each source pixel. Area resampling includes the last
/// row/column of odd-sized sources.
fn downsample_rows(w: u32, h: u32, src: &[u8], rows: std::ops::Range<u32>, out: &mut Vec<u8>, linear: impl Fn(&[u8; 4]) -> [f32; 4]) {
    let src = src.as_chunks::<4>().0;
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut emit = |sum: [f32; 4], area: f32| {
        let [r, g, b, a] = sum.map(|v| v / area);
        let a = byte(a);
        out.extend_from_slice(&[SRGB.encode(r, a), SRGB.encode(g, a), SRGB.encode(b, a), a]);
    };
    if w % 2 == 0 && h % 2 == 0 {
        // Each target pixel is exactly a 2×2 block.
        let w = w as usize;
        for y in rows.start as usize..rows.end as usize { for x in 0..nw as usize {
            let i = 2 * y * w + 2 * x;
            let mut sum = [0.0; 4];
            for p in [i, i + 1, i + w, i + w + 1].map(|i| linear(&src[i])) {
                for c in 0..4 { sum[c] += p[c]; }
            }
            emit(sum, 4.0);
        }}
        return;
    }
    for y in rows { for x in 0..nw {
        let (x0, x1) = (x as f32 * w as f32 / nw as f32, (x + 1) as f32 * w as f32 / nw as f32);
        let (y0, y1) = (y as f32 * h as f32 / nh as f32, (y + 1) as f32 * h as f32 / nh as f32);
        let mut sum = [0.0; 4];
        for sy in y0.floor() as u32..(y1.ceil() as u32).min(h) {
            for sx in x0.floor() as u32..(x1.ceil() as u32).min(w) {
                let weight = ((sx + 1) as f32).min(x1) - (sx as f32).max(x0);
                let weight = weight * (((sy + 1) as f32).min(y1) - (sy as f32).max(y0));
                let p = linear(&src[(sy * w + sx) as usize]);
                for c in 0..4 { sum[c] += p[c] * weight; }
            }
        }
        emit(sum, (x1 - x0) * (y1 - y0));
    }}
}
pub(super) fn byte(v: f32) -> u8 { (v.clamp(0.0, 1.0) * 255.0).round() as u8 }

/// sRGB transfer tables, so mip generation runs no `powf` per pixel.
struct Srgb {
    /// Linear light of each sRGB byte.
    decode: [f32; 256],
    /// `up[k]`: the linear value at and above which the sRGB encoding rounds past byte `k`.
    up: [f32; 256],
    /// The byte at the low edge of each 1/`STEPS` of linear light. The curve's steepest
    /// slope (12.92) crosses under one byte per step, so one `up` comparison finishes it.
    coarse: [u8; STEPS + 1],
}
const STEPS: usize = 4096;
static SRGB: std::sync::LazyLock<Srgb> = std::sync::LazyLock::new(|| {
    let mut up: [f32; 256] = std::array::from_fn(|k| crate::color::srgb_decode((k as f32 + 0.5) / 255.0));
    up[255] = f32::INFINITY;
    Srgb {
        decode: std::array::from_fn(|i| crate::color::srgb_decode(i as f32 / 255.0)),
        coarse: std::array::from_fn(|i| up.partition_point(|&t| t <= i as f32 / STEPS as f32) as u8),
        up,
    }
});
impl Srgb {
    /// The sRGB byte of premultiplied linear `v`, capped by its quantized alpha `a` so a pixel
    /// whose alpha rounded to zero emits no light.
    fn encode(&self, v: f32, a: u8) -> u8 {
        let v = v.clamp(0.0, a as f32 / 255.0);
        let b = self.coarse[(v * STEPS as f32) as usize];
        b + u8::from(v >= self.up[b as usize])
    }
}
