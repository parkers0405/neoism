//! How a text field paints its selection ([`El::selection_style`](crate::El::selection_style)):
//! the highlight under the selected text and the ink the selected glyphs take, while the window
//! is active and while it isn't, as a browser paints a field's `::selection`.

use crate::color::Color;

/// A field's selection colours: active while the window has focus, inactive while it doesn't.
/// `None` ink keeps the text's own colours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectionStyle {
    active: Color,
    active_ink: Option<Color>,
    inactive: Color,
    inactive_ink: Option<Color>,
}

impl SelectionStyle {
    /// Chromium's default selection on Windows and Linux in either color scheme: its
    /// `kDefaultActiveSelectionBgColor` (#1967D2) and `kDefaultInactiveSelectionBgColor`
    /// (#C8C8C8) made see-through by `Color::BlendWithWhite` (alpha 0.8, so they look the same
    /// over white), with white text, and #323232 text while the window is inactive.
    pub fn chromium() -> Self {
        let c = |h: &str| crate::color::hex(h).unwrap_or(Color::TRANSPARENT);
        SelectionStyle { active: c("#0041c6cc"), active_ink: Some(Color::WHITE), inactive: c("#bababacc"), inactive_ink: Some(c("#323232")) }
    }

    /// `highlight` under the selected text, active or not, the text keeping its colours.
    pub const fn new(highlight: Color) -> Self {
        SelectionStyle { active: highlight, active_ink: None, inactive: highlight, inactive_ink: None }
    }

    /// The selected glyphs take `ink` while the window is active.
    pub const fn ink(mut self, ink: Color) -> Self {
        self.active_ink = Some(ink);
        self
    }

    /// While the window is inactive, `highlight` under the selected text and `ink`, if any, on it.
    pub const fn inactive(mut self, highlight: Color, ink: Option<Color>) -> Self {
        self.inactive = highlight;
        self.inactive_ink = ink;
        self
    }

    /// The highlight and ink for a window that is `active`.
    pub(crate) fn pick(&self, active: bool) -> (Color, Option<Color>) {
        if active { (self.active, self.active_ink) } else { (self.inactive, self.inactive_ink) }
    }
}
