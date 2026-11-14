//! Frame-by-frame motion checks over a run's element tree.
//!
//! [`Recording::sample`] takes every element's presented rect, visible part and opacity each
//! frame of a run on the virtual clock; [`check`] reads them back and lists what a person would
//! see go wrong: things that appear or vanish with no transition, jump in one frame, lurch,
//! turn back, overshoot, flicker or never settle. Elements are tracked by their tree path, so a
//! keyed row keeps its identity however the rows around it change.
//!
//! Nothing here knows about the app: thresholds live in [`RULES`], one row per finding kind.
//! [`super::Run`] drives a runtime and records it; [`super::gate`] runs and reports the cells.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use crate::ElementInfo;

mod commits;
mod evidence;
mod kicks;
mod loops;
pub mod paint;
mod painted;
pub mod raster;
mod regions;
mod series;

pub use evidence::{dump, sheet};
use paint::Paint;

/// One finding kind, its threshold and what it means. The checker reads its numbers from here.
pub struct Rule {
    pub kind: &'static str,
    pub threshold: f32,
    pub means: &'static str,
}

pub const RULES: &[Rule] = &[
    Rule { kind: "pop-in", threshold: 0.5, means: "mounts at or above this opacity, at full size, with no fade, growth or scale (and no ancestor transitioning)" },
    Rule { kind: "pop-out", threshold: 0.5, means: "unmounts at or above this opacity: it vanished instead of leaving" },
    Rule { kind: "cut-exit", threshold: 0.05, means: "unmounts above this opacity, below pop-out's: a fade cut short" },
    Rule { kind: "jump", threshold: 2.0, means: "an edge moves at least this many pt in one frame between still frames, where it shows (its visible part, or what it holds that shows, moves with it, or it brings what it holds into view with no input)" },
    Rule { kind: "offset-jump", threshold: 2.0, means: "a jump of a scroll's content: every row in it moved by the same delta in one frame" },
    Rule { kind: "appear-then-jump", threshold: 2.0, means: "mounts, then moves at least this many pt on its next frame" },
    Rule { kind: "jerk", threshold: 10.0, means: "a moving edge's per-frame step changes by more than this many pt from one frame to the next" },
    Rule { kind: "stop-start", threshold: 3.0, means: "a travelling edge rests (under 0.5 s) and sets off again the same way this many times in 2 s, with no input but data arriving: it moves in kicks, not one glide" },
    Rule { kind: "reversal", threshold: 1.0, means: "a travelling edge heads this many pt the wrong way and turns back, with no input mid-flight (one monotonic path)" },
    Rule { kind: "overshoot", threshold: 1.0, means: "passes where it comes to rest by this many pt (bounce 0)" },
    Rule { kind: "jiggle", threshold: 1.0, means: "an edge reverses twice or more within one motion, over at least this many pt" },
    Rule { kind: "text-swap", threshold: 0.5, means: "a text element's words change in one frame at or above this opacity, with no roll or crossfade (streaming text excepted)" },
    Rule { kind: "fade-snap", threshold: 0.5, means: "a mounted element's opacity steps by at least this in one frame: it blinks on or off instead of fading" },
    Rule { kind: "flicker", threshold: 0.3, means: "opacity dips by at least this much and comes back within 0.3 s, or an element vanishes and comes back at its place" },
    Rule { kind: "faint", threshold: 0.25, means: "geometry checks skip frames fainter than this opacity: a row fading out can't be seen to lurch" },
    Rule { kind: "load-motion", threshold: 1.0, means: "a row that loaded with the view moves or fades by more than this after its first frame (no animation on load)" },
    Rule { kind: "never-settles", threshold: 0.05, means: "still changing by more than this in the last half second of the run, long after the last input (loops excepted)" },
    Rule { kind: "loop-jump", threshold: 0.03, means: "a loop's phase steps this many periods off its usual step in one frame: it restarted or skipped (a remount, or a new loop where one was running)" },
    Rule { kind: "wakes-after-end", threshold: 1.0, means: "still asking for frames this many seconds after the cell's motion ended (a run that finished)" },
    Rule { kind: "lands-unseen", threshold: 40.0, means: "travels at least this many pt in plain view, then its clip cuts it from view in one frame where it rests: it flew somewhere you can't see" },
    Rule { kind: "remount", threshold: 2.0, means: "words start leaving while the same words mount elsewhere within this many frames: one thing fades out at its old place as its copy fades in (its row lost its key)" },
    Rule { kind: "unpinned", threshold: 2.0, means: "a scroll pinned to its end has its content's bottom this many pt below the viewport with no input since: it stopped following" },
    Rule { kind: "scroll-commit", threshold: 0.25, means: "the app ran its view (its `poll`, a handler or a layout read) on 6 or more of a scroll's frames and at least this share of them: a scroll needs no commit, so each one is a frame's whole view rebuilt mid-gesture" },
    Rule { kind: "snap", threshold: 1.0, means: "a painted property steps by at least this in one frame between frames that hold still, with no input, `Tx::instant` commit or mark asking for it: a colour by OKLab ΔE 0.08 (weighted by the area that shows and its opacity), a corner by 3 pt, a blur by 2 pt, a border by 1 pt, a new image, or two surfaces trading places (z-order). `(pixels)`: a region of the composited window, where no element changed" },
    Rule { kind: "flash", threshold: 0.5, means: "a painted property (or a region of the window) steps out by at least this and comes at least halfway back within 0.4 s, or drifts out and snaps back: a surface that drops out for a frame, or blinks before what replaces it fades in" },
    Rule { kind: "changes-twice", threshold: 0.25, means: "a painted property (or region) changes, rests from 2 frames to this many seconds, then carries on the same way with no input between: one change landed as two" },
    Rule { kind: "twice", threshold: 0.0, means: "two siblings share one key (Rune's `#dup`), so one thing shows twice, as an optimistic copy beside the server's" },
];

/// Whether `old` → `new` is a number changing in otherwise the same words.
fn ticks(old: &str, new: &str) -> bool {
    let shape = |t: &str| {
        let mut out = String::new();
        for ch in t.chars() {
            if !ch.is_ascii_digit() {
                out.push(ch);
            } else if !out.ends_with('#') {
                out.push('#');
            }
        }
        out
    };
    shape(old) == shape(new)
}

pub fn rule(kind: &str) -> f32 {
    RULES.iter().find(|r| r.kind == kind).map_or(f32::INFINITY, |r| r.threshold)
}

const EXITING: u8 = 1;
const SCROLLING: u8 = 2;
const GROWING: u8 = 4;
const LIVE: u8 = 8;
const VIRTUAL: u8 = 16;
const TEXT: u8 = 32;
/// It or something inside it paints (a fill, words, or a leaf such as an icon) at an opacity
/// that shows. Without it an opacity change draws nothing.
const INK: u8 = 128;

#[derive(Clone, Copy, Debug)]
pub struct Snap {
    pub rect: [f32; 4],
    pub vis: [f32; 4],
    pub op: f32,
    pub scale: f32,
    pub flags: u8,
    /// A loop's phase (0 to 1); NaN without one.
    pub phase: f32,
    /// How far scrolling carries it, x and y ([`ElementInfo::scrolled`]).
    pub scrolled: [f32; 2],
}

pub struct Track {
    pub path: String,
    pub parent: Option<usize>,
    /// Frame index → presented state, for the frames it was mounted (sorted).
    pub frames: Vec<(usize, Snap)>,
    pub text: Option<String>,
    /// Its words each time they changed, from the frame they showed.
    pub words: Vec<(usize, String)>,
    /// A field's own words each time they changed (typed, pasted, set or cleared), which
    /// `words` leaves out.
    pub edits: Vec<(usize, String)>,
    /// What it painted on each of `frames` (empty when the recording has no paint).
    pub paint: Vec<Option<Paint>>,
}

impl Track {
    fn at(&self, k: usize) -> Option<&Snap> {
        self.frames.binary_search_by_key(&k, |f| f.0).ok().map(|i| &self.frames[i].1)
    }

    /// What it painted on frame `k`.
    pub fn paint_at(&self, k: usize) -> Option<&Paint> {
        let i = self.frames.binary_search_by_key(&k, |f| f.0).ok()?;
        self.paint.get(i)?.as_ref()
    }
}

#[derive(Default)]
pub struct Recording {
    pub size: (f32, f32),
    pub hz: f64,
    pub tracks: Vec<Track>,
    index: HashMap<String, usize>,
    /// Frames an input or a scripted event landed on, with what it was. One named `event…` is
    /// data arriving, not a hand on the app: it doesn't let a pinned scroll go (see `unpinned`).
    pub inputs: Vec<(usize, String)>,
    /// Frames sampled so far.
    pub frames: usize,
    /// Frames before this are the settled start: what's mounted there loaded with the view.
    pub start: usize,
    /// Frames at which the view loaded content that must arrive still (see `load-motion`).
    pub loads: Vec<usize>,
    /// Per frame: whether the runtime wanted another.
    pub awake: Vec<bool>,
    /// The frame the cell's motion ended on (see `wakes-after-end`).
    pub quiet_from: Option<usize>,
    /// The app rolls its live numbers, so a count or a clock ticking in place ("9s" → "10s") is
    /// a `text-swap` too. By default a tick passes.
    pub counts_roll: bool,
    /// Per frame [`super::Run`] stepped: whether a viewport scrolled, and the app's reasons
    /// for the view to run again (`CommitWhy` bits: its `poll`, a handler, a layout read).
    scrolled: Vec<(bool, u16)>,
    /// Per frame: a commit landed at once (`Tx::instant`), so what it changed steps by design.
    pub instant: Vec<bool>,
    /// Per frame: the composited window in cells ([`raster`]), shared while it's unchanged.
    pub grids: Vec<Option<Arc<raster::Grid>>>,
    /// Per frame: the box (x y w h, points) of the tooltip Rune drew for `El::help`, which
    /// shows and hides at once after a rest, as AppKit's does.
    pub tips: Vec<Option<[f32; 4]>>,
}

impl Recording {
    pub fn new(size: (f32, f32), hz: f64) -> Self {
        Recording { size, hz, ..Default::default() }
    }

    /// The frame just run: `scrolling` while a viewport moved, `why` its commits' reasons.
    pub(crate) fn commits(&mut self, scrolling: bool, why: u16) {
        use crate::runtime::CommitWhy as W;
        self.scrolled.push((scrolling, why & (W::POLL | W::INPUT | W::LAYOUT | W::READ)));
        self.instant.push(why & W::INSTANT != 0);
    }

    /// The frame just sampled's composited window.
    pub fn grid(&mut self, grid: Option<Arc<raster::Grid>>) {
        self.grids.resize(self.frames.saturating_sub(1), None);
        self.grids.push(grid);
    }

    fn track(&mut self, path: &str, parent: Option<usize>) -> usize {
        if let Some(&i) = self.index.get(path) {
            return i;
        }
        let i = self.tracks.len();
        self.tracks.push(Track { path: path.to_owned(), parent, frames: Vec::new(), text: None, words: Vec::new(), edits: Vec::new(), paint: Vec::new() });
        self.index.insert(path.to_owned(), i);
        i
    }

    pub fn input(&mut self, what: impl Into<String>) {
        self.inputs.push((self.frames, what.into()));
    }

    /// Takes one frame of the element tree.
    pub fn sample(&mut self, els: &[ElementInfo]) {
        self.sample_with(els, None);
    }

    /// Takes one frame of the element tree and what each element painted, by path and whether
    /// it's leaving (`Runtime::paints`).
    pub(crate) fn sample_with(&mut self, els: &[ElementInfo], paints: Option<&crate::runtime::HashMap<(String, bool), Paint>>) {
        let k = self.frames;
        self.frames += 1;
        // An element exiting beside a live one at the same path (a key re-added mid-exit) is
        // its own track.
        let live: HashSet<&str> = els.iter().filter(|e| !e.exiting).map(|e| e.path.as_str()).collect();
        let mut ids = vec![usize::MAX; els.len()];
        // Children follow their parent, so one pass from the end carries ink up the tree.
        let mut ink = vec![false; els.len()];
        let mut parent_of = vec![false; els.len()];
        for e in els {
            if let Some(p) = e.parent.filter(|&p| p < els.len()) { parent_of[p] = true; }
        }
        for (i, e) in els.iter().enumerate().rev() {
            let paints = e.fill.a > 0.01 || e.text.as_deref().is_some_and(|t| !t.is_empty()) || !parent_of[i];
            ink[i] |= paints && e.opacity > 0.02;
            if ink[i] && let Some(p) = e.parent.filter(|&p| p < els.len()) { ink[p] = true; }
        }
        for (i, e) in els.iter().enumerate() {
            let path = if e.exiting && live.contains(e.path.as_str()) { format!("{} (leaving)", e.path) } else { e.path.clone() };
            let parent = e.parent.and_then(|p| ids.get(p).copied()).filter(|&p| p != usize::MAX);
            let t = self.track(&path, parent);
            ids[i] = t;
            let mut flags = 0;
            if e.exiting { flags |= EXITING }
            if e.scrolling { flags |= SCROLLING }
            if e.in_growing_row || e.revealing { flags |= GROWING }
            if e.live_intent != 0 { flags |= LIVE }
            if e.virtual_extent { flags |= VIRTUAL }
            if e.role == "text" { flags |= TEXT }
            if ink[i] { flags |= INK }
            let track = &mut self.tracks[t];
            if track.frames.last().is_some_and(|f| f.0 == k) {
                continue; // a duplicate path this frame: keep the first
            }
            track.parent = parent.or(track.parent);
            if track.text.is_none() { track.text = e.text.clone().filter(|t| !t.is_empty()); }
            // A field's words change as they're typed (a link's words are its own).
            if let Some(words) = e.text.as_deref().filter(|t| !t.is_empty() && (!e.focusable || e.role == "link"))
                && track.words.last().is_none_or(|w| w.1 != words)
            {
                track.words.push((k, words.to_owned()));
            }
            if e.focusable && e.role != "link" {
                let words = e.text.as_deref().unwrap_or("");
                if track.edits.last().map_or(!words.is_empty(), |w| w.1 != words) { track.edits.push((k, words.to_owned())); }
            }
            track.frames.push((k, Snap { rect: e.rect, vis: e.visible, op: e.opacity, scale: e.scale, flags, phase: e.loop_phase.map_or(f32::NAN, |p| p as f32), scrolled: e.scrolled }));
            if let Some(paints) = paints {
                track.paint.resize(track.frames.len() - 1, None);
                track.paint.push(paints.get(&(e.path.clone(), e.exiting)).cloned());
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Finding {
    pub kind: &'static str,
    pub element: String,
    pub frame: usize,
    pub detail: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:<16} {:<44} @{:<4} {}", self.kind, self.element, self.frame, self.detail)
    }
}

/// A readable name: its keyed segments from the row down (`row-3/bubble`).
pub fn short(path: &str) -> String {
    let segs: Vec<&str> = path.split('/').collect();
    let keyed: Vec<usize> = (0..segs.len()).filter(|&i| !segs[i].starts_with('#')).collect();
    let from = keyed.len().saturating_sub(2);
    let start = keyed.get(from).copied().unwrap_or(0);
    segs[start..].join("/")
}

fn area(r: &[f32; 4]) -> f32 {
    r[2].max(0.0) * r[3].max(0.0)
}

fn on_window(r: &[f32; 4], (w, h): (f32, f32)) -> [f32; 4] {
    let (x0, y0) = (r[0].max(0.0), r[1].max(0.0));
    let (x1, y1) = ((r[0] + r[2]).min(w), (r[1] + r[3]).min(h));
    [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)]
}

fn seen(s: &Snap, size: (f32, f32)) -> bool {
    s.op > 0.02 && area(&on_window(&s.vis, size)) >= 1.0
}

fn edge(s: &Snap, axis: usize) -> f32 {
    s.rect[axis]
}

/// An edge of what shows: the visible part, in the window, where one wholly past the window's
/// edge would come in. An edge its clips hide on both frames moves nobody can see.
fn shown_edge(s: &Snap, axis: usize, size: (f32, f32)) -> f32 {
    let r = on_window(&s.vis, size);
    [r[0].min(size.0), r[1].min(size.1), r[2], r[3]][axis]
}

/// An edge against the content it scrolls with: a node gliding inside a list that follows its
/// end makes one motion there, however the scroll adds to it on screen.
fn own_edge(s: &Snap, axis: usize) -> f32 {
    s.rect[axis] + s.scrolled.get(axis).copied().unwrap_or(0.0)
}

const AXES: [&str; 4] = ["x", "y", "w", "h"];

pub fn check(rec: &Recording) -> Vec<Finding> {
    let size = rec.size;
    let n = rec.frames;
    let tr = &rec.tracks;
    let mut out = Vec::new();
    let input_frames: Vec<usize> = rec.inputs.iter().map(|i| i.0).collect();
    let input_between = |a: usize, b: usize| input_frames.iter().any(|&k| k >= a && k <= b);
    let born = |t: &Track| t.frames.first().map(|f| f.0);
    // A label over its own glyphs (a rolling text) changes its words as they move.
    let parents: HashSet<usize> = tr.iter().filter_map(|t| t.parent).collect();
    let died = |t: &Track| t.frames.last().map(|f| f.0 + 1).filter(|&k| k < n);
    let gaps = |t: &Track| t.frames.windows(2).filter(|w| w[1].0 != w[0].0 + 1).map(|w| (w[0].0 + 1, w[1].0)).collect::<Vec<_>>();

    // An ancestor changing opacity or scale over frames a..=b carries whatever mounts or
    // unmounts inside it (one that grows but doesn't clip it doesn't: see `grows`).
    let transitioning = |t: usize, a: usize, b: usize| -> bool {
        let mut p = tr[t].parent;
        while let Some(i) = p {
            let s: Vec<&Snap> = (a.saturating_sub(1)..=b).filter_map(|k| tr[i].at(k)).collect();
            if s.windows(2).any(|w| (w[0].op - w[1].op).abs() > 0.005 || (w[0].scale - w[1].scale).abs() > 0.001)
                || s.iter().any(|s| s.flags & (GROWING | EXITING) != 0)
            {
                return true;
            }
            p = tr[i].parent;
        }
        false
    };

    // Loops (a spinner's dots, a busy ring, a blinking caret) change on most frames they're
    // mounted, as one element or as siblings taking turns; their wraps are their design.
    let loops = loops::loops(rec);

    // Pops: births and deaths after the settled start, reported at the outermost element.
    let births: HashMap<usize, Vec<usize>> = tr.iter().enumerate().fold(HashMap::new(), |mut m, (i, t)| {
        let mut ks: Vec<usize> = born(t).into_iter().collect();
        ks.extend(gaps(t).iter().map(|g| g.1));
        for k in ks.into_iter().filter(|&k| k > rec.start) { m.entry(k).or_default().push(i); }
        m
    });
    let deaths: HashMap<usize, Vec<usize>> = tr.iter().enumerate().fold(HashMap::new(), |mut m, (i, t)| {
        let mut ks: Vec<usize> = died(t).into_iter().collect();
        ks.extend(gaps(t).iter().map(|g| g.0));
        for k in ks.into_iter().filter(|&k| k > rec.start) { m.entry(k).or_default().push(i); }
        m
    });
    let outermost = |ids: &[usize]| -> Vec<usize> {
        let set: HashSet<usize> = ids.iter().copied().collect();
        ids.iter().copied().filter(|&i| {
            let mut p = tr[i].parent;
            while let Some(j) = p {
                if set.contains(&j) { return false; }
                p = tr[j].parent;
            }
            true
        }).collect()
    };
    let mut left: HashMap<usize, Vec<(usize, Snap)>> = HashMap::new(); // frame → (track, last state) of pop-outs
    let mut ks: Vec<&usize> = deaths.keys().collect();
    ks.sort();
    for &k in ks {
        for i in outermost(&deaths[&k]) {
            let Some(last) = tr[i].at(k - 1).copied() else { continue };
            if !seen(&last, size) || area(&last.vis) < 16.0 || loops.contains(&i) || last.flags & VIRTUAL != 0 {
                continue;
            }
            // Back at the same place a few frames later: a blink, not a removal.
            if let Some(&(_, back)) = tr[i].frames.iter().find(|f| f.0 > k && f.0 <= k + 12) {
                if (back.rect[1] - last.rect[1]).abs() < 4.0 && seen(&back, size) {
                    out.push(Finding { kind: "flicker", element: short(&tr[i].path), frame: k,
                        detail: format!("vanished at opacity {:.2} and came back {} frames later at the same place", last.op, tr[i].frames.iter().find(|f| f.0 > k).unwrap().0 - k) });
                    continue;
                }
            }
            if last.op >= rule("pop-out") {
                left.entry(k).or_default().push((i, last));
                out.push(Finding { kind: "pop-out", element: short(&tr[i].path), frame: k,
                    detail: format!("unmounted at opacity {:.2}, {:.0}×{:.0} at ({:.0},{:.0}){}", last.op, last.rect[2], last.rect[3], last.rect[0], last.rect[1],
                        if transitioning(i, k - 1, k - 1) { " inside a moving container" } else { "" }) });
            } else if last.op > rule("cut-exit") {
                out.push(Finding { kind: "cut-exit", element: short(&tr[i].path), frame: k,
                    detail: format!("unmounted at opacity {:.2} before its fade reached 0", last.op) });
            }
        }
    }
    let mut ks: Vec<&usize> = births.keys().collect();
    ks.sort();
    for &k in ks {
        for i in outermost(&births[&k]) {
            let t = &tr[i];
            let Some(first) = t.at(k).copied() else { continue };
            if first.op < rule("pop-in") || !seen(&first, size) || area(&on_window(&first.vis, size)) < 16.0 || first.flags & (GROWING | VIRTUAL) != 0 || loops.contains(&i) {
                continue;
            }
            let next: Vec<Snap> = (k + 1..=k + 2).filter_map(|j| t.at(j).copied()).collect();
            let grows = next.iter().any(|s| (s.op - first.op).abs() > 0.01 || (s.scale - first.scale).abs() > 0.002
                || (s.rect[2] - first.rect[2]).abs() > 0.5 || (s.rect[3] - first.rect[3]).abs() > 0.5
                || (s.vis[2] - first.vis[2]).abs() > 0.5 || (s.vis[3] - first.vis[3]).abs() > 0.5)
                // A row mounting under a point tall that grows open, however gently it starts.
                || first.rect[3] < 1.0 && (k + 1..=k + (0.25 * rec.hz) as usize).filter_map(|j| t.at(j)).any(|s| s.rect[3] >= 1.0);
            if grows || transitioning(i, k, k + 2) {
                continue;
            }
            let moved = next.first().map(|s| (s.rect[0] - first.rect[0]).abs().max((s.rect[1] - first.rect[1]).abs())).unwrap_or(0.0);
            let handoff = left.get(&k).and_then(|l| l.iter().find(|(j, _)| tr[*j].text.is_some() && tr[*j].text == t.text || last_seg(&tr[*j].path) == last_seg(&t.path)));
            let mut detail = format!("mounted at opacity {:.2}, {:.0}×{:.0} at ({:.0},{:.0}) with no transition", first.op, first.rect[2], first.rect[3], first.rect[0], first.rect[1]);
            if let Some((j, s)) = handoff {
                detail += &format!("; replaces {} which vanished from ({:.0},{:.0}) this frame (identity lost)", short(&tr[*j].path), s.rect[0], s.rect[1]);
            }
            if moved >= rule("appear-then-jump") {
                out.push(Finding { kind: "appear-then-jump", element: short(&t.path), frame: k, detail: format!("{detail}, then moved {moved:.0} pt") });
            } else {
                out.push(Finding { kind: "pop-in", element: short(&t.path), frame: k, detail });
            }
        }
    }

    // Geometry, per track and axis.
    let riding = |i: usize, k: usize, axis: usize, d: f32| -> bool {
        if axis >= 2 { return false; }
        let Some(p) = tr[i].parent else { return false };
        match (tr[p].at(k), tr[p].at(k - 1)) {
            (Some(a), Some(b)) => (edge(a, axis) - edge(b, axis) - d).abs() < 0.25,
            _ => false,
        }
    };
    // An instant toggle (a disclosure opening on a tap) reflows what's beside it in that frame.
    // An element pushed by exactly the height another changed by, the edge between them moving
    // with it, on an input's frame, is that toggle's; it's reported where the height changed.
    let inside = |a: &str, b: &str| a.len() > b.len() && a.starts_with(b) && a.as_bytes()[b.len()] == b'/';
    let pushed = |i: usize, k: usize, axis: usize, d: f32| -> bool {
        if axis != 1 || !input_between(k - 1, k) { return false; }
        let Some(a) = tr[i].at(k - 1) else { return false };
        tr.iter().enumerate().any(|(j, o)| {
            if j == i || inside(&tr[i].path, &o.path) || inside(&o.path, &tr[i].path) { return false; }
            let (Some(oa), Some(ob)) = (o.at(k - 1), o.at(k)) else { return false };
            if ((ob.rect[3] - oa.rect[3]).abs() - d.abs()).abs() > 0.25 { return false; }
            let (top, bottom) = (ob.rect[1] - oa.rect[1], ob.rect[1] + ob.rect[3] - oa.rect[1] - oa.rect[3]);
            let above = oa.rect[1] + oa.rect[3] <= a.rect[1] + 0.5 && (bottom - d).abs() < 0.25;
            let below = oa.rect[1] >= a.rect[1] + a.rect[3] - 0.5 && (top - d).abs() < 0.25;
            above || below
        })
    };
    // How many of what `i` holds show on frames k - 1 and k and move there by `d` with it, as a
    // scroll's rows do when its content jumps though the content's visible part stays put.
    // With `entering`, also those it brings into view from just off it with no input on the
    // way: content that teleports pops in a row that didn't show the frame before.
    let carried = |i: usize, k: usize, axis: usize, d: f32, entering: bool| -> usize {
        if axis >= 2 { return 0; }
        let entering = entering && !input_between(k - 1, k);
        tr.iter().filter(|c| inside(&c.path, &tr[i].path)).filter(|c| {
            let (Some(a), Some(b)) = (c.at(k - 1), c.at(k)) else { return false };
            if !seen(b, size) || (edge(b, axis) - edge(a, axis) - d).abs() >= 0.25 { return false; }
            if seen(a, size) { return (shown_edge(b, axis, size) - shown_edge(a, axis, size) - d).abs() < 0.25 }
            entering && b.op >= rule("faint") && b.flags & INK != 0 && b.flags & (GROWING | EXITING) == 0
        }).count()
    };
    let mut geo: Vec<(usize, Finding)> = painted::painted(rec, &loops);
    for (i, t) in tr.iter().enumerate() {
        if loops.contains(&i) || t.frames.len() < 3 { continue; }
        for axis in 0..4 {
            // Deltas between consecutive frames, None across a gap.
            let fs = &t.frames;
            let d: Vec<Option<f32>> = (1..fs.len()).map(|j| (fs[j].0 == fs[j - 1].0 + 1).then(|| edge(&fs[j].1, axis) - edge(&fs[j - 1].1, axis))).collect();
            // A jump or jerk is judged on what shows: the visible part moves with it, or what it
            // holds that shows does. A fold dropping its rows above the window steps its top and
            // height with the scroll offset, and nothing that shows moves.
            let vd = |j: usize| if fs[j].0 == fs[j - 1].0 + 1 { shown_edge(&fs[j].1, axis, size) - shown_edge(&fs[j - 1].1, axis, size) } else { 0.0 };
            let clipped = |j: usize| area(&on_window(&fs[j].1.vis, size)) + 1.0 < area(&fs[j].1.rect);
            let quiet = |j: usize| fs[j].1.flags & (LIVE | VIRTUAL) != 0 || !seen(&fs[j].1, size) || fs[j].1.op < rule("faint");
            // The box of words still arriving (a streaming link's) takes each new word at once.
            let grew = |k: usize| t.words.iter().position(|w| w.0 == k).is_some_and(|p| p > 0 && t.words[p].1.starts_with(t.words[p - 1].1.as_str()));
            for j in 1..fs.len() {
                let Some(step) = d[j - 1] else { continue };
                let k = fs[j].0;
                if k <= rec.start || quiet(j) || quiet(j - 1) || fs[j].1.flags & GROWING != 0 && axis == 3 { continue; }
                // A scroll under a hand, or one gliding over frames, moves as it should. One that
                // lands in a single frame with nothing asking for it is a teleport.
                let scrolling = (fs[j].1.flags | fs[j - 1].1.flags) & SCROLLING != 0;
                let before = if j >= 2 { d[j - 2].unwrap_or(0.0) } else { 0.0 };
                let after = d.get(j).copied().flatten().unwrap_or(0.0);
                let name = short(&t.path);
                let riders = |entering| if clipped(j) || clipped(j - 1) { carried(i, k, axis, step, entering) } else { 0 };
                if step.abs() >= rule("jump") && before.abs() < 0.5 && after.abs() < 0.5 {
                    // An input's scroll cut moves a row with the content it scrolls in. A virtual
                    // list's stack can move by less, as rows above it unmount into its space.
                    let cut = axis < 2 && input_between(k - 1, k) && (own_edge(&fs[j].1, axis) - own_edge(&fs[j - 1].1, axis)).abs() < 0.25;
                    if scrolling && input_between(k - 1, k) || cut || riding(i, k, axis, step) || pushed(i, k, axis, step) || grew(k) { continue; }
                    let riders = riders(true);
                    if vd(j).abs() < rule("jump") && riders == 0 { continue; }
                    let appeared = born(t) == Some(fs[j - 1].0) && fs[j - 1].0 > rec.start;
                    let scroll_content = axis == 1 && fs[j].1.rect[3] > fs[j].1.vis[3] + 1.0 && riders >= 3;
                    let kind = if appeared { "appear-then-jump" } else if scroll_content { "offset-jump" } else { "jump" };
                    geo.push((i, Finding { kind, element: name, frame: k,
                        detail: format!("{} {:.1} → {:.1} ({:+.1} pt) in one frame{}", AXES[axis], edge(&fs[j - 1].1, axis), edge(&fs[j].1, axis), step,
                            if scroll_content { format!(", carrying {riders} elements") } else { String::new() }) }));
                } else if !scrolling && (step - before).abs() > rule("jerk") && before.abs() >= 0.5 && step.abs() >= 0.5 {
                    if riding(i, k, axis, step) { continue; }
                    if j >= 2 && (vd(j) - vd(j - 1)).abs() <= rule("jerk") && riders(false) == 0 { continue; }
                    geo.push((i, Finding { kind: "jerk", element: name, frame: k,
                        detail: format!("{} step {:+.1} → {:+.1} pt/frame ({:.0} → {:.0} pt/s)", AXES[axis], before, step, before as f64 * rec.hz, step as f64 * rec.hz) }));
                }
            }
            // Motion segments: frames that move this edge against its scroll, through the odd
            // still frame at a turn (a spring's peak), until it rests three frames.
            let d: Vec<Option<f32>> = (1..fs.len()).map(|j| (fs[j].0 == fs[j - 1].0 + 1).then(|| own_edge(&fs[j].1, axis) - own_edge(&fs[j - 1].1, axis))).collect();
            let moving = |j: usize| d[j - 1].is_some_and(|x| x.abs() > 0.01);
            let mut j = 1;
            while j < fs.len() {
                if !moving(j) { j += 1; continue; }
                let a = j;
                let mut b = j;
                while j < fs.len() && d[j - 1].is_some() && j - b <= 3 {
                    if moving(j) { b = j; }
                    j += 1;
                }
                j = b + 1; // fs[a-1..=b] is the segment
                if b - a + 1 < 3 { continue; }
                let seg = &fs[a - 1..=b];
                let (k0, k1) = (seg[0].0, seg[seg.len() - 1].0);
                if k1 <= rec.start || seg.iter().all(|f| !seen(&f.1, size) || f.1.op < rule("faint")) || seg.iter().any(|f| f.1.flags & (LIVE | VIRTUAL) != 0) { continue; }
                if input_between(k0 + 1, k1) { continue; } // retargeted mid-flight
                let vals: Vec<f32> = seg.iter().map(|f| own_edge(&f.1, axis)).collect();
                let (start, rest) = (vals[0], vals[vals.len() - 1]);
                let travel = rest - start;
                let mut signs = Vec::new();
                let mut last = start;
                for &v in &vals[1..] {
                    if (v - last).abs() > 0.05 {
                        let s = (v - last).signum();
                        if signs.last() != Some(&s) { signs.push(s); }
                        last = v;
                    }
                }
                let reversals = signs.len().saturating_sub(1);
                if reversals == 0 { continue; }
                let dir = if travel.abs() > 0.5 { travel.signum() } else { signs[0] };
                let past = vals.iter().map(|v| (v - rest) * dir).fold(0.0f32, f32::max);
                let back = vals.iter().map(|v| (start - v) * dir).fold(0.0f32, f32::max);
                let amp = vals.iter().cloned().fold(f32::MIN, f32::max) - vals.iter().cloned().fold(f32::MAX, f32::min);
                let name = short(&t.path);
                let (kind, detail) = if past >= rule("overshoot") && travel.abs() > 0.5 {
                    ("overshoot", format!("{} {start:.1} → {rest:.1} passes its rest by {past:.1} pt ({:.0}%)", AXES[axis], 100.0 * past / travel.abs()))
                } else if reversals >= 2 && amp >= rule("jiggle") {
                    ("jiggle", format!("{} reverses {reversals}× over {amp:.1} pt ({start:.1} → {rest:.1})", AXES[axis]))
                } else if back >= rule("reversal") {
                    ("reversal", format!("{} {start:.1} → {rest:.1} heads {back:.1} pt the wrong way first", AXES[axis]))
                } else if travel.abs() <= 0.5 && amp >= rule("reversal") {
                    ("reversal", format!("{} goes {amp:.1} pt out from {start:.1} and back", AXES[axis]))
                } else { continue };
                geo.push((i, Finding { kind, element: name, frame: k0 + 1, detail: format!("{detail} over frames {}–{k1}", k0 + 1) }));
            }
        }
        // Opacity dips that come back.
        let fs = &t.frames;
        let mut spans = Vec::new();
        let mut j = 0;
        while j < fs.len() {
            let s = fs[j].1;
            if fs[j].0 <= rec.start || s.op < 0.6 || s.flags & EXITING != 0 || s.flags & INK == 0 || !seen(&s, size) { j += 1; continue; }
            let horizon = fs[j].0 + (0.3 * rec.hz) as usize;
            let mut low = s.op;
            let mut low_at = j;
            let mut back = None;
            let mut m = j + 1;
            while m < fs.len() && fs[m].0 <= horizon && fs[m].0 == fs[m - 1].0 + 1 {
                if fs[m].1.op < low { low = fs[m].1.op; low_at = m; }
                if low <= s.op - rule("flicker") && fs[m].1.op >= s.op - 0.1 { back = Some(m); break; }
                m += 1;
            }
            // One input taking it away and another bringing it back (a toggle pressed twice
            // quickly) is them answered, not a flicker.
            let answered = |m: usize| rec.inputs.iter().filter(|x| x.0 > fs[j].0 && x.0 <= fs[m].0).count() >= 2;
            if let Some(m) = back.filter(|&m| answered(m)) {
                j = m + 1;
            } else if let Some(m) = back {
                geo.push((i, Finding { kind: "flicker", element: short(&t.path), frame: fs[low_at].0,
                    detail: format!("opacity {:.2} → {:.2} → {:.2} over frames {}–{}", s.op, low, fs[m].1.op, fs[j].0, fs[m].0) }));
                spans.push((fs[j].0, fs[m].0));
                j = m + 1;
            } else {
                j += 1;
            }
        }
        // Words that change in place instead of rolling or crossfading. Words that grow or
        // shrink from the same start (streaming, a reveal) aren't a swap, nor is a count or a
        // clock ticking ("1s" → "2s", "54.4K" → "64.6K") unless the app rolls its numbers.
        for w in t.words.windows(2) {
            let ((_, old), (k, new)) = (&w[0], &w[1]);
            let (Some(a), Some(b)) = (t.at(k.wrapping_sub(1)), t.at(*k)) else { continue };
            if *k <= rec.start || parents.contains(&i) || new.starts_with(old.as_str()) || old.starts_with(new.as_str()) || !rec.counts_roll && ticks(old, new) { continue; }
            if (a.flags | b.flags) & (LIVE | EXITING | VIRTUAL) != 0 || a.op.min(b.op) < rule("text-swap") || !seen(b, size) { continue; }
            geo.push((i, Finding { kind: "text-swap", element: short(&t.path), frame: *k, detail: format!("\"{old}\" → \"{new}\" in one frame") }));
        }
        // Opacity that steps instead of fading.
        for w in fs.windows(2) {
            let (a, b) = (&w[0].1, &w[1].1);
            if w[1].0 != w[0].0 + 1 || w[1].0 <= rec.start || (a.op - b.op).abs() < rule("fade-snap") { continue; }
            let shown = if b.op > a.op { b } else { a };
            if spans.iter().any(|&(a, b)| w[1].0 >= a && w[1].0 <= b) { continue; }
            if !seen(shown, size) || shown.flags & INK == 0 || area(&on_window(&shown.vis, size)) < 16.0 || (a.flags | b.flags) & (EXITING | VIRTUAL) != 0 { continue; }
            geo.push((i, Finding { kind: "fade-snap", element: short(&t.path), frame: w[1].0,
                detail: format!("opacity {:.2} → {:.2} in one frame", a.op, b.op) }));
        }
    }
    // A child that rides its parent's motion is reported at the parent.
    let reported: HashSet<(usize, &str, usize)> = geo.iter().map(|(i, f)| (*i, f.kind, f.frame)).collect();
    for (i, f) in geo.iter() {
        let mut p = tr[*i].parent;
        let mut dup = false;
        while let Some(j) = p {
            if ["reversal", "overshoot", "jiggle", "flicker", "jerk", "fade-snap", "snap", "flash", "changes-twice"].contains(&f.kind) && (0..=2).any(|o| reported.contains(&(j, f.kind, f.frame.saturating_sub(o))) || reported.contains(&(j, f.kind, f.frame + o))) {
                dup = true;
                break;
            }
            p = tr[j].parent;
        }
        if !dup { out.push(f.clone()); }
    }

    // Rows that loaded with the view stay put after their first frame.
    for &load in &rec.loads {
        let quiet_until = input_frames.iter().chain(&rec.loads).copied().filter(|&k| k > load + 1).min().unwrap_or(n);
        let mut hits: Vec<(usize, Finding)> = Vec::new();
        for (i, t) in tr.iter().enumerate() {
            let Some(b) = born(t) else { continue };
            if b < load || b > load + 2 || loops.contains(&i) { continue; }
            let Some(first) = t.at(b + 1).copied() else { continue };
            for k in b + 2..quiet_until.min(n) {
                let Some(s) = t.at(k) else { break };
                let dy = (0..4).map(|a| (s.rect[a] - first.rect[a]).abs()).fold(0.0, f32::max);
                let dop = (s.op - first.op).abs();
                if (dy > rule("load-motion") || dop > 0.1) && seen(s, size) {
                    hits.push((i, Finding { kind: "load-motion", element: short(&t.path), frame: k,
                        detail: format!("loaded at frame {b}, then {} by frame {k}", if dy > rule("load-motion") { format!("moved {dy:.1} pt") } else { format!("faded {:.2} → {:.2}", first.op, s.op) }) }));
                    break;
                }
            }
        }
        let set: HashSet<usize> = hits.iter().map(|h| h.0).collect();
        for (i, f) in hits {
            if !tr[i].parent.is_some_and(|p| set.contains(&p)) { out.push(f); }
        }
    }

    // Settling: nothing but loops changes in the last half second, if it's well after input.
    let last_input = input_frames.iter().copied().max().unwrap_or(0);
    let tail = (0.5 * rec.hz) as usize;
    if n > tail && n - tail > last_input + (0.5 * rec.hz) as usize {
        let mut still: Vec<(usize, f32)> = Vec::new();
        for (i, t) in tr.iter().enumerate() {
            if loops.contains(&i) { continue; }
            let fs: Vec<&(usize, Snap)> = t.frames.iter().filter(|f| f.0 >= n - tail).collect();
            let moved = fs.windows(2).map(|w| (0..4).map(|a| (w[1].1.rect[a] - w[0].1.rect[a]).abs()).fold((w[1].1.op - w[0].1.op).abs(), f32::max)).fold(0.0, f32::max);
            if moved > rule("never-settles") && fs.iter().any(|f| seen(&f.1, size)) { still.push((i, moved)); }
        }
        let set: HashSet<usize> = still.iter().map(|s| s.0).collect();
        for (i, moved) in still {
            if tr[i].parent.is_some_and(|p| set.contains(&p)) { continue; }
            out.push(Finding { kind: "never-settles", element: short(&tr[i].path), frame: n - tail, detail: format!("still moving {moved:.2} pt/frame in the last 0.5 s") });
        }
    }
    loops::jumps(rec, &mut out);
    commits::scroll_commits(rec, &mut out);
    kicks::kicks(rec, &loops, &mut out);
    // Words that leave as the same words mount: a row remounted under a new key.
    let window = rule("remount") as usize;
    let leaving = |i: usize, k: usize| {
        let mut p = Some(i);
        while let Some(j) = p {
            if tr[j].at(k).is_some_and(|s| s.flags & EXITING != 0) { return true; }
            p = tr[j].parent;
        }
        false
    };
    fn words_at(t: &Track, k: usize) -> Option<&str> {
        t.words.iter().rev().find(|w| w.0 <= k).map(|w| w.1.as_str()).or(t.text.as_deref())
    }
    // The same element under another key: its path differs in one segment. (An item moving to
    // another list, or turning into another element, is a different element on purpose.)
    let rekeyed = |a: &str, b: &str| {
        let (a, b) = (a.trim_end_matches(" (leaving)"), b.trim_end_matches(" (leaving)"));
        let (a, b): (Vec<_>, Vec<_>) = (a.split('/').collect(), b.split('/').collect());
        a.len() == b.len() && a.iter().zip(&b).filter(|(x, y)| x != y).count() == 1
    };
    for (i, t) in tr.iter().enumerate() {
        let Some(k) = t.frames.windows(2).find(|w| w[1].0 == w[0].0 + 1 && w[0].0 > rec.start && !leaving(i, w[0].0) && leaving(i, w[1].0) && seen(&w[0].1, size)).map(|w| w[1].0) else { continue };
        let Some(words) = words_at(t, k).filter(|w| w.trim().len() >= 8) else { continue };
        if let Some(copy) = tr.iter().enumerate().find(|&(j, o)| j != i && rekeyed(&t.path, &o.path)
            && born(o).is_some_and(|b| b > rec.start && b + window >= k && b <= k + window && !leaving(j, b) && words_at(o, b) == Some(words))).map(|(_, o)| o) {
            out.push(Finding { kind: "remount", element: short(&t.path), frame: k, detail: format!("\"{}\" leaves as a copy mounts at {}", words.chars().take(40).collect::<String>(), short(&copy.path)) });
        }
    }
    // A scroll pinned to its end lets go with no one touching it: its content's bottom slides
    // below the viewport's. Inputs named `event…` (data arriving) don't count as touching it.
    let touched = |a: usize, b: usize| rec.inputs.iter().any(|(k, what)| *k >= a && *k <= b && !what.starts_with("event"));
    for t in tr {
        let Some(parent) = t.parent.map(|p| &tr[p]) else { continue };
        let mut pinned: Option<usize> = None;
        for (k, s) in &t.frames {
            // A scroll's content: taller than the viewport it fills.
            let viewport = parent.at(*k).is_some_and(|v| (0..4).all(|a| (v.rect[a] - s.vis[a]).abs() < 0.5));
            if !viewport || s.rect[3] <= s.vis[3] + 1.0 { pinned = None; continue; }
            let below = s.rect[1] + s.rect[3] - (s.vis[1] + s.vis[3]);
            if below.abs() < 0.5 { pinned = Some(*k); continue; }
            let Some(p) = pinned else { continue };
            if *k > rec.start && below >= rule("unpinned") && !touched(p, *k) {
                out.push(Finding { kind: "unpinned", element: short(&t.path), frame: *k, detail: format!("its end, pinned at frame {p}, is {below:.1} pt below the viewport") });
                pinned = None;
            }
        }
    }
    // Rune names a second sibling under one key `key#dupN`: the same thing shows twice.
    for t in tr.iter().filter(|t| t.path.rsplit('/').next().is_some_and(|s| s.contains("#dup"))) {
        if let Some((k, _)) = t.frames.iter().find(|f| f.1.op >= rule("faint") && f.1.vis[2] > 0.0 && f.1.vis[3] > 0.0) {
            out.push(Finding { kind: "twice", element: short(&t.path), frame: *k, detail: "a second element under its key".into() });
        }
    }
    // A flight draws unclipped and keeps its size, so one that lands out of view (below the
    // window, or under a clip) shows whole all the way there, then vanishes the frame it rests.
    let window = (1.5 * rec.hz) as usize;
    let mut unseen: HashMap<usize, usize> = HashMap::new();
    for (i, t) in tr.iter().enumerate() {
        let fs = &t.frames;
        let hit = (1..fs.len()).find(|&j| {
            let ((ka, a), (kb, b)) = (fs[j - 1], fs[j]);
            let whole = a.rect[2] >= 4.0 && a.rect[3] >= 4.0 && area(&a.vis) >= 0.9 * area(&a.rect);
            let still = (0..4).all(|x| (a.rect[x] - b.rect[x]).abs() < 0.5);
            let shown = a.op >= rule("faint") && a.flags & INK != 0 && (a.flags | b.flags) & EXITING == 0;
            if kb != ka + 1 || !whole || !still || !shown || area(&on_window(&b.vis, size)) >= 1.0 { return false; }
            fs[..j].iter().rev().take_while(|f| f.0 + window >= ka).any(|f| {
                let same_size = (2..4).all(|x| (f.1.rect[x] - a.rect[x]).abs() < 1.0);
                let travel = (f.1.rect[0] - a.rect[0]).abs() + (f.1.rect[1] - a.rect[1]).abs();
                same_size && seen(&f.1, size) && travel >= rule("lands-unseen")
            })
        });
        let Some(j) = hit else { continue };
        let k = fs[j].0;
        unseen.insert(i, k);
        if t.parent.and_then(|p| unseen.get(&p)) == Some(&k) { continue; }
        out.push(Finding { kind: "lands-unseen", element: short(&t.path), frame: k, detail: "travelled in view, then its clip hid it where it rests".into() });
    }
    if let Some(q) = rec.quiet_from {
        let from = q + (f64::from(rule("wakes-after-end")) * rec.hz) as usize;
        if let Some(k) = (from..n).find(|&k| rec.awake.get(k) == Some(&true)) {
            out.push(Finding { kind: "wakes-after-end", element: "frame".into(), frame: k, detail: format!("asked for a frame {:.2} s after the motion ended", (k - q) as f64 / rec.hz) });
        }
    }
    // A load swaps the page in on its frame: what mounts, unmounts or moves then is the swap.
    out.retain(|f| !(["pop-in", "pop-out", "jump", "offset-jump", "appear-then-jump", "fade-snap"].contains(&f.kind)
        && rec.loads.iter().any(|&l| f.frame >= l && f.frame <= l + 2)));
    painted::intended(rec, &mut out);
    let pixels = regions::regions(rec, &loops, &out);
    out.extend(pixels);
    out.sort_by_key(|f| (f.frame, f.kind));
    out
}

fn last_seg(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;
