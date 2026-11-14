use super::*;
use crate::color::{srgb_decode, srgb_encode};

/// The float mip chain `mip_chain` replaced: every level kept as linear premultiplied `f32`s
/// at full size, encoded with `powf`.
fn reference(width: u32, height: u32, bytes: &[u8]) -> Vec<Mip> {
    let mut linear: Vec<[f32; 4]> = bytes.as_chunks::<4>().0.iter().map(|p| {
        let a = p[3] as f32 / 255.0;
        [srgb_decode(p[0] as f32 / 255.0) * a, srgb_decode(p[1] as f32 / 255.0) * a, srgb_decode(p[2] as f32 / 255.0) * a, a]
    }).collect();
    let (mut w, mut h) = (width, height);
    let mut mips = Vec::new();
    loop {
        let pixels = linear.iter().flat_map(|p| {
            let a = byte(p[3]);
            let rgb = [p[0], p[1], p[2]].map(|v| byte(srgb_encode(v.min(a as f32 / 255.0))));
            [rgb[0], rgb[1], rgb[2], a]
        }).collect();
        mips.push(Mip { width: w, height: h, pixels });
        if w == 1 && h == 1 { break; }
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let mut next = Vec::with_capacity((nw * nh) as usize);
        for y in 0..nh { for x in 0..nw {
            let (x0, x1) = (x as f32 * w as f32 / nw as f32, (x + 1) as f32 * w as f32 / nw as f32);
            let (y0, y1) = (y as f32 * h as f32 / nh as f32, (y + 1) as f32 * h as f32 / nh as f32);
            let mut sum = [0.0; 4];
            for sy in y0.floor() as u32..(y1.ceil() as u32).min(h) {
                for sx in x0.floor() as u32..(x1.ceil() as u32).min(w) {
                    let weight = ((sx + 1) as f32).min(x1) - (sx as f32).max(x0);
                    let weight = weight * (((sy + 1) as f32).min(y1) - (sy as f32).max(y0));
                    for c in 0..4 { sum[c] += linear[(sy * w + sx) as usize][c] * weight; }
                }
            }
            next.push(sum.map(|v| v / ((x1 - x0) * (y1 - y0))));
        }}
        linear = next;
        (w, h) = (nw, nh);
    }
    mips
}

/// Gradients, hashed noise and every alpha from clear to opaque, at odd and even sizes.
fn synthetic(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h { for x in 0..w {
        let n = (x.wrapping_mul(0x9e37_79b9) ^ y.wrapping_mul(0x85eb_ca6b)).wrapping_mul(0xc2b2_ae35) >> 24;
        let alpha = match (x / 7 + y / 5) % 5 { 0 => 255, 1 => 0, 2 => (x * 255 / w) as u8, 3 => n as u8, _ => 1 + (y % 3) as u8 };
        out.extend_from_slice(&[(x * 255 / w) as u8, (y * 255 / h) as u8, n as u8, alpha]);
    }}
    out
}

#[test]
fn the_byte_mip_chain_matches_the_float_one_within_one_per_channel() {
    for (w, h) in [(1, 1), (2, 1), (3, 1), (1, 7), (17, 9), (64, 64), (257, 131), (300, 1)] {
        let src = synthetic(w, h);
        let got = mip_chain(w, h, &src).unwrap().mips;
        let want = reference(w, h, &src);
        assert_eq!(got.len(), want.len(), "{w}x{h}: mip count");
        for (level, (g, r)) in got.iter().zip(&want).enumerate() {
            assert_eq!((g.width, g.height), (r.width, r.height), "{w}x{h} mip {level}: size");
            let worst = g.pixels.iter().zip(&r.pixels).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
            assert!(worst <= 1, "{w}x{h} mip {level}: off by {worst}");
            for (i, p) in g.pixels.as_chunks::<4>().0.iter().enumerate() {
                assert!(p[..3].iter().all(|&c| c == 0) || p[3] > 0, "{w}x{h} mip {level} pixel {i}: light from a clear pixel");
            }
        }
        let opaque = got[0].pixels.as_chunks::<4>().0.iter().zip(src.as_chunks::<4>().0).filter(|(_, s)| s[3] == 255);
        assert!(opaque.into_iter().all(|(g, s)| g == s), "{w}x{h}: an opaque pixel passes through mip 0 unchanged");
    }
}

/// The chain holds its output and nothing near a full-size float copy of the source.
#[test]
fn the_mip_chain_allocates_little_beyond_its_output() {
    let (w, h) = (512, 256);
    let src = synthetic(w, h);
    let (mips, peak) = crate::tooling::test_alloc::peak(|| mip_chain(w, h, &src).unwrap());
    let output: usize = mips.mips.iter().map(|m| m.pixels.len()).sum();
    assert!(peak < output + output / 8, "peak {peak} bytes for {output} bytes of mips");
}

/// Built a slice at a time, as on the browser's main thread, the chain is the same.
#[test]
fn a_mip_chain_built_in_slices_matches_one_built_whole() {
    for (w, h) in [(1, 1), (3, 1), (17, 9), (64, 64), (257, 131)] {
        let src = synthetic(w, h);
        let whole = mip_chain(w, h, &src).unwrap().mips;
        for budget in [1, 7, 100, 5000] {
            let mut steps = mips::MipSteps::new(w, h, &src).unwrap();
            let mut slices = 1;
            while !steps.step(budget) { slices += 1; }
            let sliced = steps.finish().mips;
            assert_eq!(sliced.len(), whole.len(), "{w}x{h} by {budget}: mip count");
            for (level, (s, o)) in sliced.iter().zip(&whole).enumerate() {
                assert!((s.width, s.height, &s.pixels) == (o.width, o.height, &o.pixels), "{w}x{h} by {budget}: mip {level} differs");
            }
            if h >= 4 && budget == 1 { assert!(slices > 1, "{w}x{h}: one row per slice takes several"); }
        }
    }
}
