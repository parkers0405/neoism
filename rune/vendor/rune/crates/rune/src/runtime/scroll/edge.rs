//! Content-aware scroll edges: `El::scroll_edge_fade` fades an edge only while content is hidden
//! under it, and `El::scroll_margin` stops `Tx::scroll_into_view` clear of that fade.
use super::*;

/// Hidden extent (points) at which an edge's fade reaches full strength.
const FADE_IN: f64 = 15.0;

/// A fade at `strength` (0..=1) as a packed ramp `(width, offset)`: the shader's smoothstep ramp
/// starts `offset` of the way in, so the edge shows `1 - strength` and reaches full alpha
/// `width` in. Zero strength is a hard edge.
fn partial(width: f32, strength: f32) -> (f32, f32) {
    if strength <= 0.0 || width <= 0.0 { return (0.0, 0.0); }
    if strength >= 1.0 { return (width, 0.0); }
    // Inverse smoothstep: where the ramp already stands at alpha `1 - strength`.
    let offset = 0.5 - ((2.0 * strength - 1.0).clamp(-1.0, 1.0).asin() / 3.0).sin();
    let offset = offset.clamp(0.0, 0.995);
    (width / (1.0 - offset), offset)
}

/// The scroll offset that shows `[top, top + height)` with `margin` to spare, moving as little
/// as possible from `offset` (unclamped). A row taller than the viewport that already fills it
/// stays put, so placing it again doesn't flip it from its end to its start.
pub(super) fn reveal(offset: f64, top: f64, height: f64, viewport: f64, margin: f64) -> f64 {
    let margin = margin.min(((viewport - height) / 2.0).max(0.0));
    let (start, end) = (top - margin, top + height + margin);
    if start <= offset && end >= offset + viewport { offset }
    else if start < offset { start }
    else if end > offset + viewport { end - viewport }
    else { offset }
}

impl<S: 'static> Runtime<S> {
    /// Node `i`'s own soft-clip widths `f` (points) as a device-pixel fade and the rect its ramps
    /// start from. With `scroll_edge_fade`, top and bottom follow the content hidden under them:
    /// the ramp starts outside `r` so the edge itself only partly fades. `bottom` is the
    /// presented bottom edge (see `stuck_offset`).
    pub(super) fn scroll_edge_fade(&self, i: usize, f: [f32; 4], r: [f32; 4], k: f32, bottom: f32) -> ([f32; 4], [f32; 4]) {
        let n = &self.nodes[i];
        let mut own = f.map(|v| v * k);
        let width = n.scroll_edges.fade;
        let Some(st) = self.scrolls.get(&n.id).filter(|_| width > 0.0 && n.scroll.is_some()) else { return (own, r) };
        let stuck = self.stuck_offset(i, bottom);
        let offset = stuck.unwrap_or(st.offset) + f64::from(st.slack.height());
        let below = if stuck.is_some() { 0.0 } else { st.max - offset };
        let mut origin = r;
        let (y, x) = (n.scroll_axes.y(), n.scroll_axes.x());
        let style = n.scroll_edges.fade_style;
        // What hides past the left and right edges: a right-to-left viewport starts at its right.
        let (left, right) = (st.x.offset, st.x.max - st.x.offset);
        let (left, right) = match st.x.flow { crate::el::Flow::Ltr => (left, right), crate::el::Flow::Rtl => (right, left) };
        let edges = [(y, 1, offset, -1.0), (y, 3, below, 1.0), (x, 0, left, -1.0), (x, 2, right, 1.0)];
        for (_, edge, hidden, out) in edges.into_iter().filter(|e| e.0 && !(style.end_only && e.3 < 0.0)) {
            let (style, width) = style.edge(width, out > 0.0);
            let (w, o) = edge_ramp(style, width * k, hidden, k);
            own[edge] = w;
            origin[edge] = r[edge] + out * o * w.abs();
        }
        (own, origin)
    }

    /// Under [`crate::ScrollFade::below_sticky`], the fade under the header heading `section`
    /// (its first child, `El::sticky_top`) while it's stuck at the top of its viewport, handed to
    /// the section's rows through its inherited paint state, from the section's own pass: the
    /// header paints above its siblings, so after them. The strength follows how far the section
    /// has scrolled past where the header sticks; from where the next section's top comes within
    /// a header of the viewport's top (it's about to push this header away) the rows are clear,
    /// as Desktop's `search-list.tsx` takes the next section as current there. The rows clip at
    /// the header's bottom, under its fill, so the ramp may start above it; the header keeps
    /// the clip it had (`sticky_bands`).
    pub(in crate::runtime) fn fade_below_header(&mut self, section: usize, s: f32) {
        let h = section + 1;
        if self.nodes.get(h).is_none_or(|c| c.parent != Some(section)) { return; }
        let Some((crate::el::StickyEdge::Top, inset, false)) = self.sticky_of(h) else { return };
        let (frame, Some(viewport)) = self.sticky_frame_of(h, crate::el::StickyEdge::Top) else { return };
        let edges = self.nodes[viewport].scroll_edges;
        if !edges.fade_style.below_sticky || edges.fade <= 0.0 { return; }
        let n = &self.nodes[h];
        let Some(p) = self.scene.presented(n.id) else { return };
        let own = self.scene.get(n.id, prop::OFFSET).unwrap_or(rune_motion::Vec2::ZERO);
        let inh = self.inh_scratch[section];
        let top = p.rect.y - own.y + inh.dy + self.row_dy[h];
        let left = p.rect.x - own.x + inh.dx;
        let height = if self.row_h[h] >= 0.0 { self.row_h[h] } else { p.rect.h };
        let shift = sticky::shift(crate::el::StickyEdge::Top, inset, [top, top + height, left, left + p.rect.w], frame);
        let (band, hidden) = (frame[0] + inset, frame[0] + inset - top);
        let next_top = frame[3] + self.nodes[section].inset[3];
        if shift <= 0.0 || next_top - band < height { return; }
        let k = inh.scale * s;
        let (w, o) = edge_ramp(edges.fade_style, edges.fade * k, f64::from(hidden), k);
        if w == 0.0 { return; }
        let bottom = (inh.anchor_to[1] + (top + shift + height - inh.anchor[1]) * inh.scale) * s;
        let inh = &mut self.inh_scratch[section];
        self.sticky_bands.push((section, inh.paint_clip[1]));
        inh.paint_clip[1] = inh.paint_clip[1].max(bottom);
        inh.paint_fade[1] = w;
        inh.paint_fade_origin[1] = bottom - o * w.abs();
    }
}

/// The packed ramp `(width, offset)` (see [`partial`]) for an edge with `hidden` points under
/// it, `width` device pixels wide at `k` device pixels a point; a negative width ramps linearly.
fn edge_ramp(style: crate::el::FadeStyle, width: f32, hidden: f64, k: f32) -> (f32, f32) {
    if style.feather {
        let w = width.min(hidden.max(0.0) as f32 * k);
        return (if style.linear { -w } else { w }, 0.0);
    }
    let full = if style.full_at > 0.0 { f64::from(style.full_at) } else { FADE_IN };
    let strength = (hidden / full).clamp(0.0, 1.0) as f32;
    if !style.linear { return partial(width, strength); }
    let (w, o) = partial_linear(width, strength);
    (-w, o)
}

/// [`partial`] on a linear ramp: it starts `1 - strength` of the way in, so the edge shows
/// `1 - strength` and the ramp reaches full alpha `width` in.
fn partial_linear(width: f32, strength: f32) -> (f32, f32) {
    if strength <= 0.0 || width <= 0.0 { return (0.0, 0.0); }
    if strength >= 1.0 { return (width, 0.0); }
    let offset = (1.0 - strength).min(0.995);
    (width / (1.0 - offset), offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_ramp_starts_at_one_minus_strength_and_ends_at_the_full_width() {
        let smooth = |x: f32| { let x = x.clamp(0.0, 1.0); x * x * (3.0 - 2.0 * x) };
        assert_eq!(partial(24.0, 0.0), (0.0, 0.0), "nothing hidden: a hard edge");
        assert_eq!(partial(24.0, 1.0), (24.0, 0.0), "fully hidden: the plain ramp");
        for s in [0.1, 0.25, 0.5, 0.75, 0.9] {
            let (w, o) = partial(24.0, s);
            assert!((smooth(o) - (1.0 - s)).abs() < 1e-4, "edge alpha {} at strength {s}", smooth(o));
            assert!(((1.0 - o) * w - 24.0).abs() < 1e-3, "full alpha 24 in at strength {s}");
        }
    }

    #[test]
    fn reveal_leaves_the_margin_unless_the_row_cannot_fit_it() {
        assert_eq!(reveal(100.0, 300.0, 20.0, 200.0, 0.0), 120.0, "flush below");
        assert_eq!(reveal(100.0, 300.0, 20.0, 200.0, 24.0), 144.0, "clear of a 24pt fade below");
        assert_eq!(reveal(100.0, 90.0, 20.0, 200.0, 24.0), 66.0, "and above");
        assert_eq!(reveal(100.0, 150.0, 20.0, 200.0, 24.0), 100.0, "already clear: no move");
        assert_eq!(reveal(100.0, 300.0, 180.0, 200.0, 24.0), 290.0, "a tall row splits what space is left");
        assert_eq!(reveal(100.0, 300.0, 500.0, 200.0, 24.0), 600.0, "a row taller than the viewport, below: its end");
        assert_eq!(reveal(600.0, 300.0, 500.0, 200.0, 24.0), 600.0, "and placing it again leaves it there");
        assert_eq!(reveal(400.0, 300.0, 500.0, 200.0, 24.0), 400.0, "a row filling the viewport: no move");
        assert_eq!(reveal(700.0, 300.0, 500.0, 200.0, 24.0), 300.0, "its end in view: its start");
    }
}
