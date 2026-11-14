//! `Tx::fly_text` and `Tx::launch` requests, and the text handoff in screen coordinates. The destination is measured after growth and scroll
//! each frame. One analytic growth-spring progress interpolates source to live destination,
//! so the copy joins the bubble without a second spring lagging behind scroll follow.

use rune_motion::{Animated, PropClass, Rect};
use crate::growth::GROWTH;
use crate::render::Instance;
use crate::Tx;

impl Tx {
    /// Fly the visible text of an existing keyed field to a newly mounted keyed text node.
    /// The runtime captures the previous presentation and measures the live destination each
    /// frame, after row growth and scroll. Either side is a key unique in the view, or a key
    /// path such as `"msg-42/body"`: the `body` text whose nearest keyed ancestor is `msg-42`,
    /// so every row can reuse the same local keys.
    pub fn fly_text(&mut self, source: impl Into<String>, destination: impl Into<String>) {
        self.text_flights.push((source.into(), destination.into()));
    }

    /// Launch an element that mounts in this commit from where another element's text shows,
    /// as a sent message leaves its composer. The element keyed `destination` starts with its
    /// first text on `source`'s text, at its own size, and travels one straight screen path to
    /// its place on the `tok::motion::REFLOW` spring (or the commit's [`Tx::motion`]). Until it
    /// lands it draws above the whole window and outside every clip, so it can rise out of a
    /// panel into a scroll view; the source can clear in the same commit. Keys and key paths
    /// work as in [`Tx::fly_text`]. Under Reduce Motion, or when the destination was already
    /// mounted, it does nothing.
    pub fn launch(&mut self, source: impl Into<String>, destination: impl Into<String>) {
        self.launches.push((source.into(), destination.into()));
    }

    /// The `El::matched` flights this change starts draw outside every clip until they land, as
    /// a [`Tx::launch`] does, rather than inside their destinations' clips: for elements that
    /// move between parts of the window that clip them, as tabs moving from a titlebar's strip
    /// into a sidebar that opens as they land, where a clip would hide them for most of the way.
    pub fn unclipped_flights(&mut self) {
        self.unclipped_flights = true;
    }
}

pub(crate) struct TextFlight {
    pub source_key: String,
    pub target_key: String,
    pub source: Rect,
    pub source_clip: [f32; 4],
    pub glyphs: Vec<Instance>,
    pub scale: f32,
    pub destination: Rect,
    progress: Animated<f32>,
    born: f64,
    pub presented: Rect,
    pub clip: [f32; 4],
    pub source_alpha: f32,
    pub target_alpha: f32,
}

impl TextFlight {
    pub fn new(source_key: String, target_key: String, source: Rect, source_clip: [f32; 4], glyphs: Vec<Instance>, scale: f32, now: f64) -> Self {
        let mut progress = Animated::new(0.0).with_class(PropClass::Opacity);
        progress.animate_to(1.0, GROWTH, now);
        Self { source_key, target_key, source, source_clip, glyphs, scale,
            destination: source, progress, born: now, presented: source,
            clip: source_clip, source_alpha: 1.0, target_alpha: 0.0 }
    }

    pub fn progress(&self, now: f64, reduced: bool) -> f32 {
        if reduced { ((now - self.born) / 0.15).clamp(0.0, 1.0) as f32 }
        else { self.progress.sample(now).clamp(0.0, 1.0) }
    }

    pub fn measure(&mut self, target: Rect, now: f64, reduced: bool) -> bool {
        self.destination = target;
        let p = if reduced { 1.0 } else { self.progress(now, false) };
        let from = self.source;
        self.presented = Rect::new(from.x + (target.x - from.x) * p, from.y + (target.y - from.y) * p,
            from.w + (target.w - from.w) * p, from.h + (target.h - from.h) * p);
        if reduced { self.progress(now, true) >= 1.0 } else { self.progress.is_settled(now) }
    }

    /// The node at `path` keyed `key` is this flight's source.
    pub fn departs(&self, path: &str, key: Option<&str>) -> bool {
        selects(&self.source_key, path, key)
    }

    /// The node at `path` keyed `key` is this flight's destination.
    pub fn lands(&self, path: &str, key: Option<&str>) -> bool {
        selects(&self.target_key, path, key)
    }

    pub fn alphas(&self, target: Rect, now: f64, reduced: bool) -> (f32, f32) {
        let p = self.progress(now, reduced);
        if reduced { return (1.0 - p, p); }
        let rewrap = (target.h - self.source.h).abs() > 2.0;
        let (out, end) = if rewrap { (0.25, 0.4) } else { (0.4, 0.5) };
        ((1.0 - p / out).clamp(0.0, 1.0),
            if rewrap { ((p - 0.1) / (end - 0.1)).clamp(0.0, 1.0) } else { (p / end).clamp(0.0, 1.0) })
    }
}

/// Whether `selector` names the node at `path` keyed `key`: either its key, or a key path such
/// as `msg-42/body` whose leading keys are the node's nearest keyed ancestors (unkeyed ones are
/// skipped), so repeated row-local keys can be told apart.
pub(crate) fn selects(selector: &str, path: &str, key: Option<&str>) -> bool {
    let Some(key) = key else { return false };
    if selector == key { return true; }
    let Some(scope) = selector.strip_suffix(key).and_then(|s| s.strip_suffix('/')) else { return false };
    let Some(above) = path.strip_suffix(key).and_then(|s| s.strip_suffix('/')) else { return false };
    let mut keyed = above.rsplit('/').filter(|seg| !seg.starts_with('#'));
    scope.rsplit('/').all(|seg| keyed.next() == Some(seg))
}

/// Whether `selector` names a highlight keyed `key` in the text at `path`: [`selects`] for
/// `{path}/{key}`, without building it.
pub(crate) fn selects_under(selector: &str, path: &str, key: &str) -> bool {
    if selector == key { return true; }
    let Some(scope) = selector.strip_suffix(key).and_then(|s| s.strip_suffix('/')) else { return false };
    let mut keyed = path.rsplit('/').filter(|seg| !seg.starts_with('#'));
    scope.rsplit('/').all(|seg| keyed.next() == Some(seg))
}

/// Transform glyph geometry without changing its atlas footprint. Clip edges share the same
/// transform; a soft inset handles lines that were only partly visible in the source editor.
pub(crate) fn transformed(mut q: Instance, from: Rect, to: Rect, scale: f32, alpha: f32, clip: [f32; 4], fade: [f32; 4]) -> Instance {
    let zoom = to.w / from.w.max(0.01);
    let map = |v: f32, from: f32, to: f32| (to + (v / scale - from) * zoom) * scale;
    if q.params[1] == crate::render::KIND_GLYPH && q.radii[3] > 0.0 {
        // An outline glyph (`text::Outline`) moves its pen and magnifies its pixel: crisp in flight.
        q.radii = [map(q.radii[0], from.x, to.x), map(q.radii[1], from.y, to.y), q.radii[2], q.radii[3] * zoom];
    } else {
        q.uv[2] = q.rect[2];
        q.uv[3] = q.rect[3];
    }
    q.rect = [
        (to.x + (q.rect[0] / scale - from.x) * zoom) * scale,
        (to.y + (q.rect[1] / scale - from.y) * zoom) * scale,
        q.rect[2] * zoom,
        q.rect[3] * zoom,
    ];
    q.fill = q.fill.map(|v| v * alpha);
    q.clip = clip.map(|v| v * scale);
    q.fade = crate::render::pack_fade(fade.map(|v| v * scale), q.clip, q.clip);
    q
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flight() -> TextFlight {
        TextFlight::new("source".into(), "target".into(), Rect::new(40.0, 400.0, 500.0, 40.0),
            [40.0, 400.0, 540.0, 440.0], Vec::new(), 2.0, 0.0)
    }

    #[test]
    fn moving_destination_uses_shared_progress_without_restarting_it() {
        let mut f = flight();
        f.measure(Rect::new(120.0, 260.0, 400.0, 22.0), 0.0, false);
        let p = f.progress.sample(0.08);
        let v = f.progress.velocity(0.08);
        f.measure(Rect::new(120.0, 220.0, 400.0, 22.0), 0.08, false);
        assert_eq!(f.progress.sample(0.08), p);
        assert_eq!(f.progress.velocity(0.08), v);
        assert_eq!(f.presented.y, 400.0 + (220.0 - 400.0) * p);
        assert!(f.measure(Rect::new(120.0, 220.0, 400.0, 22.0), 2.0, false));
    }

    #[test]
    fn flight_is_time_based_and_reduced_motion_only_fades() {
        let target = Rect::new(120.0, 260.0, 400.0, 22.0);
        let mut slow = flight();
        let mut fast = flight();
        slow.measure(target, 0.0, false);
        fast.measure(target, 0.0, false);
        for i in 1..=30 {
            slow.measure(target, i as f64 / 30.0, false);
            for j in (i * 4 - 3)..=i * 4 { fast.measure(target, j as f64 / 120.0, false); }
            assert_eq!(slow.presented, fast.presented);
        }
        let mut f = flight();
        assert!(!f.measure(target, 0.075, true));
        assert_eq!(f.presented, target);
        assert_eq!(f.alphas(target, 0.075, true), (0.5, 0.5));
        assert!(f.measure(target, 0.15, true));
    }

    #[test]
    fn selectors_match_a_key_or_its_keyed_ancestors() {
        let path = "#0/scroll/#0/msg-42/#1/body";
        assert!(selects("body", path, Some("body")), "a plain key still matches");
        assert!(selects("msg-42/body", path, Some("body")), "unkeyed wrappers are skipped");
        assert!(selects("scroll/msg-42/body", path, Some("body")));
        assert!(!selects("msg-41/body", path, Some("body")));
        assert!(!selects("scroll/body", path, Some("body")), "the scope is the nearest keyed ancestors");
        assert!(!selects("msg-42", path, Some("body")));
        assert!(!selects("body", "#0/#1", None), "unkeyed nodes never match");
        assert!(!selects("x/msg-42/body", path, Some("body")), "more scope than ancestors");
    }
}
