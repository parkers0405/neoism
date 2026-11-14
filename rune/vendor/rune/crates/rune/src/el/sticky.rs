//! Sticky elements: `El::sticky_top`, `El::sticky_top_showing`, `El::sticky_bottom`,
//! `El::sticky_left` and `El::stuck_shadow`. The shifts are worked out in `runtime/scroll/sticky.rs`.
use crate::el::El;
use crate::el::Shadow;

/// Which viewport edge an [`El::sticky_top`], [`El::sticky_bottom`] or [`El::sticky_left`]
/// element holds to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StickyEdge { Top, Bottom, Left }

/// `El::sticky_top`, `El::sticky_bottom`, `El::sticky_left` and `El::stuck_shadow`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Sticky {
    pub edge: Option<StickyEdge>,
    pub inset: f32,
    /// `inset` counts from the element's bottom (`El::sticky_top_showing`).
    pub tail: bool,
    pub shadows: Vec<Shadow>,
    /// `El::stuck_fade`: lead and span, in points.
    pub fade: Option<[f32; 2]>,
    /// `El::stuck_reveal`: the parent's shift, in points, at which this element shows whole.
    pub reveal: Option<f32>,
}

impl El {
    /// Hold this element `inset` points below the top of the nearest vertical scroll viewport
    /// around it while its parent is in view, as CSS `position: sticky; top: inset` does: a
    /// section header that stays put while its section scrolls under it. Its parent's bottom
    /// edge (inside its padding) pushes it up and out, so the next section's header takes over.
    /// It never moves above where scrolling puts it.
    ///
    /// The shift is worked out each frame from that frame's scroll offset, so it never lags a
    /// scroll. Layout doesn't move; drawing, hit testing, the focus ring, children,
    /// [`El::on_drawn_layout`] and [`ElementInfo::rect`](crate::runtime::ElementInfo) follow the
    /// drawn place, and [`ElementInfo::stuck`](crate::runtime::ElementInfo) reads the shift. It
    /// draws, and hit tests, above its later siblings, and [`El::scroll_edge_fade`] skips the
    /// edge it sticks to so its fill and text stay opaque while rows scrolling under it fade.
    /// Give it a fill, so rows don't show through, and see [`El::stuck_shadow`] for a shadow or
    /// divider while it's stuck.
    ///
    /// In [`El::virtual_rows`] a header sticks inside its own row: make each section one row
    /// (its header and its items) so the header shows whenever any of the section does. A
    /// header in a separate virtual row disappears when that row is unmounted. Without a
    /// vertical scroll ancestor this is a no-op (a horizontal-only viewport is skipped).
    /// Cached subtrees containing sticky elements draw live rather than baking their shifts.
    pub fn sticky_top(mut self, inset: f32) -> Self {
        let s = self.cold_mut().sticky.get_or_insert_with(Default::default);
        s.edge = Some(StickyEdge::Top);
        s.inset = if inset.is_finite() { inset } else { 0.0 };
        s.tail = false;
        self
    }
    /// [`El::sticky_top`] held with only its bottom `show` points in view: an item page's header
    /// that scrolls up until just its bar is left, then stays. Unlike `sticky_top(show - h)` it
    /// follows the element's height as laid out in the frame that draws it, so a header that
    /// grows or a new page's taller one sticks in the right place on its first frame.
    pub fn sticky_top_showing(mut self, show: f32) -> Self {
        self = self.sticky_top(show);
        if let Some(s) = self.cold.get_mut_if_some().and_then(|c| c.sticky.as_mut()) { s.tail = true; }
        self
    }
    /// [`El::sticky_top`] against the viewport's bottom edge: held `inset` points above it while
    /// its parent is in view, and pushed down by its parent's top edge (a section footer).
    pub fn sticky_bottom(mut self, inset: f32) -> Self {
        let s = self.cold_mut().sticky.get_or_insert_with(Default::default);
        s.edge = Some(StickyEdge::Bottom);
        s.inset = if inset.is_finite() { inset } else { 0.0 };
        self
    }
    /// Hold this element `inset` points right of the left edge of the nearest horizontal scroll
    /// viewport around it ([`El::scroll_x`] or [`El::scroll_xy`]) while its parent is in view:
    /// [`El::sticky_top`] on the other axis. Give it the viewport's width (not the scrolling
    /// content's) so it spans what shows: a row header whose end controls stay in view while
    /// the code under it scrolls sideways (horizontal scrolling starts at the left edge, so
    /// there is no `sticky_right`). Its parent's right edge pushes it out. Without a
    /// horizontal scroll ancestor this is a no-op. [`ElementInfo::stuck`](crate::runtime::ElementInfo)
    /// reads the shift to the right.
    pub fn sticky_left(mut self, inset: f32) -> Self {
        let s = self.cold_mut().sticky.get_or_insert_with(Default::default);
        s.edge = Some(StickyEdge::Left);
        s.inset = if inset.is_finite() { inset } else { 0.0 };
        self
    }
    /// A shadow drawn only while this [`El::sticky_top`], [`El::sticky_bottom`] or
    /// [`El::sticky_left`] element is stuck, fading in over its first 8 points of shift, so it
    /// follows the scroll exactly: an elevation under a stuck header, or a hairline divider with
    /// `Shadow { y: 0.5, color, ..Default::default() }` under an opaque one. Several stack.
    pub fn stuck_shadow(mut self, shadow: Shadow) -> Self {
        self.cold_mut().sticky.get_or_insert_with(Default::default).shadows.push(shadow);
        self
    }
    /// Fades this [`El::sticky_top`] element's first child (its title), not its own fill or its
    /// other children (a cover above it, a gradient under it), as the next sticky-top sibling
    /// rises to it: whole while that sibling's top is `lead + span` points or
    /// more below this one's drawn top, gone once it's `lead` below, linearly between, frame by
    /// frame with the scroll. Desktop's Home fades a stuck day header's title so as the next day
    /// comes up (`home/scroll.ts`), its fill still covering the rows under it until the next
    /// header, a later sibling drawn over it, takes its place. A header with no sticky sibling
    /// after it stays whole.
    pub fn stuck_fade(mut self, lead: f32, span: f32) -> Self {
        self.cold_mut().sticky.get_or_insert_with(Default::default).fade = Some([lead, span.max(0.0)]);
        self
    }
    /// Shows this child of a [`El::sticky_top`] element as far as its parent is stuck: hidden
    /// at rest, whole once the parent has held `span` points, linearly between, frame by frame
    /// with the scroll. A gradient hung under a stuck header with it fades in as rows scroll
    /// beneath, as Desktop's model dialog fades its header's `::after` by how far its rows have
    /// scrolled under it (`select-dialog.tsx` `--model-dialog-header-fade`).
    pub fn stuck_reveal(mut self, span: f32) -> Self {
        self.cold_mut().sticky.get_or_insert_with(Default::default).reveal = Some(span.max(1e-3));
        self
    }
}
