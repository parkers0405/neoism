//! Following where an element is: `El::on_layout` and `El::on_screen_layout` with its laid-out
//! rect, `El::on_drawn_layout` with where a frame drew it, and on a scroll viewport
//! `El::on_scroll_offset` with its offset in the frame that draws it. The runtime side is in
//! `runtime/layout_read.rs` and `runtime/scroll/watch.rs`.
use std::rc::Rc;

use crate::states::downcast;
use crate::{El, Tx};

/// Where a scroll viewport is scrolled to, for [`El::on_scroll_offset`]: `offset` points from
/// the top-left of its content, `[x, y]`, out of the `max` it can scroll on each axis, in a
/// viewport `viewport` points wide and tall. `drawn` is the size the frame draws the viewport
/// at: `viewport` at rest, and where its size has got to while a layout spring resizes it, so
/// the handler hears every frame of that.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollPos {
    pub offset: [f32; 2],
    pub max: [f32; 2],
    pub viewport: [f32; 2],
    pub drawn: [f32; 2],
}

impl El {
    /// Called with this element's laid-out rect `[x, y, w, h]` in window points, after scrolling
    /// (as [`Cx::window_layout`](crate::Cx::window_layout) reads it): once after its first
    /// layout, then when it moves or resizes by more than half a point, which inside a scroll
    /// viewport is every frame of a scroll. Never per animation frame: the rect is where layout
    /// puts the element, not where its spring is on the way there. Each report is a state
    /// change, so the view runs again, unless the handler says [`Tx::live_only`] (see
    /// [`El::on_layout_tx`]).
    #[track_caller]
    pub fn on_layout<S: 'static>(mut self, f: impl Fn(&mut S, [f32; 4]) + 'static) -> Self {
        self.handlers.on_layout = Some(Rc::new(move |s, r, _| f(downcast::<S>(s), r)));
        #[cfg(debug_assertions)]
        { self.handlers.sites.layout = Some(std::panic::Location::caller()); }
        self
    }
    /// [`El::on_layout`] with a [`Tx`]. A handler that only moves live values (a marker beside
    /// a scrolling row, by its own [`El::live_offset`]) says [`Tx::live_only`], and a scroll
    /// runs no view.
    ///
    /// ```ignore
    /// .on_layout_tx(|s: &mut Board, [_, y, _, _], tx| {
    ///     s.marker_y.set(y);
    ///     tx.live_only();
    /// })
    /// ```
    #[track_caller]
    pub fn on_layout_tx<S: 'static>(mut self, f: impl Fn(&mut S, [f32; 4], &mut Tx) + 'static) -> Self {
        self.handlers.on_layout = Some(Rc::new(move |s, r, tx| f(downcast::<S>(s), r, tx)));
        #[cfg(debug_assertions)]
        { self.handlers.sites.layout = Some(std::panic::Location::caller()); }
        self
    }
    /// [`El::on_layout`] in global screen points: `[x, y, w, h]` from the top-left of the
    /// primary display, y down. That's the space of an overlay's
    /// [`OverlayPlacement::Rect`](crate::OverlayPlacement) on
    /// [`DisplayTarget::Primary`](crate::DisplayTarget), so an overlay can open over the
    /// element. It also runs when the window moves without a relayout. On macOS points are one
    /// space across displays of any scale. Elsewhere the window's desktop pixel position is
    /// divided by its own scale, so it's exact on the primary display and wherever displays
    /// share a scale; on Wayland, which hides window positions, the origin reads `(0, 0)`.
    /// Headless runs place windows on the script's virtual desktop.
    pub fn on_screen_layout<S: 'static>(mut self, f: impl Fn(&mut S, [f32; 4]) + 'static) -> Self {
        self.handlers.on_screen_layout = Some(Rc::new(move |s, r| f(downcast::<S>(s), r)));
        self
    }
    /// [`El::on_layout`] as drawn: `[x, y, w, h]` in window points where this frame put the
    /// element, after scrolling and its ancestors' [`El::live_offset`], [`El::live_scale`],
    /// [`El::live_rect`] and [`El::scale`], so an element on a zoomed canvas reports where it
    /// shows (the rect hit tests use, and [`ElementInfo::rect`](crate::ElementInfo)). It runs
    /// once after the element is first drawn, then when that rect moves by more than half a
    /// point, including on every frame a live value moves it (a camera pan, with no commit). A
    /// spring moving it, its own or an ancestor's, reports where it comes to rest, as
    /// `on_layout` does. Its handler runs on the next frame. Keep it to the few elements
    /// something follows (a selection a toolbar floats over), not every node on a canvas: each
    /// report is a state change, so the view runs again, unless the handler says
    /// [`Tx::live_only`] (see [`El::on_drawn_layout_tx`]).
    #[track_caller]
    pub fn on_drawn_layout<S: 'static>(mut self, f: impl Fn(&mut S, [f32; 4]) + 'static) -> Self {
        self.handlers.on_drawn_layout = Some(Rc::new(move |s, r, _| f(downcast::<S>(s), r)));
        #[cfg(debug_assertions)]
        { self.handlers.sites.drawn_layout = Some(std::panic::Location::caller()); }
        self
    }
    /// [`El::on_drawn_layout`] with a [`Tx`]. A toolbar that floats over a selection by its own
    /// [`El::live_offset`] follows a camera pan with no view re-run at all: the handler sets
    /// the toolbar's live offset and says [`Tx::live_only`].
    ///
    /// ```ignore
    /// .on_drawn_layout_tx(|s: &mut Canvas, [x, y, w, _], tx| {
    ///     s.toolbar_at.set([x + w / 2.0 - TOOLBAR_W / 2.0, y - TOOLBAR_H - 8.0]);
    ///     tx.live_only();
    /// })
    /// ```
    #[track_caller]
    pub fn on_drawn_layout_tx<S: 'static>(mut self, f: impl Fn(&mut S, [f32; 4], &mut Tx) + 'static) -> Self {
        self.handlers.on_drawn_layout = Some(Rc::new(move |s, r, tx| f(downcast::<S>(s), r, tx)));
        #[cfg(debug_assertions)]
        { self.handlers.sites.drawn_layout = Some(std::panic::Location::caller()); }
        self
    }
    /// Called with `true` when this element comes on screen and `false` when it leaves, as a
    /// web page's `IntersectionObserver` reports: on screen is half of it showing in the window
    /// (or it covering half the window), where the frame drew it after scroll viewports and a
    /// web page's own scroll ([`App::web_document`](crate::App::web_document)). It starts off
    /// screen, so the first report is the first time it shows. Use it to play a demo when the
    /// reader reaches it, or to stop a loop nobody can see.
    pub fn on_visible<S: 'static>(mut self, f: impl Fn(&mut S, bool, &mut Tx) + 'static) -> Self {
        self.handlers.on_visible = Some(Rc::new(move |s, on, tx| f(downcast::<S>(s), on, tx)));
        self
    }
    /// Writes where each frame draws this element into `rect`: `[x, y, w, h]` in window points,
    /// the rect [`El::on_drawn_layout`] reports, on every frame it moves, through a scroll, a
    /// glide and its own or an ancestor's spring. It runs no handler and commits nothing, so a
    /// scroll stays free of view runs. State code reads it with [`Live::get`](crate::Live::get)
    /// when it needs to (where to open a popover). An element bound to `rect` (a highlight at
    /// the window's top-left with [`El::live_rect`]) follows it a frame behind, since the rect
    /// is known once the frame is drawn; to ride a scroll in the same frame, follow the offset
    /// with [`El::on_scroll_offset`].
    ///
    /// ```ignore
    /// let ring = Live::new([0.0; 4]);
    /// row.drawn_rect_into(&ring);
    /// // at the root, outside the scroll:
    /// el().pin(Some(0.0), Some(0.0), None, None).live_rect(&ring).border(1.0, accent)
    /// ```
    pub fn drawn_rect_into(mut self, rect: &crate::Live<[f32; 4]>) -> Self {
        self.handlers.drawn_rect = Some(rect.clone());
        self
    }
    /// On a scroll viewport: called with where it's scrolled to once it's first laid out, in
    /// the frame that draws a commit giving it a new handler, and in every frame that scrolling
    /// moves it, or it or its content resizes, before that frame draws. A [`Live`](crate::Live) it
    /// sets shows in the same frame, so an element can follow the scroll exactly: a header that
    /// shrinks as the page scrolls, or a shadow that fades in past the top. It can't change
    /// state; set live values from it.
    ///
    /// ```ignore
    /// let title_k = Live::new(1.0);
    /// let k = title_k.clone();
    /// col().scroll(ScrollSpec::default()).on_scroll_offset(move |at: ScrollPos| k.set(1.0 - (at.offset[1] / 80.0).clamp(0.0, 1.0) * 0.4))
    /// ```
    pub fn on_scroll_offset(mut self, f: impl Fn(ScrollPos) + 'static) -> Self {
        self.handlers.on_scroll_offset = Some(Rc::new(f));
        self
    }

    #[doc(hidden)]
    pub fn chain_scroll_offset(mut self, f: impl Fn(ScrollPos) + 'static) -> Self {
        self.handlers.on_scroll_offset = Some(match self.handlers.get_mut_if_some().and_then(|h| h.on_scroll_offset.take()) {
            Some(prev) => Rc::new(move |at| {
                prev(at);
                f(at);
            }),
            None => Rc::new(f),
        });
        self
    }
}
