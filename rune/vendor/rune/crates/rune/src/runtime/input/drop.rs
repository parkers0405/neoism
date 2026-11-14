//! Files dragged onto a window from another app: `El::on_drop_files`, `El::accepts_files`,
//! `El::on_drop_hover`, `El::on_drop_move`, `El::on_drop_rejected` and
//! `El::on_drop_rejected_hover`.
use super::*;
use crate::el::{DropHoverFn, PointerEvent};
use crate::el::drop::DropMoveFn;
use std::path::PathBuf;

/// One step of a file drag over the window, queued for the next frame.
pub(crate) enum FileEvent {
    /// The drag entered or moved. A position the platform can't give is `None`.
    Hover(Vec<PathBuf>, Option<(f32, f32)>),
    /// The drag moved over a target that follows it (`El::on_drop_move`); it runs as a hover
    /// at the drag's latest point.
    Move,
    Drop(Vec<PathBuf>, Option<(f32, f32)>),
    Leave,
}

#[derive(Default)]
pub(crate) struct FileDrag {
    /// A drag is over the window (set when the step is queued, before it runs).
    active: bool,
    /// The files the drag carries, for `drag_files_to`.
    paths: Vec<PathBuf>,
    /// The target of the last hover queued or run, so a move that keeps it queues nothing.
    aimed: Option<Aim>,
    /// Where the drag is now, in window points, and whether a `FileEvent::Move` waits to run.
    at: Option<(f32, f32)>,
    move_queued: bool,
    /// The drop target notified with `on_drop_hover(true)` (or, rejecting the drag,
    /// `on_drop_rejected_hover(true)`), and that callback, which still hears `false` after the
    /// element unmounts or starts to exit.
    hovered: Option<(Aim, Option<DropHoverFn>)>,
    /// This frame's drop targets: (node, visible rect in points, `hits.len()` before it). Kept
    /// apart from `hits`, so a drop zone doesn't cover the context menus and help under it.
    hits: Vec<(NodeId, [f32; 4], usize)>,
    /// File steps waiting for this frame's build (`hold_files`), and whether those leading the
    /// queue already waited one.
    held: Vec<Queued>,
    waited: bool,
}

/// A drop target's path, and whether it takes any of the dragged files (else it stops the drag
/// to reject it).
type Aim = (String, bool);

/// Where a drag or drop lands: the target, the files it takes and the rest.
struct Target {
    i: usize,
    files: Vec<PathBuf>,
    rejected: Vec<PathBuf>,
}

impl<S: 'static> Runtime<S> {
    /// A file drag from another app entered the window or moved over it, carrying `paths`; `at`
    /// is its position in points, or `None` when the platform doesn't say. The innermost
    /// [`El::on_drop_files`] under it that accepts the files hears `on_drop_hover(true)` next
    /// frame.
    pub fn drag_files(&mut self, paths: Vec<PathBuf>, at: Option<(f32, f32)>) {
        self.file_drag.active = true;
        self.file_drag.at = at;
        // A move a lost frame never ran mustn't keep this drag's moves from queueing.
        self.file_drag.move_queued = false;
        self.file_drag.aimed = self.drop_aim(&paths, at);
        self.file_drag.paths.clone_from(&paths);
        self.queue.push(Action::Files(FileEvent::Hover(paths, at)));
    }

    /// The file drag is at `at`, with the files it entered with. Queues a hover, and returns
    /// true, only when that changes the drop target or moves the drag over a target that
    /// follows it (`El::on_drop_move`, once a frame), so a host can sample the pointer often and
    /// redraw only when something listens. Sampling while still also notices a target that
    /// unmounted.
    pub fn drag_files_to(&mut self, at: Option<(f32, f32)>) -> bool {
        if !self.file_drag.active { return false; }
        let moved = std::mem::replace(&mut self.file_drag.at, at) != at;
        let target = self.drop_aim(&self.file_drag.paths, at);
        if target == self.file_drag.aimed {
            let follows = target.as_ref().is_some_and(|aim| self.drop_move_fn(aim).is_some());
            if !moved || !follows || std::mem::replace(&mut self.file_drag.move_queued, true) { return false; }
            self.queue.push(Action::Files(FileEvent::Move));
            return true;
        }
        self.file_drag.aimed = target;
        let paths = self.file_drag.paths.clone();
        self.queue.push(Action::Files(FileEvent::Hover(paths, at)));
        true
    }

    /// The file drag left the window, was cancelled, or the window hid.
    pub fn drag_files_leave(&mut self) {
        if !std::mem::take(&mut self.file_drag.active) { return; }
        self.file_drag.paths.clear();
        self.file_drag.aimed = None;
        self.file_drag.at = None;
        self.queue.push(Action::Files(FileEvent::Leave));
    }

    /// `paths` dropped at `at` (points, or `None` when the platform doesn't say). Next frame the
    /// innermost [`El::on_drop_files`] under it that accepts any of them gets those it accepts,
    /// in one call, and the drop hover ends. Its [`El::on_drop_rejected`] gets the rest, and a
    /// target with one that takes none of them gets them all there. A drop nothing takes is
    /// ignored.
    pub fn drop_files(&mut self, paths: Vec<PathBuf>, at: Option<(f32, f32)>) {
        self.file_drag.active = false;
        self.file_drag.paths.clear();
        self.file_drag.aimed = None;
        self.file_drag.at = None;
        self.queue.push(Action::Files(FileEvent::Drop(paths, at)));
    }

    /// A file drag is over the window. Hosts whose platform reports no drag positions sample
    /// the pointer while this holds.
    pub fn dragging_files(&self) -> bool { self.file_drag.active }

    /// The key (else path) of the element showing the drop hover, for tests and tools.
    pub fn drop_hovered(&self) -> Option<String> { self.hover_key(true) }

    /// The key (else path) of the element showing the rejected drop hover
    /// ([`El::on_drop_rejected_hover`]), for tests and tools.
    pub fn drop_rejected_hovered(&self) -> Option<String> { self.hover_key(false) }

    /// Drops the drag's target and the hover callback it would hear `false` on, unheard: their
    /// elements went with the state they were built for.
    pub(super) fn forget_drop_targets(&mut self) {
        self.file_drag.aimed = None;
        self.file_drag.hovered = None;
    }

    fn hover_key(&self, accepted: bool) -> Option<String> {
        let ((path, _), _) = self.file_drag.hovered.as_ref().filter(|((_, a), _)| *a == accepted)?;
        Some(self.node(path).and_then(|n| n.key.clone()).unwrap_or_else(|| path.clone()))
    }

    /// A file step that hit-tests (a hover or drop) reaching a tree changed since its last build
    /// (`stale`: state changed this frame, or while the window was covered and built nothing)
    /// waits for this frame's build, once, so it lands on what is mounted and laid out now. The
    /// file steps after it wait too, to keep their order.
    pub(super) fn hold_files(&mut self, queue: ActionQueue, stale: bool) -> ActionQueue {
        let waited = std::mem::take(&mut self.file_drag.waited);
        let aims = |a: &Action| matches!(a, Action::Files(FileEvent::Hover(..) | FileEvent::Move | FileEvent::Drop(..)));
        if waited || !stale || !queue.iter().any(aims) { return queue; }
        let (files, rest): (_, Vec<_>) = queue.into_iter().partition(|q| matches!(q.action, Action::Files(_)));
        self.file_drag.held = files;
        rest.into_iter().collect()
    }

    /// The held file steps go first next frame.
    pub(super) fn release_files(&mut self) {
        if self.file_drag.held.is_empty() { return; }
        self.file_drag.waited = true;
        self.queue.prepend(std::mem::take(&mut self.file_drag.held));
    }

    /// File steps wait for the next frame (`hold_files`).
    pub(super) fn files_held(&self) -> bool { self.file_drag.waited }

    pub(super) fn clear_drop_hits(&mut self) { self.file_drag.hits.clear(); }

    /// During `build`: drop target `i` draws visible over `rect`, after the first `seq` hits.
    pub(super) fn push_drop_hit(&mut self, i: usize, rect: [f32; 4], seq: usize) {
        if rect[2] > rect[0] && rect[3] > rect[1] {
            self.file_drag.hits.push((self.nodes[i].id, rect, seq));
        }
    }

    /// The target for `paths` at `at` and the files it takes: walking out from the topmost
    /// element under the point, the first drop target that accepts any of them or rejects
    /// what it doesn't take (`El::on_drop_rejected`), then the root.
    /// While a modal is open its root stands in for the window's, and a point outside it
    /// reaches nothing, as a drop on the dimmed window behind a sheet does. A point outside the
    /// window reaches nothing; a drop without a position goes to the root.
    fn drop_target(&self, paths: &[PathBuf], at: Option<(f32, f32)>) -> Option<Target> {
        let ok = |id: &NodeId| self.node_index.get(id).copied()
            .filter(|&i| i < self.nodes.len() && !self.subtree_exiting(i) && self.allowed(i));
        let start = match at {
            None => None,
            Some((x, y)) if !self.in_viewport(x, y) => return None,
            Some((x, y)) => {
                let hit = self.hits.iter().enumerate().rev()
                    .find_map(|(k, &(id, r))| self.hit_contains(id, r, x, y).then(|| ok(&id)).flatten().map(|i| (k, i)));
                let zone = self.file_drag.hits.iter().rev()
                    .find_map(|&(id, r, seq)| self.hit_contains(id, r, x, y).then(|| ok(&id)).flatten().map(|i| (seq, i)));
                // Whichever was drawn last is on top: a hit drawn after a zone started covers it.
                let top = match (hit, zone) {
                    (Some((k, h)), Some((seq, z))) => Some(if k >= seq { h } else { z }),
                    (h, z) => h.or(z).map(|(_, i)| i),
                };
                if top.is_none() && self.active_modal().is_some() { return None; }
                top
            }
        };
        let root = self.active_modal().unwrap_or_else(|| self.app_root());
        std::iter::successors(start, |&i| self.nodes[i].parent)
            .chain(std::iter::once(root).filter(|&r| r < self.nodes.len()))
            .filter(|&i| self.allowed(i) && !self.subtree_exiting(i))
            .find_map(|i| {
                let drop = self.nodes[i].handlers.drop.as_ref()?;
                let (files, rejected) = drop.split(paths);
                (!files.is_empty() || (!rejected.is_empty() && drop.claims_rejects())).then_some(Target { i, files, rejected })
            })
    }

    fn drop_aim(&self, paths: &[PathBuf], at: Option<(f32, f32)>) -> Option<Aim> {
        self.drop_target(paths, at).map(|t| (self.nodes[t.i].path.clone(), !t.files.is_empty()))
    }

    fn drop_hover_fn(&self, (path, accepted): &Aim) -> Option<DropHoverFn> {
        self.node(path).and_then(|n| n.handlers.drop.as_ref()?.hover_fn(*accepted))
    }

    /// The `El::on_drop_move` of a target that takes the drag.
    fn drop_move_fn(&self, (path, accepted): &Aim) -> Option<DropMoveFn> {
        self.node(path).filter(|_| *accepted).and_then(|n| n.handlers.drop.as_ref()?.on_move.clone())
    }

    /// `at` on the mounted element `path`, for `Tx::drop_point`.
    fn drop_point(&self, path: &str, at: Option<(f32, f32)>) -> Option<PointerEvent> {
        let (x, y) = at?;
        let i = self.live_index(path)?;
        let [lx, ly] = self.to_local(i, x, y);
        Some(PointerEvent { x: lx, y: ly, window_x: x, window_y: y })
    }

    /// A hover at `at`: moves the drop hover to the target there, then tells a target that
    /// follows the drag where it is.
    fn hover_files(&mut self, paths: &[PathBuf], at: Option<(f32, f32)>, tx: &mut Tx) -> bool {
        let target = self.drop_aim(paths, at);
        if self.file_drag.active { self.file_drag.aimed.clone_from(&target); }
        let mut ran = self.set_drop_hover(target.clone(), at, tx);
        if let Some(aim) = target && let Some(f) = self.drop_move_fn(&aim) && let Some(p) = self.drop_point(&aim.0, at) {
            self.trace_event(format_args!("drop-move:{}", aim.0));
            tx.drop_point = Some(p);
            f(states!(self), p, tx);
            tx.drop_point = None;
            ran = true;
        }
        ran
    }

    /// Moves the drop hover to `target`, notifying the old and new elements.
    fn set_drop_hover(&mut self, target: Option<Aim>, at: Option<(f32, f32)>, tx: &mut Tx) -> bool {
        if self.file_drag.hovered.as_ref().map(|(aim, _)| aim) == target.as_ref() { return false; }
        let old = self.file_drag.hovered.take();
        let new = target.map(|aim| {
            let f = self.drop_hover_fn(&aim);
            (aim, f)
        });
        self.file_drag.hovered.clone_from(&new);
        let mut ran = false;
        for (hover, over) in [(old, false), (new, true)] {
            let Some((aim, kept)) = hover else { continue };
            // The element's current callback, else the one it had if it has gone.
            let Some(f) = self.drop_hover_fn(&aim).or(kept) else { continue };
            let (path, accepted) = &aim;
            let kind = if *accepted { "drop-hover" } else { "drop-rejected-hover" };
            self.trace_event(format_args!("{kind}:{over}:{path}"));
            tx.drop_point = if over { self.drop_point(path, at) } else { None };
            f(states!(self), over, tx);
            tx.drop_point = None;
            ran = true;
        }
        ran
    }

    pub(super) fn run_files(&mut self, event: FileEvent, tx: &mut Tx) -> bool {
        match event {
            FileEvent::Hover(paths, at) => self.hover_files(&paths, at, tx),
            FileEvent::Move => {
                self.file_drag.move_queued = false;
                if !self.file_drag.active { return false; }
                let paths = self.file_drag.paths.clone();
                self.hover_files(&paths, self.file_drag.at, tx)
            }
            FileEvent::Leave => self.set_drop_hover(None, None, tx),
            FileEvent::Drop(paths, at) => {
                let target = self.drop_target(&paths, at);
                let mut ran = self.set_drop_hover(None, None, tx);
                let Some(Target { i, files, rejected }) = target else { return ran };
                let Some(drop) = self.nodes[i].handlers.drop.as_deref().cloned() else { return ran };
                let path = self.nodes[i].path.clone();
                tx.drop_point = self.drop_point(&path, at);
                if !files.is_empty() && let Some(f) = drop.on_drop {
                    self.trace_event(format_args!("drop-files:{}:{path}", files.len()));
                    f(states!(self), &files, tx);
                    ran = true;
                }
                if !rejected.is_empty() && let Some(f) = drop.on_reject {
                    self.trace_event(format_args!("drop-rejected:{}:{path}", rejected.len()));
                    f(states!(self), &rejected, tx);
                    ran = true;
                }
                tx.drop_point = None;
                ran
            }
        }
    }
}
