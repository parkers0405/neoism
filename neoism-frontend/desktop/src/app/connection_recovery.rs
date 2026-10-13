//! Delayed recovery UI backed by the existing daemon reconnect runner.
use super::Application;
use crate::daemon_client::DaemonClientStatus;
use neoism_ui::widgets::modal::{ModalAction, ModalButton, ModalSpec};
use neoism_window::window::WindowId;
use std::time::{Duration, Instant};

const OUTAGE_DELAY: Duration = Duration::from_secs(30);
// The pump must tick while recovery is active. A larger observation gap is
// suspension/inactivity, not proof of thirty seconds of active recovery.
const SUSPEND_GAP: Duration = Duration::from_secs(15);

#[derive(Default, Debug)]
pub(crate) struct OutageTracker {
    last_sample: Option<Instant>,
    active_elapsed: Duration,
    presented: bool,
}

impl OutageTracker {
    fn observe(&mut self, now: Instant, recovering: bool) -> bool {
        if !recovering {
            *self = Self::default();
            return false;
        }
        if let Some(last) = self.last_sample {
            let elapsed = now.saturating_duration_since(last);
            if elapsed > SUSPEND_GAP {
                // Fresh grace period on resume, including a nearly-expired
                // outage. Keep presented set: still one continuous outage.
                self.active_elapsed = Duration::ZERO;
            } else {
                self.active_elapsed = self.active_elapsed.saturating_add(elapsed);
            }
        }
        self.last_sample = Some(now);
        !self.presented && self.active_elapsed >= OUTAGE_DELAY
    }
}

fn recovery_spec(endpoint: &str, name: &str) -> ModalSpec {
    ModalSpec {
        title: format!("Connection lost to {name}"),
        body: "The workspace connection has been unavailable for 30 seconds. Neoism is still trying to reconnect. You can retry now or leave this workspace without stopping the host's shells.".into(),
        meta: endpoint.into(),
        input: None,
        buttons: vec![
            ModalButton::new("Retry now", "Enter", ModalAction::ServerReconnectRetry { endpoint: endpoint.into() }),
            ModalButton::new("Leave workspace", "", ModalAction::ServerReconnectLeave { endpoint: endpoint.into() }),
        ],
        busy: false,
        blocking: false,
    }
}

impl Application<'_> {
    /// Call on the regular window pump and runner status events. Normal idle
    /// retry gaps up to 8 seconds count; suspend-sized gaps over 15 seconds
    /// restart grace rather than turning elapsed wall time into an outage.
    pub fn update_connection_recovery(&mut self, window_id: WindowId) {
        let Some(session) = self.window_sessions.get_mut(&window_id) else {
            return;
        };
        let now = Instant::now();
        let home = self.home_daemon_endpoint.as_deref();
        let active_endpoint = session.connection.endpoint().to_string();
        let host_ended = session.host_ended();
        let statuses: Vec<_> =
            std::iter::once((&active_endpoint, session.connection.status()))
                .chain(
                    session
                        .parked_connections
                        .iter()
                        .map(|(endpoint, connection)| (endpoint, connection.status())),
                )
                .map(|(endpoint, status)| (endpoint.clone(), status))
                .collect();
        session
            .connection_recovery
            .retain(|endpoint, _| statuses.iter().any(|(key, _)| key == endpoint));
        let Some(route) = self.router.routes.get_mut(&window_id) else {
            return;
        };
        let screen = &mut route.window.screen;
        for (endpoint, status) in statuses {
            let recovering = home != Some(endpoint.as_str())
                && !(endpoint == active_endpoint && host_ended)
                && matches!(
                    status,
                    DaemonClientStatus::Connecting | DaemonClientStatus::BackingOff
                );
            let name = self
                .server_registry
                .servers()
                .iter()
                .find(|server| server.endpoint == endpoint)
                .map(|server| server.name.as_str())
                .unwrap_or(endpoint.as_str());
            let tracker = session
                .connection_recovery
                .entry(endpoint.clone())
                .or_default();
            let due = tracker.observe(now, recovering);
            if !recovering
                && screen.renderer.modal.active_server_recovery_endpoint()
                    == Some(endpoint.as_str())
            {
                screen.renderer.modal.close();
                screen.mark_dirty();
            }
            // Do not replace another dialog or interrupt an unrelated parked
            // workspace. If its own modal was dismissed, never reopen it.
            if due
                && endpoint == active_endpoint
                && screen.context_manager.current_adopted_workspace_endpoint()
                    == Some(endpoint.as_str())
                && !screen.renderer.modal.is_active()
            {
                screen.renderer.modal.open(recovery_spec(&endpoint, name));
                tracker.presented = true;
                screen.mark_dirty();
            }
        }
    }

    /// Drain Screen::take_server_recovery() here for Retry (leave=false).
    /// Wakes the existing active or parked runner; never redials a new client.
    /// Closed/HostEnded links and the home daemon are deliberately excluded.
    pub fn retry_server_connection(
        &mut self,
        window_id: WindowId,
        endpoint: &str,
    ) -> bool {
        if self.home_daemon_endpoint.as_deref() == Some(endpoint) {
            return false;
        }
        let Some(session) = self.window_sessions.get(&window_id) else {
            return false;
        };
        let active = session.connection.endpoint() == endpoint;
        if active && session.host_ended() {
            return false;
        }
        let requested = if active {
            session.connection.request_reconnect()
        } else {
            session
                .parked_connections
                .get(endpoint)
                .is_some_and(|connection| connection.request_reconnect())
        };
        if let Some(route) = self.router.routes.get_mut(&window_id) {
            let screen = &mut route.window.screen;
            if screen.renderer.modal.active_server_recovery_endpoint() == Some(endpoint) {
                screen.renderer.modal.close();
                screen.mark_dirty();
            }
        }
        requested
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn advance(tracker: &mut OutageTracker, start: Instant, from: u64, to: u64) -> bool {
        let mut due = false;
        for second in from..=to {
            due = tracker.observe(start + Duration::from_secs(second), true);
        }
        due
    }

    #[test]
    fn recovery_actions_keep_identity_separate_from_friendly_title() {
        let spec = recovery_spec("wss://example/session", "Design session");
        assert_eq!(spec.title, "Connection lost to Design session");
        assert_eq!(spec.buttons.len(), 2);
        assert_eq!(spec.buttons[0].label, "Retry now");
        assert_eq!(spec.buttons[1].label, "Leave workspace");
        assert!(
            matches!(&spec.buttons[0].action, ModalAction::ServerReconnectRetry { endpoint } if endpoint == "wss://example/session")
        );
    }

    #[test]
    fn thirty_active_seconds_and_once_per_outage() {
        let mut tracker = OutageTracker::default();
        let start = Instant::now();
        assert!(!advance(&mut tracker, start, 0, 29));
        assert!(advance(&mut tracker, start, 30, 30));
        tracker.presented = true;
        assert!(!advance(&mut tracker, start, 31, 90));
    }

    #[test]
    fn successful_reconnect_resets_the_next_outage() {
        let mut tracker = OutageTracker::default();
        let start = Instant::now();
        assert!(advance(&mut tracker, start, 0, 30));
        tracker.presented = true;
        assert!(!tracker.observe(start + Duration::from_secs(31), false));
        assert!(!advance(&mut tracker, start, 32, 61));
        assert!(advance(&mut tracker, start, 62, 62));
    }

    #[test]
    fn normal_eight_second_backoff_samples_accumulate() {
        let mut tracker = OutageTracker::default();
        let start = Instant::now();
        for second in [0, 8, 16, 24] {
            assert!(!tracker.observe(start + Duration::from_secs(second), true));
        }
        assert!(tracker.observe(start + Duration::from_secs(32), true));
    }

    #[test]
    fn sleep_gap_starts_a_fresh_grace_period_not_an_immediate_dialog() {
        let mut tracker = OutageTracker::default();
        let start = Instant::now();
        assert!(!advance(&mut tracker, start, 0, 29));
        assert!(!advance(&mut tracker, start, 3600, 3629));
        assert!(advance(&mut tracker, start, 3630, 3630));
        tracker.presented = true;
        assert!(!advance(&mut tracker, start, 7200, 7230));
    }
}
