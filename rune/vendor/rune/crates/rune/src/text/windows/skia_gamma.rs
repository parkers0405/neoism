//! Skia's text gamma and contrast (`SkMaskGamma`), as Chromium configures them on Windows: sRGB
//! device gamma and a contrast of 1. A glyph's coverage goes through a table picked by its
//! colour's luminance, quantised to three bits, before it is blended in encoded sRGB.

use crate::color::{Color, srgb_encode};

/// Chromium's `SK_GAMMA_CONTRAST` on Windows.
const CONTRAST: f32 = 1.0;

/// The luminance bucket (0–7) of text drawn in `c`, as Skia picks a grayscale mask's table:
/// Rec. 601 luma of the 8-bit sRGB colour, top three bits. Alpha plays no part.
pub(crate) fn bucket(c: Color) -> u8 {
    let byte = |v: f32| (srgb_encode(v.clamp(0.0, 1.0)) * 255.0).round() as u32;
    let lum = (byte(c.r) * 54 + byte(c.g) * 183 + byte(c.b) * 19) >> 8;
    (lum >> 5) as u8
}

/// The coverage table of luminance `bucket` (`SkTMaskGamma_build_correcting_lut`).
pub(crate) fn table(bucket: u8) -> [u8; 256] {
    let src = scale255(bucket) as f32 / 255.0;
    let dst = 1.0 - src;
    let (lin_src, lin_dst) = (to_linear(src), to_linear(dst));
    // The contrast tapers to nothing as the text approaches white.
    let contrast = CONTRAST * lin_dst;
    std::array::from_fn(|i| {
        let raw = i as f32 / 255.0;
        let a = raw + (1.0 - raw) * contrast * raw;
        let v = if (src - dst).abs() < 1.0 / 256.0 {
            a
        } else {
            // The colour a linear blend over the guessed background would show, undone through
            // the encoded blend the GPU does.
            let out = from_linear(lin_src * a + (1.0 - a) * lin_dst);
            (out - dst) / (src - dst)
        };
        (255.0 * v + 0.5).floor().clamp(0.0, 255.0) as u8
    })
}

/// A 3-bit bucket widened to a byte by repeating its bits (`sk_t_scale255<3>`).
fn scale255(bucket: u8) -> u32 {
    let base = (bucket as u32 & 7) << 5;
    base | base >> 3 | base >> 6
}

fn to_linear(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn from_linear(v: f32) -> f32 {
    if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}
