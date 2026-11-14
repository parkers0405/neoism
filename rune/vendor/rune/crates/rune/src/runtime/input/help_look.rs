//! How Rune draws [`El::help`](crate::El::help) tooltips: the system look by default, or an
//! app's own ([`Cx::help_look`](crate::Cx::help_look)), as a web page styles its tooltips.
use crate::el::Shadow;
use crate::{Color, TextStyle};

/// The most box shadows a tooltip draws.
const SHADOWS: usize = 4;

/// A tooltip's surface, type, placement and timing. [`HelpLook::system`] is Rune's own look;
/// each builder overrides one thing, e.g. a design system's compact tooltip:
/// `HelpLook::system(dark).fill(layer).ink(text).radius(4.0).padding(6.0, 5.0).font(small).gap(4.0).delay(0.4)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HelpLook {
    pub(crate) fill: Color,
    pub(crate) ink: Color,
    pub(crate) ring: Option<(f32, Color)>,
    pub(crate) radius: f32,
    pub(crate) padding: [f32; 2],
    pub(crate) font: TextStyle,
    pub(crate) max_w: Option<f32>,
    /// Points between the element and a tooltip under it, and over it.
    pub(crate) gap: [f32; 2],
    /// The tooltip keeps this far inside the window.
    pub(crate) margin: f32,
    /// `None`: the runtime's `help_delay`.
    pub(crate) delay: Option<f64>,
    pub(crate) skip: f64,
    /// Seconds the tooltip fades in and out (`HelpLook::fade`); 0 shows or hides at once.
    pub(crate) fade: [f64; 2],
    /// The scale it fades in from and out to.
    pub(crate) fade_scale: f32,
    pub(crate) shadows: [Option<Shadow>; SHADOWS],
    /// [`El::help_keys`](crate::El::help_keys)' keycaps.
    pub(crate) key: KeyCap,
}

/// How a tooltip draws its keycaps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct KeyCap {
    pub(crate) fill: Color,
    pub(crate) ink: Color,
    pub(crate) font: TextStyle,
    pub(crate) height: f32,
    pub(crate) pad_x: f32,
    pub(crate) radius: f32,
    /// Points between two keycaps.
    pub(crate) gap: f32,
    /// Points between the text and the first keycap.
    pub(crate) lead: f32,
    pub(crate) upper: bool,
}

impl HelpLook {
    /// Rune's own tooltip for a light or dark window: a pale or dark grey card with a hairline
    /// edge, 12 pt text, under its element 6 pt clear (over it 4 pt clear), after the
    /// runtime's `help_delay`.
    pub fn system(dark: bool) -> HelpLook {
        let grey = |v: f32| Color { r: v, g: v, b: v, a: 1.0 };
        let (fill, ink, edge) = if dark {
            (grey(0.17), grey(0.93), Color::WHITE.alpha(0.14))
        } else {
            (grey(0.98), grey(0.11), Color::BLACK.alpha(0.16))
        };
        HelpLook {
            fill,
            ink,
            ring: Some((1.0, edge)),
            radius: 5.0,
            padding: [7.0, 4.0],
            font: TextStyle::new(12.0, 400.0).lh(16.0),
            max_w: Some(300.0),
            gap: [6.0, 4.0],
            margin: 4.0,
            delay: None,
            skip: 0.0,
            fade: [0.0; 2],
            fade_scale: 1.0,
            shadows: [None; SHADOWS],
            key: KeyCap {
                fill: ink.alpha(0.08),
                ink: ink.alpha(0.7),
                font: TextStyle::new(11.0, 500.0).lh(11.0),
                height: 16.0,
                pad_x: 4.0,
                radius: 3.0,
                gap: 2.0,
                lead: 6.0,
                upper: false,
            },
        }
    }
    /// Keycaps ([`El::help_keys`](crate::El::help_keys)) on `fill`, their labels in `font` and
    /// `ink`, uppercased when `upper`.
    pub fn keycaps(mut self, fill: Color, ink: Color, font: TextStyle, upper: bool) -> Self {
        self.key = KeyCap { fill, ink, font, upper, ..self.key };
        self
    }
    /// A keycap's height, inner padding either side (its least width is its height) and corner
    /// radius, `gap` points between two and `lead` points after the text.
    pub fn keycap_shape(mut self, height: f32, pad_x: f32, radius: f32, gap: f32, lead: f32) -> Self {
        self.key = KeyCap { height: height.max(0.0), pad_x: pad_x.max(0.0), radius: radius.max(0.0), gap: gap.max(0.0), lead: lead.max(0.0), ..self.key };
        self
    }
    pub fn fill(mut self, c: Color) -> Self {
        self.fill = c;
        self
    }
    /// The text colour.
    pub fn ink(mut self, c: Color) -> Self {
        self.ink = c;
        self
    }
    /// A border `width` points wide inside the edge; 0 removes it.
    pub fn ring(mut self, width: f32, c: Color) -> Self {
        self.ring = (width > 0.0 && width.is_finite()).then_some((width, c));
        self
    }
    pub fn radius(mut self, r: f32) -> Self {
        self.radius = r.max(0.0);
        self
    }
    /// Horizontal and vertical padding around the text.
    pub fn padding(mut self, x: f32, y: f32) -> Self {
        self.padding = [x.max(0.0), y.max(0.0)];
        self
    }
    pub fn font(mut self, f: TextStyle) -> Self {
        self.font = f;
        self
    }
    /// The widest the tooltip grows before its text wraps; `None` for the window's width.
    pub fn max_width(mut self, w: Option<f32>) -> Self {
        self.max_w = w.filter(|w| *w > 0.0);
        self
    }
    /// Points between the element and its tooltip, under it or over it (Kobalte's `gutter`).
    pub fn gap(mut self, g: f32) -> Self {
        self.gap = [g, g];
        self
    }
    /// How far the tooltip keeps inside the window's edges (a popper's overflow padding).
    pub fn margin(mut self, m: f32) -> Self {
        self.margin = m.max(0.0);
        self
    }
    /// Seconds the pointer rests on an element before its tooltip shows, in place of the
    /// runtime's `help_delay`.
    pub fn delay(mut self, secs: f64) -> Self {
        self.delay = Some(secs.max(0.0));
        self
    }
    /// For this long (seconds) after a tooltip hides, the next element's shows at once, as
    /// a web tooltip's `skipDelayDuration` does when the pointer moves along a toolbar.
    pub fn skip_delay(mut self, secs: f64) -> Self {
        self.skip = secs.max(0.0);
        self
    }
    /// The tooltip fades in over `enter` seconds (ease-out) and out over `exit` (ease-in),
    /// growing from `scale` and shrinking back to it about the middle of the edge that faces
    /// its element, as a web tooltip's open and close keyframes do. One closing as the next
    /// opens (within the skip delay) fades out under it. 0 shows or hides at once, the
    /// default; reduced motion never fades.
    pub fn fade(mut self, enter: f64, exit: f64, scale: f32) -> Self {
        self.fade = [enter.max(0.0), exit.max(0.0)];
        self.fade_scale = if scale.is_finite() { scale.clamp(0.0, 1.0) } else { 1.0 };
        self
    }
    /// Box-shadow layers, painted in order (the first four); empty for none. Inset shadows
    /// are skipped.
    pub fn shadows(mut self, layers: impl IntoIterator<Item = Shadow>) -> Self {
        self.shadows = [None; SHADOWS];
        for (slot, s) in self.shadows.iter_mut().zip(layers.into_iter().filter(|s| !s.inset)) {
            *slot = Some(s);
        }
        self
    }
}
