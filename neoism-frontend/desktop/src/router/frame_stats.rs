use neoism_window::window::WindowId;
use std::time::{Duration, Instant};

pub(crate) struct FrameCadenceStats {
    last_frame_at: Option<Instant>,
    window_started_at: Instant,
    samples: u32,
    over_budget_samples: u32,
    total_interval: Duration,
    min_interval: Option<Duration>,
    max_interval: Option<Duration>,
    render_samples: u32,
    total_render: Duration,
    min_render: Option<Duration>,
    max_render: Option<Duration>,
    frame_intervals: Vec<Duration>,
    render_durations: Vec<Duration>,
}

impl FrameCadenceStats {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            last_frame_at: None,
            window_started_at: now,
            samples: 0,
            over_budget_samples: 0,
            total_interval: Duration::ZERO,
            min_interval: None,
            max_interval: None,
            render_samples: 0,
            total_render: Duration::ZERO,
            min_render: None,
            max_render: None,
            frame_intervals: Vec::with_capacity(300),
            render_durations: Vec::with_capacity(301),
        }
    }

    pub(crate) fn record_frame_start(&mut self, now: Instant, target_interval: Duration) {
        let Some(last_frame_at) = self.last_frame_at.replace(now) else {
            self.window_started_at = now;
            return;
        };

        let interval = now.saturating_duration_since(last_frame_at);
        self.samples += 1;
        if tracing::enabled!(target: "neoism::frame_pacing", tracing::Level::INFO) {
            self.frame_intervals.push(interval);
        }
        self.total_interval += interval;
        self.min_interval =
            Some(self.min_interval.map_or(interval, |min| min.min(interval)));
        self.max_interval =
            Some(self.max_interval.map_or(interval, |max| max.max(interval)));

        if neoism_ui::lifecycle_policy::frame_over_budget(interval, target_interval) {
            self.over_budget_samples += 1;
        }
    }

    pub(crate) fn record_render_duration(&mut self, duration: Duration) {
        self.render_samples += 1;
        if tracing::enabled!(target: "neoism::frame_pacing", tracing::Level::INFO) {
            self.render_durations.push(duration);
        }
        self.total_render += duration;
        self.min_render = Some(self.min_render.map_or(duration, |min| min.min(duration)));
        self.max_render = Some(self.max_render.map_or(duration, |max| max.max(duration)));
    }

    pub(crate) fn maybe_log(
        &mut self,
        now: Instant,
        window_id: WindowId,
        target_interval: Duration,
        refresh_rate_millihertz: Option<u32>,
    ) {
        const MIN_LOG_SAMPLES: u32 = 300;
        if self.samples < MIN_LOG_SAMPLES {
            return;
        }

        let avg_frame_ms =
            self.total_interval.as_secs_f64() * 1000.0 / f64::from(self.samples);
        let min_frame_ms = self.min_interval.unwrap_or_default().as_secs_f64() * 1000.0;
        let max_frame_ms = self.max_interval.unwrap_or_default().as_secs_f64() * 1000.0;
        let target_frame_ms = target_interval.as_secs_f64() * 1000.0;
        let avg_render_ms = if self.render_samples == 0 {
            0.0
        } else {
            self.total_render.as_secs_f64() * 1000.0 / f64::from(self.render_samples)
        };
        let min_render_ms = self.min_render.unwrap_or_default().as_secs_f64() * 1000.0;
        let max_render_ms = self.max_render.unwrap_or_default().as_secs_f64() * 1000.0;
        let wait_outside_render_ms = (avg_frame_ms - avg_render_ms).max(0.0);
        let elapsed_ms = now
            .saturating_duration_since(self.window_started_at)
            .as_secs_f64()
            * 1000.0;

        if tracing::enabled!(target: "neoism::frame_pacing", tracing::Level::INFO) {
            self.frame_intervals.sort_unstable();
            self.render_durations.sort_unstable();
        }
        tracing::info!(
            target: "neoism::frame_pacing",
            ?window_id,
            refresh_rate_millihertz = ?refresh_rate_millihertz,
            target_frame_ms,
            avg_frame_ms,
            min_frame_ms,
            max_frame_ms,
            avg_render_ms,
            min_render_ms,
            max_render_ms,
            p95_frame_ms = percentile_ms(&self.frame_intervals, 95),
            p99_frame_ms = percentile_ms(&self.frame_intervals, 99),
            p95_render_ms = percentile_ms(&self.render_durations, 95),
            p99_render_ms = percentile_ms(&self.render_durations, 99),
            target_exceeded_samples = self.frame_intervals.iter().filter(|&&value| value > target_interval).count(),
            render_target_exceeded_samples = self.render_durations.iter().filter(|&&value| value > target_interval).count(),
            wait_outside_render_ms,
            samples = self.samples,
            render_samples = self.render_samples,
            over_budget_samples = self.over_budget_samples,
            elapsed_ms,
            "frame cadence summary"
        );

        self.window_started_at = now;
        self.samples = 0;
        self.over_budget_samples = 0;
        self.total_interval = Duration::ZERO;
        self.min_interval = None;
        self.max_interval = None;
        self.render_samples = 0;
        self.total_render = Duration::ZERO;
        self.min_render = None;
        self.max_render = None;
        self.frame_intervals.clear();
        self.render_durations.clear();
    }
}

// Nearest-rank percentile of sorted render-start intervals, not compositor
// presentation times. Idle periods remain in these samples, just as in the mean.
fn percentile_ms(sorted: &[Duration], percentile: usize) -> f64 {
    let rank = (sorted.len() * percentile).div_ceil(100).saturating_sub(1);
    sorted.get(rank).copied().unwrap_or_default().as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cadence_percentiles_include_slow_tail_without_averaging_it_away() {
        let mut samples = vec![Duration::from_millis(6); 95];
        samples.extend([Duration::from_millis(20); 5]);
        assert_eq!(percentile_ms(&samples, 95), 6.0);
        assert_eq!(percentile_ms(&samples, 99), 20.0);
        assert_eq!(percentile_ms(&[], 99), 0.0);
        assert_eq!(percentile_ms(&samples[..1], 99), 6.0);
    }
}
