//! Motion builders: springs, offsets, raises, leads, layout animation and transitions.

use super::*;

/// Authored motion traits for [`El::motion_intent`], inherited by descendants. Motion checks
/// (`rune::motion_gate` and `--trace`) read these marks so deliberate overshoots, kicks, shakes
/// and discrete typing cuts pass without broad allowlist rules.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct MotionIntent(u16);

impl MotionIntent {
    pub const NONE: Self = Self(0);
    /// Deliberately passes its rest value and settles back (a bouncy spring or elastic curve).
    pub const OVERSHOOT: Self = Self(1 << 0);
    /// Deliberate wind-up, anticipation or out-and-back bump (`reversal`).
    pub const RECOIL: Self = Self(1 << 1);
    /// Deliberate shake or oscillation (`jiggle`, `reversal`).
    pub const JIGGLE: Self = Self(1 << 2);
    /// Deliberate sudden launch velocity or multi-rate lead (`jerk`).
    pub const IMPULSE: Self = Self(1 << 3);
    /// Deliberate discrete step without transition (`jump`, `appear-then-jump`, `snap`).
    pub const STEP: Self = Self(1 << 4);
    /// Deliberate instant mount, unmount or swap (`pop-in`, `pop-out`, `cut-exit`, `fade-snap`, `text-swap`, `snap`).
    pub const CUT: Self = Self(1 << 5);
    /// Deliberate brief flash or dip-and-recover (`flicker`, `flash`).
    pub const PULSE: Self = Self(1 << 6);

    /// A bouncy or elastic element (`OVERSHOOT | RECOIL | JIGGLE`).
    pub const BOUNCE: Self = Self(Self::OVERSHOOT.0 | Self::RECOIL.0 | Self::JIGGLE.0);
    /// A ballistic launch or impact (`BOUNCE | IMPULSE`).
    pub const KICK: Self = Self(Self::BOUNCE.0 | Self::IMPULSE.0);
    /// Discrete typing: keycaps, typed runs, carets and placeholders (`STEP | CUT`).
    pub const TYPING: Self = Self(Self::STEP.0 | Self::CUT.0);

    #[inline]
    pub const fn bits(self) -> u16 {
        self.0
    }

    #[inline]
    pub const fn from_bits(bits: u16) -> Self {
        Self(bits)
    }

    #[inline]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    #[inline]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl std::ops::BitOr for MotionIntent {
    type Output = Self;
    #[inline]
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for MotionIntent {
    #[inline]
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl El {
    /// Marks deliberate motion traits (`OVERSHOOT`, `KICK`, `JIGGLE`, `STEP`, `CUT`, `PULSE`) on
    /// this element and its subtree so motion checks treat them as authored rather than bugs.
    pub fn motion_intent(mut self, intent: MotionIntent) -> Self {
        if intent != MotionIntent::NONE {
            self.vis.cold_mut().motion_intent |= intent;
        }
        self
    }

    /// The spring for this subtree's layout and colour changes (inherited).
    /// Presence uses the transition's own enter/exit springs.
    ///
    /// [`Spring::NONE`] written here lands this element's changes at once, and motion checks
    /// (`rune::motion_gate`) take that as the design for the properties it governs on this
    /// element: its fill, border, shadow, gradient and outline colours, corners, shadow and
    /// blur. Not for its children, which inherit the spring but not the mark. A spring from a
    /// theme or token goes through [`token_motion`](Self::token_motion) instead.
    pub fn motion(mut self, s: Spring) -> Self {
        self.set_motion(s, s == Spring::NONE);
        self
    }
    /// [`motion`](Self::motion) with a spring a theme or token chose rather than this element:
    /// a [`Spring::NONE`] here still lands at once, but motion checks still report what snaps.
    pub fn token_motion(mut self, s: Spring) -> Self {
        self.set_motion(s, false);
        self
    }
    fn set_motion(&mut self, s: Spring, authored_instant: bool) {
        self.cold_mut().motion = Some(s);
        if authored_instant || self.vis.instant_motion {
            self.vis.cold_mut().instant_motion = authored_instant;
        }
    }
    /// Shift this element and its subtree by `(x, y)` points without moving it in layout:
    /// siblings keep their places, and hit tests follow the shift. A change springs on the
    /// element's layout spring (or [`Self::offset_motion`]); under Reduce Motion, and in an
    /// instant transaction, it lands at once. A reordered row slides aside this way while a
    /// dragged one passes it. When a commit both changes the offset and moves the element in a
    /// growth or virtual list (whose rows lay out at once), the offset starts from where the
    /// element was drawn, so a dropped row that clears its offset slides into its new place.
    /// Not for an element with a `live_offset` or a sliding loop, which own the same shift.
    pub fn offset(mut self, x: f32, y: f32) -> Self {
        let ok = |v: f32| if v.is_finite() { v } else { 0.0 };
        let o = [ok(x), ok(y)];
        if o != [0.0; 2] || !self.vis.cold.is_empty() {
            self.vis.cold_mut().offset = o;
        }
        self
    }
    /// The curve [`Self::offset`] changes on.
    pub fn offset_motion(mut self, anim: impl Into<Anim>) -> Self {
        self.vis.cold_mut().offset_motion = Some(Box::new(anim.into()));
        self
    }
    /// Paint this element and its subtree above everything else in the window, and give it
    /// pointer input first, as if it came last in the tree. Its layout, focus order and
    /// accessibility order stay where it is, and its ancestors' clips, offsets and opacity
    /// still apply, though an ancestor that composites its children as a group (a partial
    /// opacity, a blur) no longer groups it. Raised elements stack in tree order. A floating
    /// sidebar sliding over the content beside it, or a dragged row over its neighbours.
    pub fn raise(mut self) -> Self {
        self.vis_flags.set(VisualFlags::RAISE, true);
        self
    }
    /// Where this element paints among its siblings, as CSS `z-index` orders positioned
    /// siblings: a higher order paints over siblings with a lower one, wherever they come in
    /// the tree, and takes the pointer first where they overlap; equal orders paint in tree
    /// order. 0 by default. Its focus order and accessibility order stay where it is in the
    /// tree, and unlike [`Self::raise`] it stays inside its parent's layers. A floating button
    /// placed before the page in the tree, for its Tab order, still paints over the page.
    pub fn paint_order(mut self, order: i16) -> Self {
        if order != 0 || !self.vis.cold.is_empty() {
            self.vis.cold_mut().paint_order = order;
        }
        self
    }
    /// Override the curve this element's own opacity changes on, with an optional delay
    /// ([`AnimExt::delay`](crate::AnimExt::delay)). The first paint and explicit
    /// instant/settle transactions still land immediately. A new element's entrance runs on
    /// its [`transition`](Self::transition) and multiplies this opacity; hold all of it with
    /// [`enter_delay`](Self::enter_delay).
    pub fn opacity_motion(mut self, anim: impl Into<Anim>) -> Self {
        self.cold_mut().opacity_motion = Some(Box::new(anim.into()));
        self
    }
    /// When this element travels, its leading edge moves on `lead` and its trailing edge on `trail`
    /// (width leads).
    pub fn lead(mut self, lead: Spring, trail: Spring) -> Self {
        self.cold_mut().lead = Some((lead, trail));
        self
    }
    /// The X axis springs faster (`0.78×` duration) when this element resizes or travels.
    pub fn width_leads(mut self) -> Self {
        self.flags.set(ElFlags::WIDTH_LEADS, true);
        self
    }
    /// Spring this subtree's horizontal layout (x and width) inside a growth-stack row, and
    /// fade in children it gains while the row is in place. A row's contents otherwise snap
    /// while the row's height carries the motion, so a label that changes width in place jumps
    /// its neighbours. Vertical place stays with the row's growth.
    pub fn animate_layout(mut self) -> Self {
        self.flags.set(ElFlags::ANIMATE_LAYOUT, true);
        self
    }
    /// Land every change inside this element at once, as a [`Tx::instant`] commit would, even
    /// in a commit that animates the rest of the window: its own and its subtree's layout,
    /// opacity and transforms, the elements it gains or loses, and growth rows. Its own entrance
    /// and exit keep their transition, and colours (a hover highlight) keep their springs. A
    /// popup of search results under a field whose edits animate the layout around it.
    pub fn instant_changes(mut self) -> Self {
        self.flags.set(ElFlags::INSTANT_CHANGES, true);
        self
    }
    /// How this element enters and exits when mounted or removed after the first frame
    /// (default: [`Transition::opacity()`] — a 0.2 s enter and 0.15 s exit fade). Because
    /// every element has this default fade, removing an element in an animated commit
    /// (`AppSpec::on_key`, `El::on_key`, `on_submit`, `on_press`) fades it out over 0.15 s,
    /// whereas commits that set [`Tx::instant`] or [`Tx::settle`] (such as typing in a
    /// focused input or clicking into a text field) unmount removed elements in the same
    /// frame. Call `tx.instant()` or `tx.settle()` in the handler when a key or button
    /// action should unmount elements at once. Rows inserted into a growth stack only grow
    /// open unless they set one, so `.transition(Transition::offset(0.0, 8.0).combined(Transition::opacity()))`
    /// on a row also slides and fades it in. Rows a virtual list remounts on scroll never replay it.
    /// [`enter_delay`](Self::enter_delay) holds the whole entrance, as a staggered list does.
    pub fn transition(mut self, t: Transition) -> Self {
        self.cold_mut().transition = Some(Box::new(t));
        self
    }
    /// Shared-element motion (SwiftUI `matchedGeometryEffect`, Motion `layoutId`). When an
    /// element with `id` leaves the tree in the same commit that another with `id` mounts
    /// somewhere else, the new one starts exactly where the old one was seen, with its
    /// velocity, corner radius, fill and opacity, and springs to its own layout on one straight
    /// path. The old one disappears at once; neither plays its transition.
    ///
    /// Ids are per window. While it flies the element draws above the whole window at its own
    /// opacity, inside its destination's clips (a scroll viewport included), and its children
    /// keep their final layout, scaled uniformly to cover the moving frame and clipped to it,
    /// so a `pixels_fit(.., SurfaceFit::Fill)` picture zooms without distorting. Text moves
    /// with its box but keeps its size, as it does under any scale. The spring is
    /// `tok::motion::REFLOW` unless the commit's `Tx::motion` or this element's
    /// [`motion`](Self::motion) sets one. Under Reduce Motion nothing is matched: the old one
    /// exits and the new one enters by their own transitions, in place.
    /// When two elements share an id in one tree, the last one in tree order is matched.
    pub fn matched(mut self, id: impl Into<String>) -> Self {
        self.cold_mut().matched = Some(Rc::from(id.into()));
        self
    }
    /// While this [`matched`](Self::matched) element flies, its children keep their final
    /// layout at their own size, from the moving frame's leading edge and centred on its
    /// height, cropped to it, rather than scaled to cover it: for a row whose width changes as
    /// it flies, as a tab moving between a titlebar's strip and a sidebar, whose avatar and title
    /// would otherwise scale up or slide off its leading edge.
    pub fn matched_crop(mut self) -> Self {
        self.vis.cold_mut().flight_crop = true;
        self
    }
    /// When an animated commit changes this element's width (a side panel springing open
    /// beside a column of text), it and everything in it take the layout they land at in
    /// that frame, so text wraps once rather than on every frame of the travel, and the whole
    /// slides as one: its first child that drew last frame starts where it was drawn and
    /// glides to its new place on this element's spring. Heights its growth rows change land
    /// with it. Motion checks take that frame's re-layout as designed. A window resize, an
    /// instant commit and Reduce Motion land as they otherwise would.
    pub fn hold_landing_width(mut self) -> Self {
        self.vis.cold_mut().hold_landing = true;
        self
    }
}
