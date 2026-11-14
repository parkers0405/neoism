//! Tooltips (`El::help`), as SwiftUI's `.help`. Help elements are hit-tested on their own, so
//! they take no hover or press from anything. The pointer resting on one for `help_delay`
//! shows its tooltip under the element; a press, scroll or key hides it until the pointer
//! leaves the element, as does the pointer leaving. The tooltip draws over everything and
//! follows its element while that moves under the pointer. A text's links can carry their own
//! help (`Link::help`), hit-tested on their glyphs and shown under the link.
//!
//! A macOS window shows AppKit's own tooltips instead (`set_native_help`): the host registers
//! [`Runtime::help_regions`] with its view and AppKit owns the delay, look and hiding, unless
//! the view sets its own look (`Cx::help_look`), which Rune draws everywhere.
//!
//! Where the tooltip is in its life is one [`Tip`], stepped by [`step`] once a frame with what
//! the pointer is over, and on a dismissal; it mirrors `@opencode/ui`'s tooltip machine
//! (`ui/src/overlays/tooltip/machine.ts`) with Kobalte's open and skip delays. A press on the
//! element keeps its tooltip, as there; a popup or modal over the element suppresses it until
//! the pointer leaves. The look's fade (`HelpLook::fade`) plays as the tooltip opens and
//! closes, the closing one fading out under the next.
use crate::machine::Effects;

use super::*;

#[derive(Default)]
pub(super) struct HelpState {
    /// This frame's help elements: (node, visible rect in points, `hits.len()` before it, the
    /// link of its text when the help is that link's).
    hits: Vec<(NodeId, [f32; 4], usize, Option<usize>)>,
    state: Tip,
    /// The tooltip showing this frame: its text and box in points.
    tip: Option<(String, [f32; 4])>,
    /// The open tooltip as it last drew, for its fade out.
    drawn: Option<Drawn>,
    /// A closed tooltip fading out, and when (clock seconds) it closed.
    leaving: Option<(Drawn, f64)>,
    /// The host shows system tooltips from `help_regions`.
    native: bool,
    regions: Vec<([f32; 4], String)>,
}

/// A help element, or one link of its text, by path.
#[derive(Clone, Debug, PartialEq)]
struct Target {
    path: String,
    link: Option<usize>,
}

/// Where the tooltip is in its life. Times are clock seconds.
#[derive(Clone, Debug, Default, PartialEq)]
enum Tip {
    /// No help element under the pointer, or a tooltip hid more than the skip delay ago.
    #[default]
    Idle,
    /// A tooltip hid as the pointer left its element: until `until`, the next one opens at once.
    Warm { until: f64 },
    /// The pointer rests on `target`; its tooltip opens at `due`.
    Waiting { target: Target, due: f64 },
    /// `target`'s tooltip shows, fading in from `since`.
    Open { target: Target, since: f64 },
    /// A press elsewhere, a scroll or key, or a popup over `target` hid its tooltip until the
    /// pointer leaves it.
    Suppressed { target: Target },
}

impl Tip {
    fn target(&self) -> Option<&Target> {
        match self {
            Tip::Waiting { target, .. } | Tip::Open { target, .. } | Tip::Suppressed { target } => Some(target),
            Tip::Idle | Tip::Warm { .. } => None,
        }
    }
}

/// What the pointer is over as a frame draws.
#[derive(Debug, PartialEq)]
enum Over {
    /// No pointer in the window: it left, or a menu took it.
    Away,
    /// No help element.
    Nothing,
    Help(Target),
    /// A help element, under a popup or modal that the pointer can't reach it through.
    Covered(Target),
}

#[derive(Debug, PartialEq)]
enum Event {
    Frame { over: Over, now: f64 },
    /// A press away from the open tooltip's element, a scroll or a key.
    Dismiss,
}

#[derive(Debug, PartialEq)]
enum Effect {
    /// Wake at this clock time, when a wait ends.
    WakeAt(f64),
    /// The open tooltip closed: fade it out.
    FadeOut,
}

/// The look's open and skip delays, in seconds.
#[derive(Clone, Copy)]
struct Delays {
    open: f64,
    skip: f64,
}

fn step(tip: &mut Tip, event: Event, delays: Delays) -> Effects<Effect> {
    let (over, now) = match event {
        Event::Dismiss => return suppress(tip),
        Event::Frame { over, now } => (over, now),
    };
    let effects = match over {
        // A menu the element opened took the pointer: the tooltip stays hidden over it after.
        Over::Away if matches!(tip, Tip::Suppressed { .. }) => Effects::none(),
        Over::Covered(at) if tip.target() == Some(&at) => suppress(tip),
        Over::Help(at) if tip.target() == Some(&at) => Effects::none(),
        Over::Help(at) => leave(tip, Some(at), now, delays),
        Over::Away | Over::Nothing | Over::Covered(_) => leave(tip, None, now, delays),
    };
    match std::mem::take(tip) {
        Tip::Warm { until } if now > until => {}
        // The wait is over (or there was none: a zero delay opens in the frame it starts).
        Tip::Waiting { target, due } if now + 1e-6 >= due => *tip = Tip::Open { target, since: now },
        other => *tip = other,
    }
    effects
}

/// The pointer left the tooltip's element for `at` (or nothing): an open tooltip closes, and
/// within the skip delay of one closing the next opens at once.
fn leave(tip: &mut Tip, at: Option<Target>, now: f64, delays: Delays) -> Effects<Effect> {
    let open = matches!(tip, Tip::Open { .. });
    let warm = (open && delays.skip > 0.0) || matches!(tip, Tip::Warm { until } if now <= *until);
    let mut effects = if open { Effects::one(Effect::FadeOut) } else { Effects::none() };
    *tip = match at {
        Some(target) if warm => Tip::Open { target, since: now },
        Some(target) => {
            let due = now + delays.open;
            effects.push(Effect::WakeAt(due));
            Tip::Waiting { target, due }
        }
        None if open && delays.skip > 0.0 => Tip::Warm { until: now + delays.skip },
        None if warm => std::mem::take(tip),
        None => Tip::Idle,
    };
    effects
}

/// Hide the tooltip until the pointer leaves its element.
fn suppress(tip: &mut Tip) -> Effects<Effect> {
    match std::mem::take(tip) {
        Tip::Open { target, .. } => {
            *tip = Tip::Suppressed { target };
            Effects::one(Effect::FadeOut)
        }
        Tip::Waiting { target, .. } | Tip::Suppressed { target } => {
            *tip = Tip::Suppressed { target };
            Effects::none()
        }
        idle @ (Tip::Idle | Tip::Warm { .. }) => {
            *tip = idle;
            Effects::none()
        }
    }
}

/// A tooltip as drawn: its text, the box it hangs from (window points, x0 y0 x1 y1), whether
/// it sits above that box, and its opacity.
#[derive(Clone, Debug)]
struct Drawn {
    text: String,
    anchor: [f32; 4],
    above: bool,
    alpha: f32,
    scale: f32,
}

fn overlaps(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

/// `a` minus `b` (x0 y0 x1 y1), as up to four rects: the bands above and below `b`, then the
/// pieces left and right of it.
fn subtract(a: [f32; 4], b: [f32; 4]) -> Vec<[f32; 4]> {
    if !overlaps(a, b) { return vec![a]; }
    let (y0, y1) = (b[1].max(a[1]), b[3].min(a[3]));
    [[a[0], a[1], a[2], y0], [a[0], y1, a[2], a[3]], [a[0], y0, b[0].max(a[0]), y1], [b[2].min(a[2]), y0, a[2], y1]]
        .into_iter().filter(|r| r[2] > r[0] && r[3] > r[1]).collect()
}

impl<S: 'static> Runtime<S> {
    /// The host shows AppKit's tooltips: on macOS, while the view keeps the system look.
    fn native_help(&self) -> bool { self.help.native && self.defaults.help.is_none() }

    /// The look tooltips draw in now: the view's, or the system's for this window.
    fn help_look(&self) -> super::HelpLook {
        self.defaults.help.unwrap_or_else(|| super::HelpLook::system(self.backdrop_dark().unwrap_or(self.env.system_dark)))
    }

    /// The host shows the system's own tooltips for [`Runtime::help_regions`], so Rune draws none.
    #[cfg(target_os = "macos")]
    pub(crate) fn set_native_help(&mut self, on: bool) { self.help.native = on; }

    /// Each visible help element's rect (window points, x0 y0 x1 y1) and text, in paint order,
    /// for a host that shows system tooltips.
    pub fn help_regions(&self) -> &[([f32; 4], String)] { &self.help.regions }

    /// The tooltip Rune shows now: its text and box (window points, x y w h).
    pub fn help_tooltip(&self) -> Option<(&str, [f32; 4])> {
        self.help.tip.as_ref().map(|(t, r)| (t.as_str(), *r))
    }

    /// A tooltip waits for a frame at or after its delay to draw, or fades in or out.
    pub(super) fn tooltip_waiting(&self) -> bool {
        matches!(self.help.state, Tip::Waiting { due, .. } if self.clock + 1e-6 < due) || self.tooltip_fading()
    }

    /// The open tooltip fades in, or a closed one fades out, so frames keep coming.
    fn tooltip_fading(&self) -> bool {
        let look = self.help_look();
        let opening = matches!(self.help.state, Tip::Open { since, .. } if self.clock < since + look.fade[0]);
        opening || self.help.leaving.as_ref().is_some_and(|(_, at)| self.clock < at + look.fade[1])
    }

    /// The look's delays, the runtime's `help_delay` where it sets none.
    fn help_delays(&self) -> Delays {
        let look = self.help_look();
        Delays { open: look.delay.unwrap_or(self.help_delay), skip: look.skip }
    }

    /// Steps the tooltip's state and carries out what it asks.
    fn step_help(&mut self, event: Event) {
        let delays = self.help_delays();
        for effect in step(&mut self.help.state, event, delays) {
            match effect {
                Effect::WakeAt(due) => self.schedule_wakes([std::time::Duration::from_secs_f64((due - self.clock).max(0.0))]),
                Effect::FadeOut => {
                    self.help.tip = None;
                    let fades = self.help_look().fade[1] > 0.0 && !self.reduced_motion;
                    self.help.leaving = self.help.drawn.take().filter(|_| fades).map(|d| (d, self.clock));
                }
            }
        }
    }

    /// During `build`: node `i` has help and draws visible over `rect` (points, x0 y0 x1 y1),
    /// after the first `seq` hits.
    pub(super) fn push_help_hit(&mut self, i: usize, rect: [f32; 4], seq: usize) {
        if rect[2] > rect[0] && rect[3] > rect[1] {
            self.help.hits.push((self.nodes[i].id, rect, seq, None));
        }
    }

    /// During `build`: the line boxes of node `i`'s links that have help, for border box
    /// `rect` and clipped to `clip` (points, x0 y0 x1 y1), after the first `seq` hits.
    pub(super) fn push_link_help_hits(&mut self, i: usize, rect: [f32; 4], clip: [f32; 4], seq: usize) {
        let Some(t) = self.nodes[i].text.as_ref() else { return };
        if !t.links.iter().any(|l| l.help.is_some()) { return; }
        let id = self.nodes[i].id;
        for (k, r) in self.link_boxes(i, rect) {
            if self.nodes[i].text.as_ref().and_then(|t| t.links.get(k)).is_none_or(|l| l.help.is_none()) { continue; }
            let c = [r[0].max(clip[0]), r[1].max(clip[1]), r[2].min(clip[2]), r[3].min(clip[3])];
            if c[2] > c[0] && c[3] > c[1] { self.help.hits.push((id, c, seq, Some(k))); }
        }
    }

    /// The help text of node `i`, or of its link `link`.
    fn help_text(&self, i: usize, link: Option<usize>) -> Option<&String> {
        match link {
            Some(k) => self.nodes[i].text.as_ref()?.links.get(k)?.help.as_ref(),
            None => self.nodes[i].help.as_ref(),
        }
    }

    pub(super) fn clear_help_hits(&mut self) { self.help.hits.clear(); }

    /// A scroll or key: hide the tooltip until the pointer leaves its element.
    pub(super) fn dismiss_help(&mut self) {
        if self.help.state.target().is_some() { self.step_help(Event::Dismiss); }
    }

    /// A press at (x, y): on the open tooltip's element it keeps the tooltip, as a web
    /// tooltip's trigger does through its click; anywhere else it dismisses it.
    pub(super) fn press_help(&mut self, x: f32, y: f32) {
        let on_open = match &self.help.state {
            Tip::Open { target, .. } => self.help_at(x, y).is_some_and(|(i, k)| self.nodes[i].path == target.path && k == target.link),
            _ => false,
        };
        if !on_open { self.dismiss_help(); }
    }

    /// The help element under (x, y): the topmost one, unless something interactive drawn over
    /// it (not inside it) is there too.
    fn help_at(&self, x: f32, y: f32) -> Option<(usize, Option<usize>)> {
        self.help_under(x, y).filter(|u| u.2).map(|(h, link, _)| (h, link))
    }

    /// The topmost help element under (x, y), and whether the pointer reaches it: not behind a
    /// modal, and nothing interactive drawn over it (not inside it) is there too.
    fn help_under(&self, x: f32, y: f32) -> Option<(usize, Option<usize>, bool)> {
        let &(id, _, seq, link) = self.help.hits.iter().rev().find(|&&(id, r, _, _)| self.hit_contains(id, r, x, y))?;
        let h = *self.node_index.get(&id)?;
        if self.subtree_exiting(h) { return None; }
        let over = self.hits.iter().enumerate().rev().find(|&(_, &(id, r))| self.hit_contains(id, r, x, y));
        let covered = over.is_some_and(|(k, (top, _))| {
            k > seq && self.node_index.get(top).is_some_and(|&t| !std::iter::successors(Some(t), |&p| self.nodes[p].parent).any(|p| p == h))
        });
        Some((h, link, self.allowed(h) && !covered))
    }

    /// Whether moving the pointer to (x, y) changes the help element under it, so a host that
    /// skips frames for unchanged pixels still starts or ends a tooltip's wait.
    pub(super) fn help_target_changes(&self, x: f32, y: f32) -> bool {
        let target = self.help.state.target();
        if self.native_help() || self.help.hits.is_empty() && target.is_none() { return false; }
        let at = self.help_at(x, y).map(|(i, k)| (self.nodes[i].path.as_str(), k));
        at != target.map(|t| (t.path.as_str(), t.link))
    }

    /// What the pointer is over for the tooltip, and the node of a help element it reaches.
    fn help_over(&self) -> (Over, Option<usize>) {
        // A menu is up or about to open and has the pointer: the element it came from keeps
        // its tooltip hidden.
        if !self.popups.is_empty() { return (self.help.state.target().cloned().map_or(Over::Away, Over::Covered), None); }
        let Some((x, y)) = self.pointer.filter(|&(x, y)| self.in_viewport(x, y)) else { return (Over::Away, None) };
        let target = |i: usize, link| Target { path: self.nodes[i].path.clone(), link };
        // While moving layout holds hover, the tooltip keeps its element, and nothing sliding
        // under the pointer starts a wait.
        if self.held_hover().is_some() {
            let kept = self.help.state.target().and_then(|t| Some((self.hover_live(&t.path)?, t.link)));
            return kept.map_or((Over::Nothing, None), |(i, k)| (Over::Help(target(i, k)), Some(i)));
        }
        let Some((i, link, reaches)) = self.help_under(x, y) else { return (Over::Nothing, None) };
        let at = target(i, link);
        // A press holds the pointer: it keeps its own element's tooltip, and a drag across
        // other elements starts none.
        if self.pointer_captured() && self.help.state.target() != Some(&at) {
            return (Over::Nothing, None);
        }
        if reaches { (Over::Help(at), Some(i)) } else { (Over::Covered(at), None) }
    }

    /// The parts of help element `h`'s rect `r` that nothing interactive drawn over it (not
    /// inside it), from hit `seq` on, covers.
    fn unoccluded(&self, h: usize, r: [f32; 4], seq: usize) -> Vec<[f32; 4]> {
        let mut parts = vec![self.hit_screen_rect(self.nodes[h].id, r)];
        for &(id, o) in &self.hits[seq.min(self.hits.len())..] {
            let o = self.hit_screen_rect(id, o);
            if !parts.iter().any(|p| overlaps(*p, o)) { continue; }
            let Some(&t) = self.node_index.get(&id) else { continue };
            if std::iter::successors(Some(t), |&p| self.nodes[p].parent).any(|p| p == h) { continue; }
            parts = parts.into_iter().flat_map(|p| subtract(p, o)).collect();
        }
        parts
    }

    /// After `build`'s hits: track the rest, show or hide the tooltip and draw it.
    pub(super) fn update_help(&mut self) {
        self.help.regions.clear();
        if self.native_help() {
            // Nothing behind a modal, and only where nothing else is drawn over the element.
            let mut regions = Vec::new();
            for &(id, r, seq, link) in &self.help.hits {
                let Some(&i) = self.node_index.get(&id) else { continue };
                let Some(text) = self.help_text(i, link) else { continue };
                if self.subtree_exiting(i) || !self.allowed(i) { continue; }
                let words = crate::el::help_words(text);
                regions.extend(self.unoccluded(i, r, seq).into_iter().map(|p| (p, words.clone().into_owned())));
            }
            self.help.regions = regions;
            self.help.tip = None;
            self.help.state = Tip::Idle;
            self.help.drawn = None;
            self.help.leaving = None;
            return;
        }
        let (over, under) = self.help_over();
        self.step_help(Event::Frame { over, now: self.clock });
        self.help.tip = None;
        let look = self.help_look();
        let motion = !self.reduced_motion;
        // The closed tooltip fades out first, under the next one.
        if let Some((d, at)) = self.help.leaving.take() {
            let t = if motion && look.fade[1] > 0.0 { ((self.clock - at) / look.fade[1]) as f32 } else { 1.0 };
            if t < 1.0 {
                let e = rune_motion::Easing::EASE_IN.eval(t);
                let scale = d.scale + (look.fade_scale - d.scale) * e;
                self.draw_tooltip(&Drawn { alpha: d.alpha * (1.0 - e), scale, ..d.clone() });
                self.help.leaving = Some((d, at));
            }
        }
        self.help.drawn = None;
        if let Tip::Open { target, since } = &self.help.state {
            let (link, since) = (target.link, *since);
            let node = under.or_else(|| self.hover_live(&target.path));
            if let Some(i) = node && let Some(text) = self.help_text(i, link).cloned() {
                let e = self.drawn_box(i);
                let anchor = link.and_then(|k| self.link_boxes(i, e).into_iter().filter(|(j, _)| *j == k).map(|(_, r)| r)
                    .reduce(|u, r| [u[0].min(r[0]), u[1].min(r[1]), u[2].max(r[2]), u[3].max(r[3])]));
                let above = link.is_none() && self.nodes[i].cold.get().is_some_and(|c| c.help_above);
                let t = if motion && look.fade[0] > 0.0 { ((self.clock - since) / look.fade[0]) as f32 } else { 1.0 };
                let e_in = rune_motion::Easing::EASE_OUT.eval(t.clamp(0.0, 1.0));
                let d = Drawn { text, anchor: anchor.unwrap_or(e), above, alpha: e_in, scale: look.fade_scale + (1.0 - look.fade_scale) * e_in };
                let [x, y, w, h] = self.draw_tooltip(&d);
                self.help.tip = Some((crate::el::help_words(&d.text).into_owned(), [x, y, w, h]));
                self.help.drawn = Some(d);
            }
        }
        if self.tooltip_fading() { self.schedule_wakes([std::time::Duration::ZERO]); }
    }

    /// Draws `d`'s text (and its keycaps) in a tooltip in the current look under its box
    /// (above it when there's no room below), or above it (under it when there's no room
    /// above) when it says so, kept inside the window. Returns the tooltip's box at full size
    /// (window points, x y w h); it draws at `d.alpha`, scaled by `d.scale` about the middle
    /// of the edge that faces its box.
    fn draw_tooltip(&mut self, d: &Drawn) -> [f32; 4] {
        let (e, text, above) = (d.anchor, d.text.as_str(), d.above);
        let s = self.env.scale;
        let look = self.help_look();
        let key = look.key;
        let mut parts = text.split(crate::el::HELP_KEY);
        let words = parts.next().unwrap_or_default();
        let keys: Vec<String> = parts.map(|k| if key.upper { k.to_uppercase() } else { k.to_owned() }).collect();
        let caps: Vec<_> = keys.iter().map(|k| {
            let l = self.text.layout_rich(k, &[], key.font, s, None);
            let w = ((l.full_width() / s).ceil() + 2.0 * key.pad_x).max(key.height);
            (l, w)
        }).collect();
        let keys_w = if caps.is_empty() { 0.0 } else {
            caps.iter().map(|(_, w)| w).sum::<f32>() + key.gap * (caps.len() - 1) as f32 + if words.is_empty() { 0.0 } else { key.lead }
        };
        let [pad_x, pad_y] = look.padding;
        let (vw, vh) = self.viewport;
        let margin = look.margin;
        let max_w = look.max_w.unwrap_or(vw - 2.0 * margin - 2.0 * pad_x).max(1.0) - keys_w;
        let layout = self.text.layout_rich(words, &[], look.font, s, Some(max_w.max(1.0) * s));
        let (tw, th) = if words.is_empty() { (0.0, 0.0) } else { ((layout.full_width() / s).ceil(), (layout.height() / s).ceil()) };
        let ch = if caps.is_empty() { th } else { th.max(key.height) };
        let (w, h) = (tw + keys_w + 2.0 * pad_x, ch + 2.0 * pad_y);
        let [below, over] = look.gap;
        let mut y = e[3] + below;
        if above && e[1] - over - h >= margin {
            y = e[1] - over - h;
        } else if y + h > vh - margin && e[1] - below - h >= margin {
            y = e[1] - below - h;
        }
        let x = ((e[0] + e[2] - w) * 0.5).clamp(margin, (vw - w - margin).max(margin));
        let y = y.clamp(margin, (vh - h - margin).max(margin));
        let full = [x * s, y * s, w * s, h * s].map(|v| v.round());
        // Scales about the middle of the edge facing the element (Kobalte's transform origin).
        let (k, alpha) = (d.scale, d.alpha);
        let pivot = [((e[0] + e[2]) * 0.5 * s).clamp(full[0], full[0] + full[2]), if y >= e[3] { full[1] } else { full[1] + full[3] }];
        let at_k = |p: [f32; 2]| [pivot[0] + (p[0] - pivot[0]) * k, pivot[1] + (p[1] - pivot[1]) * k];
        let rect_k = |r: [f32; 4]| { let [x, y] = at_k([r[0], r[1]]); [x, y, r[2] * k, r[3] * k] };
        let r = rect_k(full);
        let srgb = self.srgb_blend;
        let radii = [look.radius * s * k; 4];
        let cast = super::shadow_paint::Caster {
            rect: r, radii, shape: radii, k_px: s, scale: s, opacity: alpha, srgb, clip: NO_CLIP, fade: [0.0; 4], hole: None,
            opaque: look.fill.a >= 1.0,
        };
        for sh in look.shadows.iter().flatten().filter(|sh| sh.color.a > 0.0) {
            super::shadow_paint::push_shadow(&mut self.instances, sh, sh.color, &cast);
        }
        let (ring_w, ring) = look.ring.map_or((0.0, Color::TRANSPARENT), |(rw, c)| ((rw * s).max(1.0), c));
        self.instances.push(Instance {
            rect: r,
            radii,
            fill: premul(look.fill, alpha, srgb),
            border: premul(ring, alpha, srgb),
            params: [ring_w, KIND_RECT, 0.0, 1.0],
            uv: [0.0; 4],
            clip: NO_CLIP,
            fade: crate::render::pack_fade([0.0; 4], NO_CLIP, NO_CLIP),
        });
        let top = full[1] + pad_y * s;
        let mut at = full[0] + pad_x * s;
        if !words.is_empty() {
            self.draw_tip_text(&layout, at_k([at, top + ((ch - th) * 0.5 * s).round()]), k, look.ink, alpha);
            at += (tw + key.lead) * s;
        }
        for (l, cw) in &caps {
            let cap = [at, top + ((ch - key.height) * 0.5 * s).round(), cw * s, key.height * s].map(|v| v.round());
            self.instances.push(Instance {
                rect: rect_k(cap),
                radii: [key.radius * s * k; 4],
                fill: premul(key.fill, alpha, srgb),
                border: [0.0; 4],
                params: [0.0, KIND_RECT, 0.0, 1.0],
                uv: [0.0; 4],
                clip: NO_CLIP,
                fade: crate::render::pack_fade([0.0; 4], NO_CLIP, NO_CLIP),
            });
            let lw = l.full_width();
            let origin = [cap[0] + ((cap[2] - lw) * 0.5).round(), cap[1] + ((cap[3] - l.height()) * 0.5).round()];
            self.draw_tip_text(l, at_k(origin), k, key.ink, alpha);
            at += (cw + key.gap) * s;
        }
        [x, y, w, h]
    }

    /// Draws a tooltip's shaped text with its top left at `origin` (device pixels), scaled by
    /// `k` and at `alpha`.
    fn draw_tip_text(&mut self, layout: &parley::Layout<crate::text::Ink>, origin: [f32; 2], k: f32, ink: Color, alpha: f32) {
        let srgb = self.srgb_blend;
        self.glyph_scratch.clear();
        self.text.set_glyph_inks(ink, &[]);
        self.text.glyphs_for_layout_scaled(layout, origin, k, false, &mut self.glyph_scratch);
        for q in &self.glyph_scratch {
            self.instances.extend(glyph_or_image(q, &self.text, &mut self.images, premul(ink, alpha, srgb), NO_CLIP, [0.0; 4]));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Cx, El, Env, el, row};
    use super::{Runtime, subtract};

    #[derive(Default)]
    struct S { modal: bool, cover: bool }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let mut root = el().w_full().h_full()
            .child(el().key("copy").size(40.0, 20.0).pin(Some(20.0), Some(20.0), None, None).help("Copy"))
            .child(el().key("low").size(40.0, 20.0).pin(Some(20.0), Some(200.0), None, None).help("Low"));
        if s.cover {
            // A popover drawn over the right half of "copy".
            root = root.child(el().key("pop").size(100.0, 60.0).pin(Some(40.0), Some(10.0), None, None).on_press(|_: &mut S| {}));
        }
        if s.modal {
            root = root.child(row().key("sheet").size(80.0, 40.0).pin(Some(200.0), Some(200.0), None, None)
                .modal(false, |_: &mut S| {})
                .child(el().key("inside").size(30.0, 20.0).help("Inside")));
        }
        root
    }

    fn runtime(s: S, native: bool) -> Runtime<S> {
        let mut rt = Runtime::new(s, view, (300.0, 300.0), Env::new(true, 2.0));
        rt.help.native = native;
        rt.frame(0.0);
        rt
    }

    fn texts(rt: &Runtime<S>) -> Vec<&str> { rt.help_regions().iter().map(|(_, t)| t.as_str()).collect() }

    #[test]
    fn native_regions_skip_what_a_modal_or_popover_covers() {
        let rt = runtime(S::default(), true);
        assert_eq!(texts(&rt), ["Copy", "Low"]);
        let rt = runtime(S { modal: true, ..S::default() }, true);
        assert_eq!(texts(&rt), ["Inside"], "behind a modal, nothing registers");
        let rt = runtime(S { cover: true, ..S::default() }, true);
        let copy: Vec<[f32; 4]> = rt.help_regions().iter().filter(|(_, t)| t == "Copy").map(|(r, _)| *r).collect();
        assert_eq!(copy, [[20.0, 20.0, 40.0, 40.0]], "only the uncovered half of copy");
    }

    #[test]
    fn subtract_leaves_what_is_outside() {
        assert_eq!(subtract([0.0, 0.0, 10.0, 10.0], [20.0, 0.0, 30.0, 10.0]), [[0.0, 0.0, 10.0, 10.0]]);
        assert_eq!(subtract([0.0, 0.0, 10.0, 10.0], [-1.0, -1.0, 11.0, 11.0]), Vec::<[f32; 4]>::new());
        assert_eq!(subtract([0.0, 0.0, 10.0, 10.0], [4.0, 4.0, 6.0, 6.0]).len(), 4);
    }

    #[test]
    fn reaching_help_wakes_a_host_that_skips_unchanged_frames() {
        // An overlay draws only when observe_pointer says something changed; help that has no
        // hover paint of its own must still start its wait.
        let mut rt = runtime(S::default(), false);
        assert!(!rt.observe_pointer(150.0, 150.0), "nothing there");
        assert!(rt.observe_pointer(30.0, 30.0), "onto help");
        rt.frame(1.0 / 60.0);
        assert!(!rt.observe_pointer(31.0, 30.0), "still on it");
        rt.frame(rt.help_delay);
        assert!(rt.help_tooltip().is_some());
        assert!(rt.observe_pointer(150.0, 150.0), "off it hides the tooltip");
    }

    fn linked(k: &f32, _: &mut Cx<f32>) -> El {
        let label = crate::text("MMMM MMMM")
            .font(crate::TextStyle::new(16.0, 400.0))
            .link(crate::Link::new(0..4, "a"))
            .link(crate::Link::new(5..9, "b").help("Goes to b"))
            .on_link(|_: &mut f32, _, _| {});
        el().w_full().h_full().child(el().pin(Some(10.0), Some(10.0), None, None).scale(*k).child(label))
    }

    /// Rests on (x, y) past the delay and returns the tooltip shown.
    fn rest(rt: &mut Runtime<f32>, x: f32, y: f32) -> Option<(String, [f32; 4])> {
        rt.pointer_move(x, y);
        rt.frame(1.0 / 60.0);
        rt.frame(rt.help_delay + 0.1);
        rt.help_tooltip().map(|(t, r)| (t.to_owned(), r))
    }

    #[test]
    fn a_link_shows_its_own_help_under_its_glyphs() {
        for k in [1.0, 1.6] {
            let mut rt = Runtime::new(k, linked, (400.0, 300.0), Env::new(true, 2.0));
            rt.frame(0.0);
            let i = rt.nodes.iter().position(|n| n.text.is_some()).unwrap();
            let boxes = rt.link_boxes(i, rt.drawn_box(i));
            let b = boxes.iter().find(|(j, _)| *j == 1).unwrap().1;
            let a = boxes.iter().find(|(j, _)| *j == 0).unwrap().1;
            assert!(rest(&mut rt, (a[0] + a[2]) * 0.5, (a[1] + a[3]) * 0.5).is_none(), "zoom {k}: a link without help shows none");
            let (text, r) = rest(&mut rt, (b[0] + b[2]) * 0.5, (b[1] + b[3]) * 0.5).expect("tip on the link");
            assert_eq!(text, "Goes to b");
            assert!(r[1] >= b[3] && r[0] < b[2] && r[0] + r[2] > b[0], "zoom {k}: tip {r:?} sits under the link {b:?}");
            assert!(rest(&mut rt, 300.0, 250.0).is_none(), "leaving hides it");
        }
    }

    #[test]
    fn native_regions_carry_a_links_help() {
        let mut rt = Runtime::new(1.0, linked, (400.0, 300.0), Env::new(true, 2.0));
        rt.help.native = true;
        rt.frame(0.0);
        let texts: Vec<&str> = rt.help_regions().iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(texts, ["Goes to b"]);
    }
}
