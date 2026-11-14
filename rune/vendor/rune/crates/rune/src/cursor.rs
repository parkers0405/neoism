//! The pointer's shape over an element.

pub(crate) mod image;
pub(crate) mod os;

/// The pointer's shape over an element (CSS `cursor`). Elements without one inherit their
/// nearest ancestor's; text fields default to [`Cursor::Text`], everything else to the arrow.
///
/// A macOS overlay is rarely the key window, so it sets its cursor directly while the pointer
/// is over its hit area. macOS honours that only while the overlay's app is frontmost, unless
/// `rune` is built with the `macos-background-cursor` feature. That feature uses a private
/// WindowServer property (`SetsCursorInBackground`), so App Store review may reject the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Cursor {
    #[default]
    Default,
    Pointer,
    Text,
    Grab,
    Grabbing,
    NotAllowed,
    Move,
    ResizeColumn,
    ResizeRow,
    /// A precise-selection cross (CSS `crosshair`), for picking a point or dragging out a rect.
    Crosshair,
    /// No OS cursor (CSS `none`), for views that draw their own. It shows again as soon as the
    /// pointer leaves the element or the window, the element goes away, or the host loses the
    /// pointer: a window losing focus, an overlay hiding.
    Hidden,
    /// An app's own image (CSS `url()`), such as a pencil over a canvas. The OS draws it, so
    /// it keeps up with the pointer and shows right to the window's edge. It comes and goes
    /// like any other cursor. If its PNG doesn't decode, the arrow shows instead, and that's
    /// logged once.
    Image(CursorImage),
}

/// The picture for [`Cursor::Image`]: a PNG drawn at 2x, so a 64×64 px PNG makes a
/// 32×32 pt cursor, and its hot spot (the point that clicks) in points from the top-left,
/// clamped into the image.
///
/// ```ignore
/// const PENCIL: CursorImage = CursorImage::png(include_bytes!("pencil.png"), [2.0, 30.0]);
/// canvas.cursor(Cursor::Image(PENCIL))
/// ```
///
/// Two images are the same cursor when they have the same bytes and hot spot. Comparing one
/// image with itself checks only the address, so it costs nothing per frame; a `const` may be
/// copied to another address at each use, which then compares the bytes. Each image is
/// decoded once and its OS cursor kept, up to 32 of them; a side may be up to 256 px.
#[derive(Clone, Copy)]
pub struct CursorImage {
    bytes: &'static [u8],
    hot: [f32; 2],
}

impl CursorImage {
    /// A PNG drawn at 2x, with its hot spot in points from the top-left.
    pub const fn png(bytes: &'static [u8], hot: [f32; 2]) -> Self {
        CursorImage { bytes, hot }
    }

    pub(crate) fn bytes(&self) -> &'static [u8] {
        self.bytes
    }

    pub(crate) fn hot(&self) -> [f32; 2] {
        self.hot
    }

}

impl PartialEq for CursorImage {
    fn eq(&self, other: &Self) -> bool {
        self.hot.map(f32::to_bits) == other.hot.map(f32::to_bits)
            && (std::ptr::eq(self.bytes, other.bytes) || self.bytes == other.bytes)
    }
}

impl Eq for CursorImage {}

impl std::hash::Hash for CursorImage {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (self.bytes.len(), self.hot.map(f32::to_bits)).hash(state);
    }
}

impl std::fmt::Debug for CursorImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CursorImage").field("png_bytes", &self.bytes.len()).field("hot", &self.hot).finish()
    }
}
