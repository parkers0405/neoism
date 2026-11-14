//! AVIF stills (`avif` feature), decoded by rav1d, a Rust port of dav1d, as straight-alpha
//! sRGB RGBA8. An animated AVIF shows its first frame.
use super::ImageError;

/// Whether `bytes` are an ISO-BMFF file of an AVIF brand.
pub(super) fn is_avif(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[4..8] == b"ftyp" && matches!(&bytes[8..12], b"avif" | b"avis")
}

#[cfg(all(feature = "avif", not(target_arch = "wasm32")))]
pub(super) fn rgba(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), ImageError> {
    use avif_decode::Image;
    let error = |e: avif_decode::Error| ImageError(format!("AVIF: {e}"));
    let image = avif_decode::Decoder::from_avif(bytes).map_err(error)?.to_image().map_err(error)?;
    let eight = |v: u16| ((u32::from(v) * 255 + 32767) / 65535) as u8;
    let (w, h, pixels): (usize, usize, Vec<u8>) = match image {
        Image::Rgb8(i) => (i.width(), i.height(), i.pixels().flat_map(|p| [p.r, p.g, p.b, 255]).collect()),
        Image::Rgba8(i) => (i.width(), i.height(), i.pixels().flat_map(|p| [p.r, p.g, p.b, p.a]).collect()),
        Image::Rgb16(i) => (i.width(), i.height(), i.pixels().flat_map(|p| [eight(p.r), eight(p.g), eight(p.b), 255]).collect()),
        Image::Rgba16(i) => (i.width(), i.height(), i.pixels().flat_map(|p| [eight(p.r), eight(p.g), eight(p.b), eight(p.a)]).collect()),
        Image::Gray8(i) => (i.width(), i.height(), i.pixels().flat_map(|p| { let v = p.value(); [v, v, v, 255] }).collect()),
        Image::Gray16(i) => (i.width(), i.height(), i.pixels().flat_map(|p| { let v = eight(p.value()); [v, v, v, 255] }).collect()),
    };
    if w == 0 || h == 0 || w > 8192 || h > 8192 { return Err(ImageError("AVIF is too large".into())); }
    Ok((w as u32, h as u32, pixels))
}

#[cfg(not(all(feature = "avif", not(target_arch = "wasm32"))))]
pub(super) fn rgba(_: &[u8]) -> Result<(u32, u32, Vec<u8>), ImageError> {
    Err(ImageError("AVIF images need rune's `avif` feature".into()))
}
