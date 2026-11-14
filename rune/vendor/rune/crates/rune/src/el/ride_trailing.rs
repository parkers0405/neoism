//! `El::ride_trailing`: an element that keeps its place against its parent's trailing edge.
use crate::el::El;

impl El {
    /// While its parent's width animates, this element keeps its place against the parent's
    /// trailing (right) edge instead of riding its leading one. Children normally ride the
    /// parent's left edge, so in a row that widens to the left (right-aligned, or against a
    /// spacer) a child that holds its column moves right in the row on its own spring while the
    /// row's left edge moves left on another, and on screen it drifts out and back. Riding the
    /// trailing edge it doesn't move at all, whatever springs the ancestors use; one that also
    /// moves against that edge travels only that difference, on its own curve. Children
    /// leaving hold their place the same way.
    pub fn ride_trailing(mut self) -> Self {
        self.vis_flags.set(crate::el::VisualFlags::RIDE_TRAILING, true);
        self
    }
}
