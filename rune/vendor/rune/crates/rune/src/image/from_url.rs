//! `ImageCache::url`: an image by URL. On web the browser fetches it and decodes it with its
//! own codecs (`createImageBitmap`, so WebP and AVIF too) off the main thread, and the straight
//! RGBA lands through the same mip path as `ImageCache::rgba`. On native a URL without a scheme
//! is a path and a `file:` URL names one; both load like `ImageCache::file`.
use super::{Decoded, Image, ImageCache, ImageError, Input, icon_pixels, mip_chain, svg_rgba};

/// The largest side a fetched image may have, as for decoded files: 8192 pixels, so at most
/// 256 MiB of decoded RGBA before mips.
#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
pub(super) const MAX_SIDE: u32 = 8192;

impl ImageCache {
    /// The image at `url`, shared by URL, size and scale like [`ImageCache::file`] (SVG
    /// rasterizes to fit `size_pt` at `scale`; other formats ignore both). Pending until it
    /// arrives, then the cache's waker redraws.
    ///
    /// On web the browser fetches `url`, relative to the page, and decodes it off the main
    /// thread: PNG, JPEG, GIF's first frame, WebP and AVIF, whatever the browser reads, turned
    /// upright by EXIF orientation. `image/svg+xml` and `.svg`/`.svgz` URLs rasterize with
    /// Rune's SVG renderer. Images over 8192 pixels a side, failed requests and cross-origin
    /// images without CORS resolve to [`Image::error`]. On native, where Rune has no HTTP
    /// client, a path or `file:` URL loads like [`ImageCache::file`] and other URLs fail; fetch
    /// those bytes yourself and call [`ImageCache::load`].
    pub fn url(&self, url: &str, size_pt: f32, scale: f32) -> Image {
        let px = icon_pixels(size_pt, scale);
        let mut hash = blake3::Hasher::new();
        hash.update(b"rune-url\0");
        hash.update(url.as_bytes());
        hash.update(&[0]);
        hash.update(&px.to_le_bytes());
        self.request(*hash.finalize().as_bytes(), Input::Url(url.to_owned(), px))
    }

    /// One image in several sizes, as an HTML `srcset`: `sources` are `(url, width in pixels)`,
    /// and this loads the narrowest at least `size_pt × scale` pixels wide (or the widest), as
    /// [`ImageCache::url`]. Pass the width the image shows at, so a phone fetches the small
    /// file; images are shared by the file chosen, so a window resizing within one file's range
    /// loads nothing new. An `--html` export writes every source, so the browser picks one
    /// too. SVG sources rasterize at their listed width.
    pub fn url_set(&self, sources: &[(&str, u32)], size_pt: f32, scale: f32) -> Image {
        let need = (size_pt * scale).ceil();
        let mut sorted = sources.to_vec();
        sorted.sort_by_key(|s| s.1);
        let Some(&(url, width)) = sorted.iter().find(|s| s.1 as f32 >= need).or(sorted.last()) else {
            return self.url("", size_pt, scale);
        };
        let srcset: String = sorted.iter().map(|(u, w)| format!("{u} {w}w")).collect::<Vec<_>>().join(", ");
        let mut hash = blake3::Hasher::new();
        hash.update(b"rune-url-set\0");
        hash.update(url.as_bytes());
        hash.update(&[0]);
        hash.update(srcset.as_bytes());
        self.request_set(*hash.finalize().as_bytes(), Input::Url(url.to_owned(), width.max(1)), Some(srcset.into()))
    }
}

/// What the browser handed back for a URL.
#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
pub(super) enum Fetched {
    /// Straight-alpha sRGB RGBA8 the browser decoded.
    Rgba(u32, u32, Vec<u8>),
    /// SVG or SVGZ bytes, which Rune rasterizes at `px`.
    Svg(Vec<u8>),
}

/// The mips for a fetched image, or why not, naming the URL.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn land(url: &str, fetched: Fetched, px: u32) -> Result<Decoded, ImageError> {
    match fetched {
        Fetched::Rgba(w, h, bytes) => mip_chain(w, h, &bytes),
        Fetched::Svg(bytes) => svg_rgba(&bytes, px).and_then(|(w, h, bytes)| mip_chain(w, h, &bytes)),
    }
    .map_err(|e| ImageError(format!("{url}: {e}")))
}

/// [`land`] on the browser's main thread, where frames draw too: the mips are built a slice at
/// a time with a task between slices, so no slice holds a frame or input up for long.
#[cfg(target_arch = "wasm32")]
pub(super) async fn land_in_slices(url: &str, fetched: Fetched, px: u32) -> Result<Decoded, ImageError> {
    /// About a few milliseconds of mip work on a phone.
    const SLICE: usize = 1 << 15;
    let named = |e: ImageError| ImageError(format!("{url}: {e}"));
    let (w, h, bytes) = match fetched {
        Fetched::Rgba(w, h, bytes) => (w, h, bytes),
        Fetched::Svg(bytes) => svg_rgba(&bytes, px).map_err(named)?,
    };
    let mut steps = super::mips::MipSteps::new(w, h, &bytes).map_err(named)?;
    while !steps.step(SLICE) {
        let _ = wasm_bindgen_futures::JsFuture::from(crate::platform::web::platform::next_task()).await;
    }
    Ok(steps.finish())
}

/// Fetch and decode `url` in the browser, resolving in a task of its own so each image's mips
/// are a separate piece of main-thread work.
#[cfg(target_arch = "wasm32")]
pub(super) async fn fetch(url: &str) -> Result<Fetched, ImageError> {
    browser_reply(url, crate::platform::web::platform::fetch_image(url, MAX_SIDE)).await
}

/// Encoded image bytes decoded by the browser's own codecs, off the main thread, so a web build
/// carries no JPEG decoder of its own.
#[cfg(target_arch = "wasm32")]
pub(super) async fn decode_in_browser(bytes: &[u8]) -> Result<Fetched, ImageError> {
    browser_reply("image", crate::platform::web::platform::decode_image(bytes, MAX_SIDE)).await
}

#[cfg(target_arch = "wasm32")]
async fn browser_reply(url: &str, promise: js_sys::Promise) -> Result<Fetched, ImageError> {
    use wasm_bindgen::JsCast;
    let reply = wasm_bindgen_futures::JsFuture::from(promise).await.map_err(|e| {
        let why = e.dyn_ref::<js_sys::Error>().map_or_else(|| format!("{e:?}"), |e| String::from(e.message()));
        ImageError(format!("{url}: {why}"))
    })?;
    let reply: js_sys::Array = reply.unchecked_into();
    let side = |i| reply.get(i).as_f64().unwrap_or(0.0) as u32;
    let bytes = reply.get(2).unchecked_into::<js_sys::Uint8Array>().to_vec();
    Ok(if side(0) == 0 { Fetched::Svg(bytes) } else { Fetched::Rgba(side(0), side(1), bytes) })
}

/// The file a native `url` names: a path with no scheme, or a `file:` URL.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(super) fn file_path(url: &str) -> Result<std::path::PathBuf, ImageError> {
    let scheme = url.split_once(':').map(|(s, _)| s)
        // A one-letter "scheme" is a Windows drive.
        .filter(|s| s.len() > 1 && s.starts_with(|c: char| c.is_ascii_alphabetic())
            && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')));
    let Some(scheme) = scheme else { return Ok(url.into()) };
    let unsupported = || ImageError(format!("{url}: only web fetches URLs; on native fetch the bytes and call ImageCache::load"));
    if !scheme.eq_ignore_ascii_case("file") { return Err(unsupported()); }
    let rest = &url[scheme.len() + 1..];
    let path = match rest.strip_prefix("//") {
        Some(rest) => rest.strip_prefix("localhost").unwrap_or(rest),
        None => rest,
    };
    if !path.starts_with('/') { return Err(unsupported()); }
    let path = percent_decode(path).ok_or_else(|| ImageError(format!("{url}: not a valid file URL")))?;
    // `file:///C:/x` names `C:/x` on Windows.
    #[cfg(windows)]
    let path = match path.as_bytes() { [b'/', d, b':', ..] if d.is_ascii_alphabetic() => path[1..].to_owned(), _ => path };
    Ok(path.into())
}

/// The bytes of the font file a native `url` names (`App::font_url`).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn font_file(url: &str) -> Result<Vec<u8>, ImageError> {
    let path = file_path(url)?;
    std::fs::read(&path).map_err(|e| ImageError(format!("{}: {e}", path.display())))
}

fn percent_decode(s: &str) -> Option<String> {
    let mut out = Vec::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b != b'%' { out.push(b); continue; }
        let digit = |b: Option<u8>| (b? as char).to_digit(16);
        out.push((digit(bytes.next())? * 16 + digit(bytes.next())?) as u8);
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
#[path = "from_url_tests.rs"]
mod tests;
