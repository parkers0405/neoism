//! Text flights drawn as an overlay of glyph instances, from their source text to their landed text.
use super::*;

impl<S: 'static> Runtime<S> {
    /// All destination rectangles below come from this frame's completed growth/scroll build.
    /// The overlay only writes draw instances, never layout or app state.
    pub(super) fn draw_text_flights(&mut self) -> bool {
        let s = self.env.scale;
        let srgb = self.srgb_blend;
        let now = self.t;
        let reduced = self.reduced_motion;
        let viewport = [0.0, 0.0, self.viewport.0, self.viewport.1];
        let nodes = &self.nodes;
        let scene = &self.scene;
        self.flights.retain(|f| nodes.iter().any(|n| f.lands(&n.path, n.key.as_deref())
            && !matches!(scene.phase(n.id), Some(PresencePhase::Exiting { .. }))));
        let mut done = Vec::new();
        for (fi, f) in self.flights.iter_mut().enumerate() {
            let Some((i, n)) = nodes.iter().enumerate().find(|(_, n)| f.lands(&n.path, n.key.as_deref())) else { continue };
            let (Some(spec), Some(layout)) = (&n.text, &n.text_layout) else { done.push(fi); continue };
            // Both ends are measured by their laid-out text, so a destination box wider than its
            // text (a body stretched across a column) doesn't scale the flight.
            let (target, origin) = text_extent(n, spec, layout, self.drawn[i], s);
            if f.measure(target, now, reduced) { done.push(fi); }
            // Land sub-quantum size residuals before snapping both edges. Otherwise a settled
            // height can alternate by one quantum while its still-moving top crosses the grid.
            let quantum = 1.0 / (64.0 * s);
            if (f.presented.w - target.w).abs() < quantum { f.presented.w = target.w; }
            if (f.presented.h - target.h).abs() < quantum { f.presented.h = target.h; }
            let edges = f.presented.edges().map(|v| grid(v, s));
            f.presented = MRect::new(edges[0], edges[1], edges[2] - edges[0], edges[3] - edges[1]);
            let p = f.progress(now, reduced);
            let (source_alpha, target_alpha) = f.alphas(target, now, reduced);
            let actual_clip = screen_clip_of(n.parent, &self.inh_scratch, &self.turn_scratch).map(|v| v / s);
            let actual_clip = [actual_clip[0].max(0.0), actual_clip[1].max(0.0), actual_clip[2].min(viewport[2]), actual_clip[3].min(viewport[3])];
            let current = f.presented;
            let zoom = current.w / f.source.w;
            let start_clip = [current.x + (f.source_clip[0] - f.source.x) * zoom,
                current.y + (f.source_clip[1] - f.source.y) * zoom,
                current.x + (f.source_clip[2] - f.source.x) * zoom,
                current.y + (f.source_clip[3] - f.source.y) * zoom];
            // A travelling overlay is above the composer and the growing bubble's mask. Using
            // that destination mask here can hide the text before it has reached the bubble.
            // The normal ancestor clip takes over only once the flight has landed.
            let end_clip = viewport;
            let clip = std::array::from_fn(|j| start_clip[j] + (end_clip[j] - start_clip[j]) * p);
            f.clip = if reduced { actual_clip } else { clip };
            f.source_alpha = source_alpha;
            f.target_alpha = target_alpha;
            let final_fade = n.parent.and_then(|p| self.inh_scratch.get(p)).map_or([0.0; 4], |h| h.fade.map(|v| v / s));
            let fade = [0.0; 4];
            if source_alpha > 0.0 {
                let source_rect = if reduced { f.source } else { current };
                let source_clip = if reduced { f.source_clip } else { clip };
                for q in &f.glyphs {
                    self.instances.push(transformed(*q, f.source, source_rect, f.scale, source_alpha, source_clip, fade));
                }
            }
            self.glyph_scratch.clear();
            self.text.set_glyph_inks(spec.color, &[]);
            self.text.set_emit(self.text.moving_emit());
            self.text.glyphs_for_layout(layout, s, [origin[0] * s, origin[1] * s], false, &mut self.glyph_scratch);
            self.text.set_emit(crate::text::outline::Emit::Atlas);
            for q in &self.glyph_scratch {
                let Some(q) = glyph_or_image(q, &self.text, &mut self.images, premul(spec.color, 1.0, srgb), NO_CLIP, [0.0; 4]) else { continue };
                self.instances.push(transformed(q, target, current, s, target_alpha,
                    if reduced { actual_clip } else { clip }, if reduced { final_fade } else { fade }));
            }
        }
        for i in done.into_iter().rev() { self.flights.remove(i); }
        !self.flights.is_empty()
    }
}
