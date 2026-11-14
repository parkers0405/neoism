//! Reading an element's laid-out rect: `Cx::layout` (or only what a view computes from it,
//! `Cx::layout_with` and `Cx::layout_size`) for views, `El::on_layout` for handlers, and
//! `El::on_screen_layout` for handlers in global screen points (the rect plus the window origin
//! its host reports through `Runtime::set_window_origin`).
//!
//! A `layout_with` read keeps its function, and re-runs its view only when the function gives
//! the new rect something other than it gave the old one, so a size read doesn't re-run when
//! the element only moves, and a "scrolled past the top" flag only when it flips.
//!
//! All of them see where layout puts an element, never a frame of its spring, so they don't
//! re-run through animations. View reads see it unscrolled (window points with every viewport
//! at its origin), so scrolling re-runs no view. None of them apply an ancestor's visual
//! transform (`El::scale`): they're layout points. `Cx::window_layout` and
//! `Cx::window_layout_with` read it after scrolling, under a key no element can have
//! ([`window_key`]), and handlers hear it after scrolling. A view reading something its own
//! output resizes (its own size, say) can feed back into itself; that settles instead of
//! looping:
//! - Inside a commit, the view re-runs while what it read moved, at most [`PASSES`] more times.
//!   The values it is given next are then the final layout's, so nothing re-runs by itself.
//! - Between commits, only scrolling moves a window read, which re-runs readers next frame.
//! - Commits caused only by layout reads or `on_layout`, with no scroll moving, count up; past
//!   [`ECHOES`] in a row, reads and `on_layout` stop causing commits until anything else
//!   commits or a scroll moves. Reads get the current rects at the next commit.
//!
//! What a view is given for each key stays what its reads last saw until a read of the key
//! moves or a commit runs the view again, so movement below a read's threshold adds up until
//! it counts.
use std::any::TypeId;
use std::rc::Rc;

use super::*;
use super::key_index::{self, KeyIndex};

#[path = "selection/rects.rs"]
mod selection_rects;
pub(crate) use selection_rects::{SELECTION, SelectedRange, boxes_moved};

/// Extra measurements one commit may take while a read moves.
const PASSES: usize = 2;
/// Commits in a row that layout feedback alone may cause.
const ECHOES: u32 = 4;
/// A rect counts as moved when an edge moves more than this (points).
const MOVED: f32 = 0.5;

type SameFn = dyn Fn([f32; 4], [f32; 4]) -> bool;

/// Whether two rects give a `Cx::layout_with` read's function the same value.
#[derive(Clone)]
pub(crate) enum Same {
    /// A function that captures nothing (`layout_size`'s, say): no allocation, and two reads
    /// with the same function are one dependency.
    Static(&'static SameFn, TypeId),
    /// One that captures something.
    Shared(Rc<SameFn>),
}

impl Same {
    pub(crate) fn new<T: PartialEq, F: Fn([f32; 4]) -> T + 'static>(f: F) -> Same {
        let same = move |a: [f32; 4], b: [f32; 4]| f(a) == f(b);
        if std::mem::size_of_val(&same) == 0 {
            // Leaking a zero-sized value allocates and leaks nothing.
            Same::Static(Box::leak(Box::new(same)), TypeId::of::<F>())
        } else {
            Same::Shared(Rc::new(same))
        }
    }

    /// Whether `a` and `b` give the function the same value.
    pub(crate) fn holds(&self, a: [f32; 4], b: [f32; 4]) -> bool {
        match self {
            Same::Static(f, _) => f(a, b),
            Same::Shared(f) => f(a, b),
        }
    }

    /// Whether `self` and `other` are the same function.
    pub(crate) fn is(&self, other: &Same) -> bool {
        match (self, other) {
            (Same::Static(_, a), Same::Static(_, b)) => a == b,
            (Same::Shared(a), Same::Shared(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// Whether the function's value differs between rects `a` and `b`; an element appearing or
    /// going counts.
    pub(crate) fn moved(&self, a: Option<[f32; 4]>, b: Option<[f32; 4]>) -> bool {
        match (a, b) {
            (Some(a), Some(b)) => !self.holds(a, b),
            (a, b) => a.is_some() != b.is_some(),
        }
    }
}

/// One layout read a view made: the element's key, and what of its rect the view depends on.
#[derive(Clone)]
pub(crate) struct LayoutRead {
    pub(crate) key: String,
    /// The `layout_with` function's check, or `None` for the whole rect (`Cx::layout`).
    pub(crate) same: Option<Same>,
    /// Where the view called the `Cx` read, for the debug scroll-commit warning.
    pub(crate) at: Option<&'static std::panic::Location<'static>>,
}

impl LayoutRead {
    fn moved(&self, a: Option<[f32; 4]>, b: Option<[f32; 4]>) -> bool {
        match &self.same {
            None => moved(a, b),
            Some(same) => same.moved(a, b),
        }
    }
}

/// The layout reads a view made.
pub(crate) type LayoutReads = Vec<LayoutRead>;

/// Counts a read of `key` in `reads`, once per kind: a whole-rect read once, and each
/// `layout_with` function once. A whole-rect read doesn't cover a function's read, since it
/// ignores changes a function may see (a 0.4 pt resize, say).
pub(crate) fn note_read(reads: &mut LayoutReads, key: &str, same: Option<Same>) {
    note_read_at(reads, key, same, None);
}

/// [`note_read`] of a read made at `at`.
pub(crate) fn note_read_at(reads: &mut LayoutReads, key: &str, same: Option<Same>, at: Option<&'static std::panic::Location<'static>>) {
    let counted = reads.iter().any(|r| r.key == key && match (&r.same, &same) {
        (None, None) => true,
        (Some(a), Some(b)) => a.is(b),
        _ => false,
    });
    if !counted { reads.push(LayoutRead { key: key.to_owned(), same, at }); }
}

/// What each key a view read resolved to, as last given to the view, and the selection's line
/// boxes when it read them ([`SELECTION`]).
#[derive(Clone, Default)]
pub(crate) struct LayoutRects {
    rects: HashMap<String, Option<[f32; 4]>>,
    pub(crate) selection: Rc<[[f32; 4]]>,
}

impl std::ops::Deref for LayoutRects {
    type Target = HashMap<String, Option<[f32; 4]>>;
    fn deref(&self) -> &Self::Target { &self.rects }
}

impl std::ops::DerefMut for LayoutRects {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.rects }
}

/// The last view's layout reads, and what each key resolved to.
#[derive(Default)]
pub(crate) struct LayoutSeen {
    pub(crate) rects: LayoutRects,
    pub(super) reads: LayoutReads,
}

impl LayoutSeen {
    pub(crate) fn is_empty(&self) -> bool {
        self.reads.is_empty()
    }
}

/// The rect `rects` holds for `key`, when the view read it.
pub(crate) fn seen_rect(rects: &LayoutRects, key: &str) -> Option<[f32; 4]> {
    rects.get(key).copied().flatten()
}

impl<S> Cx<S> {
    /// The laid-out rect `[x, y, w, h]` of the element keyed `key` (or named by a key path such
    /// as `"msg-42/body"`, as `Tx::fly_text` takes), or `None` when there is none. It is where
    /// layout puts the element, not a frame of its spring, in window points as if every scroll
    /// viewport it sits in were scrolled to the top-left: scrolling doesn't move it, so a read
    /// inside a scrolling list costs no view run per frame. Outside any scroll viewport that's
    /// where the window shows it. A view that needs where the element is in the window now,
    /// after scrolling (to float something outside the list over a row, say), reads
    /// [`Self::window_layout`] instead.
    ///
    /// Window points here are layout points: a visual transform on an ancestor, such as
    /// [`El::scale`](crate::El::scale), isn't applied, because layout keeps the unscaled size.
    /// Under a scaled ancestor, map a rect through that ancestor's scale and origin yourself.
    /// The same holds for every layout read, [`Self::selection_rects`], [`Self::caret`] and
    /// [`El::on_layout`](crate::El::on_layout).
    ///
    /// A view that reads it re-runs when that rect moves or resizes past half a point, in the
    /// commit that moved it. Reading an element the view itself resizes (its own size, say)
    /// settles: the commit re-runs the view at most twice more while the value moves, then
    /// keeps the last layout, and layout reads alone can't keep causing commits. A view whose
    /// output never reaches a fixed point gets a stale value rather than oscillating.
    ///
    /// A view that uses only part of the rect should read that part with [`Self::layout_size`]
    /// or [`Self::layout_with`], which don't re-run it while the part it uses holds.
    #[track_caller]
    pub fn layout(&self, key: &str) -> Option<[f32; 4]> {
        note_read_at(&mut self.layout_reads.borrow_mut(), key, None, Some(std::panic::Location::caller()));
        let rect = seen_rect(&self.layout, key);
        crate::memo::read_layout(key, rect);
        rect
    }

    /// `f` of the rect [`Self::layout`] returns, re-running the view only when `f` of the new
    /// rect differs from `f` of the old one (or the element appears or goes). Prefer it to
    /// `layout` whenever the view needs something that changes less often than the rect:
    /// `layout_with("card", |r| r[2] >= 400.0)` re-runs on the resizes that cross the
    /// breakpoint, not on every resize. It settles on the first layout like `layout`, so the
    /// first frame already has the right value.
    ///
    /// `f` runs again on each later layout until the view re-runs, so keep it cheap and pure.
    /// A function item or a closure that captures nothing costs no more than `layout`, and
    /// reading it again in the same view counts once. A closure that captures something, or a
    /// `fn` pointer, isn't zero-sized: it costs an allocation per read, and each read counts
    /// (and is checked) separately, so keep those out of loops.
    #[track_caller]
    pub fn layout_with<T: PartialEq + 'static>(&self, key: &str, f: impl Fn([f32; 4]) -> T + 'static) -> Option<T> {
        let rect = seen_rect(&self.layout, key);
        let value = rect.map(&f);
        let same = Same::new(f);
        crate::memo::read_layout_with(key, &same, rect);
        note_read_at(&mut self.layout_reads.borrow_mut(), key, Some(same), Some(std::panic::Location::caller()));
        value
    }

    /// `[w, h]` of the rect [`Self::layout`] returns: [`Self::layout_with`] the size. Prefer it
    /// whenever the view uses only the size: it re-runs when the width or height changes, not
    /// when the element only moves.
    #[track_caller]
    pub fn layout_size(&self, key: &str) -> Option<[f32; 2]> {
        self.layout_with(key, size)
    }

    /// [`Self::layout`] where the window shows the element now, after its viewports' scrolling.
    /// A view that reads it re-runs the next frame a scroll moves the element past half a
    /// point, so every frame of a scroll: read it only for what has to follow the scroll, and
    /// prefer [`Self::window_layout_with`] for a value that changes less often.
    #[track_caller]
    pub fn window_layout(&self, key: &str) -> Option<[f32; 4]> {
        self.layout(&window_key(key))
    }

    /// [`Self::layout_with`] of the rect [`Self::window_layout`] returns:
    /// `window_layout_with("r1", |r| r[1] < 0.0)` re-runs a header on the frames row 1 scrolls
    /// past the top of the window, not on every frame of the scroll.
    #[track_caller]
    pub fn window_layout_with<T: PartialEq + 'static>(&self, key: &str, f: impl Fn([f32; 4]) -> T + 'static) -> Option<T> {
        self.layout_with(&window_key(key), f)
    }
}

/// The layout-read key a window-position read of `key` is noted under.
pub(crate) fn window_key(key: &str) -> String {
    format!("{WINDOW}{key}")
}

/// The element key a window-position read's key stands for, or `None` for an unscrolled read.
#[cfg(debug_assertions)]
pub(crate) fn window_read_key(key: &str) -> Option<&str> {
    key.strip_prefix(WINDOW)
}

/// The prefix of a window-position read's key, which no element key starts with.
const WINDOW: &str = "\u{1}window:";

/// `[w, h]` of `rect`.
pub(crate) fn size(rect: [f32; 4]) -> [f32; 2] {
    [rect[2], rect[3]]
}

/// Whether a whole-rect read counts `a` to `b` as a move: an edge moved past [`MOVED`], or
/// the element appeared or went.
pub(crate) fn moved(a: Option<[f32; 4]>, b: Option<[f32; 4]>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.iter().zip(b).any(|(x, y)| (x - y).abs() > MOVED),
        (None, None) => false,
        _ => true,
    }
}

impl<S: 'static> Runtime<S> {
    /// Node `i`'s laid-out rect `[x, y, w, h]` in window points, after its viewports' scrolling.
    fn laid_out(&self, nodes: &[Node], i: usize) -> [f32; 4] {
        self.placed(nodes, i, true)
    }

    /// [`Self::laid_out`], or when not `scrolled`, where it would be with every viewport
    /// scrolled to its origin, which only a commit moves.
    fn placed(&self, nodes: &[Node], i: usize, scrolled: bool) -> [f32; 4] {
        let unscrolled;
        let scrolls = if scrolled { &self.scrolls } else { unscrolled = HashMap::default(); &unscrolled };
        let t = nodes[i].target;
        let (mut dx, mut dy) = (0.0, 0.0);
        let mut up = nodes[i].parent.filter(|_| nodes[i].above.is_none() && scrolled);
        while let Some(p) = up {
            if let Some(st) = scrolls.get(&nodes[p].id) {
                dy += st.offset as f32;
                dx += st.x.shift() as f32;
            }
            up = nodes[p].parent.filter(|_| nodes[p].above.is_none());
        }
        if let Some(anc) = std::iter::successors(Some(i), |&p| nodes[p].parent).find(|&p| nodes[p].above.is_some())
            && let Some(a) = &nodes[anc].above
            && let Some(k) = anchor_index(&nodes, anc, a)
        {
            let at = sticky::anchored_target(nodes, scrolls, k, self.viewport);
            let (w, h) = (nodes[anc].target[2] - nodes[anc].target[0], nodes[anc].target[3] - nodes[anc].target[1]);
            let (ax, ay) = anchor_position(a, at, (w, h), self.viewport);
            dx -= ax - nodes[anc].target[0];
            dy -= ay - nodes[anc].target[1];
        }
        // Layout's own size, not the edges' difference, which rounds differently where it sits.
        [t[0] - dx, t[1] - dy, nodes[i].local.w, nodes[i].local.h]
    }

    /// Node `i`'s unscrolled layout rect, under its own and its ancestors' static `El::scale`,
    /// `El::origin` and `El::rotate`.
    fn static_layout(&self, i: usize) -> [f32; 4] {
        let t = self.nodes[i].target;
        let (k, turn) = std::iter::successors(Some(i), |&p| self.nodes[p].parent)
            .fold((1.0_f32, 0.0_f32), |(k, a), p| {
                let [ox, oy] = self.nodes[p].vis.origin.unwrap_or([0.5, 0.5]);
                (k * self.nodes[p].static_scale, a + self.nodes[p].vis.rotate + (ox - 0.5) + (oy - 0.5))
            });
        [t[0] * k + turn, t[1] * k + turn, (t[2] - t[0]) * k, (t[3] - t[1]) * k]
    }

    fn layout_of(&self, nodes: &[Node], index: Option<&KeyIndex>, key: &str) -> Option<[f32; 4]> {
        if key == SELECTION { return None; }
        let (key, scrolled) = key.strip_prefix(WINDOW).map_or((key, false), |key| (key, true));
        let first = match index {
            Some(index) => index.first(nodes, key),
            None => key_index::scan_first(nodes, key),
        };
        if let Some(i) = first {
            return Some(self.placed(nodes, i, scrolled));
        }
        let (i, [x0, y0, x1, y1]) = self.highlight_target_with(nodes, index, None, key)?;
        let node_rect = self.placed(nodes, i, scrolled);
        let t = nodes[i].target;
        Some([node_rect[0] + (x0 - t[0]), node_rect[1] + (y0 - t[1]), x1 - x0, y1 - y0])
    }

    /// Checks `reads` (the last view's when `None`) against `nodes` (the current frame's when
    /// `None`), by what each read depends on. When one moved, or when `force`, the next view
    /// gets the current rects; otherwise it keeps what the reads last saw, so drift below a
    /// read's threshold adds up. Returns whether one moved.
    pub(super) fn layout_moved(&mut self, reads: Option<LayoutReads>, pass: Option<(&[Node], &[SelectedRange])>, force: bool) -> bool {
        let reads = reads.unwrap_or_else(|| std::mem::take(&mut self.layout_seen.reads));
        #[cfg(debug_assertions)]
        let nodes_given = pass.is_some();
        // A pass's own nodes are seen once; the current nodes are seen again every frame.
        let own;
        let (nodes, index) = match pass {
            Some((nodes, _)) => {
                own = (reads.len() > key_index::SCAN_READS).then(|| KeyIndex::of(nodes));
                (nodes, own.as_ref())
            }
            None => {
                self.key_index.prepare(&self.nodes, self.commit_gen, reads.len());
                (&self.nodes[..], self.key_index.get(&self.nodes, self.commit_gen))
            }
        };
        let now: Vec<Option<[f32; 4]>> = reads.iter().map(|r| self.layout_of(nodes, index, &r.key)).collect();
        let boxes = reads.iter().any(|r| r.key == SELECTION).then(|| match pass {
            Some((nodes, ranges)) => self.selection_boxes_in(nodes, ranges),
            None => self.selection_boxes(),
        });
        let rects = &mut self.layout_seen.rects;
        let moved = reads.iter().zip(&now).position(|(r, rect)| rects.get(&r.key).is_none_or(|old| r.moved(*old, *rect)));
        let changed = moved.is_some() || boxes.as_ref().is_some_and(|b| boxes_moved(&rects.selection, b));
        // The once-a-frame check names the read for the scroll warning.
        #[cfg(debug_assertions)]
        if !nodes_given && let Some(k) = moved { self.watch.meter.read(&reads[k].key, reads[k].at); }
        if changed || force {
            if let Some(b) = boxes { rects.selection = b.into(); }
            for (r, rect) in reads.iter().zip(now) {
                match rects.get_mut(&r.key) {
                    Some(seen) => *seen = rect,
                    None => { rects.insert(r.key.clone(), rect); }
                }
            }
        }
        self.layout_seen.reads = reads;
        changed
    }

    /// Before a commit runs the view: what it is given for each key it read becomes the
    /// current rect, since it re-runs anyway.
    pub(super) fn refresh_layout_seen(&mut self) {
        if self.layout_seen.rects.is_empty() { return; }
        self.key_index.prepare(&self.nodes, self.commit_gen, self.layout_seen.rects.len());
        let index = self.key_index.get(&self.nodes, self.commit_gen);
        let now: Vec<_> = self.layout_seen.rects.keys().map(|k| self.layout_of(&self.nodes, index, k)).collect();
        for (seen, rect) in self.layout_seen.rects.values_mut().zip(now) { *seen = rect; }
        if self.layout_seen.rects.contains_key(SELECTION) { self.layout_seen.rects.selection = self.selection_boxes().into(); }
    }

    /// Once a frame: whether a layout read moved, so the view re-runs. Scrolling reopens
    /// feedback first, so the first scroll frame after feedback stopped still checks.
    pub(super) fn layout_reads_moved(&mut self) -> bool {
        if self.layout_seen.is_empty() { return false; }
        if self.scrolling() { self.layout_echoes = 0; }
        if self.selection_read_moved() { self.dirty = true; }
        self.layout_feedback_open() && self.layout_moved(None, None, false)
    }

    /// Re-measures `measured` while a layout read moved, at most [`PASSES`] times, then leaves
    /// the reads matching the final layout so a view that can't settle doesn't loop. Keys no
    /// pass read any more go only at the end, so one read behind a gate the next pass opens
    /// keeps its rect.
    pub(super) fn settle_layout_reads(&mut self, mut measured: Measured, tx: &Tx) -> Measured {
        if measured.layout_reads.is_empty() {
            self.layout_seen = LayoutSeen::default();
            return measured;
        }
        let mut settled = false;
        for _ in 0..PASSES {
            if !self.layout_moved(Some(std::mem::take(&mut measured.layout_reads)), Some((&measured.nodes, &measured.selection)), false) {
                settled = true;
                break;
            }
            self.discard_measured(measured);
            measured = self.measure(tx);
        }
        if !settled { self.layout_moved(Some(std::mem::take(&mut measured.layout_reads)), Some((&measured.nodes, &measured.selection)), true); }
        let LayoutSeen { rects, reads } = &mut self.layout_seen;
        rects.retain(|k, _| reads.iter().any(|r| r.key == *k));
        if !rects.contains_key(SELECTION) { rects.selection = Rc::default(); }
        measured
    }

    /// Whether layout feedback may still cause a commit (see the module docs).
    pub(super) fn layout_feedback_open(&self) -> bool {
        self.layout_echoes < ECHOES
    }

    /// Counts a frame's commit toward [`ECHOES`]: `feedback` when layout reads or `on_layout`
    /// caused it, `other` when anything else did.
    pub(super) fn count_layout_echo(&mut self, feedback: bool, other: bool) {
        if other || self.scrolling() {
            self.layout_echoes = 0;
        } else if feedback {
            self.layout_echoes += 1;
        }
    }

    /// A hand, momentum, an instant scroll or a glide moves a viewport.
    pub(crate) fn scrolling(&self) -> bool {
        self.scrolls.values().any(|st| st.direct || st.moving())
    }

    /// Queues `on_layout` and `on_screen_layout` for elements whose laid-out rect changed (or
    /// that were just laid out).
    pub(super) fn report_layout(&mut self) {
        let quiet = !self.layout_feedback_open();
        self.report_rects(quiet, false);
        self.report_rects(quiet, true);
    }

    /// The host's window content moved to `origin`, its top-left in global screen points.
    /// Queues `on_screen_layout` for the elements that moved with it; true when it queued any,
    /// so the host draws a frame to deliver them. A window move is outside feedback, like a
    /// scroll, so it reopens layout feedback (see the module docs).
    pub(crate) fn set_window_origin(&mut self, origin: (f32, f32)) -> bool {
        if self.window_origin == origin { return false; }
        self.window_origin = origin;
        self.layout_echoes = 0;
        self.report_rects(false, true)
    }

    /// Queues `on_drawn_layout` for elements whose drawn rect moved since it last reported,
    /// once their own and their ancestors' springs rest. A move its laid-out rect explains is
    /// layout feedback and falls quiet with the rest of it; one it doesn't (a live value) always
    /// reports, as scrolling reopens feedback. True when it queued any.
    pub(super) fn report_drawn(&mut self) -> bool {
        if !self.has_drawn_layout && self.drawn_fired.is_empty() { return false; }
        let mut queued = false;
        let quiet = !self.layout_feedback_open();
        let last = std::mem::take(&mut self.drawn_fired);
        let mut fired = HashMap::with_capacity_and_hasher(last.len(), Default::default());
        for i in 0..self.nodes.len() {
            let n = &self.nodes[i];
            if n.handlers.get().is_none_or(|h| h.on_drawn_layout.is_none()) || self.subtree_exiting(i) { continue; }
            let Some(&drawn) = self.drawn.get(i) else { continue };
            let id = n.id;
            if self.rect_springing(i) {
                if let Some(old) = last.get(&id) { fired.insert(id, *old); }
                continue;
            }
            let laid = self.static_layout(i);
            let old = last.get(&id).copied();
            if !moved(old.map(|o| o.0), Some(drawn)) {
                fired.insert(id, old.unwrap_or((drawn, laid)));
                continue;
            }
            let external = old.is_some_and(|(_, l)| !moved(Some(l), Some(laid)));
            if quiet && !external {
                fired.insert(id, (drawn, laid));
                continue;
            }
            let path = n.path.clone();
            self.queue.retain(|a| !matches!(a, Action::DrawnLayout(p, ..) if *p == path));
            self.queue.push(Action::DrawnLayout(path, drawn, external));
            fired.insert(id, (drawn, laid));
            queued = true;
        }
        self.drawn_fired = fired;
        queued
    }

    /// Writes each element's drawn rect to its `El::drawn_rect_into` value, on every frame it
    /// moves (scrolls, glides and springs included), with no handler and no commit. True when
    /// one this window binds changed, so the next frame shows what follows it.
    pub(super) fn write_drawn_rects(&mut self) -> bool {
        if !self.has_drawn_layout { return false; }
        let mut wrote = false;
        for (i, n) in self.nodes.iter().enumerate() {
            let Some(live) = n.handlers.get().and_then(|h| h.drawn_rect.as_ref()) else { continue };
            if let Some(&drawn) = self.drawn.get(i) && !self.subtree_exiting(i) { wrote |= live.set(drawn); }
        }
        wrote && self.live_pending()
    }

    /// Whether a spring is moving node `i`'s drawn rect: its own, its anchor's, or an ancestor's.
    pub(super) fn rect_springing(&self, i: usize) -> bool {
        let store = self.scene.store();
        let n = &self.nodes[i];
        self.flights.iter().any(|f| f.lands(&n.path, n.key.as_deref()))
            || std::iter::successors(Some(i), |&p| step_up(&self.nodes, p)).any(|p| {
                let id = self.nodes[p].id;
                store.is_animating(id, prop::FRAME) || store.is_animating(id, prop::TRAVEL) || store.is_animating(id, prop::PRESENCE)
                    || store.is_animating(id, prop::OFFSET) || store.is_animating(id, prop::SCALE) || store.is_animating(id, P_PRESS_SCALE)
                    || store.is_animating(id, P_SCALE) || store.is_animating(id, prop::ROTATION)
                    || self.grows.get(&id).is_some_and(|g| g.moving())
                    || self.stacks.get(&id).is_some_and(|s| s.moving)
                    || self.scrolls.get(&id).is_some_and(|s| s.slack.height() > 0.0 || s.glide().is_some())
            })
    }

    /// Queues `on_layout`, or `on_screen_layout` when `screen`, for rects that moved; true when
    /// it queued any.
    fn report_rects(&mut self, quiet: bool, screen: bool) -> bool {
        let last = std::mem::take(if screen { &mut self.screen_fired } else { &mut self.layout_fired });
        let (ox, oy) = if screen { self.window_origin } else { (0.0, 0.0) };
        let mut fired = HashMap::with_capacity_and_hasher(last.len(), Default::default());
        let mut queued = false;
        for i in 0..self.nodes.len() {
            let n = &self.nodes[i];
            if n.handlers.get().is_none_or(|h| if screen { h.on_screen_layout.is_none() } else { h.on_layout.is_none() }) || self.subtree_exiting(i) { continue; }
            let id = n.id;
            let [x, y, w, h] = self.laid_out(&self.nodes, i);
            let r = [x + ox, y + oy, w, h];
            if !quiet && moved(last.get(&id).copied(), Some(r)) {
                let path = self.nodes[i].path.clone();
                queued = true;
                if screen {
                    // Several moves between frames (a window drag) deliver only the latest.
                    self.queue.retain(|a| !matches!(a, Action::ScreenLayout(p, _) if *p == path));
                    self.queue.push(Action::ScreenLayout(path, r));
                } else {
                    self.queue.push(Action::Layout(path, r));
                }
            } else if let Some(old) = last.get(&id).filter(|_| !quiet) {
                // Unchanged: keep the rect last reported, so drift below the threshold adds up.
                fired.insert(id, *old);
                continue;
            }
            fired.insert(id, r);
        }
        *(if screen { &mut self.screen_fired } else { &mut self.layout_fired }) = fired;
        queued
    }
}

#[cfg(test)]
mod tests {
    use crate::{Env, Runtime, col, el};

    #[derive(Default)]
    struct S { screen: Vec<[f32; 4]>, local: Vec<[f32; 4]> }

    fn view(_: &S, _: &mut crate::Cx<S>) -> crate::El {
        col().w_full().h_full().padding(10.0).child(el().key("card").w(100.0).h(40.0)
            .on_layout(|s: &mut S, r| s.local.push(r))
            .on_screen_layout(|s: &mut S, r| s.screen.push(r)))
    }

    fn frames(rt: &mut Runtime<S>, n: usize) {
        for _ in 0..n { rt.frame(1.0 / 120.0); }
    }

    #[test]
    fn reads_of_one_key_count_once_per_function_and_once_for_the_whole_rect() {
        use super::{LayoutRead, Same, note_read, size};
        let mut reads: Vec<LayoutRead> = Vec::new();
        for _ in 0..3 { note_read(&mut reads, "a", Some(Same::new(size))); }
        assert!(matches!(reads[..], [LayoutRead { same: Some(Same::Static(..)), .. }]), "no allocation, one dependency");
        let top = 3.0;
        note_read(&mut reads, "a", Some(Same::new(move |r: [f32; 4]| r[1] < top)));
        assert!(matches!(reads[1].same, Some(Same::Shared(_))));
        note_read(&mut reads, "b", Some(Same::new(size)));
        note_read(&mut reads, "a", None);
        note_read(&mut reads, "a", Some(Same::new(size)));
        let kept: Vec<_> = reads.iter().map(|r| (r.key.as_str(), r.same.is_some())).collect();
        assert_eq!(kept, [("a", true), ("a", true), ("b", true), ("a", false)], "a whole-rect read keeps the value reads");
    }

    #[test]
    fn a_screen_rect_is_the_laid_out_rect_plus_the_window_origin() {
        let mut rt = Runtime::new(S::default(), view, (300.0, 200.0), Env::new(true, 2.0));
        assert!(!rt.set_window_origin((400.0, 250.0)), "nothing laid out yet");
        rt.frame(0.0);
        frames(&mut rt, 4);
        assert_eq!(rt.state.local, [[10.0, 10.0, 100.0, 40.0]]);
        assert_eq!(rt.state.screen, [[410.0, 260.0, 100.0, 40.0]], "first layout, origin added");
    }

    #[test]
    fn moving_the_window_reports_screen_rects_without_a_relayout() {
        let mut rt = Runtime::new(S::default(), view, (300.0, 200.0), Env::new(true, 2.0));
        rt.set_window_origin((400.0, 250.0));
        rt.frame(0.0);
        frames(&mut rt, 4);
        let commits = rt.stats.commits;

        // A move draws a frame that delivers the new rect; on_layout hears nothing.
        assert!(rt.set_window_origin((1700.0, -80.0)), "queued, so the host draws");
        frames(&mut rt, 2);
        assert_eq!(rt.state.screen.last(), Some(&[1710.0, -70.0, 100.0, 40.0]));
        assert_eq!(rt.state.local.len(), 1, "on_layout is window points: unmoved");

        // Below half a point it waits, and the drift adds up until it counts.
        assert!(!rt.set_window_origin((1700.3, -80.0)));
        assert!(rt.set_window_origin((1700.6, -80.0)));
        frames(&mut rt, 2);
        assert_eq!(rt.state.screen.len(), 3, "{:?}", rt.state.screen);
        assert!((rt.state.screen[2][0] - 1710.6).abs() < 1e-3, "{:?}", rt.state.screen);

        // A drag's moves between two frames deliver only the latest.
        rt.set_window_origin((1800.0, 0.0));
        rt.set_window_origin((1900.0, 0.0));
        frames(&mut rt, 2);
        assert_eq!(&rt.state.screen[3..], [[1910.0, 10.0, 100.0, 40.0]]);
        assert!(rt.stats.commits > commits, "handlers changed state");

        // Settled: nothing more runs.
        let (screen, commits) = (rt.state.screen.len(), rt.stats.commits);
        frames(&mut rt, 10);
        assert_eq!((rt.state.screen.len(), rt.stats.commits), (screen, commits));
    }

    #[test]
    fn a_window_move_with_no_screen_handler_asks_for_no_frame() {
        let mut rt = Runtime::new(S::default(), |_: &S, _: &mut crate::Cx<S>| {
            el().key("card").w(100.0).h(40.0).on_layout(|s: &mut S, r| s.local.push(r))
        }, (300.0, 200.0), Env::new(true, 2.0));
        rt.frame(0.0);
        frames(&mut rt, 2);
        assert!(!rt.set_window_origin((50.0, 50.0)));
        assert_eq!(rt.state.local.len(), 1);
    }
}
