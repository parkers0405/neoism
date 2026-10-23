//! Platform-independent prompt admission and Windows-only clock correction.
use super::{instant_from_epoch_millis, Instant};
use neoism_ui::panels::agent_pane::status_policy::PromptAdmission;

#[derive(Clone, Debug)]
pub(super) struct DesktopStatusTiming {
    pub(super) handoff: neoism_ui::panels::agent_pane::status_policy::QueueHandoff,
    enabled: bool,
    local_run: bool,
    admission: PromptAdmission,
    awaiting_admission: bool,
    server_started_at: Option<u64>,
    retired_execution: Option<String>,
    execution_wall: Option<(String, Instant, u64)>,
}

impl Default for DesktopStatusTiming {
    fn default() -> Self {
        Self {
            handoff: Default::default(),
            enabled: cfg!(target_os = "windows"),
            local_run: false,
            admission: PromptAdmission::default(),
            awaiting_admission: false,
            server_started_at: None,
            retired_execution: None,
            execution_wall: None,
        }
    }
}

impl DesktopStatusTiming {
    #[cfg(test)]
    pub(super) fn for_test(enabled: bool) -> Self {
        Self {
            enabled,
            ..Default::default()
        }
    }

    pub(super) fn retire_execution(&mut self, id: &str) {
        self.retired_execution = Some(id.to_owned());
    }

    pub(super) fn accepts_execution(&self, id: &str) -> bool {
        self.retired_execution
            .as_deref()
            .is_none_or(|retired| id > retired)
    }

    pub(super) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(super) fn admit(&mut self, message_id: &str) {
        self.local_run = true;
        self.admission.admit(message_id);
        self.awaiting_admission = true;
    }

    pub(super) fn awaiting_admission(&self) -> bool {
        self.awaiting_admission
    }

    pub(super) fn confirm_admission(&mut self) {
        self.awaiting_admission = false;
    }

    pub(super) fn pending_model_clock(&self) -> bool {
        self.local_run
    }

    #[cfg(test)]
    pub(super) fn queue_count(&mut self, count: usize, started_at: Option<u64>) -> usize {
        self.queue_snapshot_count(count, started_at.is_some(), None)
    }

    pub(super) fn queue_snapshot_count(
        &mut self,
        count: usize,
        running: bool,
        preview: Option<&str>,
    ) -> usize {
        self.admission.project(count, running, preview)
    }

    pub(super) fn queue_preview(&self, preview: Option<String>) -> Option<String> {
        self.admission.project_preview(preview)
    }

    pub(super) fn dequeue(&mut self, message_id: Option<&str>) -> bool {
        let primary = self.admission.dequeue(message_id);
        if primary {
            self.confirm_admission();
        }
        primary
    }

    pub(super) fn settle(&mut self) {
        self.local_run = false;
        self.awaiting_admission = false;
        self.admission.settle();
        self.server_started_at = None;
    }

    pub(super) fn started_at(&mut self, current: Option<Instant>, epoch: u64) -> Instant {
        let same_run = self.server_started_at == Some(epoch)
            || (self.server_started_at.is_none() && self.local_run);
        self.server_started_at = Some(epoch);
        if self.enabled && same_run {
            if let Some(current) = current {
                return current;
            }
        }
        instant_from_epoch_millis(epoch)
    }

    pub(super) fn execution_wall_ms(
        &mut self,
        id: &str,
        now: Instant,
        wall_ms: u64,
    ) -> u64 {
        if !self.enabled {
            return wall_ms;
        }
        let (_, observed, wall) = self
            .execution_wall
            .get_or_insert_with(|| (id.to_owned(), now, wall_ms));
        let projected = wall
            .saturating_add(now.saturating_duration_since(*observed).as_millis() as u64);
        if self
            .execution_wall
            .as_ref()
            .is_some_and(|(previous, _, _)| previous != id)
        {
            self.execution_wall = Some((id.to_owned(), now, wall_ms));
            wall_ms
        } else {
            projected
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    pub(super) fn policy(enabled: bool) -> DesktopStatusTiming {
        DesktopStatusTiming {
            enabled,
            ..Default::default()
        }
    }

    #[test]
    fn platform_default_is_windows_only() {
        assert_eq!(
            DesktopStatusTiming::default().enabled,
            cfg!(target_os = "windows")
        );
    }

    #[test]
    fn model_clock_waits_for_execution_on_every_platform() {
        for enabled in [false, true] {
            let mut status = policy(enabled);
            assert!(!status.pending_model_clock());
            status.admit("first");
            assert!(status.pending_model_clock());
            status.confirm_admission();
            assert!(status.pending_model_clock());
            status.retire_execution("execution-a");
            assert!(!status.accepts_execution("execution-a"));
            assert!(status.accepts_execution("execution-b"));
            status.settle();
            assert!(!status.pending_model_clock());
        }
    }

    #[test]
    fn windows_split_frames_do_not_count_admission_as_waiting() {
        let mut status = policy(true);
        status.admit("first");
        assert_eq!(status.queue_count(1, None), 0);
        assert_eq!(status.queue_count(2, None), 1);
        assert!(!status.dequeue(Some("other")));
        assert!(status.dequeue(Some("first")));
        assert_eq!(status.queue_count(1, None), 1);
        assert_eq!(status.queue_count(1, Some(100)), 1);
        assert!(!status.dequeue(Some("first")));
    }

    #[test]
    fn windows_running_hydration_closes_reservation_when_dequeue_was_delayed() {
        let mut status = policy(true);
        // Cold snapshots have no locally reserved request to subtract.
        assert_eq!(status.queue_count(1, None), 1);
        status.admit("first");
        assert_eq!(status.queue_count(1, None), 0);
        assert_eq!(status.queue_count(1, Some(100)), 1);
        // Reconnect may miss dequeue. Do not subtract again between runs.
        assert_eq!(status.queue_count(1, None), 1);
        // A late dequeue of our primary prompt still must not consume a
        // waiting follow-up, even after a running hydration overtook it.
        assert!(status.dequeue(Some("first")));
        assert_eq!(status.queue_count(1, None), 1);
        status.settle();
        assert_eq!(status.queue_count(1, None), 1);
    }

    #[test]
    fn linux_macos_admission_changes_without_changing_wall_clock() {
        let mut status = policy(false);
        status.admit("first");
        assert_eq!(status.queue_count(1, None), 0);
        assert_eq!(status.queue_count(2, None), 1);
        assert!(status.dequeue(Some("first")));
        assert_eq!(status.queue_count(1, Some(100)), 1);
        let now = Instant::now();
        assert_eq!(status.execution_wall_ms("run", now, 100), 100);
        assert_eq!(status.execution_wall_ms("run", now, 999), 999);
    }

    #[test]
    fn windows_repeated_status_keeps_local_monotonic_start() {
        let mut status = policy(true);
        status.admit("first");
        let start = Instant::now() - Duration::from_secs(5);
        assert_eq!(status.started_at(Some(start), 100), start);
        assert_eq!(status.started_at(Some(start), 100), start);
        status.settle();
        status.admit("next");
        let next = Instant::now();
        assert_eq!(status.started_at(Some(next), 200), next);
        assert_eq!(status.queue_count(1, None), 0);
    }

    #[test]
    fn windows_execution_snapshots_ignore_forward_and_backward_wall_steps() {
        let mut status = policy(true);
        let now = Instant::now();
        assert_eq!(status.execution_wall_ms("run", now, 10_000), 10_000);
        assert_eq!(
            status.execution_wall_ms("run", now + Duration::from_secs(2), 90_000),
            12_000
        );
        assert_eq!(
            status.execution_wall_ms("run", now + Duration::from_secs(3), 1),
            13_000
        );
        assert_eq!(
            status.execution_wall_ms("next", now + Duration::from_secs(4), 20_000),
            20_000
        );
    }
}
