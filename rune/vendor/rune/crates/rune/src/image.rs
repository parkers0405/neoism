//! Content-addressed raster images. Decode and linear-light mip generation run on one
//! sleeping worker per cache, never in a view or a render frame.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc;

pub type ImageId = [u8; 32];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageError(pub String);

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) }
}
impl std::error::Error for ImageError {}

/// Pixels are sRGB-encoded *linear-premultiplied* RGB plus linear alpha. Sampling with
/// Rgba8UnormSrgb yields linear-premultiplied RGBA, including between mip levels.
pub(crate) struct Mip {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}
pub(crate) struct Decoded {
    /// The image, or an animation's first frame.
    pub mips: Vec<Mip>,
    /// An animation's other frames and timing.
    pub motion: Option<Box<animated::Motion>>,
}
struct State {
    id: ImageId,
    pixels: crate::Pixels,
    decoded: OnceLock<Result<Arc<Decoded>, ImageError>>,
}

/// A cheap shared handle. Pending images paint nothing until their worker wakes the app.
#[derive(Clone)]
pub struct Image(Arc<State>, crate::Pixels);
impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Image").field("id", &self.id()).field("size", &self.size()).finish()
    }
}
impl Image {
    /// A handle on `state`, whose pixels keep it in the cache while drawn.
    fn new(state: Arc<State>) -> Image {
        let pixels = state.pixels.clone().keeping(state.clone());
        Image(state, pixels)
    }

    /// The shared static pixel handle, ready to pass to `El::pixels` after decoding. While
    /// any clone of it lives (an element drawing it), the cache hands back this image.
    pub fn pixels(&self) -> &crate::Pixels { &self.1 }
    pub fn id(&self) -> ImageId { self.0.id }
    pub fn size(&self) -> Option<[u32; 2]> {
        let mip = self.decoded()?.ok()?.mips.first()?;
        Some([mip.width, mip.height])
    }
    pub fn is_ready(&self) -> bool { self.0.decoded.get().is_some() }
    pub fn error(&self) -> Option<&ImageError> { self.0.decoded.get()?.as_ref().err() }
    /// Wait for fixture assets before advancing a deterministic headless scene. Interactive
    /// apps use the cache's waker instead; decode still happens on the cache worker.
    pub fn wait(&self) -> Result<[u32; 2], &ImageError> {
        #[cfg(target_arch = "wasm32")]
        let decoded = self.0.decoded.get().expect("Image::wait cannot block on web; use is_ready and the cache waker");
        #[cfg(not(target_arch = "wasm32"))]
        let decoded = self.0.decoded.wait();
        let mip = &decoded.as_ref()?.mips[0];
        Ok([mip.width, mip.height])
    }
    pub(crate) fn decoded(&self) -> Option<Result<&Decoded, &ImageError>> {
        self.0.decoded.get().map(|r| r.as_ref().map(Arc::as_ref))
    }
    // The handle is ready before its pixels wake any window, so a woken frame sees its size.
    fn finish(&self, decoded: Result<Decoded, ImageError>) {
        let decoded = decoded.map(Arc::new);
        let mips = decoded.as_ref().ok().cloned();
        let _ = self.0.decoded.set(decoded);
        if let Some(mips) = mips { self.0.pixels.set_mips(mips); }
    }
}

enum Input {
    Encoded(Arc<[u8]>),
    Rgba(u32, u32, Arc<[u8]>),
    Icon(std::path::PathBuf, u32),
    Svg(Arc<[u8]>, u32),
    #[cfg(not(target_arch = "wasm32"))]
    File(std::path::PathBuf, u32),
    /// `ImageCache::url`, and `ImageCache::file` on web.
    Url(String, u32),
    /// A decoder that panics, standing in for a codec bug.
    #[cfg(test)]
    Panic,
}

impl Input {
    fn decode(self) -> Result<Decoded, ImageError> {
        match self {
            Input::Encoded(bytes) => decode(&bytes),
            Input::Rgba(w, h, bytes) => mip_chain(w, h, &bytes),
            Input::Svg(bytes, px) => svg_rgba(&bytes, px).and_then(|(w, h, bytes)| mip_chain(w, h, &bytes)),
            #[cfg(not(target_arch = "wasm32"))]
            Input::Icon(path, px) => app_icon_rgba(&path, px).and_then(|(w, h, bytes)| mip_chain(w, h, &bytes)),
            #[cfg(not(target_arch = "wasm32"))]
            Input::File(path, px) => {
                let bytes = std::fs::read(&path).map_err(|e| ImageError(format!("{}: {e}", path.display())))?;
                if has_svg_extension(&path) || is_svg(&bytes) {
                    svg_rgba(&bytes, px).and_then(|(w, h, bytes)| mip_chain(w, h, &bytes))
                } else {
                    decode(&bytes)
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            Input::Url(url, px) => Input::File(from_url::file_path(&url)?, px).decode(),
            #[cfg(target_arch = "wasm32")]
            Input::Icon(..) => Err(ImageError("filesystem images are unsupported on web".into())),
            // Fetched asynchronously by `ImageCache::request`.
            #[cfg(target_arch = "wasm32")]
            Input::Url(url, _) => Err(ImageError(format!("{url}: fetched, not decoded"))),
            #[cfg(test)]
            Input::Panic => panic!("decoder bug"),
        }
    }

    /// `decode`, with a panic in a codec (a malformed file can still trip one) failing this
    /// image instead of unwinding the worker, which would leave it pending and fail every later one.
    #[cfg(not(target_arch = "wasm32"))]
    fn decode_caught(self) -> Result<Decoded, ImageError> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.decode())).unwrap_or_else(|panic| {
            let why = panic.downcast_ref::<&str>().copied().or_else(|| panic.downcast_ref::<String>().map(String::as_str));
            Err(ImageError(format!("image decoder panicked: {}", why.unwrap_or("no message"))))
        })
    }
}
struct Job { image: Image, input: Input }
/// Decoder threads per cache, at most one per core.
#[cfg(not(target_arch = "wasm32"))]
const DECODERS: usize = 4;

/// Construct once during app setup. The cache deduplicates by content, not a path or pointer;
/// dropping all handles releases decoded pixels. Dropping the cache stops its worker after
/// queued requests complete. The waker should be the one supplied by `AppSpec::setup`.
pub struct ImageCache {
    entries: Mutex<HashMap<ImageId, Weak<State>>>,
    #[cfg(not(target_arch = "wasm32"))]
    jobs: mpsc::Sender<Job>,
    #[cfg(target_arch = "wasm32")]
    wake: crate::Waker,
}
impl ImageCache {
    #[cfg(target_arch = "wasm32")]
    pub fn new(wake: crate::Waker) -> Self {
        Self { entries: Mutex::new(HashMap::new()), wake }
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(waker: crate::Waker) -> Self {
        let (jobs, receive) = mpsc::channel::<Job>();
        // A session opening on several screenshots decodes them side by side, not one by one.
        let receive = Arc::new(Mutex::new(receive));
        for _ in 0..DECODERS.min(std::thread::available_parallelism().map_or(1, |n| n.get())) {
            let (receive, waker) = (receive.clone(), waker.clone());
            std::thread::Builder::new().name("rune-images".into()).spawn(move || {
                loop {
                    let Ok(job) = receive.lock().unwrap_or_else(|e| e.into_inner()).recv() else { return };
                    job.image.finish(job.input.decode_caught());
                    waker();
                }
            }).expect("image worker");
        }
        Self { entries: Mutex::new(HashMap::new()), jobs }
    }

    /// PNG (APNG animates), JPEG, GIF and WebP (both animate), BMP, ICO, AVIF (with the `avif`
    /// feature; its first frame), modern ICNS containing a PNG representation, or SVG (plain or
    /// gzipped SVGZ, recognised by its content). An SVG rasterizes at twice its own size; use
    /// [`ImageCache::svg`] to rasterize it at the size it draws at. Legacy ICNS codecs return an
    /// explicit error; callers can provide platform-decoded RGBA instead. An animation decodes
    /// every frame on the worker, and plays only while an element draws it in a shown window.
    /// On web the browser decodes everything but PNG, SVG and ICNS, so GIF and WebP show their
    /// first frame there.
    pub fn load(&self, bytes: impl Into<Arc<[u8]>>) -> Image {
        let bytes = bytes.into();
        let mut hash = blake3::Hasher::new();
        hash.update(b"rune-encoded\0");
        hash.update(&bytes);
        let id = *hash.finalize().as_bytes();
        self.request(id, Input::Encoded(bytes))
    }

    /// SVG or SVGZ bytes rasterized to fit `size_pt` points square at `scale` device pixels per
    /// point (pass the window's [`Env::scale`](crate::Env)), keeping the drawing's aspect ratio.
    /// Rasterize at the largest size the image draws at: smaller draws use its mips, so 16 pt at
    /// 2× stays crisp. Handles are shared by content, size and scale.
    pub fn svg(&self, bytes: impl Into<Arc<[u8]>>, size_pt: f32, scale: f32) -> Image {
        let bytes = bytes.into();
        let px = icon_pixels(size_pt, scale);
        let mut hash = blake3::Hasher::new();
        hash.update(b"rune-svg\0");
        hash.update(&px.to_le_bytes());
        hash.update(&bytes);
        self.request(*hash.finalize().as_bytes(), Input::Svg(bytes, px))
    }

    /// Bytes with a MIME type, as from a data URL or a `Content-Type` header: `image/svg+xml`
    /// rasterizes like [`ImageCache::svg`], anything else decodes like [`ImageCache::load`].
    pub fn load_mime(&self, bytes: impl Into<Arc<[u8]>>, mime: &str, size_pt: f32, scale: f32) -> Image {
        let essence = mime.split(';').next().unwrap_or("").trim();
        if essence.eq_ignore_ascii_case("image/svg+xml") { self.svg(bytes, size_pt, scale) } else { self.load(bytes) }
    }

    /// An image file, read and decoded on the worker. `.svg` and `.svgz` files (or SVG content)
    /// rasterize like [`ImageCache::svg`]; other formats decode like [`ImageCache::load`], ignoring
    /// the size. Handles are shared by path, size and scale, so an edited file needs a new handle
    /// once the old ones drop. On web the path is a URL relative to the page, fetched as
    /// [`ImageCache::url`] does, so `cache.file("assets/logo.webp", ..)` works in both builds.
    pub fn file(&self, path: impl AsRef<std::path::Path>, size_pt: f32, scale: f32) -> Image {
        let path = path.as_ref().to_path_buf();
        let px = icon_pixels(size_pt, scale);
        let mut hash = blake3::Hasher::new();
        hash.update(b"rune-file\0");
        hash.update(path.as_os_str().as_encoded_bytes());
        hash.update(&[0]);
        hash.update(&px.to_le_bytes());
        #[cfg(target_arch = "wasm32")]
        let input = Input::Url(path.to_string_lossy().into_owned(), px);
        #[cfg(not(target_arch = "wasm32"))]
        let input = Input::File(path, px);
        self.request(*hash.finalize().as_bytes(), input)
    }

    /// Straight-alpha sRGB RGBA8. Validation, premultiplication and mips happen off-thread.
    pub fn rgba(&self, width: u32, height: u32, bytes: impl Into<Arc<[u8]>>) -> Image {
        let bytes = bytes.into();
        let mut hash = blake3::Hasher::new();
        hash.update(b"rune-rgba8\0");
        hash.update(&width.to_le_bytes());
        hash.update(&height.to_le_bytes());
        hash.update(&bytes);
        self.request(*hash.finalize().as_bytes(), Input::Rgba(width, height, bytes))
    }

    /// The Finder icon for an app bundle or any file path, rasterized at 2× `size_pt`.
    /// See [`ImageCache::app_icon_scaled`].
    pub fn app_icon(&self, path: impl AsRef<std::path::Path>, size_pt: f32) -> Image {
        self.app_icon_scaled(path, size_pt, 2.0)
    }

    /// The Finder icon for `path`, rasterized at `size_pt * scale` pixels square (pass the
    /// window's display scale). Handles are shared by (path, size, scale) rather than by
    /// content, so repeated calls never re-rasterize while a handle is alive. Lookup and
    /// rasterization run on the cache worker like any decode, so this is safe to call from a view.
    /// Missing paths still get the system's generic document icon on macOS. Windows reads an
    /// executable's or icon file's first icon; Linux reads a `.desktop` entry's `Icon=` from the
    /// hicolor theme or pixmaps (PNG or SVG), or an image file itself. Paths without an icon
    /// resolve to an error, so callers can draw their own fallback.
    pub fn app_icon_scaled(&self, path: impl AsRef<std::path::Path>, size_pt: f32, scale: f32) -> Image {
        let path = path.as_ref().to_path_buf();
        let px = icon_pixels(size_pt, scale);
        let mut hash = blake3::Hasher::new();
        hash.update(b"rune-app-icon\0");
        hash.update(path.as_os_str().as_encoded_bytes());
        hash.update(&[0]);
        hash.update(&size_pt.to_bits().to_le_bytes());
        hash.update(&scale.to_bits().to_le_bytes());
        self.request(*hash.finalize().as_bytes(), Input::Icon(path, px))
    }

    fn request(&self, id: ImageId, input: Input) -> Image {
        self.request_set(id, input, None)
    }

    fn request_set(&self, id: ImageId, input: Input, srcset: Option<Arc<str>>) -> Image {
        let mut entries = self.entries.lock().unwrap();
        if let Some(state) = entries.get(&id).and_then(Weak::upgrade) { return Image::new(state); }
        entries.retain(|_, value| value.strong_count() > 0);
        let url: Option<Arc<str>> = match &input {
            Input::Url(url, _) => Some(url.as_str().into()),
            #[cfg(not(target_arch = "wasm32"))]
            Input::File(path, _) => Some(path.to_string_lossy().into()),
            _ => None,
        };
        let image = Image::new(Arc::new(State { id, pixels: crate::Pixels::from_url(url, srcset), decoded: OnceLock::new() }));
        entries.insert(id, Arc::downgrade(&image.0));
        #[cfg(not(target_arch = "wasm32"))]
        if self.jobs.send(Job { image: image.clone(), input }).is_err() {
            let _ = image.0.decoded.set(Err(ImageError("image worker stopped".into())));
        }
        #[cfg(target_arch = "wasm32")]
        {
            // The task holds the image weakly, so a request dropped before it runs (a view's
            // guess, replaced in the same frame once layout knows the size) fetches nothing,
            // and one dropped while its bytes download is never decoded.
            let weak = Arc::downgrade(&image.0);
            let wake = self.wake.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let decoded = match input {
                    Input::Url(url, px) => {
                        if !wanted(&weak) { return; }
                        let fetched = from_url::fetch(&url).await;
                        if !wanted(&weak) { return; }
                        match fetched { Ok(fetched) => from_url::land_in_slices(&url, fetched, px).await, Err(e) => Err(e) }
                    }
                    Input::Encoded(bytes) if browser_decodes(&bytes) => {
                        let decoded = from_url::decode_in_browser(&bytes).await;
                        if !wanted(&weak) { return; }
                        match decoded { Ok(fetched) => from_url::land_in_slices("image", fetched, 0).await, Err(e) => Err(e) }
                    }
                    input => input.decode(),
                };
                let Some(state) = weak.upgrade() else { return };
                Image::new(state).finish(decoded);
                wake();
            });
        }
        image
    }
}

/// Encoded bytes a web build hands to the browser's codecs: everything but PNG (decoded exactly,
/// alpha and all, by Rune), SVG and ICNS.
#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
fn browser_decodes(bytes: &[u8]) -> bool {
    !bytes.starts_with(b"\x89PNG\r\n\x1a\n") && !bytes.starts_with(b"icns") && !is_svg(bytes)
}

/// Whether anything still holds the image a fetch is for.
#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
fn wanted(state: &std::sync::Weak<State>) -> bool { state.strong_count() > 0 }

fn icon_pixels(size_pt: f32, scale: f32) -> u32 {
    let px = (size_pt * scale).ceil();
    if px.is_finite() { px.clamp(1.0, 1024.0) as u32 } else { 1 }
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn has_svg_extension(path: &std::path::Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("svg") || e.eq_ignore_ascii_case("svgz"))
}

/// Gzip (SVGZ), or markup after an optional BOM and whitespace. No raster format starts with `<`.
fn is_svg(bytes: &[u8]) -> bool {
    if bytes.starts_with(&[0x1f, 0x8b]) { return true; }
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    bytes.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'<')
}

/// Rasterize the platform icon for an app bundle or file at about `px`×`px`, as straight-alpha
/// sRGB RGBA8 ready for [`ImageCache::rgba`]. Most code wants [`ImageCache::app_icon`], which
/// calls this on its worker and caches the result.
///
/// macOS: `NSWorkspace iconForFile:` drawn into a thread-local `NSBitmapImageRep` context.
/// Neither needs the main thread: `NSWorkspace` is not main-actor isolated in the SDK, and
/// AppKit allows drawing an `NSImage` into a bitmap context owned by a secondary thread.
/// Windows: the file's first icon from `PrivateExtractIconsW` at `px` (at most 256).
/// Linux: the closest theme size at or above `px`, or an SVG rasterized to fit `px`; a
/// non-square pixmap keeps its aspect ratio.
pub fn app_icon_rgba(path: &std::path::Path, px: u32) -> Result<(u32, u32, Vec<u8>), ImageError> {
    if px == 0 || px > 1024 { return Err(ImageError("app icon size must be 1..=1024 pixels".into())); }
    #[cfg(target_os = "macos")]
    { macos_icon(path, px) }
    #[cfg(target_os = "windows")]
    { windows_icon(path, px) }
    #[cfg(target_os = "linux")]
    { linux_icon(path, px) }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    { let _ = path; Err(ImageError("app_icon is unsupported on this platform".into())) }
}

#[cfg(target_os = "windows")]
fn windows_icon(path: &std::path::Path, px: u32) -> Result<(u32, u32, Vec<u8>), ImageError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDIBits, GetObjectW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO, PrivateExtractIconsW};
    let err = |what: &str| ImageError(format!("{what}: {}", path.display()));
    // PrivateExtractIconsW reads a fixed MAX_PATH buffer.
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    if wide.len() > 260 { return Err(err("icon path is longer than MAX_PATH")); }
    let mut buffer = [0u16; 260];
    buffer[..wide.len()].copy_from_slice(&wide);
    let side = px.min(256) as i32;
    let mut icon: HICON = std::ptr::null_mut();
    let extracted = unsafe { PrivateExtractIconsW(buffer.as_ptr(), 0, side, side, &mut icon, std::ptr::null_mut(), 1, 0) };
    if extracted == 0 || extracted == u32::MAX || icon.is_null() { return Err(err("no icon")); }
    let mut info: ICONINFO = unsafe { std::mem::zeroed() };
    if unsafe { GetIconInfo(icon, &mut info) } == 0 {
        unsafe { DestroyIcon(icon) };
        return Err(err("unreadable icon"));
    }
    let result = (|| {
        if info.hbmColor.is_null() { return Err(err("monochrome icon")); }
        let mut bitmap: BITMAP = unsafe { std::mem::zeroed() };
        if unsafe { GetObjectW(info.hbmColor, std::mem::size_of::<BITMAP>() as i32, (&raw mut bitmap).cast()) } == 0 {
            return Err(err("unreadable icon bitmap"));
        }
        let (w, h) = (bitmap.bmWidth, bitmap.bmHeight);
        if w <= 0 || h <= 0 || w > 1024 || h > 1024 { return Err(err("icon bitmap has no pixels")); }
        let mut header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // negative: rows top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..unsafe { std::mem::zeroed() }
            },
            ..unsafe { std::mem::zeroed() }
        };
        let mut pixels = vec![0u8; w as usize * h as usize * 4];
        let dc = unsafe { CreateCompatibleDC(std::ptr::null_mut()) };
        let rows = unsafe { GetDIBits(dc, info.hbmColor, 0, h as u32, pixels.as_mut_ptr().cast(), &mut header, DIB_RGB_COLORS) };
        unsafe { DeleteDC(dc) };
        if rows == 0 { return Err(err("unreadable icon pixels")); }
        // BGRA with straight alpha; icons without an alpha channel leave it zero and draw opaque.
        let opaque = pixels.as_chunks::<4>().0.iter().all(|p| p[3] == 0);
        for p in pixels.as_chunks_mut::<4>().0 {
            p.swap(0, 2);
            if opaque { p[3] = 255; }
        }
        Ok((w as u32, h as u32, pixels))
    })();
    unsafe {
        if !info.hbmColor.is_null() { DeleteObject(info.hbmColor); }
        if !info.hbmMask.is_null() { DeleteObject(info.hbmMask); }
        DestroyIcon(icon);
    }
    result
}

#[cfg(target_os = "linux")]
fn linux_icon(path: &std::path::Path, px: u32) -> Result<(u32, u32, Vec<u8>), ImageError> {
    let file = if path.extension().is_some_and(|e| e == "desktop") {
        freedesktop::icon_file(path, px, &freedesktop::data_dirs())
            .ok_or_else(|| ImageError(format!("no icon for {}", path.display())))?
    } else { path.to_path_buf() };
    let bytes = std::fs::read(&file).map_err(|e| ImageError(format!("{}: {e}", file.display())))?;
    if bytes.len() > 16 << 20 { return Err(ImageError(format!("{} is too large for an icon", file.display()))); }
    if has_svg_extension(&file) { return svg_rgba(&bytes, px); }
    let image = image::load_from_memory(&bytes).map_err(|e| ImageError(format!("{}: {e}", file.display())))?.into_rgba8();
    if image.width() > 1024 || image.height() > 1024 { return Err(ImageError(format!("{} is too large for an icon", file.display()))); }
    Ok((image.width(), image.height(), image.into_raw()))
}

/// An SVG scaled to fit `px` square, keeping its aspect ratio, as straight-alpha sRGB RGBA8.
/// `px == 0` rasterizes at twice the drawing's own size, at most 4096 pixels per side.
#[cfg(feature = "svg")]
pub(crate) fn svg_rgba(bytes: &[u8], px: u32) -> Result<(u32, u32, Vec<u8>), ImageError> {
    use resvg::{tiny_skia, usvg};
    if bytes.len() > 32 << 20 { return Err(ImageError("SVG is too large".into())); }
    let tree = usvg::Tree::from_data(bytes, &usvg::Options::default()).map_err(|e| ImageError(format!("SVG: {e}")))?;
    let size = tree.size();
    let px = if px == 0 { (size.width().max(size.height()) * 2.0).ceil().clamp(1.0, 4096.0) as u32 } else { px };
    let k = px as f32 / size.width().max(size.height());
    let (w, h) = (((size.width() * k).round() as u32).clamp(1, px), ((size.height() * k).round() as u32).clamp(1, px));
    let mut pixmap = tiny_skia::Pixmap::new(w, h).ok_or_else(|| ImageError("empty SVG".into()))?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(k, k), &mut pixmap.as_mut());
    Ok((w, h, pixmap.pixels().iter().flat_map(|p| {
        let c = p.demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    }).collect()))
}

#[cfg(not(feature = "svg"))]
pub(crate) fn svg_rgba(_: &[u8], _: u32) -> Result<(u32, u32, Vec<u8>), ImageError> {
    Err(ImageError("SVG images need rune's `svg` feature".into()))
}

/// Freedesktop icon lookup: a `.desktop` entry's `Icon=` as an absolute path, or by name in the
/// hicolor theme (every size directory, `@2` included) and then `pixmaps`.
#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod freedesktop {
    use std::path::{Path, PathBuf};

    pub(super) fn data_dirs() -> Vec<PathBuf> {
        let home = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
        let system = std::env::var("XDG_DATA_DIRS").ok().filter(|dirs| !dirs.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        home.into_iter().chain(system.split(':').filter(|d| !d.is_empty()).map(PathBuf::from)).collect()
    }

    /// The `Icon=` key of the `[Desktop Entry]` group.
    pub(super) fn icon_name(entry: &str) -> Option<&str> {
        let mut group = "";
        for line in entry.lines().map(str::trim) {
            if line.starts_with('[') { group = line; continue; }
            if group != "[Desktop Entry]" { continue; }
            if let Some((key, value)) = line.split_once('=') && key.trim() == "Icon" {
                return Some(value.trim()).filter(|v| !v.is_empty());
            }
        }
        None
    }

    pub(super) fn icon_file(entry: &Path, px: u32, data_dirs: &[PathBuf]) -> Option<PathBuf> {
        let text = std::fs::read_to_string(entry).ok()?;
        let name = icon_name(&text)?;
        if Path::new(name).is_absolute() { return Path::new(name).is_file().then(|| name.into()); }
        let name = name.strip_suffix(".png").or_else(|| name.strip_suffix(".svg")).unwrap_or(name);
        // (effective pixel size, path); scalable sorts as "any size".
        let mut found: Vec<(Option<u32>, PathBuf)> = Vec::new();
        for dir in data_dirs {
            let Ok(sizes) = std::fs::read_dir(dir.join("icons/hicolor")) else { continue };
            for size in sizes.flatten() {
                let label = size.file_name().to_string_lossy().into_owned();
                let effective = if label == "scalable" { None } else {
                    let (dims, scale) = label.split_once('@').unwrap_or((&label, "1"));
                    let size = dims.split_once('x').and_then(|(w, _)| w.parse::<u32>().ok());
                    let Some(n) = size.zip(scale.parse::<u32>().ok()).map(|(w, s)| w * s.max(1)) else { continue };
                    Some(n)
                };
                for ext in ["png", "svg"] {
                    let file = size.path().join("apps").join(format!("{name}.{ext}"));
                    if file.is_file() { found.push((effective, file)); }
                }
            }
        }
        // Smallest bitmap that covers `px`, else scalable, else the largest bitmap.
        let covering = found.iter().filter(|(n, _)| n.is_some_and(|n| n >= px)).min_by_key(|(n, _)| *n);
        let scalable = found.iter().find(|(n, _)| n.is_none());
        let largest = found.iter().max_by_key(|(n, _)| *n);
        if let Some((_, file)) = covering.or(scalable).or(largest) { return Some(file.clone()); }
        data_dirs.iter().flat_map(|dir| ["png", "svg"].map(|ext| dir.join(format!("pixmaps/{name}.{ext}")))).find(|f| f.is_file())
    }
}

#[cfg(target_os = "macos")]
fn macos_icon(path: &std::path::Path, px: u32) -> Result<(u32, u32, Vec<u8>), ImageError> {
    use objc2::runtime::{AnyObject, Bool};
    use objc2::msg_send;
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    let path = path.to_str().ok_or_else(|| ImageError("app icon path is not UTF-8".into()))?;
    let workspace = macos_class("NSWorkspace")?;
    objc2::rc::autoreleasepool(|_| unsafe {
        let workspace: *mut AnyObject = msg_send![workspace, sharedWorkspace];
        let icon: *mut AnyObject = msg_send![workspace, iconForFile: &*NSString::from_str(path)];
        if icon.is_null() { return Err(ImageError("NSWorkspace returned no icon".into())); }
        let zero = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0));
        // IconServices picks its rendition by the size asked for, and on macOS 26 and later a
        // small size of an app without a modern icon is the "legacy" one: a shrunken tile in a
        // grey frame. Draw at 256 pixels or more, then scale down.
        let big = px.max(256);
        // NSCompositingOperationCopy (1); a zero fromRect draws the whole best-fit representation.
        let large = macos_bitmap(big, |_, rect| { let _: () = msg_send![icon, drawInRect: rect fromRect: zero operation: 1usize fraction: 1.0f64]; })?;
        let small = if big == px { large } else {
            macos_bitmap(px, |ctx, rect| {
                // NSImageInterpolationHigh.
                let _: () = msg_send![ctx, setImageInterpolation: 3usize];
                let none: *mut AnyObject = std::ptr::null_mut();
                let _: Bool = msg_send![large, drawInRect: rect fromRect: zero operation: 1usize fraction: 1.0f64 respectFlipped: Bool::NO hints: none];
            })?
        };
        let side = px as isize;
        let data: *const u8 = msg_send![small, bitmapData];
        let stride: isize = msg_send![small, bytesPerRow];
        if data.is_null() || stride < side * 4 { return Err(ImageError("icon bitmap has no pixel data".into())); }
        let mut out = Vec::with_capacity((px * px * 4) as usize);
        for y in 0..side {
            let row = std::slice::from_raw_parts(data.offset(y * stride), (side * 4) as usize);
            for p in row.chunks_exact(4) {
                let a = p[3];
                let straight = |c: u8| if a == 0 { 0 } else { ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8 };
                out.extend_from_slice(&[straight(p[0]), straight(p[1]), straight(p[2]), a]);
            }
        }
        Ok((px, px, out))
    })
}

#[cfg(target_os = "macos")]
fn macos_class(name: &str) -> Result<&'static objc2::runtime::AnyClass, ImageError> {
    objc2::runtime::AnyClass::get(name).ok_or_else(|| ImageError(format!("{name} is unavailable")))
}

/// A `side`×`side` sRGB bitmap with `draw(context, rect)` run into it as the current context.
/// Autoreleased: call inside an autorelease pool.
#[cfg(target_os = "macos")]
unsafe fn macos_bitmap(
    side: u32,
    draw: impl FnOnce(*mut objc2::runtime::AnyObject, objc2_foundation::NSRect),
) -> Result<*mut objc2::runtime::AnyObject, ImageError> {
    use objc2::runtime::{AnyObject, Bool};
    use objc2::rc::{Allocated, Retained};
    use objc2::{msg_send, msg_send_id};
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    let (bitmap, context, color_space) = (macos_class("NSBitmapImageRep")?, macos_class("NSGraphicsContext")?, macos_class("NSColorSpace")?);
    let n = side as isize;
    // SAFETY: AppKit bitmap and context calls on objects this function retains or that live in
    // the caller's autorelease pool.
    unsafe {
        let rep: Allocated<AnyObject> = msg_send_id![bitmap, alloc];
        let null: *mut *mut u8 = std::ptr::null_mut();
        // Default bitmapFormat is premultiplied alpha-last RGBA, the only layout AppKit draws into.
        let rep: Option<Retained<AnyObject>> = msg_send_id![rep,
            initWithBitmapDataPlanes: null pixelsWide: n pixelsHigh: n bitsPerSample: 8isize
            samplesPerPixel: 4isize hasAlpha: Bool::YES isPlanar: Bool::NO
            colorSpaceName: &*NSString::from_str("NSDeviceRGBColorSpace") bitmapFormat: 0usize
            bytesPerRow: n * 4 bitsPerPixel: 32isize];
        let rep = rep.ok_or_else(|| ImageError("could not allocate an icon bitmap".into()))?;
        let srgb: *mut AnyObject = msg_send![color_space, sRGBColorSpace];
        let tagged: *mut AnyObject = msg_send![&*rep, bitmapImageRepByRetaggingWithColorSpace: srgb];
        let tagged = if tagged.is_null() { Retained::autorelease_return(rep) } else { tagged };
        let ctx: *mut AnyObject = msg_send![context, graphicsContextWithBitmapImageRep: tagged];
        if ctx.is_null() { return Err(ImageError("could not create an icon context".into())); }
        let _: () = msg_send![context, saveGraphicsState];
        let _: () = msg_send![context, setCurrentContext: ctx];
        draw(ctx, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(side as f64, side as f64)));
        let _: () = msg_send![ctx, flushGraphics];
        let _: () = msg_send![context, restoreGraphicsState];
        Ok(tagged)
    }
}

fn decode(bytes: &[u8]) -> Result<Decoded, ImageError> {
    if is_svg(bytes) {
        let (w, h, rgba) = svg_rgba(bytes, 0)?;
        return mip_chain(w, h, &rgba);
    }
    if avif::is_avif(bytes) {
        let (w, h, rgba) = avif::rgba(bytes)?;
        return mip_chain(w, h, &rgba);
    }
    let bytes = if bytes.starts_with(b"icns") { icns_png(bytes)? } else { bytes };
    if let Some(animation) = animated::decode(bytes) { return animation; }
    use image::ImageDecoder;
    let error = |e: image::ImageError| ImageError(e.to_string());
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format().map_err(|e| ImageError(e.to_string()))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits.clone());
    // `ImageReader::decode`, keeping the decoder to read its EXIF orientation (a phone's JPEG
    // stores its pixels as the sensor saw them and says how to turn them upright).
    let mut decoder = reader.into_decoder().map_err(error)?;
    limits.reserve(decoder.total_bytes()).map_err(error)?;
    decoder.set_limits(limits).map_err(error)?;
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image = image::DynamicImage::from_decoder(decoder).map_err(error)?;
    image.apply_orientation(orientation);
    let image = image.into_rgba8();
    mip_chain(image.width(), image.height(), image.as_raw())
}

fn icns_png(bytes: &[u8]) -> Result<&[u8], ImageError> {
    let invalid = || ImageError("invalid ICNS container or no supported PNG representation".into());
    let len = u32::from_be_bytes(bytes.get(4..8).ok_or_else(invalid)?.try_into().unwrap()) as usize;
    if len != bytes.len() { return Err(invalid()); }
    let mut pos = 8;
    let mut best = None;
    let mut area = 0u64;
    while pos < len {
        let header = bytes.get(pos..pos + 8).ok_or_else(invalid)?;
        let size = u32::from_be_bytes(header[4..8].try_into().unwrap()) as usize;
        if size < 8 { return Err(invalid()); }
        let end = pos.checked_add(size).filter(|end| *end <= len).ok_or_else(invalid)?;
        let chunk = &bytes[pos + 8..end];
        if chunk.starts_with(b"\x89PNG\r\n\x1a\n") && chunk.len() >= 24 {
            let w = u32::from_be_bytes(chunk[16..20].try_into().unwrap());
            let h = u32::from_be_bytes(chunk[20..24].try_into().unwrap());
            let candidate = u64::from(w) * u64::from(h);
            if w <= 8192 && h <= 8192 && candidate > area { best = Some(chunk); area = candidate; }
        }
        pos = end;
    }
    best.ok_or_else(invalid)
}

pub(crate) mod animated;
mod avif;
mod from_url;
mod mips;
use mips::mip_chain;
#[cfg(test)]
use mips::byte;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use from_url::font_file;

#[cfg(test)]
mod mip_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mips_filter_linear_premultiplied_pixels_including_odd_edges() {
        let d = mip_chain(3, 1, &[255, 0, 0, 255, 0, 255, 0, 0, 255, 0, 0, 255]).unwrap();
        assert_eq!(d.mips.len(), 2);
        assert_eq!(&d.mips[0].pixels[4..8], &[0, 0, 0, 0]);
        assert_eq!(d.mips[1].pixels[3], 170);
        assert_eq!(d.mips[1].pixels[1], 0); // no transparent-green fringe
        assert!((d.mips[1].pixels[0] as i32 - 213).abs() <= 1);
        let gray = mip_chain(2, 1, &[0, 0, 0, 255, 255, 255, 255, 255]).unwrap();
        assert_eq!(gray.mips[1].pixels, [188, 188, 188, 255]);
        let tail = mip_chain(3, 1, &[255, 255, 255, 1, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap();
        assert_eq!(tail.mips[1].pixels, [0, 0, 0, 0], "alpha rounded to zero cannot emit light");
    }

    #[test]
    fn decoded_pixels_wake_windows_only_once_the_image_reports_ready() {
        let image = Image::new(Arc::new(State { id: [0; 32], pixels: crate::Pixels::new(), decoded: OnceLock::new() }));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (probe, record) = (image.clone(), seen.clone());
        let wake = crate::live::Wake::new(Arc::new(move || {
            record.lock().unwrap().push((probe.is_ready(), probe.size()));
        }));
        let _subscription = image.pixels().source.subscribe(&wake);
        image.finish(mip_chain(2, 1, &[0, 0, 0, 255, 255, 255, 255, 255]));
        assert_eq!(*seen.lock().unwrap(), [(true, Some([2, 1]))]);
    }

    #[test]
    fn a_small_image_queued_behind_a_large_one_decodes_without_waiting_for_it() {
        if std::thread::available_parallelism().map_or(1, |n| n.get()) < 2 { return; }
        let (tx, rx) = mpsc::channel();
        let cache = ImageCache::new(Arc::new(move || { let _ = tx.send(()); }));
        let large = cache.rgba(3000, 3000, vec![128; 3000 * 3000 * 4]);
        let small = cache.rgba(1, 1, vec![20, 40, 60, 255]);
        rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(small.is_ready() && !large.is_ready());
        rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
        assert!(large.is_ready());
    }

    #[test]
    fn cache_deduplicates_and_wakes_after_async_completion() {
        let (tx, rx) = mpsc::channel();
        let cache = ImageCache::new(Arc::new(move || { tx.send(()).unwrap(); }));
        let first = cache.rgba(1, 1, vec![20, 40, 60, 255]);
        let second = cache.rgba(1, 1, vec![20, 40, 60, 255]);
        assert!(Arc::ptr_eq(&first.0, &second.0));
        rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(first.size(), Some([1, 1]));
        assert_eq!(first.decoded().unwrap().unwrap().mips[0].pixels, [20, 40, 60, 255]);
        let invalid = cache.load(vec![0, 1, 2]);
        rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(invalid.error().is_some());
    }

    #[test]
    fn a_fetch_is_wanted_while_a_handle_or_its_drawn_pixels_live() {
        let image = Image::new(Arc::new(State { id: [0; 32], pixels: crate::Pixels::new(), decoded: OnceLock::new() }));
        let weak = Arc::downgrade(&image.0);
        let drawn = image.pixels().clone();
        drop(image);
        assert!(wanted(&weak), "an element still draws it");
        drop(drawn);
        assert!(!wanted(&weak), "a guess replaced before its fetch runs fetches nothing");
    }

    #[test]
    fn drawn_pixels_keep_their_image_cached_after_the_handle_drops() {
        let (tx, rx) = mpsc::channel();
        let cache = ImageCache::new(Arc::new(move || { tx.send(()).unwrap(); }));
        let image = cache.rgba(1, 1, vec![20, 40, 60, 255]);
        rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        // A view keeps only the pixels in its element, then asks again on the next build.
        let drawn = image.pixels().clone();
        let first = Arc::downgrade(&image.0);
        drop(image);
        let again = cache.rgba(1, 1, vec![20, 40, 60, 255]);
        assert!(std::ptr::eq(first.as_ptr(), Arc::as_ptr(&again.0)), "the same entry, not a new request");
        assert!(again.is_ready());
        drop((again, drawn));
        // The worker drops its own handle just after it wakes us.
        for _ in 0..200 {
            if first.upgrade().is_none() { break }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(first.upgrade().is_none(), "nothing left draws it, so it's released");
    }

    #[test]
    fn png_jpeg_and_icns_decode_and_malformed_containers_fail() {
        let source = image::RgbImage::from_pixel(8, 4, image::Rgb([20, 40, 60]));
        for format in [image::ImageFormat::Png, image::ImageFormat::Jpeg] {
            let mut encoded = std::io::Cursor::new(Vec::new());
            source.write_to(&mut encoded, format).unwrap();
            let bytes = encoded.into_inner();
            assert_eq!(browser_decodes(&bytes), format == image::ImageFormat::Jpeg, "web decodes JPEG in the browser, PNG itself");
            let d = decode(&bytes).unwrap();
            assert_eq!((d.mips[0].width, d.mips[0].height), (8, 4));
            assert_eq!((d.mips.last().unwrap().width, d.mips.last().unwrap().height), (1, 1));
            if format == image::ImageFormat::Png {
                let mut icns = b"icns".to_vec();
                icns.extend_from_slice(&((bytes.len() + 16) as u32).to_be_bytes());
                icns.extend_from_slice(b"icp4");
                icns.extend_from_slice(&((bytes.len() + 8) as u32).to_be_bytes());
                icns.extend_from_slice(&bytes);
                assert!(!browser_decodes(&icns));
                assert_eq!(decode(&icns).unwrap().mips[0].pixels, d.mips[0].pixels);
                icns[12..16].copy_from_slice(&0u32.to_be_bytes());
                assert!(decode(&icns).is_err());
            }
        }
        assert!(mip_chain(0, 1, &[]).is_err());
        assert!(mip_chain(1, 1, &[0]).is_err());
    }

    #[test]
    fn a_jpeg_turns_upright_by_its_exif_orientation() {
        // Red on the left, blue on the right, stored as the camera saw it.
        let source = image::RgbImage::from_fn(16, 8, |x, _| if x < 8 { image::Rgb([255, 0, 0]) } else { image::Rgb([0, 0, 255]) });
        let mut encoded = std::io::Cursor::new(Vec::new());
        source.write_to(&mut encoded, image::ImageFormat::Jpeg).unwrap();
        let jpeg = encoded.into_inner();
        // An APP1 Exif segment after SOI: a little-endian TIFF header and one IFD entry,
        // Orientation (0x0112) = 6, "rotate 90° clockwise to display".
        let mut tiff = b"II*\0".to_vec();
        tiff.extend_from_slice(&8u32.to_le_bytes());
        tiff.extend_from_slice(&1u16.to_le_bytes());
        tiff.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0]);
        tiff.extend_from_slice(&0u32.to_le_bytes());
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff);
        let mut rotated = jpeg[..2].to_vec();
        rotated.extend_from_slice(&[0xff, 0xe1]);
        rotated.extend_from_slice(&(app1.len() as u16 + 2).to_be_bytes());
        rotated.extend_from_slice(&app1);
        rotated.extend_from_slice(&jpeg[2..]);

        let plain = decode(&jpeg).unwrap();
        assert_eq!((plain.mips[0].width, plain.mips[0].height), (16, 8));
        let upright = decode(&rotated).unwrap();
        let top = &upright.mips[0];
        assert_eq!((top.width, top.height), (8, 16), "turned a quarter");
        let at = |x: u32, y: u32| &top.pixels[((y * top.width + x) * 4) as usize..][..3];
        assert!(at(4, 2)[0] > 200 && at(4, 2)[2] < 60, "the left (red) half is now on top: {:?}", at(4, 2));
        assert!(at(4, 13)[2] > 200 && at(4, 13)[0] < 60, "the right (blue) half below: {:?}", at(4, 13));
    }

    #[test]
    fn a_decoder_panic_fails_its_image_and_the_worker_goes_on() {
        let (tx, rx) = mpsc::channel();
        let cache = ImageCache::new(Arc::new(move || { tx.send(()).unwrap(); }));
        let broken = cache.request([7; 32], Input::Panic);
        rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(broken.error(), Some(&ImageError("image decoder panicked: decoder bug".into())));
        let next = cache.rgba(1, 1, vec![20, 40, 60, 255]);
        rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(next.size(), Some([1, 1]), "later images still load");
    }

    #[cfg(feature = "svg")]
    #[test]
    fn svg_rasterizes_at_the_requested_device_size_from_bytes_mime_files_and_svgz() {
        const SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="8" viewBox="0 0 16 8"><rect width="8" height="8" fill="#ff0000"/></svg>"##;
        let crc = |data: &[u8]| !data.iter().fold(!0u32, |c, &b| (0..8).fold(c ^ b as u32, |c, _| (c >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(c & 1))));
        // Gzip with one stored deflate block.
        let mut svgz = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff, 1];
        svgz.extend_from_slice(&(SVG.len() as u16).to_le_bytes());
        svgz.extend_from_slice(&(!(SVG.len() as u16)).to_le_bytes());
        svgz.extend_from_slice(SVG);
        svgz.extend_from_slice(&crc(SVG).to_le_bytes());
        svgz.extend_from_slice(&(SVG.len() as u32).to_le_bytes());
        let dir = std::env::temp_dir().join(format!("rune-svg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mark.svg"), SVG).unwrap();
        std::fs::write(dir.join("mark.svgz"), &svgz).unwrap();
        let cache = ImageCache::new(Arc::new(|| {}));
        let cases = [
            ("load: twice its own size", cache.load(SVG), [32, 16]),
            ("load: sniffed svgz", cache.load(svgz.clone()), [32, 16]),
            ("svg: 24 pt at 2x", cache.svg(SVG, 24.0, 2.0), [48, 24]),
            ("svg: 16 pt at 3x", cache.svg(svgz.clone(), 16.0, 3.0), [48, 24]),
            ("mime", cache.load_mime(SVG, "image/svg+xml; charset=utf-8", 8.0, 2.0), [16, 8]),
            ("file .svg", cache.file(dir.join("mark.svg"), 32.0, 2.0), [64, 32]),
            ("file .svgz", cache.file(dir.join("mark.svgz"), 32.0, 1.0), [32, 16]),
        ];
        for (what, image, size) in cases {
            assert_eq!(image.wait(), Ok(size), "{what}");
            let mip = &image.decoded().unwrap().unwrap().mips[0];
            let at = |x: u32, y: u32| &mip.pixels[((y * mip.width + x) * 4) as usize..][..4];
            assert_eq!(at(1, size[1] / 2), [255, 0, 0, 255], "{what}: the left half is red");
            assert_eq!(at(size[0] - 2, size[1] / 2), [0, 0, 0, 0], "{what}: the right half is clear");
        }
        assert!(cache.load_mime(b"<svg".to_vec(), "image/svg+xml", 16.0, 2.0).wait().is_err(), "malformed SVG fails");
        assert!(cache.file(dir.join("missing.svg"), 16.0, 2.0).wait().is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn desktop_entries_resolve_the_closest_theme_size_then_scalable_then_pixmaps() {
        let root = std::env::temp_dir().join(format!("rune-icon-theme-{}", std::process::id()));
        let (user, system) = (root.join("user"), root.join("system"));
        let put = |path: std::path::PathBuf| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"icon").unwrap();
            path
        };
        let entry = |icon: &str| {
            let path = root.join(format!("{}.desktop", icon.replace('/', "_")));
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(&path, format!("[Desktop Entry]\nName=App\nIcon={icon}\n[Desktop Action new]\nIcon=other\n")).unwrap();
            path
        };
        let dirs = [user.clone(), system.clone()];
        put(system.join("icons/hicolor/32x32/apps/editor.png"));
        let at_64 = put(user.join("icons/hicolor/64x64/apps/editor.png"));
        let at_2x = put(system.join("icons/hicolor/48x48@2/apps/editor.png"));
        put(system.join("icons/hicolor/256x256/apps/editor.png"));
        let svg = put(system.join("icons/hicolor/scalable/apps/editor.svg"));
        assert_eq!(freedesktop::icon_file(&entry("editor"), 48, &dirs), Some(at_64.clone()), "smallest size covering 48px");
        assert_eq!(freedesktop::icon_file(&entry("editor"), 90, &dirs), Some(at_2x), "48x48@2 is 96px");
        assert_eq!(freedesktop::icon_file(&entry("editor.png"), 48, &dirs), Some(at_64), "an extension in Icon= is ignored");
        std::fs::remove_dir_all(system.join("icons/hicolor/256x256")).unwrap();
        assert_eq!(freedesktop::icon_file(&entry("editor"), 512, &dirs), Some(svg), "scalable beats a too-small bitmap");
        let pixmap = put(system.join("pixmaps/legacy.png"));
        assert_eq!(freedesktop::icon_file(&entry("legacy"), 48, &dirs), Some(pixmap.clone()));
        assert_eq!(freedesktop::icon_file(&entry(pixmap.to_str().unwrap()), 48, &dirs), Some(pixmap), "absolute Icon= path");
        assert_eq!(freedesktop::icon_file(&entry("missing"), 48, &dirs), None);
        assert_eq!(freedesktop::icon_name("[Desktop Action x]\nIcon=no\n"), None, "only the main group counts");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn app_icon_reads_an_executables_icon() {
        let notepad = std::path::Path::new(r"C:\Windows\System32\notepad.exe");
        if !notepad.is_file() { return; }
        let (w, h, pixels) = app_icon_rgba(notepad, 48).unwrap();
        assert_eq!((w, h), (48, 48));
        assert!(pixels.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn app_icon_rasterizes_calculator_off_the_main_thread_and_shares_handles() {
        let cache = ImageCache::new(Arc::new(|| {}));
        let icon = cache.app_icon("/System/Applications/Calculator.app", 32.0);
        assert!(Arc::ptr_eq(&icon.0, &cache.app_icon("/System/Applications/Calculator.app", 32.0).0));
        assert!(!Arc::ptr_eq(&icon.0, &cache.app_icon_scaled("/System/Applications/Calculator.app", 32.0, 1.0).0));
        assert_eq!(icon.wait(), Ok([64, 64]));
        let pixels = &icon.decoded().unwrap().unwrap().mips[0].pixels;
        let opaque = pixels.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(opaque > 64 * 64 / 4, "icon is mostly empty: {opaque} visible pixels");
    }

    /// A small icon is the large one scaled down, not a different (on macOS 26 and later, a
    /// framed "legacy") rendition.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_small_app_icon_matches_the_large_one_scaled_down() {
        let dir = std::env::temp_dir().join(format!("rune-icon-bundle-{}", std::process::id()));
        let app = synthetic_app(&dir);
        let small = app_icon_rgba(&app, 32).unwrap().2;
        let large = app_icon_rgba(&app, 256).unwrap().2;
        let _ = std::fs::remove_dir_all(&dir);
        let (mut total, mut count) = (0u32, 0u32);
        for y in 0..32 {
            for x in 0..32 {
                // Straight colour is noise where the icon is nearly transparent.
                let solid = small[(y * 32 + x) * 4 + 3] >= 200;
                for c in (if solid { 0 } else { 3 })..4 {
                    let block: u32 = (0..8).flat_map(|dy| (0..8).map(move |dx| (dx, dy)))
                        .map(|(dx, dy)| u32::from(large[((y * 8 + dy) * 256 + x * 8 + dx) * 4 + c])).sum();
                    total += (block / 64).abs_diff(u32::from(small[(y * 32 + x) * 4 + c]));
                    count += 1;
                }
            }
        }
        // The resampling filter rings on the tile's hard edge, so compare the mean, not the worst
        // pixel. A different rendition (a framed or shrunken tile) differs across the whole icon.
        let mean = total as f32 / count as f32;
        assert!(mean < 12.0, "the 32 px icon differs from the 256 px one by {mean} on average");
    }

    /// A bundle whose only icon is a generated 256 px gradient, so the test does not depend on
    /// the icons the installed macOS ships.
    #[cfg(target_os = "macos")]
    fn synthetic_app(dir: &std::path::Path) -> std::path::PathBuf {
        let app = dir.join("Synthetic.app");
        let resources = app.join("Contents/Resources");
        std::fs::create_dir_all(&resources).unwrap();
        std::fs::write(app.join("Contents/Info.plist"), concat!(
            r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>"#,
            "<key>CFBundleIdentifier</key><string>dev.rune.synthetic-icon</string>",
            "<key>CFBundleName</key><string>Synthetic</string>",
            "<key>CFBundlePackageType</key><string>APPL</string>",
            "<key>CFBundleIconFile</key><string>icon</string></dict></plist>",
        )).unwrap();
        let mut rgba = Vec::with_capacity(256 * 256 * 4);
        for y in 0..256u32 {
            for x in 0..256u32 {
                let inside = (16..240).contains(&x) && (16..240).contains(&y);
                rgba.extend_from_slice(&[x as u8, y as u8, (x + y) as u8 / 2, if inside { 255 } else { 0 }]);
            }
        }
        let mut png = Vec::new();
        let mut encoder = png::Encoder::new(&mut png, 256, 256);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().unwrap().write_image_data(&rgba).unwrap();
        let mut icns = b"icns".to_vec();
        icns.extend_from_slice(&((png.len() as u32 + 16).to_be_bytes()));
        icns.extend_from_slice(b"ic08");
        icns.extend_from_slice(&((png.len() as u32 + 8).to_be_bytes()));
        icns.extend_from_slice(&png);
        std::fs::write(resources.join("icon.icns"), icns).unwrap();
        app
    }
}
