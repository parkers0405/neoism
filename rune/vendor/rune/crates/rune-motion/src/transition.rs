//! SwiftUI-style composable transitions, as data the scene evaluates and the renderer draws.
//!
//! An [`Effect`] is what a node looks like when fully *absent*; presence progress `p` blends it
//! toward identity (`p = 1`). Transitions pair an insertion and a removal effect.

use crate::spring::SpringPair;
use crate::tokens;
use crate::value::Vec2;

/// A side of a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Edge {
    Leading,
    Trailing,
    Top,
    Bottom,
}

impl Edge {
    /// Unit direction pointing out of the node through this edge (LTR).
    pub fn direction(self) -> Vec2 {
        match self {
            Edge::Leading => Vec2::new(-1.0, 0.0),
            Edge::Trailing => Vec2::new(1.0, 0.0),
            Edge::Top => Vec2::new(0.0, -1.0),
            Edge::Bottom => Vec2::new(0.0, 1.0),
        }
    }
    pub fn opposite(self) -> Edge {
        match self {
            Edge::Leading => Edge::Trailing,
            Edge::Trailing => Edge::Leading,
            Edge::Top => Edge::Bottom,
            Edge::Bottom => Edge::Top,
        }
    }
}

/// A soft-edged wipe: the mask's `edge` side is inset by `(1 − p) ×` the node's size, with a
/// `feather`-point gradient ramp (Kit rule 11: no hard edges).
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Reveal {
    /// The side the content is hidden behind when absent (it grows away from the opposite side).
    pub edge: Edge,
    pub feather: f32,
}

/// What an absent node looks like. Every field's identity value means "no change".
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Effect {
    /// Opacity multiplier when absent (identity 1).
    pub opacity: f32,
    /// Uniform scale when absent (identity 1). Kept within [0.9, 1.1], or
    /// [[`SMALL_SCALE_FLOOR`], 1.1] with `small_scale`: never scale to or from 0.
    pub scale: f32,
    /// Scale anchor in unit coordinates of the node (0.5, 0.5 = centre).
    pub anchor: Vec2,
    /// Offset in points when absent.
    pub offset: Vec2,
    /// Offset in fractions of the node's own size when absent (`.move(edge)`, `.push`).
    pub offset_fraction: Vec2,
    /// Gaussian blur sigma in points when absent (identity 0). At most 4; in-place replacement only.
    pub blur: f32,
    pub reveal: Option<Reveal>,
    /// Lets `scale` go down to [`SMALL_SCALE_FLOOR`] instead of 0.9, for small glyphs
    /// ([`Transition::allow_small_scale`]).
    #[cfg_attr(feature = "serde", serde(default))]
    pub small_scale: bool,
    /// How much of the node's own shape is drawn when absent (identity 1): its strokes trace on
    /// along their length in order, and each fill fades in as the drawing reaches it
    /// ([`Transition::draw`]). Under Reduce Motion it is drawn whole.
    #[cfg_attr(feature = "serde", serde(default = "drawn_whole"))]
    pub draw: f32,
}

#[cfg(feature = "serde")]
fn drawn_whole() -> f32 {
    1.0
}

/// The smallest scale a transition with [`Transition::allow_small_scale`] starts or ends at.
/// Below it a glyph reads as appearing from a point, which the scale rule exists to avoid.
pub const SMALL_SCALE_FLOOR: f32 = 0.5;

impl Default for Effect {
    fn default() -> Self {
        Effect::IDENTITY
    }
}

impl Effect {
    pub const IDENTITY: Effect = Effect {
        opacity: 1.0,
        scale: 1.0,
        anchor: Vec2 { x: 0.5, y: 0.5 },
        offset: Vec2::ZERO,
        offset_fraction: Vec2::ZERO,
        blur: 0.0,
        reveal: None,
        small_scale: false,
        draw: 1.0,
    };

    pub fn opacity(opacity: f32) -> Effect {
        Effect { opacity, ..Effect::IDENTITY }
    }
    pub fn scale(scale: f32) -> Effect {
        Effect { scale, ..Effect::IDENTITY }
    }
    pub fn blur(sigma: f32) -> Effect {
        Effect { blur: sigma, ..Effect::IDENTITY }
    }

    /// Both effects at once: opacities and scales multiply, offsets add, blur takes the max.
    pub fn combined(self, o: Effect) -> Effect {
        Effect {
            opacity: self.opacity * o.opacity,
            scale: self.scale * o.scale,
            anchor: if self.scale != 1.0 { self.anchor } else { o.anchor },
            offset: Vec2::new(self.offset.x + o.offset.x, self.offset.y + o.offset.y),
            offset_fraction: Vec2::new(
                self.offset_fraction.x + o.offset_fraction.x,
                self.offset_fraction.y + o.offset_fraction.y,
            ),
            blur: self.blur.max(o.blur),
            reveal: self.reveal.or(o.reveal),
            small_scale: self.small_scale || o.small_scale,
            draw: self.draw * o.draw,
        }
    }

    pub fn travels(&self) -> bool {
        self.offset != Vec2::ZERO || self.offset_fraction != Vec2::ZERO
    }

    /// Kit's rules, enforced: scale in [0.9, 1.1] (never to or from 0; down to
    /// [`SMALL_SCALE_FLOOR`] with `small_scale`), blur sigma ≤ 4 pt, no blur on anything that
    /// travels (sliding content stays sharp), and nothing leaves at full opacity.
    /// Whether the node looks the same absent as present: no fade, scale, offset, blur, reveal
    /// or drawing (the anchor alone moves nothing).
    pub fn shows_nothing(&self) -> bool {
        Effect { anchor: Effect::IDENTITY.anchor, small_scale: false, ..*self } == Effect::IDENTITY
    }

    pub fn kit_rules(mut self, removal: bool) -> Effect {
        let floor = if self.small_scale { SMALL_SCALE_FLOOR } else { 0.9 };
        self.scale = self.scale.clamp(floor, 1.1);
        self.blur = if self.travels() { 0.0 } else { self.blur.clamp(0.0, 4.0) };
        self.opacity = self.opacity.clamp(0.0, 1.0);
        self.draw = self.draw.clamp(0.0, 1.0);
        if removal {
            self.opacity = 0.0;
        }
        self
    }
}

/// Insertion and removal effects plus how the node behaves while it leaves.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Transition {
    pub insertion: Effect,
    pub removal: Effect,
    /// Motion's `AnimatePresence mode="popLayout"`: an exiting node leaves layout flow at once
    /// (siblings reflow while it fades in place). The default.
    pub pop_layout: bool,
    /// Enter and exit springs. Exits run 10–25 % shorter.
    pub springs: SpringPair,
}

impl Default for Transition {
    fn default() -> Self {
        Transition::opacity()
    }
}

impl Transition {
    fn symmetric(e: Effect) -> Transition {
        Transition { insertion: e, removal: e, pop_layout: true, springs: tokens::pair::SETTLE }
    }

    /// No visual change (presence still holds removal until its fade settles).
    pub fn identity() -> Transition {
        Transition::symmetric(Effect::IDENTITY)
    }
    /// No motion at all: a node shows whole the frame it mounts and is gone the frame it's
    /// removed, through the usual presence lifecycle (it exits invisible and inert, then
    /// unmounts, and focus returns as for any transition). Unlike `Tx::instant`, the rest of
    /// the change still animates; unlike a very short spring, Reduce Motion doesn't stretch it
    /// into a fade that keeps the node on screen.
    pub fn none() -> Transition {
        Transition::identity().springs(SpringPair::NONE)
    }
    /// `.opacity`.
    pub fn opacity() -> Transition {
        Transition::symmetric(Effect::opacity(0.0))
    }
    /// `.scale(s)` about the centre, plus a fade.
    pub fn scale(scale: f32) -> Transition {
        Transition::symmetric(Effect { scale, opacity: 0.0, ..Effect::IDENTITY })
    }
    /// `.scale(s, anchor:)`.
    pub fn scale_anchor(scale: f32, anchor: Vec2) -> Transition {
        Transition::symmetric(Effect { scale, anchor, opacity: 0.0, ..Effect::IDENTITY })
    }
    /// `.offset(x:y:)`.
    pub fn offset(x: f32, y: f32) -> Transition {
        Transition::symmetric(Effect { offset: Vec2::new(x, y), ..Effect::IDENTITY })
    }
    /// `.move(edge:)`: in from and out to `edge`, by the node's full size.
    pub fn move_edge(edge: Edge) -> Transition {
        Transition::symmetric(Effect { offset_fraction: edge.direction(), ..Effect::IDENTITY })
    }
    /// `.push(from:)`: in from `edge`, out through the opposite edge, fading.
    pub fn push(edge: Edge) -> Transition {
        let e = |d: Edge| Effect { offset_fraction: d.direction(), opacity: 0.0, ..Effect::IDENTITY };
        Transition { insertion: e(edge), removal: e(edge.opposite()), ..Transition::symmetric(Effect::IDENTITY) }
    }
    /// `.slide`: in from the leading edge, out through the trailing edge.
    pub fn slide() -> Transition {
        let e = |d: Edge| Effect { offset_fraction: d.direction(), ..Effect::IDENTITY };
        Transition { insertion: e(Edge::Leading), removal: e(Edge::Trailing), ..Transition::symmetric(Effect::IDENTITY) }
    }
    /// `.blurReplace`: whole-element in-place replacement: sigma 1.5 pt, scale .96, fade.
    /// The old 4 pt shape-edge ramp left glyphs sharp; it is not equivalent to sigma 4.
    /// This preset stays within the two-pass Gaussian path at 1×/2×. Explicit `blur(sigma)`
    /// remains available for stronger effects.
    pub fn blur_replace() -> Transition {
        Transition::symmetric(Effect { opacity: 0.0, scale: 0.96, blur: 1.5, ..Effect::IDENTITY })
    }
    /// Gaussian blur (sigma in points) and fade only.
    pub fn blur(sigma: f32) -> Transition {
        Transition::symmetric(Effect { opacity: 0.0, blur: sigma, ..Effect::IDENTITY })
    }
    /// A shape that draws itself on, as a connector draws from its source to its target: its
    /// strokes trace from their first point to their last, one after another, and each fill (an
    /// arrowhead) fades in as the line reaches it. Applies to the node's own shape (`canvas`)
    /// only; children enter by their own transitions.
    pub fn draw() -> Transition {
        Transition::symmetric(Effect { draw: 0.0, ..Effect::IDENTITY })
    }
    /// A soft-edged wipe from `edge` (mask reveal), 24 pt feather.
    pub fn reveal(edge: Edge) -> Transition {
        Transition::symmetric(Effect { reveal: Some(Reveal { edge, feather: 24.0 }), ..Effect::IDENTITY })
    }
    /// `.asymmetric(insertion:removal:)`.
    pub fn asymmetric(insertion: Transition, removal: Transition) -> Transition {
        Transition { insertion: insertion.insertion, removal: removal.removal, ..insertion }
    }
    /// `.combined(with:)`.
    pub fn combined(self, other: Transition) -> Transition {
        Transition {
            insertion: self.insertion.combined(other.insertion),
            removal: self.removal.combined(other.removal),
            ..self
        }
    }
    pub fn springs(mut self, springs: SpringPair) -> Transition {
        self.springs = springs;
        self
    }
    /// Lets this transition's scale go below 0.9, down to [`SMALL_SCALE_FLOOR`] (0.5), for a
    /// small glyph such as an icon in a button, which barely seems to move at 0.9:
    /// `Transition::scale(0.6).allow_small_scale()`. Larger elements keep the default range, and
    /// nothing scales to or from 0.
    pub fn allow_small_scale(mut self) -> Transition {
        self.insertion.small_scale = true;
        self.removal.small_scale = true;
        self
    }
    /// Keep the exiting node in flow (its slot collapses as it leaves) instead of popping it out.
    pub fn hold_layout(mut self) -> Transition {
        self.pop_layout = false;
        self
    }
}
