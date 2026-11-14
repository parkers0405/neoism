//! A window onto a taller document (`App::web_document`): the root lays out at its natural
//! height, as a window that fits its content does, and the frame draws the band the browser
//! scrolls through. The host sets the offset; nothing in the runtime scrolls it.
//!
//! The canvas is not a viewport-sized window fixed to the screen: it is a *band* of the document,
//! taller than the viewport by a skirt of overscan at each end, that the browser's own compositor
//! scrolls with the page. Rune draws a new band only when the viewport comes near one of its
//! edges, so a phone pans static pixels at its display's rate instead of waiting for a frame.
use super::*;

/// How close the viewport may come to the band's edge before a frame draws a new band around it.
const MARGIN: f32 = 64.0;

/// Where the band the window shows starts in the document, in points.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Page {
    /// The window shows a band of a taller document rather than the whole root.
    pub on: bool,
    /// The browser's own scroll position, in points on the device-pixel grid: negative while it
    /// rubber-bands past the top.
    pub scroll: f32,
    /// Where the canvas's texture starts in the document, in points on the same grid.
    pub top: f32,
    /// The canvas's height in points: the band of the document it covers.
    pub band: f32,
    /// How much of the band the page shows at once (the layout viewport's height, from the
    /// largest viewport so a phone's toolbar doesn't change it).
    pub view: f32,
}

impl<S: 'static> Runtime<S> {
    /// Lay the root out at its natural height and draw the band of it the host scrolls to
    /// with [`Self::set_page_scroll`]. The viewport stays the band's size.
    pub(crate) fn page_mode(&mut self) {
        self.page.on = true;
        self.page.view = self.viewport.1;
        self.fit_height = true;
        self.relayout = true;
    }

    /// The document's height in points: the root's laid-out height.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn page_height(&self) -> f32 {
        self.nodes.first().map_or(self.viewport.1, |root| root.local.h.max(0.0))
    }

    /// The band's size in points: the canvas covers `band` points of the document, of which
    /// `view` are on screen at once. The host sets it from the canvas's own size. Returns whether
    /// the band moved (the next frame draws it).
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn set_band(&mut self, band: f32, view: f32) -> bool {
        if !self.page.on { return false; }
        let (band, view) = (band.max(0.0), view.max(0.0));
        if self.page.band == band && self.page.view == view { return false; }
        self.page.band = band;
        self.page.view = view;
        let top = self.band_top_for(self.page.scroll).unwrap_or(self.page.top);
        let moved = top != self.page.top;
        self.page.top = top;
        moved
    }

    /// Draw the band holding `y`, the browser's `scrollY` (rounded to device pixels, so text
    /// keeps its grid). `y` goes negative while a phone rubber-bands past the top: the band
    /// follows it, so the page bounces with the finger rather than stopping at the first pixel.
    /// Returns whether a frame must draw a new band; while the viewport stays inside the skirt the
    /// browser's compositor scrolls the canvas it already has and nothing is drawn.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn set_page_scroll(&mut self, y: f32) -> bool {
        if !self.page.on { return false; }
        let y = if y.is_finite() { self.snap_band(y) } else { 0.0 };
        if y == self.page.scroll { return false; }
        self.page.scroll = y;
        let Some(top) = self.band_top_for(y) else { return false };
        if top == self.page.top { return false; }
        self.page.top = top;
        true
    }

    /// Where the band starts for a scroll position, or `None` while the viewport is comfortably
    /// inside the band already drawn. A band no taller than the viewport has no room to coast in:
    /// the canvas follows the page every frame, as a fixed viewport-sized one did.
    fn band_top_for(&self, scroll: f32) -> Option<f32> {
        let (view, band) = (self.page.view, self.page.band);
        if band <= view + 2.0 * MARGIN { return Some(self.snap_band(scroll)); }
        let top = self.page.top;
        if scroll >= top + MARGIN && scroll + view <= top + band - MARGIN { return None; }
        let want = scroll - (band - view) * 0.5;
        Some(self.snap_band(want.clamp(0.0, (self.page_height() - band).max(0.0))))
    }

    fn snap_band(&self, y: f32) -> f32 {
        let s = self.env.scale.max(1e-3);
        (y * s).round() / s
    }

    /// The browser's scroll position, in points.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn page_scroll_y(&self) -> f32 {
        if self.page.on { self.page.scroll } else { 0.0 }
    }

    /// The part of the band that is on screen: where it starts in the band's points, and how tall
    /// it is. `(0, viewport)` outside a page.
    pub(crate) fn page_window(&self) -> (f32, f32) {
        if !self.page.on { return (0.0, self.viewport.1); }
        let view = if self.page.view > 0.0 { self.page.view } else { self.viewport.1 };
        (self.page.scroll - self.page.top, view)
    }

    /// Draw a horizontal viewport at `path` scrolled `x` points from its content's left edge. A
    /// page's horizontal viewports are the browser's: it scrolls its own mirror's container and
    /// tells Rune where it went. Returns whether the content moved (the next frame draws it).
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn set_page_scroll_x(&mut self, path: &str, x: f32) -> bool {
        if !self.page.on || !x.is_finite() { return false; }
        let Some(i) = self.live(path) else { return false };
        let Some(st) = self.scrolls.get_mut(&self.nodes[i].id) else { return false };
        let offset = f64::from(x.max(0.0)).min(st.x.max.max(0.0));
        if (st.x.offset - offset).abs() < 0.01 { return false; }
        st.x.offset = offset;
        st.x.v = 0.0;
        st.x.glide = None;
        true
    }

    /// What the root hands its subtree in the paint pass: the band's offset as a rigid scroll.
    pub(super) fn root_inh(&self) -> Inh {
        let mut inh = Inh::default();
        let dy = -self.page_offset();
        inh.dy = dy;
        inh.snap.rigid[1] = dy;
        inh
    }

    /// The band's top down the document, in points (0 outside page mode). A mirror's document
    /// position is what a frame drew plus this.
    pub(crate) fn page_offset(&self) -> f32 {
        if self.page.on { self.page.top } else { 0.0 }
    }
}

#[cfg(test)]
#[path = "page_tests.rs"]
mod tests;
