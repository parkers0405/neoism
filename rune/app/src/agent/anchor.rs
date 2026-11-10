//! Row-anchored scrolling of a transcript.
//!
//! The shared scroller (`ui/scroll.rs`) steps a wheel glide or a trackpad coast and says how
//! far the view should move this frame. Placing that as an absolute offset is wrong for a
//! transcript: older pages land above the reader and their rows are measured as they mount,
//! Rune moves the offset to keep the reader's row where it is, and an absolute offset placed
//! in the same frame is applied again after that correction (`Runtime::scroll_to_offsets`),
//! so the view jumped by the difference between a page's estimated and real height.
//!
//! A placement that holds a row instead (`Tx::scroll_key_to`) is worked out again from the
//! new layout. So each mounted row reports where it is drawn, one of them is the anchor, and
//! a step of `delta` points moves the anchor's top by `-delta`. Nothing is paused or stopped
//! when a page lands; the reader cannot tell that it did.

use crate::app::App;
use rune::Tx;
use std::time::{Duration, Instant};

/// How long after its last placement an anchor still says where its row is. Past it (the view
/// rested, or moved some other way) the rows' own reports are current and a fresh one is taken.
const FRESH: Duration = Duration::from_millis(100);
/// Older history is asked for this many viewports before the reader reaches its start, and
/// only while they are heading up (a wheel or trackpad delta, or a coast, this recently).
const AHEAD_VIEWPORTS: f32 = 3.0;
const HEADING_UP: Duration = Duration::from_millis(1500);

/// The row a moving transcript is held by: its top sits `at` points below the viewport's top.
pub struct Anchor {
    row: String,
    at: f32,
    used: Instant,
}

/// The last step placed by an anchor: the offset the view reported then, to tell a
/// placement that found no row (the view never moved) from one that did.
pub struct Placed {
    offset: f32,
    delta: f32,
}

impl super::Session {
    /// A mounted row reported where it is drawn: its top `at` points below the viewport's
    /// top, `h` points tall.
    pub fn row_at(&mut self, key: &str, at: f32, h: f32) {
        match self.row_tops.get_mut(key) {
            Some(place) => *place = (at, h),
            None => { self.row_tops.insert(key.to_owned(), (at, h)); }
        }
    }

    /// The row to hold, and where, for the view to end `delta` points further down the
    /// transcript. `None` when no row in view has reported yet.
    pub fn anchor(&mut self, delta: f32, now: Instant) -> Option<(String, f32)> {
        let view = self.viewport_h.max(1.0);
        let fresh = |a: &Anchor| now.duration_since(a.used) < FRESH;
        // Kept while it is surely mounted (rows stay built two viewports either side).
        let keep = self.anchor.as_ref().is_some_and(|a| fresh(a) && a.at.abs() <= 1.5 * view && self.row_tops.contains_key(&a.row));
        if !keep {
            let (row, top) = self.row_tops.iter()
                .filter(|(_, (at, h))| *at < view && at + h > 0.0)
                .min_by(|a, b| a.1.0.abs().total_cmp(&b.1.0.abs()))
                .map(|(row, (at, _))| (row.clone(), *at))?;
            // Mid-motion the reports trail the placements by a frame; the distance between
            // two rows in one report does not.
            let at = match self.anchor.as_ref().filter(|a| fresh(a)).and_then(|a| Some((a.at, self.row_tops.get(&a.row)?.0))) {
                Some((held, reported)) => held + (top - reported),
                None => top,
            };
            self.anchor = Some(Anchor { row, at, used: now });
        }
        let anchor = self.anchor.as_mut()?;
        anchor.at -= delta;
        anchor.used = now;
        Some((anchor.row.clone(), anchor.at))
    }

    /// Forget where rows were drawn (the view jumped: rows left without reporting).
    pub fn forget_rows(&mut self) {
        self.row_tops.clear();
        self.anchor = None;
        self.placed = None;
    }
}

impl App {
    /// Place this frame's steps of the transcripts the scroller moves (`Scrollers::take_moves`).
    pub fn place_conversations(&mut self, tx: &mut Tx) {
        let now = Instant::now();
        for step in self.scrollers.take_moves() {
            let Some(id) = step.key.strip_prefix("conversation:").map(str::to_owned) else {
                tx.scroll_to_instant(step.key, 0.0, step.to);
                continue;
            };
            let delta = step.to - step.from;
            let held = self.sessions.get_mut(&id).and_then(|session| {
                let offset = session.scroll.get().0;
                // The last anchor's row was gone (a jump unmounted it): the view never moved.
                if session.placed.take().is_some_and(|last| last.delta.abs() > 0.5 && (last.offset - offset).abs() < 0.01) {
                    if std::env::var_os("NEOISM_RUNE_TRACE_SCROLL").is_some() { eprintln!("[anchor] lost {:?}", session.anchor.as_ref().map(|a| (&a.row, a.at))); }
                    session.forget_rows();
                }
                if delta < 0.0 { session.scrolled_up_at = Some(now); }
                let held = session.anchor(delta, now)?;
                session.placed = Some(Placed { offset, delta });
                Some(held)
            });
            match held {
                Some((row, at)) => tx.scroll_key_to(row, at),
                None => tx.scroll_to_instant(step.key, 0.0, step.to),
            }
            if delta < 0.0 { self.request_older(&id); }
        }
    }

    /// Ask for the page of history above the loaded one, ahead of the reader: while they head
    /// up and are within `AHEAD_VIEWPORTS` of the start, one request at a time. The page lands
    /// above the view without moving it (see the module docs), so it is asked for early
    /// enough to be there before the reader is.
    pub fn request_older(&mut self, session_id: &str) {
        let Some(session) = self.sessions.get_mut(session_id) else { return };
        let Some(cursor) = session.older_cursor.clone() else { return };
        if session.loading_older { return; }
        // Parked near the start, nothing loads by itself.
        if !session.scrolled_up_at.is_some_and(|at| at.elapsed() < HEADING_UP) { return; }
        if session.scroll.get().0 > AHEAD_VIEWPORTS * session.viewport_h.max(1.0) { return; }
        session.loading_older = true;
        let limit = session.older_limit.max(8);
        self.send_agent(neoism_protocol::agent::AgentClientMessage::GetHistory { session_id: session_id.into(), cursor: Some(cursor), limit: Some(limit) });
    }
}

#[cfg(test)]
mod tests {
    use super::super::Session;
    use super::*;

    fn session() -> Session {
        let mut s = Session { viewport_h: 600.0, ..Session::default() };
        s.row_at("above", -500.0, 400.0);
        s.row_at("top", -100.0, 300.0);
        s.row_at("mid", 200.0, 300.0);
        s.row_at("below", 700.0, 300.0);
        s
    }

    #[test]
    fn a_step_holds_the_row_nearest_the_top_and_moves_it_by_the_step() {
        let mut s = session();
        let now = Instant::now();
        // 40 points up the transcript: the held row moves 40 points down the view.
        assert_eq!(s.anchor(-40.0, now), Some(("top".into(), -60.0)));
        // The next step carries on from the placement, not from the (stale) report.
        assert_eq!(s.anchor(-40.0, now), Some(("top".into(), -20.0)));
        assert_eq!(s.anchor(25.0, now), Some(("top".into(), -45.0)));
    }

    #[test]
    fn a_new_anchor_keeps_its_distance_from_the_old_one() {
        let mut s = session();
        let now = Instant::now();
        s.anchor(-40.0, now);
        // The held row has travelled past where rows stay mounted: hold another, placed by its
        // reported distance from the old one (300 below it), not by its stale report (200).
        s.anchor.as_mut().unwrap().at = 1000.0;
        let (row, at) = s.anchor(-10.0, now).unwrap();
        assert_eq!(row, "top", "the nearest row in view by the reports");
        let _ = at;
        s.anchor.as_mut().unwrap().row = "above".into();
        s.anchor.as_mut().unwrap().at = 1000.0;
        let (row, at) = s.anchor(0.0, now).unwrap();
        assert_eq!((row.as_str(), at), ("top", 1000.0 + 400.0), "`top` reports 400 below `above`");
    }

    #[test]
    fn a_rested_view_takes_its_anchor_from_the_reports_again() {
        let mut s = session();
        let start = Instant::now();
        s.anchor(-40.0, start);
        s.row_at("top", 50.0, 300.0);
        assert_eq!(s.anchor(-10.0, start + FRESH * 2), Some(("top".into(), 60.0)));
    }

    #[test]
    fn no_row_in_view_means_no_anchor() {
        let mut s = Session { viewport_h: 600.0, ..Session::default() };
        s.row_at("far", 4000.0, 300.0);
        assert_eq!(s.anchor(-40.0, Instant::now()), None);
    }
}
