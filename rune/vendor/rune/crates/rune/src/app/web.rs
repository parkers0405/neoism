//! The builders that place an app in a web page: its canvas, a whole-page document and its
//! fonts by URL.

use super::*;

/// `fonts` and one more, kept for the life of the process as `AppSpec::fonts` requires.
pub(crate) fn with_font(fonts: &'static [&'static [u8]], bytes: Vec<u8>) -> &'static [&'static [u8]] {
    let mut all = fonts.to_vec();
    all.push(Box::leak(bytes.into_boxed_slice()));
    Box::leak(all.into_boxed_slice())
}

impl<S: 'static> App<S> {
    /// Mount onto an existing `<canvas id="…">` in the web document instead of appending a
    /// full-viewport canvas and locking `document.body`. The canvas sits inline in the page at
    /// its parent's width: pair with [`Self::fit_content_height`] to embed a Rune view in a
    /// scrollable page, or add [`Self::full_viewport`] to cover the screen. Ignored on native.
    pub fn canvas_id(mut self, id: impl Into<String>) -> Self {
        self.canvas_id = Some(id.into());
        self
    }

    /// On web, cover the browser viewport even when [`Self::canvas_id`] names the canvas, as an
    /// app without one does: the canvas is fixed to the visual viewport, the body stops
    /// scrolling, `Cx::safe_area` reports the page's insets and the title sets
    /// `document.title`. For a single-page app that mounts Rune full screen in a route's own
    /// `<canvas>`. [`Self::fit_content_height`] wins over it. Ignored on native.
    pub fn full_viewport(mut self) -> Self {
        self.full_viewport = true;
        self
    }

    /// On web, make the app the page: the root lays out at its natural height for the
    /// viewport's width and the browser scrolls it like any document, with its own momentum,
    /// keys, anchors, find-in-page and zoom. The canvas is a band of the document that scrolls
    /// with it, drawn tall enough to cover the window plus a little overscan at each end, so the
    /// browser's compositor moves the pixels Rune drew and a frame is drawn only when the window
    /// nears the band's edge; a semantic DOM mirror as tall as the content carries the scroll, the
    /// text, the headings, landmarks and real `<a href>` links. Give the root no fixed or `fill`
    /// height. Elsewhere it acts like [`Self::fit_content_height`], and `--html` exports it as the
    /// page's static first frame.
    pub fn web_document(mut self) -> Self {
        self.web_document = true;
        self.fit_content_height = true;
        self
    }

    /// Add a font file (TrueType or OpenType, not WOFF) to [`AppSpec::fonts`] by URL, so a web
    /// app shares one download with the page's own `@font-face`. On web the browser fetches
    /// it, relative to the page, while the GPU starts, and the first frame waits for it; a
    /// font that fails to load is reported in the console and skipped. On native a path or a
    /// `file:` URL is read now, as [`ImageCache::url`](crate::ImageCache::url) reads one.
    pub fn font_url(mut self, url: &str) -> Self {
        #[cfg(target_arch = "wasm32")]
        self.font_urls.push(url.to_owned());
        #[cfg(not(target_arch = "wasm32"))]
        match crate::image::font_file(url) {
            Ok(bytes) => self.spec.fonts = with_font(self.spec.fonts, bytes),
            Err(e) => eprintln!("rune: font {url}: {e}"),
        }
        self
    }
}
