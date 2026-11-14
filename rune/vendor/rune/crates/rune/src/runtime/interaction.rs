//! The runtime's side of interaction timing (`tooling::interaction`): naming the route and
//! control each input action reached, and finishing a timed frame once its host submits it.
use std::cmp::Reverse;
use std::sync::Arc;

use super::*;
use crate::tooling::interaction::{self, Finished, Note, Slow};

/// A keyed ancestor covering at least this share of the window is a page.
const PAGE_AREA: f32 = 0.5;

impl<S: 'static> Runtime<S> {
    /// What `action` was, when timing is on and a person caused it; `None` for everything else
    /// (layout reports, timers, clipboard and dialog answers).
    pub(super) fn interaction_note(&mut self, action: &Action) -> Option<Note> {
        if !interaction::on() { return None; }
        let focused = self.focus.path();
        let editing = || focused.and_then(|p| self.index_of(p)).is_some_and(|i| self.nodes[i].input.is_some());
        let (name, rank, path): (&'static str, u8, Option<&str>) = match action {
            Action::Press(p, _) => ("on_press", 3, Some(p)),
            Action::DoublePress(p) => ("on_double_press", 3, Some(p)),
            Action::SecondaryPress(p, _) => ("on_secondary_press", 3, Some(p)),
            Action::Link(p, _) => ("on_link", 3, Some(p)),
            Action::LinkSecondary(p, ..) => ("on_link_secondary", 3, Some(p)),
            Action::Dismiss(p) => ("dismiss", 3, Some(p)),
            Action::Key { .. } | Action::Physical(..) => (if editing() { "key_edit" } else { "key" }, 3, focused),
            Action::Ime(_) => ("key_edit", 3, focused),
            Action::Paste(..) => ("paste", 3, focused),
            Action::Files(_) => ("on_drop_files", 3, None),
            Action::DragStart(p) => ("on_drag_start", 2, Some(p)),
            Action::Drag(p, ..) | Action::DragRepeat(p, ..) => ("on_drag", 2, Some(p)),
            Action::DragEnd(p, _) => ("on_drag_end", 2, Some(p)),
            Action::Wheel(_, chain, _) => ("on_wheel", 2, chain.first().map(String::as_str)),
            Action::Gesture(chain, _) => ("gesture", 2, chain.first().map(String::as_str)),
            Action::Hover(p) => ("on_hover", 1, Some(p)),
            Action::HoverChange(p, _) => ("on_hover_change", 1, Some(p)),
            Action::PointerMove(p, _) => ("on_pointer_move", 1, Some(p)),
            _ => return None,
        };
        let at = path.and_then(|p| self.index_of(p));
        let page = at.and_then(|i| self.page_of(i)).map(|i| self.nodes[i].path.as_str());
        let modal = self.active_modal().map(|i| self.nodes[i].path.as_str());
        let names = &mut self.watch.interaction.names;
        let control = match path {
            Some(p) => names.control(p),
            None => names.fixed("window"),
        };
        let route = names.route(page, modal);
        let note = Note { route, control, action: name, rank };
        if self.watch.interaction.breached(&note) { interaction::hurry(); }
        Some(note)
    }

    fn index_of(&self, path: &str) -> Option<usize> {
        self.path_ids.get(path).and_then(|id| self.node_index.get(id)).copied().filter(|&i| self.nodes.get(i).is_some_and(|n| n.path == path))
    }

    /// The innermost keyed element around node `i`, `i` included, that covers a page's share
    /// of the window.
    fn page_of(&self, mut i: usize) -> Option<usize> {
        let area = (self.viewport.0 * self.viewport.1 * PAGE_AREA).max(1.0);
        loop {
            let n = &self.nodes[i];
            let [x0, y0, x1, y1] = n.target;
            if n.key.is_some() && (x1 - x0) * (y1 - y0) >= area { return Some(i); }
            i = n.parent?;
        }
    }

    /// A frame starts.
    pub(super) fn interaction_frame_begins(&mut self) {
        if !interaction::on() { return; }
        let queued = !self.queue.is_empty();
        if let Some(finished) = self.watch.interaction.frame_begins(queued) { self.interaction_finished(finished); }
    }

    /// An action that `interaction_note` named ran its handlers from `started`.
    pub(super) fn interaction_ran(&mut self, note: Note, started: crate::time::Instant) {
        let handler = started.elapsed();
        self.watch.interaction.noted(note, crate::input::time::current(), started, handler);
    }

    /// A commit in a timed frame reused `spans` of nodes from `memo`s.
    pub(super) fn interaction_reused(&mut self, spans: &[super::memo_reuse::ReusedSpan]) {
        let reused = &mut self.watch.interaction.reused;
        reused.clear();
        reused.extend(spans.iter().map(|s| (s.new_start, s.len)));
    }

    /// The frame is built, ready for its host to draw.
    pub(super) fn interaction_built(&mut self) {
        if !interaction::on() { return; }
        if let Some(frame) = self.watch.interaction.built(self.stats.shaped, self.nodes.len()) { interaction::send_frame(frame); }
    }

    /// The host submitted the frame's commands at `submitted`, to show at `presents_at` when a
    /// display link paces it.
    pub(crate) fn interaction_presented(&mut self, submitted: crate::time::Instant, presents_at: Option<crate::time::Instant>) {
        if !interaction::on() { return; }
        if let Some(finished) = self.watch.interaction.finish(submitted, presents_at) { self.interaction_finished(finished); }
    }

    /// The process is about to exit: sends what's counted and waits briefly for it to go.
    pub(crate) fn interaction_flush(&mut self) {
        if !interaction::on() { return; }
        if let Some(finished) = self.watch.interaction.frame_begins(false) { self.interaction_finished(finished); }
        self.watch.interaction.roll(true);
        interaction::flush();
    }

    #[cfg(test)]
    pub(crate) fn interaction_tracker(&mut self) -> &mut interaction::Tracker { &mut self.watch.interaction }

    fn interaction_finished(&mut self, f: Finished) {
        if f.slow {
            let rebuilt = self.rebuilt_subtrees();
            let reused = self.watch.interaction.reused.iter().map(|r| r.1).sum();
            let Finished { timed, built, phases, shaped, submitted, present, cause, .. } = f;
            interaction::send(Slow {
                route: timed.note.route.text, control: timed.note.control.text, action: timed.note.action,
                arrived: timed.arrived, frame_start: timed.frame_start, built, submitted, present,
                handler_ms: timed.handler_ms, phases, why: timed.why, commits: timed.commits, shaped,
                nodes: self.nodes.len(), reused, rebuilt,
                cpu_ms: timed.cpu.1, cause: cause.unwrap_or(interaction::Cause::Queued), stacks: timed.stacks,
            });
        }
        self.watch.interaction.reused.clear();
        self.watch.interaction.roll(false);
    }

    /// The keyed subtrees with the most nodes the last commit rebuilt rather than reused from a
    /// `memo`, largest first; each the most specific keyed element holding nearly all of its
    /// count. Runs only for a slow interaction.
    fn rebuilt_subtrees(&mut self) -> Vec<(Arc<str>, usize)> {
        let n = self.nodes.len();
        let mut reused = vec![false; n];
        for &(start, len) in &self.watch.interaction.reused {
            for r in reused.iter_mut().skip(start).take(len) { *r = true; }
        }
        let mut count = vec![0usize; n];
        for i in (0..n).filter(|&i| !reused[i]) {
            let mut at = Some(i);
            while let Some(j) = at {
                if self.nodes[j].key.is_some() { count[j] += 1; }
                at = self.nodes[j].parent;
            }
        }
        // The largest count among each keyed element's nearest keyed descendants.
        let mut inner = vec![0usize; n];
        for i in (0..n).filter(|&i| count[i] > 0 && self.nodes[i].key.is_some()) {
            let mut at = self.nodes[i].parent;
            while let Some(j) = at {
                if self.nodes[j].key.is_some() { inner[j] = inner[j].max(count[i]); break; }
                at = self.nodes[j].parent;
            }
        }
        let mut top: Vec<usize> = (0..n).filter(|&i| count[i] > 0 && self.nodes[i].key.is_some() && (inner[i] as f64) < 0.9 * count[i] as f64).collect();
        top.sort_by_key(|&i| Reverse(count[i]));
        top.truncate(3);
        top.into_iter().map(|i| (self.watch.interaction.names.control(&self.nodes[i].path).text, count[i])).collect()
    }
}
