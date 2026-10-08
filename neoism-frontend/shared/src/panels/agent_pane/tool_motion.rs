//! Bounded, session-scoped paint-only motion for live tool rows.
use std::{collections::HashMap, time::Duration};
use web_time::Instant;

const DURATION: Duration = Duration::from_millis(180);
const MAX_ENTRIES: usize = 256;

#[derive(Clone, Copy, Debug)]
pub struct ToolMotionSample {
    pub opacity: f32,
    pub offset_y: f32,
    pub outgoing_status: Option<&'static str>,
    pub status_progress: f32,
    pub deadline: Option<Instant>,
}
impl Default for ToolMotionSample {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            offset_y: 0.0,
            outgoing_status: None,
            status_progress: 1.0,
            deadline: None,
        }
    }
}
struct Entry {
    arrival: Option<Instant>,
    transition: Option<(Instant, &'static str)>,
    updated: Instant,
}
pub struct ToolMotionState {
    enabled: bool,
    session: Option<String>,
    entries: HashMap<String, Entry>,
    visible_until: Option<Instant>,
}
impl Default for ToolMotionState {
    fn default() -> Self {
        Self {
            enabled: true,
            session: None,
            entries: HashMap::new(),
            visible_until: None,
        }
    }
}
fn known_status(status: &str) -> Option<&'static str> {
    match status {
        "pending" => Some("pending"),
        "running" => Some("running"),
        "streaming" => Some("streaming"),
        "completed" => Some("completed"),
        "error" => Some("error"),
        "failed" => Some("failed"),
        "cancelled" => Some("cancelled"),
        _ => None,
    }
}
impl ToolMotionState {
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.clear();
        }
    }
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
    pub fn clear(&mut self) {
        self.entries.clear();
        self.visible_until = None;
    }
    /// Transfer recent identity without restarting clocks or replacing a canonical entry.
    /// Callers must scope to the active session before rekeying.
    pub fn rekey(&mut self, old_id: &str, new_id: &str) {
        if old_id.is_empty() || new_id.is_empty() || old_id == new_id {
            return;
        }
        if let Some(entry) = self.entries.remove(old_id) {
            self.entries.entry(new_id.to_owned()).or_insert(entry);
        }
    }

    pub fn scope(&mut self, session: Option<&str>) {
        if self.session.as_deref() != session {
            self.clear();
            self.session = session.map(str::to_owned);
        }
    }
    pub fn begin_frame(&mut self) {
        self.visible_until = None;
        self.prune(Instant::now());
    }
    fn prune(&mut self, now: Instant) {
        self.entries
            .retain(|_, e| now.saturating_duration_since(e.updated) < DURATION);
    }
    /// Bounded recent-ledger check; settled panes must not scan source groups.
    pub fn has_active_motion(&mut self) -> bool {
        if !self.enabled {
            return false;
        }
        let now = Instant::now();
        self.prune(now);
        self.entries.values().any(|entry| {
            entry
                .arrival
                .is_some_and(|at| now.saturating_duration_since(at) < DURATION)
                || entry
                    .transition
                    .is_some_and(|(at, _)| now.saturating_duration_since(at) < DURATION)
        })
    }

    pub fn is_animating_for(&self, session: Option<&str>) -> bool {
        self.enabled
            && self.session.as_deref() == session
            && self
                .visible_until
                .is_some_and(|until| until > Instant::now())
    }
    pub fn record(&mut self, id: &str, before_status: Option<&str>, after_status: &str) {
        if !self.enabled || id.is_empty() || before_status == Some(after_status) {
            return;
        }
        let outgoing = before_status
            .filter(|s| matches!(*s, "pending" | "running" | "streaming"))
            .filter(|_| {
                matches!(after_status, "completed" | "error" | "failed" | "cancelled")
            })
            .and_then(known_status);
        if before_status.is_some() && outgoing.is_none() {
            return;
        }
        let now = Instant::now();
        self.prune(now);
        if !self.entries.contains_key(id) && self.entries.len() >= MAX_ENTRIES {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.updated)
                .map(|(id, _)| id.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        let entry = self.entries.entry(id.to_owned()).or_insert(Entry {
            arrival: None,
            transition: None,
            updated: now,
        });
        if before_status.is_none() && entry.arrival.is_none() {
            entry.arrival = Some(now);
        }
        if let Some(status) = outgoing {
            entry.transition = Some((now, status));
        }
        entry.updated = now;
    }
    pub fn sample(&mut self, id: &str) -> ToolMotionSample {
        let id = id.split_once("::child::").map_or(id, |(_, child)| child);
        self.sample_group(id.split(".."))
    }

    /// Sample actual source members: only the first owns arrival, while the
    /// newest active completion anywhere in the group owns the status fade.
    pub fn sample_group<'a>(
        &mut self,
        members: impl IntoIterator<Item = &'a str>,
    ) -> ToolMotionSample {
        if !self.enabled {
            return ToolMotionSample::default();
        }
        let now = Instant::now();
        self.prune(now);
        let mut members = members.into_iter();
        let Some(first_id) = members.next() else {
            return ToolMotionSample::default();
        };
        let arrival = self.entries.get(first_id).and_then(|entry| entry.arrival);
        let transition = std::iter::once(first_id)
            .chain(members)
            .filter_map(|member| {
                self.entries.get(member).and_then(|entry| entry.transition)
            })
            .filter(|(at, _)| now.saturating_duration_since(*at) < DURATION)
            .max_by_key(|(at, _)| *at);
        let mut sample = ToolMotionSample::default();
        let progress = |at: Instant| {
            let t = (now.saturating_duration_since(at).as_secs_f32()
                / DURATION.as_secs_f32())
            .clamp(0.0, 1.0);
            1.0 - (1.0 - t).powi(3)
        };
        if let Some(at) =
            arrival.filter(|at| now.saturating_duration_since(*at) < DURATION)
        {
            sample.opacity = progress(at);
            sample.offset_y = 3.5 * (1.0 - sample.opacity);
            sample.deadline = Some(at + DURATION);
        }
        if let Some((at, status)) = transition {
            sample.outgoing_status = Some(status);
            sample.status_progress = progress(at);
            sample.deadline = Some(
                sample
                    .deadline
                    .map_or(at + DURATION, |d| d.max(at + DURATION)),
            );
        }
        sample
    }
    pub fn mark_visible(&mut self, sample: ToolMotionSample) {
        if self.enabled {
            if let Some(deadline) = sample.deadline.filter(|d| *d > Instant::now()) {
                self.visible_until =
                    Some(self.visible_until.map_or(deadline, |d| d.max(deadline)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn active_motion_fastpath_expires_without_visible_paint() {
        let mut state = ToolMotionState::default();
        assert!(!state.has_active_motion());
        state.record("row", None, "running");
        assert!(state.has_active_motion());
        let expired = Instant::now() - Duration::from_millis(240);
        let entry = state.entries.get_mut("row").unwrap();
        entry.arrival = Some(expired);
        entry.updated = expired;
        assert!(!state.has_active_motion());
        assert!(state.entries.is_empty());
        state.record("row", Some("running"), "completed");
        assert!(state.has_active_motion());
        state.set_enabled(false);
        assert!(!state.has_active_motion());
    }

    #[test]
    fn synthetic_group_uses_latest_completion_without_restarting_first_arrival() {
        let mut state = ToolMotionState::default();
        let now = Instant::now();
        let expired = now - Duration::from_millis(240);
        let recent = now - Duration::from_millis(40);
        state.entries.insert(
            "first".into(),
            Entry {
                arrival: Some(expired),
                transition: Some((expired, "running")),
                updated: expired,
            },
        );
        state.entries.insert(
            "second".into(),
            Entry {
                arrival: None,
                transition: Some((now - Duration::from_millis(100), "streaming")),
                updated: recent,
            },
        );
        state.entries.insert(
            "last".into(),
            Entry {
                arrival: Some(recent),
                transition: Some((recent, "running")),
                updated: recent,
            },
        );
        let group = state.sample_group(["first", "second", "third", "fourth", "last"]);
        assert_eq!(
            group.opacity, 1.0,
            "later member births cannot re-arrive a group"
        );
        assert_eq!(group.offset_y, 0.0);
        assert_eq!(group.outgoing_status, Some("running"));
        assert!(group.status_progress > 0.0 && group.status_progress < 1.0);
        assert_eq!(group.deadline, Some(recent + DURATION));
        assert!(!state.entries.contains_key("first"));
        assert!(state.entries.len() <= MAX_ENTRIES);
        let child = state.sample("first..second..last::child::last");
        assert!(child.opacity < 1.0 && child.offset_y > 0.0);
        assert_eq!(child.outgoing_status, Some("running"));
        assert_eq!(child.deadline, group.deadline);
        assert!(state
            .sample("first..second::child::missing")
            .deadline
            .is_none());
    }

    #[test]
    fn synthetic_group_arrival_keeps_first_members_original_deadline() {
        let mut state = ToolMotionState::default();
        state.record("first", None, "running");
        let first_at = state.entries["first"].arrival.unwrap();
        state.record("later", None, "running");
        let before = state.sample("first");
        let group = state.sample("first..later");
        assert_eq!(group.deadline, Some(first_at + DURATION));
        assert!(group.opacity >= before.opacity && group.opacity - before.opacity < 0.02);
        assert!(group.outgoing_status.is_none());
        assert_eq!(
            state.entries.len(),
            2,
            "sampling creates no synthetic ledger entry"
        );
    }

    #[test]
    fn rekey_retains_arrival_clock_and_destination() {
        let mut state = ToolMotionState::default();
        state.scope(Some("session"));
        state.record("legacy", None, "running");
        let started = Instant::now() - Duration::from_millis(60);
        state.entries.get_mut("legacy").unwrap().arrival = Some(started);
        let before = state.sample("legacy");
        state.rekey("legacy", "canonical");
        state.record("canonical", Some("running"), "completed");
        let after = state.sample("canonical");
        assert_eq!(state.entries["canonical"].arrival, Some(started));
        assert!(after.opacity >= before.opacity && after.opacity - before.opacity < 0.02);
        assert!(
            after.offset_y <= before.offset_y && before.offset_y - after.offset_y < 0.07
        );
        assert_eq!(after.outgoing_status, Some("running"));
        assert!(!state.entries.contains_key("legacy"));
        state.record("other", None, "running");
        let destination = state.entries["other"].arrival;
        state.rekey("canonical", "other");
        assert_eq!(state.entries["other"].arrival, destination);
        assert!(state.entries["other"].transition.is_none());
        state.rekey("other", "");
        assert!(state.entries.contains_key("other"));
        state.scope(Some("different"));
        state.rekey("other", "canonical");
        assert!(state.sample("canonical").deadline.is_none());
    }

    use super::*;
    #[test]
    fn arrival_completion_error_and_equal_snapshot() {
        let mut state = ToolMotionState::default();
        state.record("a", None, "running");
        let started = state.entries["a"].arrival;
        let sample = state.sample("a..b");
        assert!(sample.opacity < 1.0 && sample.offset_y > 0.0);
        state.record("a", Some("running"), "running");
        assert_eq!(started, state.entries["a"].arrival);
        state.record("a", Some("running"), "completed");
        assert_eq!(state.sample("a").outgoing_status, Some("running"));
        let completion_at = state.entries["a"].transition.unwrap().0;
        state.record("a", Some("completed"), "completed");
        assert_eq!(state.entries["a"].transition.unwrap().0, completion_at);
        state.record("b", Some("streaming"), "error");
        assert_eq!(state.sample("b").outgoing_status, Some("streaming"));
        state.record("c", Some("unknown"), "error");
        assert!(state.sample("c").deadline.is_none());
    }
    #[test]
    fn history_offscreen_scope_and_disable_are_sharp() {
        let mut state = ToolMotionState::default();
        assert!(state.sample("history").deadline.is_none());
        state.scope(Some("one"));
        state.record("a", None, "running");
        assert!(!state.is_animating_for(Some("one")));
        let sample = state.sample("a");
        state.mark_visible(sample);
        assert!(state.is_animating_for(Some("one")));
        state.begin_frame();
        assert!(!state.is_animating_for(Some("one")));
        let past = Instant::now() - Duration::from_secs(1);
        let entry = state.entries.get_mut("a").unwrap();
        entry.arrival = Some(past);
        entry.updated = past;
        assert!(state.sample("a").deadline.is_none());
        state.record("a", Some("running"), "running");
        assert!(state.sample("a").deadline.is_none());
        state.record("a", None, "running");
        state.scope(Some("two"));
        assert!(state.sample("a").deadline.is_none());
        state.record("a", None, "running");
        state.set_enabled(false);
        assert_eq!(state.sample("a").opacity, 1.0);
        state.set_enabled(true);
        assert!(state.sample("a").deadline.is_none());
    }
    #[test]
    fn entries_are_bounded_and_empty_ids_skip() {
        let mut state = ToolMotionState::default();
        state.record("", None, "running");
        for i in 0..MAX_ENTRIES + 30 {
            state.record(&i.to_string(), None, "running");
        }
        assert_eq!(state.entries.len(), MAX_ENTRIES);
        assert!(!state.entries.contains_key(""));
    }
}
