//! Meter ballistics shared by the fake microphone and the macOS backend.

/// The quietest level a meter reports, in dBFS, as AVAudioRecorder does.
pub(crate) const FLOOR_DB: f32 = -160.0;
/// The level `Level::meter` maps to 0.
const METER_FLOOR_DB: f32 = -60.0;
/// RMS rises with this time constant (seconds)...
const ATTACK: f64 = 0.03;
/// ...and falls with this one, so speech reads as a steady bar rather than flicker.
const RELEASE: f64 = 0.3;
/// The peak jumps up at once and falls this many dB a second.
const PEAK_FALL: f64 = 20.0;

/// An input level, smoothed like a meter: the peak jumps up at once and falls at 20 dB/s, and
/// the RMS rises over about 30 ms and falls over about 300 ms. Silence is -160 dBFS.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Level {
    peak_db: f32,
    rms_db: f32,
}

impl Level {
    /// No input.
    pub const SILENT: Level = Level { peak_db: FLOOR_DB, rms_db: FLOOR_DB };

    pub(crate) fn new(peak_db: f32, rms_db: f32) -> Self {
        Self { peak_db: peak_db.clamp(FLOOR_DB, 0.0), rms_db: rms_db.clamp(FLOOR_DB, 0.0) }
    }
    /// The smoothed peak sample in dBFS (-160 to 0).
    pub fn peak_db(&self) -> f32 {
        self.peak_db
    }
    /// The smoothed RMS in dBFS (-160 to 0). A full-scale sine reads about -3.
    pub fn rms_db(&self) -> f32 {
        self.rms_db
    }
    /// The RMS for a level bar: -60 dBFS and below is 0, full scale is 1.
    pub fn meter(&self) -> f32 {
        to_meter(self.rms_db)
    }
}

fn to_meter(db: f32) -> f32 {
    ((db - METER_FLOOR_DB) / -METER_FLOOR_DB).clamp(0.0, 1.0)
}

fn db(amplitude: f64) -> f64 {
    if amplitude > 0.0 { (20.0 * amplitude.log10()).max(FLOOR_DB as f64) } else { FLOOR_DB as f64 }
}

/// Smooths raw block measurements into a [`Level`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct Meter {
    peak_db: f64,
    /// Smoothed mean square.
    ms: f64,
}

impl Default for Meter {
    fn default() -> Self {
        Self { peak_db: FLOOR_DB as f64, ms: 0.0 }
    }
}

impl Meter {
    /// Folds in one block lasting `dt` seconds: its peak amplitude and mean square.
    pub fn push(&mut self, peak: f64, mean_square: f64, dt: f64) {
        if !(dt > 0.0) {
            return;
        }
        let mean_square = if mean_square.is_finite() { mean_square.max(0.0) } else { 0.0 };
        let tau = if mean_square > self.ms { ATTACK } else { RELEASE };
        self.ms += (mean_square - self.ms) * (1.0 - (-dt / tau).exp());
        let peak = if peak.is_finite() { db(peak.abs()) } else { FLOOR_DB as f64 };
        self.peak_db = peak.max(self.peak_db - PEAK_FALL * dt).max(FLOOR_DB as f64);
    }

    /// Folds in a block measured in dBFS, as AVAudioRecorder reports it.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn push_db(&mut self, peak_db: f32, rms_db: f32, dt: f64) {
        let amplitude = |db: f32| if db.is_finite() && db > FLOOR_DB { 10f64.powf(db as f64 / 20.0) } else { 0.0 };
        let rms = amplitude(rms_db);
        self.push(amplitude(peak_db), rms * rms, dt);
    }

    /// Folds in a block of samples lasting `samples.len() / rate` seconds.
    pub fn push_samples(&mut self, samples: impl Iterator<Item = f32>, rate: u32) {
        let (mut peak, mut sum, mut n) = (0f64, 0f64, 0usize);
        for s in samples {
            let s = if s.is_finite() { s as f64 } else { 0.0 };
            peak = peak.max(s.abs());
            sum += s * s;
            n += 1;
        }
        if n > 0 {
            self.push(peak, sum / n as f64, n as f64 / rate as f64);
        }
    }

    pub fn level(&self) -> Level {
        let rms_db = if self.ms > 0.0 { (10.0 * self.ms.log10()).max(FLOOR_DB as f64) } else { FLOOR_DB as f64 };
        Level::new(self.peak_db as f32, rms_db as f32)
    }
}
