//! Windows desktop admission/timing correction. Other desktops deliberately keep
//! their existing policy. Tests exercise both policies without requiring Windows.
use super::{instant_from_epoch_millis, Instant};

#[derive(Clone, Debug)]
struct PromptAdmission {
    message_id: String,
    waiting_for_worker: bool,
}

#[derive(Clone, Debug)]
pub(super) struct DesktopStatusTiming {
    enabled: bool,
    local_run: bool,
    admission: Option<PromptAdmission>,
    server_started_at: Option<u64>,
    retired_execution: Option<String>,
    execution_wall: Option<(String, Instant, u64)>,
}

impl Default for DesktopStatusTiming {
    fn default() -> Self {
        Self {
            enabled: cfg!(target_os = "windows"),
            local_run: false,
            admission: None,
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
        if self.enabled {
            self.retired_execution = Some(id.to_owned());
        }
    }

    pub(super) fn accepts_execution(&self, id: &str) -> bool {
        !self.enabled
            || self
                .retired_execution
                .as_deref()
                .is_none_or(|retired| id > retired)
    }

    pub(super) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(super) fn admit(&mut self, message_id: &str) {
        if self.enabled {
            self.local_run = true;
            self.admission = Some(PromptAdmission {
                message_id: message_id.to_owned(),
                waiting_for_worker: true,
            });
        }
    }

    pub(super) fn queue_count(&mut self, count: usize, started_at: Option<u64>) -> usize {
        // A running snapshot ends the admission window even if its dequeue
        // event was lost during reconnect. Otherwise a later between-run
        // queue snapshot could incorrectly subtract a genuine follow-up.
        if self.enabled && started_at.is_some() {
            if let Some(admission) = self.admission.as_mut() {
                admission.waiting_for_worker = false;
            }
        }
        // enqueue_v2_prompt publishes busy BEFORE spawning the worker. The
        // first prompt is then counted in the storage queue, but is already
        // represented by our optimistic primary activity. After dequeue, even
        // a status with no startedAt can contain real waiting follow-ups.
        count.saturating_sub(usize::from(
            self.enabled
                && self
                    .admission
                    .as_ref()
                    .is_some_and(|admission| admission.waiting_for_worker)
                && started_at.is_none(),
        ))
    }

    pub(super) fn dequeue(&mut self, message_id: Option<&str>) -> bool {
        if self
            .admission
            .as_ref()
            .is_some_and(|admission| Some(admission.message_id.as_str()) == message_id)
        {
            self.admission = None;
            // This prompt wasn't part of the displayed waiting count.
            true
        } else {
            false
        }
    }

    pub(super) fn settle(&mut self) {
        self.local_run = false;
        self.admission = None;
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
    fn linux_macos_queue_and_wall_clock_semantics_unchanged() {
        let mut status = policy(false);
        status.admit("first");
        assert_eq!(status.queue_count(1, None), 1);
        assert_eq!(status.queue_count(2, None), 2);
        assert!(!status.dequeue(Some("first")));
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
