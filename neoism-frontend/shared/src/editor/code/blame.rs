//! Host-neutral blame state. Hosts pump requests outside paint and return
//! immutable HEAD snapshots. Rendering consumes only revision-matched rows.
use super::types::CodeBuffer;
use base64::{engine::general_purpose::STANDARD, Engine};
use neoism_protocol::git::{GitBlameSnapshot, GitServerMessage};
use web_time::{Duration, Instant};

#[derive(Clone, Debug, Default)]
pub struct CodeBlame {
    pub enabled: bool,
    /// Set by the host, so inactive split panes do not show inline annotations.
    pub focused: bool,
    pub hover: Option<(usize, u64, Instant)>,
    pub hit_rect: Option<[f32; 4]>,
    pub tooltip_rect: Option<[f32; 4]>,
    pub snapshot: Option<GitBlameSnapshot>,
    /// Wire base64 decoded by the host update pump, never in paint.
    pub avatars: Vec<Vec<u8>>,
    pub rows: Vec<Option<u32>>,
    pub revision: Option<u64>,
    mapping_complete: bool,
    pub error: Option<String>,
    configured: Option<bool>,
    scope: String,
    pending: Option<(u64, Instant)>,
    refreshed: Option<Instant>,
    cursor_activity: Option<(usize, usize, u64, u64)>,
    settle_since: Option<Instant>,
    viewport: Option<[f32; 3]>,
    pointer: Option<[f32; 2]>,
    hover_suppressed: bool,
    delay_ms: u64,
    hide_on_scroll: bool,
    scroll_since: Option<Instant>,
}

const SCROLL_SETTLE: Duration = Duration::from_millis(150);
impl CodeBlame {
    /// Hot-reloaded options; changing them never discards cached attribution.
    pub fn set_options(&mut self, delay_ms: u64, hide_on_scroll: bool) {
        self.delay_ms = delay_ms;
        self.hide_on_scroll = hide_on_scroll;
    }

    pub(super) fn note_scroll(&mut self) {
        self.scroll_since = Some(Instant::now());
        self.dismiss_hover();
    }

    /// Observe document/caret state, not physical keys. Runs in both the host
    /// request pump and paint, so no request or cached annotation slips through
    /// before the first frame following a motion/edit.
    pub fn observe_cursor(&mut self, buffer: &CodeBuffer) {
        self.observe_cursor_at(buffer, Instant::now());
    }

    fn observe_cursor_at(&mut self, buffer: &CodeBuffer, now: Instant) {
        let activity = (
            buffer.cursor_line,
            buffer.cursor_col,
            buffer.revision,
            buffer.cursor_placement_revision,
        );
        if self.cursor_activity != Some(activity) {
            self.cursor_activity = Some(activity);
            self.settle_since = Some(now);
            self.dismiss_hover();
            self.hit_rect = None;
        }
    }

    pub fn settling(&self) -> bool {
        self.settling_at(Instant::now())
    }

    fn settling_at(&self, now: Instant) -> bool {
        self.enabled
            && (self.settle_since.is_some_and(|since| {
                now.duration_since(since) < Duration::from_millis(self.delay_ms)
            }) || (self.hide_on_scroll
                && self
                    .scroll_since
                    .is_some_and(|since| now.duration_since(since) < SCROLL_SETTLE)))
    }

    pub(super) fn dismiss_hover(&mut self) {
        self.hover = None;
        self.tooltip_rect = None;
        // A stationary pointer must not reopen a dismissed card while the
        // document scrolls underneath it. Actual pointer motion re-arms hover.
        self.hover_suppressed = true;
    }

    /// Scrolling dismisses details, but does NOT restart the cursor timer.
    pub fn observe_viewport(&mut self, viewport: [f32; 3]) {
        if self.viewport.is_some_and(|previous| previous != viewport) {
            self.note_scroll();
        }
        self.viewport = Some(viewport);
    }

    pub fn observe_pointer(&mut self, pointer: Option<[f32; 2]>) {
        if self.pointer != pointer {
            self.hover_suppressed = false;
        }
        self.pointer = pointer;
    }

    pub fn hover_allowed(&self) -> bool {
        !self.hover_suppressed && !self.settling()
    }

    pub fn contains_pointer(&self, x: f32, y: f32) -> bool {
        self.enabled
            && self.focused
            && self
                .hit_rect
                .iter()
                .chain(self.tooltip_rect.iter())
                .any(|r| x >= r[0] && x < r[0] + r[2] && y >= r[1] && y < r[1] + r[3])
    }

    pub fn configure(&mut self, enabled: bool) {
        self.configured = Some(enabled);
        self.set_enabled(enabled);
    }
    pub fn apply_default(&mut self, enabled: bool) {
        if self.configured != Some(enabled) {
            self.configure(enabled);
        }
    }
    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.refreshed = None;
        if !enabled {
            self.pending = None;
            self.snapshot = None;
            self.avatars.clear();
            self.hover = None;
            self.hit_rect = None;
            self.tooltip_rect = None;
            self.rows.clear();
            self.revision = None;
            self.error = None;
        }
    }
    pub fn toggle(&mut self) {
        self.set_enabled(!self.enabled);
    }
    /// Scope includes endpoint/connection, workspace and exact host path.
    /// Poll HEAD even on clean buffers: commits/checkouts need no text edit.
    pub fn needs_request(&mut self, scope: String) -> bool {
        if self.scope != scope {
            let enabled = self.enabled;
            let configured = self.configured;
            *self = Self {
                enabled,
                configured,
                scope,
                cursor_activity: self.cursor_activity,
                settle_since: self.settle_since,
                delay_ms: self.delay_ms,
                hide_on_scroll: self.hide_on_scroll,
                scroll_since: self.scroll_since,
                viewport: self.viewport,
                pointer: self.pointer,
                hover_suppressed: true,
                ..Self::default()
            };
        }
        if !self.enabled || self.settling() {
            return false;
        }
        if self
            .pending
            .is_some_and(|(_, since)| since.elapsed() < Duration::from_secs(45))
        {
            return false;
        }
        if self.pending.is_some() {
            self.error = Some("Blame request timed out".into());
        }
        self.pending = None;
        self.refreshed
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(15))
    }
    pub fn requested(&mut self, id: u64) {
        self.pending = Some((id, Instant::now()));
    }
    pub fn accept(&mut self, id: u64, reply: &GitServerMessage) -> bool {
        if !self.pending.is_some_and(|(pending, _)| pending == id)
            || !matches!(
                reply,
                GitServerMessage::Blame { .. } | GitServerMessage::Error { .. }
            )
        {
            return false;
        }
        self.pending = None;
        self.avatars.clear();
        self.refreshed = Some(Instant::now());
        match reply {
            GitServerMessage::Blame { snapshot } => {
                // Bound even an untrusted remote peer before allocating mappings/images.
                if snapshot.baseline.len() > 20_000
                    || snapshot.baseline.iter().map(String::len).sum::<usize>()
                        > 512 * 1024
                    || snapshot.lines.len() != snapshot.baseline.len()
                    || snapshot.commits.len() > 4096
                    || snapshot.avatars.len() > 32
                    || snapshot.avatars.iter().any(|a| a.len() != 5464)
                    || snapshot
                        .lines
                        .iter()
                        .flatten()
                        .any(|&i| i as usize >= snapshot.commits.len())
                    || snapshot.commits.iter().any(|c| {
                        c.sha.len() != 40
                            || !c.sha.bytes().all(|b| b.is_ascii_hexdigit())
                            || c.author.len() > 512
                            || c.email.len() > 1024
                            || c.summary.len() > 2048
                            || c.avatar
                                .is_some_and(|i| i as usize >= snapshot.avatars.len())
                    })
                    || snapshot
                        .commits
                        .iter()
                        .map(|c| {
                            c.sha.len() + c.author.len() + c.email.len() + c.summary.len()
                        })
                        .sum::<usize>()
                        > 1024 * 1024
                {
                    self.snapshot = None;
                    self.error = Some("Invalid blame snapshot".into());
                } else {
                    self.avatars = snapshot
                        .avatars
                        .iter()
                        .map(|data| {
                            STANDARD
                                .decode(data)
                                .ok()
                                .filter(|bytes| bytes.len() == 4096)
                                .unwrap_or_default()
                        })
                        .collect();
                    self.snapshot = Some(snapshot.clone());
                    self.error = None;
                }
            }
            GitServerMessage::Error { message } => {
                self.snapshot = None;
                self.error = Some(message.clone());
            }
            _ => return false,
        }
        self.revision = None;
        self.rows.clear();
        true
    }
    /// Called by the host update pump, not the renderer. Conservative fallback
    /// for large dirty buffers; never present guessed identities.
    pub fn update(&mut self, buffer: &CodeBuffer) {
        self.observe_cursor(buffer);
        if !self.enabled || self.revision == Some(buffer.revision) {
            return;
        }
        self.rows.clear();
        self.mapping_complete = false;
        if let Some(snapshot) = &self.snapshot {
            if buffer.lines.len() <= 20_000
                && buffer.lines.iter().map(String::len).sum::<usize>() <= 512 * 1024
            {
                let (mapping, complete) = super::gitdiff::unchanged_line_map_checked(
                    &snapshot.baseline,
                    &buffer.lines,
                );
                self.mapping_complete = complete;
                self.rows = mapping
                    .into_iter()
                    .map(|old| {
                        old.and_then(|line| snapshot.lines.get(line).copied().flatten())
                    })
                    .collect();
            }
        }
        self.revision = Some(buffer.revision);
    }
    pub fn commit(
        &self,
        line: usize,
        revision: u64,
    ) -> Option<&neoism_protocol::git::GitBlameCommit> {
        if self.revision != Some(revision) {
            return None;
        }
        let index = self.rows.get(line).copied().flatten()?;
        self.snapshot.as_ref()?.commits.get(index as usize)
    }
    pub fn empty_label(&self, revision: u64) -> &str {
        if self.error.is_some() {
            "Blame unavailable"
        } else if self.snapshot.is_none() || self.revision != Some(revision) {
            "Loading blame…"
        } else if !self.mapping_complete {
            "Attribution unavailable"
        } else {
            "Uncommitted"
        }
    }
}

pub fn relative_date(timestamp: i64, now: i64) -> String {
    let seconds = now.saturating_sub(timestamp).max(0);
    for (unit, label) in [
        (365 * 86400, "year"),
        (30 * 86400, "month"),
        (86400, "day"),
        (3600, "hour"),
        (60, "minute"),
    ] {
        if seconds >= unit {
            let count = seconds / unit;
            return format!("{count} {label}{} ago", if count == 1 { "" } else { "s" });
        }
    }
    "just now".into()
}

/// UTC date for the details popover. Pure Gregorian conversion: safe on wasm,
/// independent of the host timezone and never calls platform clock APIs.
pub fn absolute_date(timestamp: i64) -> String {
    let timestamp = timestamp.clamp(-62_167_219_200, 253_402_300_799);
    let z = timestamp.div_euclid(86400) + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);
    let minute = timestamp.rem_euclid(86400) / 60;
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        minute / 60,
        minute % 60
    )
}

#[cfg(test)]
mod tests {
    use super::super::gitdiff::unchanged_line_map;
    fn lines(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }
    #[test]
    fn insert_edit_delete_and_repeated_lines() {
        assert_eq!(
            unchanged_line_map(&lines(&["a", "b", "c"]), &lines(&["new", "a", "B", "c"])),
            vec![None, Some(0), None, Some(2)]
        );
        assert_eq!(
            unchanged_line_map(&lines(&["x", "a", "x", "b"]), &lines(&["x", "x", "b"])),
            vec![Some(0), Some(2), Some(3)]
        );
        assert_eq!(unchanged_line_map(&[], &lines(&["new"])), vec![None]);
    }
    #[test]
    fn settle_tracks_position_edits_and_repeated_placements_without_keys() {
        let mut state = super::CodeBlame::default();
        state.set_options(250, false);
        state.configure(true);
        let mut buffer = super::CodeBuffer::from_text("abc\ndef");
        let start = super::Instant::now();
        state.observe_cursor_at(&buffer, start);
        assert!(state.settling_at(start + super::Duration::from_millis(249)));
        assert!(!state.settling_at(start + super::Duration::from_millis(250)));
        // An idle render/update must not reset the deadline.
        state.observe_cursor_at(&buffer, start + super::Duration::from_millis(200));
        assert!(!state.settling_at(start + super::Duration::from_millis(250)));
        buffer.cursor_col = 1; // arrows, vim and programmatic motions converge here
        state.observe_cursor_at(&buffer, start + super::Duration::from_millis(210));
        assert!(state.settling_at(start + super::Duration::from_millis(459)));
        assert!(!state.settling_at(start + super::Duration::from_millis(460)));
        buffer.set_cursor_position(0, 1, false); // repeat click on the SAME cell
        state.observe_cursor_at(&buffer, start + super::Duration::from_millis(470));
        assert!(state.settling_at(start + super::Duration::from_millis(600)));
        buffer.revision += 1; // typing/undo/remote edit, even with unchanged caret
        state.observe_cursor_at(&buffer, start + super::Duration::from_millis(610));
        assert!(!state.settling_at(start + super::Duration::from_millis(860)));
        state.configure(false);
        assert!(!state.settling_at(start + super::Duration::from_millis(611)));
    }

    #[test]
    fn scrolling_dismisses_details_not_annotation_and_requires_new_hover() {
        let mut state = super::CodeBlame::default();
        state.configure(true);
        let buffer = super::CodeBuffer::from_text("abc");
        let now = super::Instant::now();
        state.observe_cursor_at(&buffer, now - super::Duration::from_secs(1));
        state.observe_pointer(Some([100.0, 100.0]));
        state.observe_viewport([0.0, 0.0, 0.0]);
        state.hover = Some((0, 0, now));
        state.tooltip_rect = Some([100.0, 100.0, 100.0, 100.0]);
        state.observe_viewport([10.0, 0.0, 40.0]);
        assert!(!state.settling_at(now));
        assert!(state.hover.is_none());
        assert!(state.tooltip_rect.is_none());
        state.observe_pointer(Some([100.0, 100.0]));
        assert!(!state.hover_allowed());
        state.observe_pointer(Some([101.0, 100.0]));
        assert!(state.hover_allowed());
    }

    #[test]
    fn requests_wait_for_settle_across_scope_reset_and_reuse_images() {
        let mut state = super::CodeBlame::default();
        state.set_options(250, false);
        state.configure(true);
        let mut buffer = super::CodeBuffer::from_text("abc");
        state.observe_cursor(&buffer);
        assert!(!state.needs_request("host/file".into()));
        state.settle_since =
            Some(super::Instant::now() - super::Duration::from_millis(250));
        assert!(state.needs_request("host/file".into()));
        state.avatars = vec![vec![1, 2, 3]];
        buffer.cursor_col = 1;
        state.observe_cursor(&buffer);
        assert!(!state.needs_request("host/file".into()));
        assert_eq!(state.avatars, vec![vec![1, 2, 3]]);
    }

    #[test]
    fn immediate_defaults_and_independent_scroll_hiding_hot_reload() {
        let mut state = super::CodeBlame::default();
        state.configure(true);
        let buffer = super::CodeBuffer::from_text("abc");
        let now = super::Instant::now();
        state.observe_cursor_at(&buffer, now);
        state.scroll_since = Some(now);
        assert!(!state.settling_at(now)); // old immediate behavior by default
        state.set_options(0, true);
        assert!(state.settling_at(now + super::Duration::from_millis(149)));
        assert!(!state.settling_at(now + super::Duration::from_millis(150)));
        state.set_options(250, true);
        assert!(state.settling_at(now + super::Duration::from_millis(200)));
        assert!(!state.settling_at(now + super::Duration::from_millis(250)));
        state.set_options(0, false); // live config change restores immediately
        assert!(!state.settling_at(now));
    }

    #[test]
    fn scroll_requests_are_gated_across_scope_reset_without_losing_cache() {
        let mut state = super::CodeBlame::default();
        state.configure(true);
        state.set_options(0, true);
        state.note_scroll();
        assert!(!state.needs_request("host/file".into()));
        assert_eq!(state.delay_ms, 0);
        assert!(state.hide_on_scroll);
        state.avatars = vec![vec![1, 2, 3]];
        state.set_options(0, false);
        assert!(state.needs_request("host/file".into()));
        assert_eq!(state.avatars, vec![vec![1, 2, 3]]);
        state.set_options(0, true);
        state.scroll_since = Some(super::Instant::now() - super::SCROLL_SETTLE);
        assert!(state.needs_request("host/file".into()));
    }

    #[test]
    fn disable_invalidates_pending_and_stops_requests() {
        let mut state = super::CodeBlame::default();
        state.configure(true);
        assert!(state.needs_request("repo/file".into()));
        state.requested(7);
        state.configure(false);
        assert!(!state.needs_request("repo/file".into()));
        assert!(!state.accept(
            7,
            &neoism_protocol::git::GitServerMessage::Error {
                message: "late".into()
            }
        ));
        state.toggle();
        state.apply_default(false); // same config preserves the explicit palette override
        assert!(state.enabled);
        state.configure(false); // a hot reload resets the override
        assert!(!state.enabled);
    }
    #[test]
    fn dates_are_portable_and_match_inline_words() {
        assert_eq!(super::relative_date(0, 172800), "2 days ago");
        assert_eq!(super::relative_date(0, 60), "1 minute ago");
        assert_eq!(super::absolute_date(0), "1970-01-01 00:00 UTC");
        assert_eq!(super::absolute_date(-1), "1969-12-31 23:59 UTC");
        assert_eq!(super::absolute_date(951782400), "2000-02-29 00:00 UTC");
    }
    #[test]
    fn bounded_diff_does_not_guess_middle_identities() {
        let mut old = vec!["prefix".into()];
        old.extend((0..200).map(|i| format!("old-{i}")));
        old.push("suffix".into());
        let mut new = vec!["prefix".into()];
        new.extend((0..200).map(|i| format!("new-{i}")));
        new.push("suffix".into());
        let (map, complete) =
            super::super::gitdiff::unchanged_line_map_checked(&old, &new);
        assert!(!complete);
        assert_eq!(map[0], Some(0));
        assert_eq!(map[201], Some(201));
        assert!(map[1..201].iter().all(Option::is_none));
    }
    #[test]
    fn stale_reply_rejected_after_scope_change() {
        let mut state = super::CodeBlame::default();
        state.toggle();
        assert!(state.needs_request("host-a/file".into()));
        state.requested(1);
        assert!(state.needs_request("host-b/file".into()));
        state.requested(2);
        let reply = neoism_protocol::git::GitServerMessage::Error {
            message: "offline".into(),
        };
        assert!(!state.accept(1, &reply));
        assert!(state.accept(2, &reply));
    }
}
