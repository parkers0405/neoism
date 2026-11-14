//! [`Cursor::Image`](crate::Cursor::Image) pixels for the OS: decoding, scaling, and a bounded
//! cache of the OS cursors made from them. On web the browser decodes and scales the PNG
//! itself, from the CSS [`css`] writes.

use crate::cursor::CursorImage;

/// Pixels per point in a [`CursorImage`] PNG.
pub(crate) const SOURCE_SCALE: f32 = 2.0;
/// The largest side a cursor PNG may have, in pixels.
const MAX_SIDE: u32 = 256;
pub(crate) const MAX_ENTRIES: usize = 32;
#[cfg(not(target_arch = "wasm32"))]
const MAX_BYTES: usize = 8 << 20;

/// A PNG's size in pixels, read from its header.
#[cfg(any(target_arch = "wasm32", test))]
fn png_size(bytes: &[u8]) -> Option<[u32; 2]> {
    let header = bytes.get(..24)?;
    let side = |at: usize| u32::from_be_bytes(header[at..at + 4].try_into().unwrap());
    (header[..8] == *b"\x89PNG\r\n\x1a\n" && header[12..16] == *b"IHDR").then(|| [side(16), side(20)])
}

/// The CSS `cursor` value for `image` loaded from `url`: the PNG at its 2x, so the browser draws
/// it at its size in points on any display, with the hot spot clamped into it in CSS pixels.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn css(image: CursorImage, url: &str) -> Result<String, String> {
    let [w, h] = png_size(image.bytes()).ok_or("not a PNG")?;
    if w > MAX_SIDE || h > MAX_SIDE || w == 0 || h == 0 {
        return Err(format!("{w}×{h} px is empty or over the {MAX_SIDE} px limit"));
    }
    let [x, y] = [(image.hot()[0], w), (image.hot()[1], h)]
        .map(|(p, side)| (p.floor().max(0.0) as u32).min((side as f32 / SOURCE_SCALE).ceil() as u32 - 1));
    Ok(format!("image-set(url(\"{url}\") {SOURCE_SCALE}x) {x} {y}, auto"))
}

/// A cursor image at `scale` pixels per point: straight-alpha RGBA, hot spot in pixels.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct Decoded {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub hot: [u32; 2],
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub scale: f32,
}

/// Decodes `image` and resamples it to `scale` pixels per point (the OS cursor's own scale),
/// clamping its hot spot into the result.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn decode(image: CursorImage, scale: f32) -> Result<Decoded, String> {
    let src = ::image::load_from_memory_with_format(image.bytes(), ::image::ImageFormat::Png).map_err(|e| e.to_string())?.into_rgba8();
    let (w, h) = src.dimensions();
    if w > MAX_SIDE || h > MAX_SIDE {
        return Err(format!("{w}×{h} px is over the {MAX_SIDE} px limit"));
    }
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { SOURCE_SCALE };
    let k = scale / SOURCE_SCALE;
    let (width, height) = (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1));
    let rgba = if (width, height) == (w, h) { src.into_raw() } else { resample(src, width, height) };
    let hot = [(image.hot()[0], width), (image.hot()[1], height)].map(|(p, side)| ((p * scale).floor().max(0.0) as u32).min(side - 1));
    Ok(Decoded { rgba, width, height, hot, scale })
}

/// Resizes in premultiplied alpha, so transparent pixels' colour doesn't bleed into edges.
#[cfg(not(target_arch = "wasm32"))]
fn resample(mut src: ::image::RgbaImage, width: u32, height: u32) -> Vec<u8> {
    for p in src.pixels_mut() {
        let a = p[3] as u32;
        for c in &mut p.0[..3] { *c = ((*c as u32 * a + 127) / 255) as u8; }
    }
    let mut out = ::image::imageops::resize(&src, width, height, ::image::imageops::FilterType::Triangle);
    for p in out.pixels_mut() {
        let a = p[3] as u32;
        for c in &mut p.0[..3] { *c = if a == 0 { 0 } else { ((*c as u32 * 255 + a / 2) / a).min(255) as u8 }; }
    }
    out.into_raw()
}

#[cfg(not(target_arch = "wasm32"))]
struct Entry<T> {
    key: (CursorImage, u32),
    /// `None` when the image didn't decode or the OS refused it: the arrow shows.
    cursor: Option<T>,
    bytes: usize,
}

/// OS cursors made from cursor images, oldest first, bounded in entries and decoded bytes.
/// A failure is kept too, so it's decoded and logged once.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct ImageCursors<T> {
    entries: Vec<Entry<T>>,
    bytes: usize,
    pub decodes: usize,
}

#[cfg(not(target_arch = "wasm32"))]
impl<T: Clone> ImageCursors<T> {
    pub const fn new() -> Self {
        ImageCursors { entries: Vec::new(), bytes: 0, decodes: 0 }
    }

    /// `Some` once `image` at `scale` has been made: its cursor, or `None` if it failed.
    pub fn cached(&mut self, image: CursorImage, scale: f32) -> Option<Option<T>> {
        let key = (image, scale.to_bits());
        let i = self.entries.iter().position(|e| e.key == key)?;
        let entry = self.entries.remove(i);
        let cursor = entry.cursor.clone();
        self.entries.push(entry);
        Some(cursor)
    }

    /// The OS cursor for `image` at `scale`, decoded and handed to `make` the first time.
    pub fn get(&mut self, image: CursorImage, scale: f32, make: impl FnOnce(&Decoded) -> Result<T, String>) -> Option<T> {
        if let Some(cursor) = self.cached(image, scale) { return cursor; }
        self.decodes += 1;
        let made = decode(image, scale).and_then(|d| make(&d).map(|cursor| (cursor, d.rgba.len())));
        let (cursor, bytes) = match made {
            Ok((cursor, bytes)) => (Some(cursor), bytes),
            Err(problem) => {
                eprintln!("rune: cursor image ({} bytes) shows the arrow instead: {problem}", image.bytes().len());
                (None, 0)
            }
        };
        self.bytes += bytes;
        self.entries.push(Entry { key: (image, scale.to_bits()), cursor: cursor.clone(), bytes });
        while self.entries.len() > 1 && (self.entries.len() > MAX_ENTRIES || self.bytes > MAX_BYTES) {
            self.bytes -= self.entries.remove(0).bytes;
        }
        cursor
    }
}

#[cfg(test)]
#[path = "image_tests.rs"]
mod tests;
