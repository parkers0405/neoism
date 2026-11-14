//! Anchored overlays: `El::anchored`, `El::anchored_fit` and `El::above`. Where they go is
//! worked out in `runtime/anchor.rs`.
use crate::el::{Align, El};

/// Which side of its anchor an [`El::anchored`] overlay sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Top,
    Bottom,
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Anchor {
    pub key: String,
    pub side: Side,
    pub align: Align,
    pub gap: f32,
    /// [`El::anchored_fit`]: at most as long as the room on the side it goes to.
    pub fit: bool,
    /// [`El::anchored_unscaled`]: at window scale, not its anchor's zoom.
    pub unscaled: bool,
    /// [`El::anchored_round`]: at whole points.
    pub round: bool,
}

impl El {
    /// Position an out-of-flow overlay above an earlier keyed element, clamped to the window.
    /// Attach the overlay at the root to escape the anchor's clipping ancestors.
    pub fn above(self, key: impl Into<String>, gutter: f32) -> Self {
        self.anchored(key, Side::Top, Align::Start, gutter)
    }
    /// Position an out-of-flow overlay on `side` of an earlier keyed element, aligned along that
    /// side, `gap` points away. It flips to the opposite side when it doesn't fit, and stays
    /// 8 points inside the window. Attach it at the root (after the anchor) so it draws above
    /// later siblings and escapes clipping ancestors.
    ///
    /// It draws at its anchor's zoom: inside a page drawn at [`El::scale`]`(1.6)`, the overlay
    /// and its text draw 1.6 times their size too, growing away from the anchor. Its gap zooms
    /// with it, and flipping, fitting and the 8-point window margin use its drawn size. Its own
    /// [`El::scale`] replaces the inherited zoom; [`El::anchored_unscaled`] keeps it at window
    /// scale.
    pub fn anchored(mut self, key: impl Into<String>, side: Side, align: Align, gap: f32) -> Self {
        self.style.set_position_absolute();
        self.cold_mut().above = Some(Anchor { key: key.into(), side, align, gap, fit: false, unscaled: false, round: false });
        self
    }
    /// [`El::anchored`], sized to the window as Floating UI's `size` middleware sizes a
    /// dropdown: when the overlay fits on neither side, it goes to the side with more room and
    /// is at most that room long (tall beside a Top or Bottom anchor, wide beside Left or
    /// Right), 8 points inside the window. Let a scroll viewport inside it shrink (`.shrink()`
    /// and `.min_h(0.0)` on it and on the columns around it), so a long list scrolls instead of
    /// running off the window.
    pub fn anchored_fit(self, key: impl Into<String>, side: Side, align: Align, gap: f32) -> Self {
        let mut e = self.anchored(key, side, align, gap);
        if let Some(a) = e.cold.get_mut_if_some().and_then(|c| c.above.as_mut()) {
            a.fit = true;
        }
        e
    }
    /// Keep this [`El::anchored`] overlay at window scale instead of its anchor's zoom, for an
    /// overlay that zooms its own content (or must not zoom). Call it after `anchored`.
    pub fn anchored_unscaled(mut self) -> Self {
        if let Some(a) = self.cold.get_mut_if_some().and_then(|c| c.above.as_mut()) {
            a.unscaled = true;
        }
        self
    }
    /// Put this [`El::anchored`] overlay at whole points, rounding where it goes as Kobalte's
    /// popper rounds Floating UI's position (`Math.round` on x and y), so a menu under a trigger
    /// at half a point lands where a web menu does. Call it after `anchored`.
    pub fn anchored_round(mut self) -> Self {
        if let Some(a) = self.cold.get_mut_if_some().and_then(|c| c.above.as_mut()) {
            a.round = true;
        }
        self
    }
}

/// Where an overlay of `size` (width, height) goes next to `trigger` (x0, y0, x1, y1) in a
/// `window` of that size, as the rect (x0, y0, x1, y1) [`El::anchored`] would place it at: on
/// `side` of the trigger, aligned along it, `gap` points away, flipped to the opposite side
/// (and alignment) when it doesn't fit, then kept 8 points inside the window.
///
/// ```
/// use rune::{Align, Side, anchor_rect};
/// // A 100 x 80 menu under a trigger near the bottom of a 400 x 300 window flips above it.
/// let r = anchor_rect([20.0, 250.0, 60.0, 270.0], (100.0, 80.0), Side::Bottom, Align::Start, 4.0, (400.0, 300.0));
/// assert_eq!(r, [20.0, 166.0, 120.0, 246.0]);
/// ```
pub fn anchor_rect(trigger: [f32; 4], size: (f32, f32), side: Side, align: Align, gap: f32, window: (f32, f32)) -> [f32; 4] {
    let a = Anchor { key: String::new(), side, align, gap, fit: false, unscaled: false, round: false };
    let (x, y) = crate::runtime::anchor_position(&a, trigger, size, window);
    [x, y, x + size.0, y + size.1]
}
