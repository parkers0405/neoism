//! `El::pixel_snap`. The snapping itself is in `runtime/paint/pixel_snap.rs`.
use crate::el::El;

impl El {
    /// Whether this element and its children snap their fills, borders and clips to whole
    /// device pixels when they paint. On by default: a box laid out at a fractional position
    /// (a 1.1 pt gap at 2×) still draws hard edges, and boxes that share an edge still meet
    /// with no seam. Layout, hit testing and [`Cx::layout_size`](crate::Cx::layout_size) keep
    /// the exact fractional rects either way.
    ///
    /// While an element animates, its correction eases toward the pixels for where it comes to
    /// rest, in step with the distance left, so motion stays smooth and starts and lands
    /// without a jump. Content under an `El::scale` (a zoomed canvas) draws at its exact
    /// sub-pixel edges while the scale moves and eases onto whole pixels once it holds still.
    /// Turn it off for an element an app moves itself frame by frame at sub-pixel speeds
    /// (`El::offset` from a value it animates), so it glides instead of stepping a pixel at a
    /// time.
    pub fn pixel_snap(mut self, on: bool) -> Self {
        self.vis_flags.set(crate::el::VisualFlags::NO_PIXEL_SNAP, !on);
        self
    }
}
