//! Scroll viewport builders: the axes a viewport moves on, its bottom callbacks, edge fades
//! and reveal margin.

use super::*;

impl El {
    /// A vertical scroll viewport over its children.
    ///
    /// The viewport needs a bounded height to scroll: a fixed `.h(..)`, or a `.grow(1.0)` share
    /// of a parent whose height is itself bounded. Every flex column between that bounded
    /// ancestor and the viewport needs `.min_h(0.0)`, or it grows to fit the content and the
    /// viewport never overflows. Debug builds warn once per call site when a viewport inside a
    /// `.grow()` container resolves to 0 pt tall while its content is not.
    #[cfg_attr(debug_assertions, track_caller)]
    #[inline]
    pub fn scroll(mut self, spec: ScrollSpec) -> Self {
        self.set_scroll(spec);
        self
    }
    /// [`El::scroll`] in place.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn set_scroll(&mut self, spec: ScrollSpec) -> &mut Self {
        self.viewport(spec, ScrollAxes::Y, taffy::Overflow::Hidden, taffy::Overflow::Scroll);
        self
    }
    #[cfg_attr(debug_assertions, track_caller)]
    fn viewport(&mut self, spec: ScrollSpec, axes: ScrollAxes, x: taffy::Overflow, y: taffy::Overflow) {
        let cold = self.cold_mut();
        cold.scroll = Some(spec);
        cold.scroll_axes = axes;
        #[cfg(debug_assertions)]
        { cold.scroll_source = Some(std::panic::Location::caller()); }
        self.style.set_overflow(x, y);
    }
    /// A horizontal scroll viewport over its children: a filmstrip, a wide code block or
    /// table, a tab strip that overflows.
    ///
    /// The content is laid out at its natural width, unwrapped as SwiftUI's
    /// `ScrollView(.horizontal)` proposes, and at least as wide as the viewport (CSS
    /// `width: max-content; min-width: 100%`), and never shrinks. `w_full` content does the
    /// same; any other width it sets stays, narrower than the viewport too. The viewport takes its parent's width as any element does; its height is the
    /// content's unless set, and the content fills a set height unless it sets its own.
    ///
    /// The wheel, a trackpad and touch move it like a vertical viewport, with Rune's own
    /// momentum after a fling. A mostly-vertical trackpad swipe or touch pan that starts over
    /// it scrolls the vertical viewport around it instead, and the axis holds for the whole
    /// gesture, as in AppKit. Shift with a mouse wheel scrolls it on Windows, Linux and the web
    /// (macOS does that itself). Left and Right arrows don't scroll it (text fields own them),
    /// nor do Page Up and Page Down. [`El::scroll_edge_fade`] fades its left and right edges,
    /// and [`Tx::scroll_into_view`] scrolls it to show a row. `El::virtualize` and
    /// `El::virtual_rows` window vertical lists only.
    #[cfg_attr(debug_assertions, track_caller)]
    #[inline]
    pub fn scroll_x(mut self) -> Self {
        self.set_scroll_x();
        self
    }
    /// [`El::scroll_x`] in place.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn set_scroll_x(&mut self) -> &mut Self {
        self.viewport(ScrollSpec { stick_bottom: false }, ScrollAxes::X, taffy::Overflow::Scroll, taffy::Overflow::Hidden);
        self
    }
    /// A viewport that scrolls both ways over its children (a wide timeline, a large
    /// table): [`El::scroll`] and [`El::scroll_x`] together. A trackpad gesture moves it along
    /// the axis it starts on, as AppKit's predominant-axis scrolling does. It keeps the
    /// `stick_bottom` of an earlier [`El::scroll`] call. Needs a bounded height like
    /// [`El::scroll`]. The content takes its natural width as in [`El::scroll_x`] and its
    /// natural height.
    #[cfg_attr(debug_assertions, track_caller)]
    #[inline]
    pub fn scroll_xy(mut self) -> Self {
        self.set_scroll_xy();
        self
    }
    /// [`El::scroll_xy`] in place.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn set_scroll_xy(&mut self) -> &mut Self {
        let spec = self.cold.scroll.unwrap_or(ScrollSpec { stick_bottom: false });
        self.viewport(spec, ScrollAxes::Both, taffy::Overflow::Scroll, taffy::Overflow::Scroll);
        self
    }
    /// On a scroll viewport, called with whether it shows its bottom (within a point): once
    /// after its first layout, then only when that changes, never per frame. Pair with
    /// [`Tx::scroll_to_bottom`] for a "jump to latest" button.
    pub fn on_at_bottom<S: 'static>(mut self, f: impl Fn(&mut S, bool) + 'static) -> Self {
        self.handlers.on_at_bottom = Some(Rc::new(move |s, at| f(downcast::<S>(s), at)));
        self
    }
    /// On a scroll viewport, called with whether it stands more than its own height, and at
    /// least `min` points, above its bottom: where a chat shows its "jump to latest" button and
    /// hides it again near the end. Once after its first layout, then only when that changes.
    pub fn on_far_from_bottom<S: 'static>(mut self, min: f32, f: impl Fn(&mut S, bool) + 'static) -> Self {
        self.handlers.on_far_from_bottom = Some((min, Rc::new(move |s, far| f(downcast::<S>(s), far))));
        self
    }
    /// On a scroll viewport, fade its top and bottom edges over `width` points (16–32 reads well),
    /// each only while content is hidden under it: the fade's strength is the hidden extent / 15
    /// points, so a viewport at its top (or bottom, or stuck to it) shows that edge hard. Replaces
    /// any top/bottom [`El::clip_fade`] widths; left/right ones still apply. On a horizontal
    /// viewport ([`El::scroll_x`]) the left and right edges fade the same way instead, and on
    /// [`El::scroll_xy`] all four. Sticky elements ([`El::sticky_top`], [`El::sticky_bottom`],
    /// [`El::sticky_left`]) are not faded on the edge they stick to, so a header's fill stays
    /// opaque while rows scrolling under it fade. This also applies before the element sticks;
    /// explicit [`El::clip_fade`] masks and outer viewports' fades still apply. Pair with
    /// [`El::scroll_margin`] so rows brought into view stop clear of the fade.
    pub fn scroll_edge_fade(self, width: f32) -> Self {
        self.scroll_fade(ScrollFade::strength(width))
    }
    /// On a scroll viewport, fade its edges with the content hidden under them, as `fade`
    /// describes: [`ScrollFade::strength`] (as [`El::scroll_edge_fade`]), a
    /// [`ScrollFade::feather`] as wide as what's hidden, linear ramps, or a top fade under the
    /// header stuck there. Everything [`El::scroll_edge_fade`] says of sticky elements, explicit
    /// masks and [`El::scroll_margin`] applies.
    pub fn scroll_fade(mut self, fade: ScrollFade) -> Self {
        self.cold_mut().scroll_edges.fade = fade.width;
        self.cold_mut().scroll_edges.fade_style = fade.style;
        self
    }
    /// On a scroll viewport, the space [`Tx::scroll_into_view`] leaves between a revealed row and
    /// the edge it scrolls to, like CSS `scroll-padding`. The content's own ends still stop flush.
    pub fn scroll_margin(mut self, pts: f32) -> Self {
        self.cold_mut().scroll_edges.margin = pts.max(0.0);
        self
    }
    /// On a vertical scroll viewport, keep the row at the top of the view where it is when a
    /// commit changes the content above it (rows inserted, removed, grown or reflowed by a new
    /// width), as CSS `overflow-anchor: auto` does in a browser. The row is the first child
    /// drawn in the view, or within a child that starts above the view, the first of its
    /// children, and so on down. It holds only while the viewport is still and scrolled off
    /// its top, and not following its bottom (`stick_bottom`): at the very top new content
    /// above shows. A [`Tx::keep_on_screen`] in the same commit wins, and so does a scroll.
    /// [`El::virtual_rows`] lists already keep their first visible row this way.
    pub fn scroll_anchor(mut self) -> Self {
        self.cold_mut().scroll_edges.anchor = true;
        self
    }
}

/// How a scroll viewport's edges fade with the content hidden under them ([`El::scroll_fade`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollFade {
    width: f32,
    style: FadeStyle,
}

/// The parts of a [`ScrollFade`] besides its width, kept on the viewport.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct FadeStyle {
    /// The fade is as wide as what's hidden, up to its width, rather than full width at a
    /// strength that follows it.
    pub(crate) feather: bool,
    pub(crate) linear: bool,
    /// The top fade starts under the header stuck there, for the rows of that header's section.
    pub(crate) below_sticky: bool,
    /// Only the bottom (or right) edge follows the scroll; the other keeps the viewport's clip.
    pub(crate) end_only: bool,
    /// Points hidden at which a strength fade is full; 0 for 15.
    pub(crate) full_at: f32,
    /// The end edge's own fade (`ScrollFade::with_end`): its width, linear ramp and full-at.
    pub(crate) end: Option<EndFade>,
}

/// How the end edge (bottom, or right) fades when it differs from the start (`ScrollFade::with_end`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct EndFade {
    pub(crate) width: f32,
    pub(crate) linear: bool,
    pub(crate) full_at: f32,
}

impl FadeStyle {
    pub(crate) const SMOOTH: FadeStyle = FadeStyle { feather: false, linear: false, below_sticky: false, end_only: false, full_at: 0.0, end: None };

    /// The style and width (points) the edge with `end` set fades by: the end edge's own when
    /// [`ScrollFade::with_end`] gave it one.
    pub(crate) fn edge(self, width: f32, end: bool) -> (FadeStyle, f32) {
        match self.end.filter(|_| end) {
            Some(e) => (FadeStyle { linear: e.linear, full_at: e.full_at, end: None, ..self }, e.width),
            None => (self, width),
        }
    }
}

impl ScrollFade {
    /// Each edge fades over `width` points (16-32 reads well) at a strength that follows the
    /// content hidden under it, full once 15 points are hidden: the edge itself shows
    /// `1 - strength`. A viewport at its top (or bottom, or stuck to it) shows that edge hard.
    /// Desktop's search lists fade so (`search-list.tsx`).
    pub fn strength(width: f32) -> Self {
        ScrollFade { width: width.max(0.0), style: FadeStyle::SMOOTH }
    }
    /// Each edge feathers by as much as is hidden under it, up to `max` points, on a linear ramp
    /// from nothing at the edge: CSS `mask-image: linear-gradient(transparent, black
    /// min(max, scrollTop))`, as Desktop's vertical tabs do. It appears and leaves without a jump.
    pub fn feather(max: f32) -> Self {
        ScrollFade { width: max.max(0.0), style: FadeStyle { feather: true, linear: true, ..FadeStyle::SMOOTH } }
    }
    /// A [`Self::strength`] fade is full once `points` are hidden, rather than 15: a small value
    /// shows the whole fade as soon as anything is hidden, as a mask a scroll listener toggles
    /// on and off does (Desktop's projects list).
    pub fn full_at(mut self, points: f32) -> Self {
        self.style.full_at = points.max(0.01);
        self
    }
    /// Only the end the content runs on past (the bottom, or the right of a sideways viewport)
    /// follows the scroll; the start keeps the viewport's own [`El::clip_fade`] or
    /// [`El::clip_fade_linear`], such as a fixed fade over a field's top padding.
    pub fn end_only(mut self) -> Self {
        self.style.end_only = true;
        self
    }
    /// Ramps alpha linearly across the fade, as a CSS `linear-gradient` mask does, rather than
    /// on smoothstep.
    pub fn linear(mut self) -> Self {
        self.style.linear = true;
        self
    }
    /// The top fade starts under the [`El::sticky_top`] header stuck at the viewport's top, for
    /// the rows of that header's section (the header's later siblings), at the strength of what
    /// the section hides under the header; a header being pushed away by the next leaves the
    /// rows clear, and the incoming one starts clear. With no header stuck, the viewport's own
    /// top fades. Desktop's palettes fade their sections so (`search-list.css`), drawing the
    /// fade over the rows in the list's surface colour.
    pub fn below_sticky(mut self) -> Self {
        self.style.below_sticky = true;
        self
    }
    /// The end edge (the bottom, or the right of a sideways viewport) fades as `end` does, with
    /// its own width, ramp and strength, while the start keeps this fade: Desktop's model dialog
    /// fades its rows 16 points under a stuck header on an eased curve but 24 points at its bottom
    /// on a linear mask (`search-list.css`). Only `end`'s width, ramp and full-at count.
    pub fn with_end(mut self, end: ScrollFade) -> Self {
        self.style.end = Some(EndFade { width: end.width, linear: end.style.linear, full_at: end.style.full_at });
        self
    }
}
