//! The fake microphone headless runs record from, and the WAV files it reads and writes.
use super::MicPermission;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The default fake input's sample rate.
const RATE: u32 = 48_000;

/// A microphone for headless runs and tests: recorders started where no window has run (and
/// any recorder given one with [`Recorder::fake`](super::Recorder::fake)) record from it
/// instead of a device. It never opens a device, never asks for permission and never makes a
/// sound. Elapsed time follows the virtual clock, levels come from its samples, and `stop`
/// writes them to a 16-bit mono WAV that [`Player::audio`](crate::Player::audio) can open.
///
/// Its input loops when the recording outlasts it. With none installed, headless recorders
/// hear [`FakeMicrophone::default`]: a 220 Hz tone that swells and fades every 1.6 s, so a
/// meter in `--frames` output moves.
///
/// ```
/// use rune::{FakeMicrophone, MicPermission};
/// // Recorders started on this thread now hear a -6 dBFS 1 kHz sine.
/// FakeMicrophone::sine(1000.0, 0.5).install();
/// // Or test the denied path.
/// FakeMicrophone::silence().permission(MicPermission::Denied).install();
/// ```
#[derive(Clone, Debug)]
pub struct FakeMicrophone {
    signal: Signal,
    pub(super) permission: MicPermission,
    pub(super) answer: MicPermission,
    pub(super) unplug: Option<f64>,
}

#[derive(Clone, Debug)]
enum Signal {
    Swell,
    Silence,
    Sine { hz: f32, amplitude: f32 },
    Samples { rate: u32, samples: Arc<[f32]> },
    Wav(PathBuf),
}

impl Default for FakeMicrophone {
    /// A 220 Hz tone whose amplitude swells from silence to 0.3 and back every 1.6 s.
    fn default() -> Self {
        Self::with(Signal::Swell)
    }
}

thread_local! {
    static INSTALLED: std::cell::RefCell<Option<FakeMicrophone>> = const { std::cell::RefCell::new(None) };
}

impl FakeMicrophone {
    fn with(signal: Signal) -> Self {
        Self { signal, permission: MicPermission::Granted, answer: MicPermission::Granted, unplug: None }
    }
    /// Digital silence.
    pub fn silence() -> Self {
        Self::with(Signal::Silence)
    }
    /// A steady sine at `hz` with peak `amplitude` (1 is full scale), sampled at 48 kHz.
    pub fn sine(hz: f32, amplitude: f32) -> Self {
        Self::with(Signal::Sine { hz, amplitude })
    }
    /// Mono samples at `rate` Hz (-1 to 1).
    pub fn samples(rate: u32, samples: impl Into<Vec<f32>>) -> Self {
        Self::with(Signal::Samples { rate: rate.max(1), samples: samples.into().into() })
    }
    /// A WAV file (8, 16, 24 or 32-bit PCM, or 32-bit float), mixed to mono, read when a
    /// recording starts. A file that can't be read fails the recording with the reason.
    pub fn wav(path: impl Into<PathBuf>) -> Self {
        Self::with(Signal::Wav(path.into()))
    }
    /// What [`Recorder::permission`](super::Recorder::permission) reports (default
    /// `Granted`). Starting with `Denied` or `Restricted` fails as a real microphone would.
    pub fn permission(mut self, permission: MicPermission) -> Self {
        self.permission = permission;
        self
    }
    /// How a permission request resolves while the permission is `Undetermined` (default
    /// `Granted`). Like the system's answer, it sticks.
    pub fn answers(mut self, answer: MicPermission) -> Self {
        self.answer = answer;
        self
    }
    /// The microphone goes away after `seconds` of recording, as when a headset is unplugged:
    /// the recorder keeps what it captured and reports
    /// [`RecorderState::Interrupted`](super::RecorderState::Interrupted).
    pub fn unplug_after(mut self, seconds: f64) -> Self {
        self.unplug = Some(seconds.max(0.0));
        self
    }
    /// Recorders started on this thread while no window has run hear this microphone.
    pub fn install(self) {
        INSTALLED.with_borrow_mut(|m| *m = Some(self));
    }
    /// Back to [`FakeMicrophone::default`] on this thread.
    pub fn uninstall() {
        INSTALLED.with_borrow_mut(|m| *m = None);
    }

    pub(super) fn installed() -> Self {
        INSTALLED.with_borrow(|m| m.clone()).unwrap_or_default()
    }
    /// Answers a permission request on the installed microphone, which remembers the answer.
    pub(super) fn request_installed() -> MicPermission {
        INSTALLED.with_borrow_mut(|m| m.get_or_insert_with(Self::default).resolve())
    }
    pub(super) fn resolve(&mut self) -> MicPermission {
        if self.permission == MicPermission::Undetermined {
            self.permission = self.answer;
        }
        self.permission
    }

    /// The samples to record from, read once per recording.
    pub(super) fn open(&self) -> Result<Input, String> {
        Ok(match &self.signal {
            Signal::Swell => Input { rate: RATE, kind: Kind::Swell },
            Signal::Silence => Input { rate: RATE, kind: Kind::Silence },
            Signal::Sine { hz, amplitude } => Input { rate: RATE, kind: Kind::Sine { hz: *hz as f64, amplitude: *amplitude as f64 } },
            Signal::Samples { rate, samples } => Input { rate: *rate, kind: Kind::Samples(samples.clone()) },
            Signal::Wav(path) => {
                let (rate, samples) = read_wav(path)?;
                Input { rate, kind: Kind::Samples(samples.into()) }
            }
        })
    }
}

pub(super) struct Input {
    pub rate: u32,
    kind: Kind,
}

enum Kind {
    Swell,
    Silence,
    Sine { hz: f64, amplitude: f64 },
    Samples(Arc<[f32]>),
}

impl Input {
    /// Sample `i`, looping a finite input.
    pub fn sample(&self, i: u64) -> f32 {
        let t = i as f64 / self.rate as f64;
        let tau = std::f64::consts::TAU;
        match &self.kind {
            Kind::Silence => 0.0,
            Kind::Sine { hz, amplitude } => (amplitude * (tau * hz * t).sin()) as f32,
            Kind::Swell => (0.3 * (0.5 - 0.5 * (tau * t / 1.6).cos()) * (tau * 220.0 * t).sin()) as f32,
            Kind::Samples(s) if s.is_empty() => 0.0,
            Kind::Samples(s) => s[(i % s.len() as u64) as usize],
        }
    }
}

/// Writes the first `frames` samples of `input` to a 16-bit mono WAV.
pub(super) fn write_wav(path: &Path, input: &Input, frames: u64) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("couldn't write {}: {e}", path.display());
    let bytes = frames.checked_mul(2).filter(|b| *b <= u32::MAX as u64 - 36).ok_or_else(|| "the recording is too long for a WAV file".to_string())?;
    let mut out = BufWriter::new(std::fs::File::create(path).map_err(fail)?);
    out.write_all(&wav_header(input.rate, bytes as u32)).map_err(fail)?;
    for i in 0..frames {
        let s = (input.sample(i).clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.write_all(&s.to_le_bytes()).map_err(fail)?;
    }
    out.flush().map_err(fail)
}

fn wav_header(rate: u32, data: u32) -> [u8; 44] {
    let mut h = [0u8; 44];
    let put = |h: &mut [u8; 44], at: usize, b: &[u8]| h[at..at + b.len()].copy_from_slice(b);
    put(&mut h, 0, b"RIFF");
    put(&mut h, 4, &(36 + data).to_le_bytes());
    put(&mut h, 8, b"WAVEfmt ");
    put(&mut h, 16, &16u32.to_le_bytes());
    put(&mut h, 20, &1u16.to_le_bytes()); // PCM
    put(&mut h, 22, &1u16.to_le_bytes()); // mono
    put(&mut h, 24, &rate.to_le_bytes());
    put(&mut h, 28, &(rate * 2).to_le_bytes());
    put(&mut h, 32, &2u16.to_le_bytes());
    put(&mut h, 34, &16u16.to_le_bytes());
    put(&mut h, 36, b"data");
    put(&mut h, 40, &data.to_le_bytes());
    h
}

/// Reads a PCM or float WAV, mixed to mono: (sample rate, samples).
pub(crate) fn read_wav(path: &Path) -> Result<(u32, Vec<f32>), String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path).and_then(|mut f| f.read_to_end(&mut bytes)).map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
    parse_wav(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

pub(super) fn parse_wav(b: &[u8]) -> Result<(u32, Vec<f32>), &'static str> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err("not a WAV file");
    }
    let u16_at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let (mut fmt, mut data) = (None, None);
    let mut at = 12;
    while at + 8 <= b.len() {
        let len = u32_at(at + 4) as usize;
        let (start, end) = (at + 8, (at + 8).saturating_add(len).min(b.len()));
        match &b[at..at + 4] {
            b"fmt " if end - start >= 16 => fmt = Some(start),
            b"data" => data = Some(start..end),
            _ => {}
        }
        at = end.saturating_add(len & 1);
    }
    let (Some(f), Some(data)) = (fmt, data) else { return Err("no fmt or data chunk") };
    let mut format = u16_at(f);
    if format == 0xFFFE && f + 26 <= b.len() {
        format = u16_at(f + 24); // WAVE_FORMAT_EXTENSIBLE: the sub-format's first two bytes.
    }
    let (channels, rate, bits) = (u16_at(f + 2) as usize, u32_at(f + 4), u16_at(f + 14));
    let width = bits as usize / 8;
    if channels == 0 || rate == 0 || width == 0 {
        return Err("invalid fmt chunk");
    }
    let sample: fn(&[u8]) -> f32 = match (format, bits) {
        (1, 8) => |s| (s[0] as f32 - 128.0) / 128.0,
        (1, 16) => |s| i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0,
        (1, 24) => |s| (i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8) as f32 / 8_388_608.0,
        (1, 32) => |s| i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f32 / 2_147_483_648.0,
        (3, 32) => |s| f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
        _ => return Err("unsupported WAV sample format (use PCM or 32-bit float)"),
    };
    let frame = width * channels;
    let samples = b[data]
        .chunks_exact(frame)
        .map(|f| f.chunks_exact(width).map(sample).sum::<f32>() / channels as f32)
        .collect();
    Ok((rate, samples))
}
