//! `--strip`: frames, or their `--crop` rects, tiled into one image.

use super::*;

/// Tiles `--strip`'s frames, `cols` to a row, into one PNG.
pub(crate) fn write_strip(p: &std::path::Path, strip: &[Vec<u8>], (wp, hp): (u32, u32), crops_px: &[[u32; 4]], gap: u32, cols: usize) {
    if strip.is_empty() { return; }
    let (cw, ch) = tile_size(wp, hp, crops_px, gap);
    let cols = strip.len().min(cols) as u32;
    let rows = (strip.len() as u32).div_ceil(cols);
    let (sw, sh) = (cols * cw + (cols - 1) * gap, rows * ch + (rows - 1) * gap);
    let mut img: Vec<u8> = [128, 128, 128, 255].repeat((sw * sh) as usize);
    for (i, f) in strip.iter().enumerate() {
        let (cx, cy) = ((i as u32 % cols) * (cw + gap), (i as u32 / cols) * (ch + gap));
        for y in 0..ch {
            let dst = (((cy + y) * sw + cx) * 4) as usize;
            let src = (y * cw * 4) as usize;
            img[dst..dst + (cw * 4) as usize].copy_from_slice(&f[src..src + (cw * 4) as usize]);
        }
    }
    write_png(p, sw, sh, &img).unwrap_or_else(|e| die(&format!("{}: {e}", p.display())));
}

pub(crate) fn tile_size(w: u32, h: u32, crops: &[[u32; 4]], gap: u32) -> (u32, u32) {
    if crops.is_empty() {
        return (w, h);
    }
    let cw = crops.iter().map(|c| c[2]).max().unwrap_or(w);
    let ch = crops.iter().map(|c| c[3]).sum::<u32>() + gap * (crops.len() as u32 - 1);
    (cw, ch)
}

/// The crops of one frame stacked vertically (the whole frame when there are none).
pub(crate) fn tile(px: &[u8], w: u32, h: u32, crops: &[[u32; 4]], gap: u32) -> Vec<u8> {
    if crops.is_empty() {
        return px.to_vec();
    }
    let (tw, th) = tile_size(w, h, crops, gap);
    let mut out: Vec<u8> = [128, 128, 128, 255].repeat((tw * th) as usize);
    let mut oy = 0;
    for &[x, y, cw, ch] in crops {
        assert!(x + cw <= w && y + ch <= h, "--crop outside the frame");
        for row in 0..ch {
            let s = (((y + row) * w + x) * 4) as usize;
            let d = (((oy + row) * tw) * 4) as usize;
            out[d..d + (cw * 4) as usize].copy_from_slice(&px[s..s + (cw * 4) as usize]);
        }
        oy += ch + gap;
    }
    out
}
