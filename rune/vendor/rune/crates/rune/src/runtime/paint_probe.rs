//! What each node paints this frame, for `rune::motion_gate`: its presented colours, corners,
//! blurs, shadow, effect payload, image and place in the paint order.
use super::*;
use crate::motion_gate::check::paint::{C_BORDER, C_FILL, C_GRADIENT, C_OUTLINE, C_SHADOW, C_TEXT, Paint, S_BACKDROP, S_BLUR, S_BORDER, S_RADIUS, S_SHADOW};

impl<S: 'static> Runtime<S> {
    /// Every presented node's paint, keyed by its path and whether it's leaving. Paint ranks come
    /// from the last frame built once [`Runtime::paint_rank`] is on.
    pub(crate) fn paints(&self) -> HashMap<(String, bool), Paint> {
        let mut out = HashMap::default();
        for (i, n) in self.nodes.iter().enumerate() {
            let Some(p) = self.scene.presented(n.id) else { continue };
            let v = &n.vis;
            let mut overlay = Color::TRANSPARENT;
            let mut pointer = [0.0; 2];
            let mut over_c = |c: Color, k: f32| {
                if k > 0.0 && c.a > 0.0 { overlay = unpremul(over(c.premul(k), overlay.premul(1.0))); }
            };
            if v.hover().a > 0.0 || self.scene.store().is_animating(n.id, P_HOVER_C) {
                pointer[0] = self.scene.get(n.id, P_HOVER).unwrap_or(0.0).clamp(0.0, 1.0);
                over_c(self.color(n.id, P_HOVER_C, v.hover()), pointer[0]);
            }
            if v.press().a > 0.0 || self.scene.store().is_animating(n.id, P_PRESS_C) {
                pointer[1] = self.scene.get(n.id, P_PRESS).unwrap_or(0.0).clamp(0.0, 1.0);
                over_c(self.color(n.id, P_PRESS_C, v.press()), pointer[1]);
            }
            let base = self.color(n.id, P_FILL, v.fill);
            let fill = unpremul(over(overlay.premul(1.0), base.premul(1.0)));
            // Static fades, as they show: its own `dim` and the layer opacity of it and every
            // element it sits in.
            let k = v.paint_alpha
                * std::iter::successors(Some(i), |&j| self.nodes[j].parent).map(|j| self.nodes[j].vis.layer_opacity).product::<f32>();
            let mut colors = [Color::TRANSPARENT; 6];
            colors[C_FILL] = fill.alpha(fill.a * k);
            if v.border_width() > 0.0 { colors[C_BORDER] = self.color(n.id, P_BORDER, v.border()).alpha_times(k); }
            if let Some(c) = paint_tracks::text_color(n) { colors[C_TEXT] = self.color(n.id, P_TEXT, c).alpha_times(k); }
            let mut sizes = [0.0; 5];
            sizes[S_RADIUS] = p.radius.0[0];
            sizes[S_BORDER] = v.border_width();
            sizes[S_BLUR] = self.inh_scratch.get(i).map_or(p.blur, |h| h.blur);
            sizes[S_BACKDROP] = v.backdrop_blur;
            if let Some(sh) = v.shadows.iter().find(|s| !s.inset) {
                let at = self.shadow_at(n.id, 0, sh);
                colors[C_SHADOW] = self.color(n.id, P_SHADOWS[0], sh.color).alpha_times(k);
                sizes[S_SHADOW] = at.blur + at.spread.abs() + at.x.abs().max(at.y.abs());
            }
            let mut gradient_end = Color::TRANSPARENT;
            if let Some(g) = &v.gradient {
                colors[C_GRADIENT] = self.color(n.id, P_GRADIENT[0], g[0]).alpha_times(k);
                gradient_end = self.color(n.id, P_GRADIENT[1], g[1]).alpha_times(k);
            } else if let Some(g) = &v.radial && let (Some(first), Some(last)) = (g.stops.first(), g.stops.last()) {
                colors[C_GRADIENT] = self.color(n.id, P_RADIAL[0], first.1).alpha_times(k);
                let end = (g.stops.len() - 1).min(P_RADIAL.len() - 1);
                gradient_end = if g.stops.len() - 1 < P_RADIAL.len() { self.color(n.id, P_RADIAL[end], last.1) } else { last.1 }.alpha_times(k);
            }
            if let Some(o) = v.outline.filter(|o| o.width > 0.0) { colors[C_OUTLINE] = self.color(n.id, P_OUTLINE, o.color); }
            let intent = std::iter::successors(Some(i), |&j| self.nodes[j].parent).fold(0u16, |bits, j| bits | self.nodes[j].vis.motion_intent.bits());
            let paint = Paint {
                colors,
                sizes,
                gradient_end,
                effect: v.effect.as_ref().map(|e| e.data),
                image: v.pixels.as_ref().map(crate::pixels::Pixels::signature),
                rank: self.paint_rank.as_ref().and_then(|r| r.get(i)).copied().unwrap_or(i as u32),
                intent,
                instant: n.instant_changes,
                at_once: crate::motion_gate::check::paint::at_once(v.instant_motion, v.blur_motion.as_deref().map(|a| a.curve == rune_motion::Curve::Instant)),
                pointer,
            };
            out.entry((n.path.clone(), self.subtree_exiting(i))).or_insert(paint);
        }
        out
    }
}

fn unpremul([r, g, b, a]: [f32; 4]) -> Color {
    if a <= 1e-6 { Color::TRANSPARENT } else { Color { r: r / a, g: g / a, b: b / a, a } }
}

trait AlphaTimes {
    fn alpha_times(self, k: f32) -> Self;
}

impl AlphaTimes for Color {
    fn alpha_times(self, k: f32) -> Color {
        self.alpha(self.a * k)
    }
}
