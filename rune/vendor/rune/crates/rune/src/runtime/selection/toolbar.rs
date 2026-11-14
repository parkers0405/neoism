//! What a toolbar floating over a text selection needs from the selection: which way it was
//! made ([`TextSelection::backward`](crate::TextSelection::backward)), and controls a press on
//! which leaves the selection and the keyboard focus as they are
//! ([`El::keeps_text_selection`]). Where its line boxes are is
//! [`Cx::selection_rects`](crate::Cx::selection_rects).
use super::*;

/// What a press on an element and its descendants leaves as it is.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct OnPress {
    /// The focused field ([`El::preserve_focus_on_press`]).
    pub(crate) focus: bool,
    /// The text selection and the focus ([`El::keeps_text_selection`]).
    pub(crate) selection: bool,
}

impl El {
    /// A press on this element or anything inside it leaves the
    /// [`El::selectable`] text selection and the keyboard focus as they are, as a web page's
    /// `preventDefault` on `mousedown` does: a Quote or Copy button floating over a selection
    /// can read it when pressed ([`Tx::text_selection`]), and a drag that starts on it doesn't
    /// start a new one. Focusable controls inside don't take focus either. The press still
    /// reaches `on_press` and the other handlers.
    pub fn keeps_text_selection(mut self) -> Self {
        self.cold_mut().focus.preserve_on_press.selection = true;
        self
    }
}

impl<S: 'static> Runtime<S> {
    /// What a press on node `i` leaves as it is, from it and its ancestors.
    fn on_press_keeps(&self, i: usize) -> OnPress {
        std::iter::successors(Some(i), |&p| self.nodes[p].parent)
            .take_while(|&p| self.allowed(p))
            .map(|p| self.nodes[p].focus.preserve_on_press)
            .fold(OnPress::default(), |a, b| OnPress { focus: a.focus || b.focus, selection: a.selection || b.selection })
    }

    /// A press on node `i` keeps the text selection (and so the focus too).
    pub(in crate::runtime) fn press_keeps_selection(&self, i: usize) -> bool {
        self.on_press_keeps(i).selection
    }

    /// A press on node `i` leaves the focused field focused.
    pub(in crate::runtime) fn press_keeps_focus(&self, i: usize) -> bool {
        let keeps = self.on_press_keeps(i);
        keeps.focus || keeps.selection
    }

    /// The selection side of a primary press on `target` at (x, y): it starts or clears a
    /// selection, unless the press is on an [`El::keeps_text_selection`] element.
    pub(in crate::runtime) fn press_selection(&mut self, target: Option<usize>, x: f32, y: f32) {
        if target.is_some_and(|t| self.press_keeps_selection(t)) { return; }
        self.selection_down(self.selectable_at(x, y), x, y, false);
    }

    /// The selection's focus comes before its anchor.
    pub(in crate::runtime) fn selection_backward(&self) -> bool {
        let Some(h) = self.selection_holder_end() else { return false };
        match self.focus_holder_end(h) {
            Some(f) if f != h => self.tree_order(f) < self.tree_order(h),
            Some(_) => self.text_sel.focus < self.text_sel.anchor,
            None => false,
        }
    }
}
