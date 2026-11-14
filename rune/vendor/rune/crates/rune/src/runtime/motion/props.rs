//! The runtime's own animated properties beside `rune_motion::prop`: their ids, trace names and settle time.
use super::*;

pub(super) const P_FILL: Prop<Color> = Prop::custom(64, PropClass::Color);
pub(super) const P_BORDER: Prop<Color> = Prop::custom(65, PropClass::Color);
pub(super) const P_TEXT: Prop<Color> = Prop::custom(66, PropClass::Color);
pub(super) const P_HOVER_C: Prop<Color> = Prop::custom(67, PropClass::Color);
pub(super) const P_PRESS_C: Prop<Color> = Prop::custom(68, PropClass::Color);
pub(super) const P_SHADOWS: [Prop<Color>; 4] = [
    Prop::custom(69, PropClass::Color),
    Prop::custom(70, PropClass::Color),
    Prop::custom(71, PropClass::Color),
    Prop::custom(72, PropClass::Color),
];
pub(super) const P_HOVER: Prop<f32> = Prop::custom(73, PropClass::Opacity);
pub(super) const P_PRESS: Prop<f32> = Prop::custom(74, PropClass::Opacity);
pub(super) const P_GRADIENT: [Prop<Color>; 2] = [Prop::custom(75, PropClass::Color), Prop::custom(76, PropClass::Color)];
pub(super) const P_OUTLINE: Prop<Color> = Prop::custom(77, PropClass::Color);
pub(super) const P_BORDER_BOTTOM: Prop<Color> = Prop::custom(78, PropClass::Color);
/// `El::press_scale`'s presented factor, multiplied into the element's own scale.
pub(super) const P_PRESS_SCALE: Prop<f32> = Prop::custom(79, PropClass::Scale);
pub(super) const P_RADIAL: [Prop<Color>; 8] = [
    Prop::custom(80, PropClass::Color),
    Prop::custom(81, PropClass::Color),
    Prop::custom(82, PropClass::Color),
    Prop::custom(83, PropClass::Color),
    Prop::custom(84, PropClass::Color),
    Prop::custom(85, PropClass::Color),
    Prop::custom(86, PropClass::Color),
    Prop::custom(87, PropClass::Color),
];
/// A property's name in traces.
pub(super) fn prop_name(id: u16) -> &'static str {
    const BUILTIN: [&str; 12] = ["opacity", "frame", "offset", "scale", "rotation", "blur", "radius", "fill", "mask", "scroll", "presence", "travel"];
    match id {
        0..12 => BUILTIN[id as usize],
        64 => "fill-color",
        65 => "border-color",
        66 => "text-color",
        67 => "hover-color",
        68 => "press-color",
        69..=72 => "shadow-color",
        73 => "hover",
        74 => "press",
        75 | 76 => "gradient-color",
        77 => "outline-color",
        78 => "border-bottom-color",
        79 => "press-scale",
        80..=87 => "radial-color",
        88 => "icon-turn",
        89 => "scale",
        90..=93 => "shadow",
        94 => "group-rest",
        95 => "trim",
        _ => "custom",
    }
}

/// `El::icon_rotation`'s presented turn, in radians.
pub(super) const P_ICON_TURN: Prop<f32> = Prop::custom(88, PropClass::Rotation);
/// `El::scale`'s presented factor. `prop::SCALE` belongs to `live_scale`; they multiply.
pub(super) const P_SCALE: Prop<f32> = Prop::custom(89, PropClass::Scale);
/// `El::group_opacity`'s presented rest level, so a new rest fades in rather than cutting.
pub(super) const P_GROUP_REST: Prop<f32> = Prop::custom(94, PropClass::Opacity);
/// `El::live_trim`'s presented span of a shape's strokes, start and end as 0…1 fractions.
pub(super) const P_TRIM: Prop<rune_motion::Vec2> = Prop::custom(95, PropClass::Other);
/// The first four shadows' geometry: x, y, blur and spread in a rect's lanes.
pub(super) const P_SHADOW_GEO: [Prop<rune_motion::Rect>; 4] = [
    Prop::custom(90, PropClass::Shape),
    Prop::custom(91, PropClass::Shape),
    Prop::custom(92, PropClass::Shape),
    Prop::custom(93, PropClass::Shape),
];
/// Whether a shadow colour or geometry spring, or an icon's turn, is moving node `id`.
pub(super) fn paint_pose_springing(store: &rune_motion::AnimationStore, id: NodeId) -> bool {
    store.is_animating(id, P_ICON_TURN)
        || P_SHADOWS.iter().any(|&p| store.is_animating(id, p))
        || P_SHADOW_GEO.iter().any(|&p| store.is_animating(id, p))
}
/// Seconds a changed `El::scale` holds still before its text rasterizes at that size.
pub(super) const STATIC_SCALE_SETTLE: f64 = 0.1;

impl<S: 'static> Runtime<S> {
    /// Moves node `id`'s group-opacity rest to `group`'s on its opacity motion (the
    /// transaction's spring without one), or lands it when `quiet`: an instant change or motion,
    /// a mount, reduced motion. No group opacity draws at full, so an element that takes one on
    /// or drops it fades from or to 1. Returns whether a dropped one is still fading to full.
    pub(super) fn set_group_rest(&mut self, id: NodeId, group: Option<[f32; 2]>, motion: Option<Anim>, spring: Spring, quiet: bool) -> bool {
        let store = self.scene.store();
        let had = store.has(id, P_GROUP_REST);
        let motion = motion.unwrap_or(spring.into());
        let quiet = quiet || matches!(motion.curve, rune_motion::Curve::Instant);
        let settled = !store.is_animating(id, P_GROUP_REST) && store.get(id, P_GROUP_REST) == Some(1.0);
        let to = group.map_or(1.0, |g| g[0]);
        if group.is_none() && (!had || quiet || settled) {
            if had { self.scene.store_mut().remove(id, P_GROUP_REST); }
            return false;
        }
        if quiet {
            self.scene.set(id, P_GROUP_REST, to);
        } else if !had {
            self.scene.set(id, P_GROUP_REST, 1.0);
            self.scene.animate(id, P_GROUP_REST, to, motion);
        } else if store.target(id, P_GROUP_REST) != Some(to) {
            self.scene.animate(id, P_GROUP_REST, to, motion);
        }
        group.is_none()
    }

    /// Drops the rest track of each dropped group opacity whose fade to full landed this frame.
    pub(super) fn land_group_fades(&mut self) {
        let landed: Vec<usize> = self.scene.store().events().iter()
            .filter_map(|e| match *e {
                rune_motion::MotionEvent::Settled { node, prop, .. } if prop == P_GROUP_REST.id => self.node_index.get(&node).copied(),
                _ => None,
            })
            .filter(|&i| self.nodes[i].group_fade && !self.scene.store().is_animating(self.nodes[i].id, P_GROUP_REST))
            .collect();
        for i in landed {
            self.nodes[i].group_fade = false;
            self.scene.store_mut().remove(self.nodes[i].id, P_GROUP_REST);
        }
    }

    /// The opacity factor `El::group_opacity` gives node `n` under a hover group at `group`.
    /// A dropped one fading to full shows at full under the hover.
    pub(super) fn group_factor(&self, n: &Node, group: f32) -> f32 {
        let [rest, hovered] = match n.vis.group_opacity {
            Some(g) => g,
            None if n.group_fade => [1.0, 1.0],
            None => return 1.0,
        };
        let rest = self.scene.get(n.id, P_GROUP_REST).unwrap_or(rest);
        // A control the keyboard reached shows as if hovered, as CSS reveals a hover-only button
        // on `:focus-visible` (or `:focus-within` around it): focus is never on something unseen.
        let focused = self.focus.keyboard() && self.focus.path().is_some_and(|f| {
            f.strip_prefix(n.path.as_str()).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        });
        let group = if focused { 1.0 } else { group };
        rest + (hovered - rest) * group
    }
}
