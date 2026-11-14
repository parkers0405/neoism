//! What a copy writes and a paste reads: [`ClipboardContent`] and its [`ClipboardImage`].
use std::path::PathBuf;
use std::sync::Arc;
use zeroize::Zeroizing;
use super::sanitize_html;

/// What a copy puts on the clipboard, or what a paste found there: plain text, and optionally
/// HTML, RTF, an image and files. Debug output never includes its contents.
///
/// ```
/// # use rune::clipboard::ClipboardContent;
/// let content = ClipboardContent::new("Hello world").with_html("<b>Hello</b> world");
/// assert_eq!(content.html(), Some("<b>Hello</b> world"));
/// ```
///
/// The text may be empty when another form carries the content: a copied image or file list
/// needs no text, and an empty text isn't written beside it. Plain fields and text `on_paste`
/// handlers don't receive such a paste; hooks that take every form do.
///
/// Read from the clipboard, `html` has had scripts and other active content removed
/// ([`sanitize_html`]), and on macOS holds RTF converted to HTML when the source offered only
/// RTF. It's still untrusted input from another app: parse it into your model, don't render it
/// in a web view as is. `rtf` is passed through untouched.
#[derive(Clone, Default)]
pub struct ClipboardContent {
    pub(super) text: Zeroizing<String>,
    html: Option<Zeroizing<String>>,
    rtf: Option<Zeroizing<String>>,
    image: Option<ClipboardImage>,
    files: Vec<PathBuf>,
    pub(super) own_pills: bool,
}
impl PartialEq for ClipboardContent {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text && self.html == other.html && self.rtf == other.rtf && self.image == other.image && self.files == other.files
    }
}
impl Eq for ClipboardContent {}
impl ClipboardContent {
    /// Plain text, the form every app can paste. Empty when only other forms carry content.
    pub fn new(text: impl Into<String>) -> Self { Self { text: Zeroizing::new(text.into()), ..Self::default() } }
    /// Adds an HTML form (a fragment or a document), for apps that paste formatted text.
    pub fn with_html(mut self, html: impl Into<String>) -> Self { self.html = Some(Zeroizing::new(html.into())); self }
    /// Adds an RTF form, for apps that read RTF but not HTML.
    pub fn with_rtf(mut self, rtf: impl Into<String>) -> Self { self.rtf = Some(Zeroizing::new(rtf.into())); self }
    /// Adds an image. macOS writes it as PNG and TIFF beside the other forms; Windows and Linux
    /// write it alone (see [`Tx::copy_rich`](crate::Tx::copy_rich)); the web as PNG.
    pub fn with_image(mut self, image: ClipboardImage) -> Self { self.image = Some(image); self }
    /// Adds files, so a paste into a file manager copies them. macOS writes one file URL item
    /// per file; Windows and Linux write the file list alone; the web skips them. Paths should
    /// be absolute.
    pub fn with_files(mut self, files: impl IntoIterator<Item = PathBuf>) -> Self { self.files.extend(files); self }
    pub fn text(&self) -> &str { &self.text }
    pub fn html(&self) -> Option<&str> { self.html.as_deref().map(String::as_str) }
    pub fn rtf(&self) -> Option<&str> { self.rtf.as_deref().map(String::as_str) }
    /// The image on the clipboard, as PNG. Only pastes to a hook carry one: a field's
    /// `on_input_paste`, a root `on_paste_content` or `Tx::read_clipboard`.
    pub fn image(&self) -> Option<&ClipboardImage> { self.image.as_ref() }
    /// The files on the clipboard. Like the image, only pastes to a hook carry them.
    pub fn files(&self) -> &[PathBuf] { &self.files }
    /// HTML, RTF, an image or files beside the text.
    pub fn is_rich(&self) -> bool { self.html.is_some() || self.rtf.is_some() || self.image.is_some() || !self.files.is_empty() }
    /// No text and no other form.
    pub fn is_empty(&self) -> bool { self.text.is_empty() && !self.is_rich() }
    /// Content from another app: its HTML sanitised.
    pub(crate) fn untrusted(mut self) -> Self {
        if let Some(html) = &mut self.html { *html = Zeroizing::new(sanitize_html(html)); }
        self
    }
    pub(crate) fn has_own_pills(&self) -> bool { self.own_pills }
    /// Text, HTML and RTF only, for a rich text target: no image or files to decode or list.
    /// None when the image or files were all it had, so a paste of them alone changes nothing;
    /// genuinely empty text stays.
    pub(super) fn text_forms(mut self) -> Option<Self> {
        let had = !self.is_empty();
        self.image = None;
        self.files.clear();
        (!had || !self.is_empty()).then_some(self)
    }
    /// Plain consumers (a field without an `on_input_paste`, a text `on_paste`) take it: it
    /// has text, or nothing else. Rich-only content (an HTML clipboard with no plain text)
    /// reaches only rich hooks, as before it reached no one.
    pub(crate) fn has_plain(&self) -> bool { !self.text.is_empty() || !self.is_rich() }
}
impl From<String> for ClipboardContent {
    fn from(text: String) -> Self { Self::new(text) }
}
impl From<&str> for ClipboardContent {
    fn from(text: &str) -> Self { Self::new(text) }
}
impl From<Zeroizing<String>> for ClipboardContent {
    fn from(text: Zeroizing<String>) -> Self { Self { text, ..Self::default() } }
}
impl std::fmt::Debug for ClipboardContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipboardContent")
            .field("text", &self.text.len())
            .field("html", &self.html.as_ref().map(|h| h.len()))
            .field("rtf", &self.rtf.as_ref().map(|r| r.len()))
            .field("image", &self.image)
            .field("files", &self.files.len())
            .finish()
    }
}

/// The largest image a paste carries, as PNG. A bigger one is left out, and the paste
/// carries the clipboard's other forms.
pub const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
/// The most pixels a pasted image may have (64 MP, e.g. an 8K screenshot: 256 MiB of RGBA).
const MAX_IMAGE_PIXELS: u64 = 64 * 1024 * 1024;

pub(crate) fn valid_image_size(width: u32, height: u32) -> bool {
    width > 0 && height > 0 && u64::from(width) * u64::from(height) <= MAX_IMAGE_PIXELS
}

/// An image as PNG bytes: one to copy, or one a paste found in whatever form the clipboard held
/// it (TIFF on macOS, a DIB on Windows). Cheap to clone. Debug output shows only its size.
///
/// ```
/// # use rune::clipboard::{ClipboardContent, ClipboardImage};
/// # let png = [&b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"[..], &[0, 0, 0, 2, 0, 0, 0, 3]].concat();
/// let image = ClipboardImage::from_png(png).unwrap();
/// assert_eq!(image.size(), (2, 3));
/// let content = ClipboardContent::new("").with_image(image);
/// assert!(content.image().is_some_and(|i| i.png().starts_with(b"\x89PNG")));
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct ClipboardImage { png: Arc<[u8]>, size: (u32, u32) }
impl ClipboardImage {
    /// PNG bytes, or `None` when they don't start with a PNG header or pass
    /// [`MAX_IMAGE_BYTES`].
    pub fn from_png(png: impl Into<Arc<[u8]>>) -> Option<Self> {
        let png = png.into();
        if png.len() > MAX_IMAGE_BYTES || png.len() < 24 || &png[..8] != b"\x89PNG\r\n\x1a\n" || &png[12..16] != b"IHDR" { return None; }
        let be = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
        let size = (be(16), be(20));
        if !valid_image_size(size.0, size.1) { return None; }
        Some(Self { png, size })
    }
    /// Encodes 8-bit RGBA pixels, row by row, as PNG.
    #[cfg_attr(any(target_os = "macos", target_os = "ios", target_os = "android", target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn from_rgba(width: u32, height: u32, rgba: &[u8]) -> Option<Self> {
        if !valid_image_size(width, height) { return None; }
        Self::from_png(crate::drag_out::encode_png(width, height, rgba)?)
    }
    /// Decodes the PNG to 8-bit RGBA, for clipboards that take pixels.
    #[cfg(any(target_os = "windows", all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android")))))]
    pub(crate) fn to_rgba(&self) -> Option<image::RgbaImage> {
        image::load_from_memory_with_format(&self.png, image::ImageFormat::Png).ok().map(|i| i.into_rgba8())
    }
    /// The PNG file's bytes, ready to save, upload or decode.
    pub fn png(&self) -> &[u8] { &self.png }
    /// Width and height in pixels, from the PNG header.
    pub fn size(&self) -> (u32, u32) { self.size }
}
impl std::fmt::Debug for ClipboardImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipboardImage").field("size", &self.size).field("bytes", &self.png.len()).finish()
    }
}
