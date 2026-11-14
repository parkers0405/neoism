//! `Tx::live_only` skip accounting and debug stale-view checks.
use super::*;

/// Seconds after a commit before debug builds check a `Tx::live_only` event (`skip_view`).
#[cfg(debug_assertions)]
const LIVE_ONLY_CHECK: f64 = 0.25;

impl<S: 'static> Runtime<S> {
    /// Events whose handler called [`Tx::live_only`] and so didn't re-run the view, since the
    /// runtime started.
    pub fn live_only_events(&self) -> u64 {
        self.live_only_skips
    }

    /// Whether debug builds check `Tx::live_only` handlers (on by default): a live-only event
    /// a quarter second or more after the last commit still commits, and Rune warns when the
    /// view built something else. A test counting commits turns it off. Release builds never
    /// check.
    pub fn check_live_only(&mut self, on: bool) {
        #[cfg(debug_assertions)]
        { self.live_only_check = on; }
        let _ = on;
    }

    /// The stale-view warnings debug builds have printed for `Tx::live_only` handlers, each
    /// naming the element and property that differed, so a test can assert there are none.
    /// Always empty in release builds.
    pub fn live_only_warnings(&self) -> &[String] {
        #[cfg(debug_assertions)]
        { &self.live_only_warnings }
        #[cfg(not(debug_assertions))]
        { &[] }
    }

    /// An event whose handler called `Tx::live_only`: false, so it doesn't commit, while this
    /// frame still latches the live values it set. Debug builds check one such event a quarter
    /// second after the last commit by committing anyway (true) and comparing what it built.
    pub(super) fn skip_view(&mut self, label: impl std::fmt::Display) -> bool {
        #[cfg(debug_assertions)]
        if self.live_only_check && self.clock - self.last_commit_clock >= LIVE_ONLY_CHECK {
            if self.live_check.is_none() { self.live_check = Some((label.to_string(), self.view_print())); }
            self.live_checks += 1;
            self.live_dirty = true;
            return true;
        }
        let _ = label;
        self.live_only_skips += 1;
        self.live_dirty = true;
        false
    }

    /// Whether the commit about to run is a live-only check's, which builds what skipping it
    /// would have left on screen, so virtual lists keep their mounted rows.
    pub(super) fn checking_live_only(&self) -> bool {
        #[cfg(debug_assertions)]
        { self.live_check.is_some() }
        #[cfg(not(debug_assertions))]
        { false }
    }

    /// What the last commit built: each element's path, layout, colours and text. Live values
    /// are left out, since a commit rebuilds them from the view and the next latch sets them.
    #[cfg(debug_assertions)]
    pub(super) fn view_print(&self) -> ViewPrint {
        ViewPrint(self.nodes.iter()
            // Retained exits can disappear between commits without a model change.
            .filter(|n| !self.is_exiting(n.id))
            .map(|n| {
                let live = super::paint_tracks::live_colors(n);
                let mut text = n.text.clone();
                if live.text && let Some(t) = &mut text { t.color = Color::TRANSPARENT; }
                NodePrint {
                    path: n.path.clone(),
                    target: n.target,
                    fill: (!live.fill).then_some(n.vis.fill),
                    border: (if live.border { Color::TRANSPARENT } else { n.vis.border() }, n.vis.border_width()),
                    opacity: n.vis.opacity,
                    radius: n.vis.radii(),
                    text,
                }
            })
            .collect())
    }

    /// After the commit a live-only check forced: warn when the view built something else, as
    /// it does when the handler changed state the view reads. Names the first element that
    /// differs and what about it, once per handler, element and property.
    #[cfg(debug_assertions)]
    pub(super) fn compare_live_check(&mut self, check: Option<(String, ViewPrint)>) {
        let Some((handler, before)) = check else { return };
        let Some(d) = before.first_difference(&self.view_print()) else { return };
        let head = format!("rune: {handler} called Tx::live_only, but the view built something else after it: {} of {:?}", d.what, d.path);
        if self.live_only_warnings.iter().any(|w| w.starts_with(&head)) { return; }
        let values = d.values.map(|(was, now)| format!(" went from {was} to {now}")).unwrap_or_default();
        let warning = format!(
            "{head}{values}. The handler may have changed state the view reads, which stays stale until another event \
             commits. Set only Live values in a live-only handler; for a time-dependent view, disable this heuristic \
             with check_live_only(false)."
        );
        eprintln!("{warning}");
        self.live_only_warnings.push(warning);
    }
}

/// What a commit built, node by node, taken only while a live-only check runs.
#[cfg(debug_assertions)]
pub(crate) struct ViewPrint(Vec<NodePrint>);

#[cfg(debug_assertions)]
struct NodePrint {
    path: String,
    target: [f32; 4],
    /// `None` while a `Live` sets it.
    fill: Option<Color>,
    border: (Color, f32),
    opacity: f32,
    radius: [f32; 4],
    text: Option<crate::cold::SharedBox<TextSpec>>,
}

/// The first element whose build differs: what differs, and the two values where they're short.
#[cfg(debug_assertions)]
#[derive(Debug, PartialEq)]
pub(crate) struct Difference {
    pub path: String,
    pub what: &'static str,
    pub values: Option<(String, String)>,
}

#[cfg(debug_assertions)]
impl ViewPrint {
    pub(crate) fn first_difference(&self, after: &ViewPrint) -> Option<Difference> {
        let (a, b) = (&self.0, &after.0);
        let has = |list: &[NodePrint], path: &str| list.iter().any(|n| n.path == path);
        let node = |what, path: &str| Difference { path: path.to_owned(), what, values: None };
        let (mut i, mut j) = (0, 0);
        while i < a.len() && j < b.len() {
            let (x, y) = (&a[i], &b[j]);
            if x.path == y.path {
                if let Some(d) = x.difference(y) { return Some(d); }
                i += 1;
                j += 1;
            } else if !has(b, &x.path) {
                return Some(node("removed element", &x.path));
            } else if !has(a, &y.path) {
                return Some(node("added element", &y.path));
            } else {
                return Some(node("order", &x.path));
            }
        }
        a.get(i).map(|x| node("removed element", &x.path)).or_else(|| b.get(j).map(|y| node("added element", &y.path)))
    }
}

/// A property's hash, comparing floats by their bits as the check always has.
#[cfg(debug_assertions)]
fn key(f: impl FnOnce(&mut std::hash::DefaultHasher)) -> u64 {
    use std::hash::Hasher;
    let mut h = std::hash::DefaultHasher::new();
    f(&mut h);
    h.finish()
}

#[cfg(debug_assertions)]
fn bits(xs: &[f32]) -> Vec<u32> {
    xs.iter().map(|x| x.to_bits()).collect()
}

#[cfg(debug_assertions)]
fn rgba(c: Color) -> Vec<u32> {
    bits(&[c.r, c.g, c.b, c.a])
}

/// `#rrggbbaa` in sRGB.
#[cfg(debug_assertions)]
fn colour(c: Color) -> String {
    let byte = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}{:02x}", byte(srgb_encode(c.r)), byte(srgb_encode(c.g)), byte(srgb_encode(c.b)), byte(c.a))
}

#[cfg(debug_assertions)]
fn points(xs: &[f32]) -> String {
    let xs: Vec<String> = xs.iter().map(|x| format!("{x:.1}")).collect();
    format!("[{}]", xs.join(", "))
}

#[cfg(debug_assertions)]
fn snippet(t: &TextSpec) -> String {
    if t.redacted { return "[redacted]".into(); }
    if t.live.is_some() { return "(live)".into(); }
    const MAX: usize = 40;
    match t.content.char_indices().nth(MAX) {
        Some((at, _)) => format!("{:?}", format!("{}…", &t.content[..at])),
        None => format!("{:?}", t.content),
    }
}

#[cfg(debug_assertions)]
impl NodePrint {
    fn difference(&self, now: &NodePrint) -> Option<Difference> {
        use std::hash::Hash;
        type Part = (&'static str, fn(&NodePrint) -> u64, fn(&NodePrint) -> String);
        let parts: [Part; 5] = [
            ("layout rect", |n| key(|h| bits(&n.target).hash(h)), |n| points(&n.target)),
            ("fill", |n| key(|h| n.fill.map(rgba).hash(h)), |n| n.fill.map_or("(live)".into(), colour)),
            ("border", |n| key(|h| (rgba(n.border.0), n.border.1.to_bits()).hash(h)), |n| format!("{} {:.1}pt", colour(n.border.0), n.border.1)),
            ("opacity", |n| key(|h| n.opacity.to_bits().hash(h)), |n| format!("{:.3}", n.opacity)),
            ("radius", |n| key(|h| bits(&n.radius).hash(h)), |n| points(&n.radius)),
        ];
        let differs = |(what, k, show): &Part| (k(self) != k(now)).then(|| Difference {
            path: self.path.clone(), what, values: Some((show(self), show(now))),
        });
        if let Some(d) = parts.iter().find_map(differs) { return Some(d); }
        let (a, b) = match (&self.text, &now.text) {
            (None, None) => return None,
            (Some(a), Some(b)) => (a, b),
            (a, b) => {
                let show = |t: &Option<crate::cold::SharedBox<TextSpec>>| t.as_ref().map_or("no text".into(), |t| snippet(t));
                return Some(Difference { path: self.path.clone(), what: "text", values: Some((show(a), show(b))) });
            }
        };
        type TextPart = (&'static str, fn(&TextSpec) -> u64, Option<fn(&TextSpec) -> String>);
        let parts: [TextPart; 7] = [
            ("text", |t| key(|h| t.live.is_none().then_some(&t.content).hash(h)), Some(snippet)),
            ("text colour", |t| key(|h| rgba(t.color).hash(h)), Some(|t| colour(t.color))),
            ("text style", |t| key(|h| bits(&[t.style.size, t.style.weight]).hash(h)), Some(|t| format!("{:.1}pt weight {}", t.style.size, t.style.weight))),
            ("pills", |t| key(|h| for p in &t.pills {
                (p.start, p.end, p.icon.map(|i| i.name), p.bg, p.fg).hash(h);
                p.bare.as_ref().map(|b| (b.size.to_bits(), b.gap.to_bits(), b.radius.to_bits(), b.image.as_ref().map(crate::pixels::Pixels::signature))).hash(h);
            }), None),
            ("rings", |t| key(|h| for &(a, b, c, w) in &t.rings { (a, b, c, w.to_bits()).hash(h) }), None),
            ("strikes", |t| key(|h| t.strikes.hash(h)), None),
            ("highlights", |t| key(|h| for hl in &t.highlights {
                (hl.start, hl.end, hl.bg, hl.fg, hl.pad.to_bits(), &hl.key, hl.ring.map(|(c, w)| (c, w.to_bits()))).hash(h)
            }), None),
        ];
        parts.iter().find(|(_, k, _)| k(a) != k(b)).map(|&(what, _, show)| Difference {
            path: self.path.clone(),
            what,
            values: show.map(|s| (s(a), s(b))),
        })
    }
}

/// Folds one handler's `Tx::live_only` into its event's: live-only when every handler that
/// took the event said so.
pub(crate) fn fold_live(live: &mut Option<bool>, took: bool, one: &Tx) {
    if took { *live = Some(live.unwrap_or(true) && one.live_only); }
}
