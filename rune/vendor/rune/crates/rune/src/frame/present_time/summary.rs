//! Pure trace analysis, also exercised by the standalone presentation_summary test target.

/// Percentiles of a sample, nearest rank.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Spread {
    pub p50: f64,
    pub p95: f64,
    pub max: f64,
}

impl Spread {
    pub(super) fn of(mut values: Vec<f64>) -> Option<Spread> {
        if values.is_empty() { return None; }
        values.sort_by(f64::total_cmp);
        let rank = |p: f64| values[((p * values.len() as f64).ceil() as usize).clamp(1, values.len()) - 1];
        Some(Spread { p50: rank(0.5), p95: rank(0.95), max: values[values.len() - 1] })
    }
}

/// Present cadence over a trace. Refresh metadata describes expected slots, never actual
/// presentations. Only present_dt_ms supplies measured intervals.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Summary {
    /// Minimum and maximum refresh used to classify measured intervals.
    pub hz: (f64, f64),
    pub frames: usize,
    pub presented: usize,
    pub skipped: usize,
    pub interval: Option<Spread>,
    pub bunched: usize,
    /// Intervals over 1.5 of their frame's refresh periods (not total lost slots).
    pub missed: usize,
    pub lag: Option<Spread>,
}

const BUNCHED_MS: f64 = 2.0;

impl Summary {
    pub(crate) fn of_trace(text: &str) -> Option<Summary> {
        let mut lines = text.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok());
        let header = lines.next()?;
        let valid_hz = |v: &serde_json::Value| v.as_f64().filter(|hz| hz.is_finite() && *hz > 0.0);
        // Compatibility for old traces only. A native header can be the link's startup
        // default, before its first callback, and cannot describe later display changes.
        let header_hz = valid_hz(&header["hz"]).unwrap_or(60.0);
        let mut latest_hz = header_hz;
        let (mut min_hz, mut max_hz) = (f64::INFINITY, 0.0_f64);
        let (mut frames, mut presented, mut skipped, mut timed, mut missed) = (0, 0, 0, false, 0);
        let (mut intervals, mut lags) = (Vec::new(), Vec::new());
        for frame in lines.filter(|l| l.get("t").is_some()) {
            frames += 1;
            latest_hz = valid_hz(&frame["refresh_hz"]).unwrap_or(header_hz);
            let Some(present) = frame.get("present_ms") else { continue };
            timed = true;
            let Some(present) = present.as_f64() else { skipped += 1; continue };
            presented += 1;
            if let Some(dt) = frame["present_dt_ms"].as_f64() {
                intervals.push(dt);
                min_hz = min_hz.min(latest_hz);
                max_hz = max_hz.max(latest_hz);
                if dt > 1.5 * 1000.0 / latest_hz { missed += 1; }
            }
            if let Some(target) = frame["target_ms"].as_f64() { lags.push(present - target); }
        }
        if !timed { return None; }
        Some(Summary {
            hz: if intervals.is_empty() { (latest_hz, latest_hz) } else { (min_hz, max_hz) },
            frames, presented, skipped, missed,
            bunched: intervals.iter().filter(|dt| **dt < BUNCHED_MS).count(),
            interval: Spread::of(intervals),
            lag: Spread::of(lags),
        })
    }
}

impl std::fmt::Display for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let spread = |s: Option<Spread>| s.map_or("none".into(), |s| format!("p50 {:.2} p95 {:.2} max {:.2} ms", s.p50, s.p95, s.max));
        let (low, high) = self.hz;
        let (rate, threshold) = if high - low < 0.01 {
            (format!("{high:.0} Hz"), format!(">{:.2} ms", 1.5 * 1000.0 / high))
        } else {
            (format!("{low:.0}–{high:.0} Hz (per-frame)"), ">1.5 per-frame refresh periods".into())
        };
        write!(f, "present: {} frames, {} presented, {} skipped at {}; interval {}; {} bunched (<{} ms), {} missed ({}); lag {}",
            self.frames, self.presented, self.skipped, rate, spread(self.interval), self.bunched, BUNCHED_MS,
            self.missed, threshold, spread(self.lag))
    }
}