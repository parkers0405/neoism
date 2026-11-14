//! Live colours past the fill: a text's colour, the border, the first shadow and a two-stop
//! gradient, bound like [`El::live_fill`] so a tint that drifts repaints without a view rerun.
use crate::live::{Binding, Bound, Property};
use crate::{Color, El};

impl El {
    /// Bind a text's colour (or an icon's), optionally smoothed with `Live::smooth`. Views and
    /// layout don't rerun; the text isn't shaped again.
    pub fn live_text_color(mut self, value: impl Into<Binding<Color>>) -> Self {
        self.bind_live(Bound::new(value, Property::TextColor));
        self
    }
    /// Bind the border's colour. The border's width comes from [`El::border`] as usual.
    pub fn live_border(mut self, value: impl Into<Binding<Color>>) -> Self {
        self.bind_live(Bound::new(value, Property::Border));
        self
    }
    /// Bind the first shadow's colour. Its offset and blur come from [`El::shadow`] as usual.
    pub fn live_shadow(mut self, value: impl Into<Binding<Color>>) -> Self {
        self.bind_live(Bound::new(value, Property::Shadow));
        self
    }
    /// Bind both stops of the vertical gradient [`El::gradient`] sets, top and bottom.
    pub fn live_gradient(mut self, top: impl Into<Binding<Color>>, bottom: impl Into<Binding<Color>>) -> Self {
        self.bind_live(Bound::new(top, Property::GradientTop));
        self.bind_live(Bound::new(bottom, Property::GradientBottom));
        self
    }
}
