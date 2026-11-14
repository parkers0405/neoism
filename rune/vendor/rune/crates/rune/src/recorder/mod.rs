//! Audio recording: a cheap, cloneable [`Recorder`] handle that records the microphone to a
//! file, with elapsed time and a smoothed input level to draw.
//!
//! In a window on macOS, AVAudioRecorder records on its own thread (AAC or WAV, metering built
//! in) and publishes the level 30 times a second while recording. Elsewhere starting fails with
//! a clear state.
//!
//! Headless runs (no window event loop yet, as with `Player`) never open a device: recorders
//! hear a [`FakeMicrophone`], elapsed time follows the virtual clock of the runtime stepping
//! on the thread that started them, levels come from the fake samples in 10 ms blocks (so they
//! don't depend on the frame rate), and `stop` writes a real WAV.

mod fake;
mod meter;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as backend;
#[cfg(not(target_os = "macos"))]
mod unsupported;
#[cfg(not(target_os = "macos"))]
use unsupported as backend;

pub use fake::FakeMicrophone;
pub use meter::Level;

use crate::live::Live;
use crate::video::headless_clock;
use meter::Meter;
use std::path::{Path, PathBuf};
use crate::time::Instant;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

/// The file format a recorder writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RecordFormat {
    /// AAC in an MPEG-4 container (`.m4a`), high quality: about 1 MB a minute, mono.
    Aac,
    /// 16-bit PCM WAV: about 5 MB a minute at 44.1 kHz mono.
    Wav,
}

/// Where a recorder is.
///
/// The `Interrupted` and `Failed` messages start lower case and have no final period, so they
/// read after a colon ("Couldn't record: {message}"). Capitalize the first letter to show one
/// on its own. Later versions keep this form.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RecorderState {
    /// Not started, or cancelled.
    Idle,
    /// Asking for permission or opening the input.
    Starting,
    Recording,
    Paused,
    /// Stopped with the file written.
    Stopped,
    /// The input went away mid-recording (unplugged, or the default input changed). What was
    /// captured is saved; `stop` returns it and `cancel` deletes it.
    Interrupted(String),
    /// Couldn't record: no permission, no input, an unwritable path, or an unsupported
    /// platform. `start` tries again.
    Failed(String),
}

/// Microphone access, as the system reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MicPermission {
    Granted,
    /// The user said no; only System Settings can change it.
    Denied,
    /// Not asked yet: [`Recorder::request_permission`] (or `start`) asks.
    Undetermined,
    /// The system won't allow recording (device management, parental controls), or this
    /// platform can't record.
    Restricted,
}

/// A finished recording.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Recording {
    /// The file, ready for [`Player::audio`](crate::Player::audio). Its extension follows
    /// the format, and a [fake microphone](Recorder::fake) always writes `.wav`, so read the
    /// type from this path rather than from the one given to [`Recorder::to_file`].
    pub path: PathBuf,
    /// Seconds of audio, pauses excluded.
    pub duration: f64,
}

/// Records the microphone to a file. Clones share one recorder; dropping the last clone while
/// recording stops it and keeps the file.
///
/// ```no_run
/// let rec = rune::Recorder::to_file("note.m4a"); // AAC; "note.wav" records WAV
/// rec.start();
/// // In a view, a level bar that redraws without re-running the view:
/// let bar = rune::el().w(4.0).h(40.0).live_scale(&rec.live_level());
/// // or read `rec.level().meter()` and `rec.elapsed()` from `poll`.
/// let note = rec.stop()?;
/// let clip = rune::Player::audio(note.path);
/// # Ok::<(), String>(())
/// ```
///
/// macOS asks for microphone access the first time an app records, and ends an app whose
/// Info.plist has no `NSMicrophoneUsageDescription`. Rune checks first: a bundled app without
/// the key fails to start with a clear message instead. An unbundled binary run from a
/// terminal is attributed to the terminal, which asks on its behalf. Hardened-runtime apps
/// also need the `com.apple.security.device.audio-input` entitlement (sandboxed apps
/// `com.apple.security.device.microphone`); an ad-hoc signed dev build may be denied without
/// a prompt.
#[derive(Clone)]
pub struct Recorder {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Recorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let c = self.shared.ctl.lock().unwrap();
        f.debug_struct("Recorder").field("path", &self.shared.path).field("state", &c.state).field("elapsed", &c.elapsed).finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Options {
    pub format: RecordFormat,
    pub sample_rate: u32,
    pub channels: u16,
    fake: Option<FakeMicrophone>,
}

pub(crate) struct Shared {
    path: PathBuf,
    opts: Mutex<Options>,
    ctl: Mutex<Control>,
    /// The published level: peak and RMS f32 bits in one word, so reads take no lock and
    /// always see a matching pair.
    level: AtomicU64,
    /// Whether a fake recording has run since the last start, so `level` knows to catch it
    /// up without locking for a device recording.
    faked: AtomicBool,
    live: Live<f32>,
    /// The app's waker and the generation it last heard about.
    waker: Mutex<Option<(crate::Waker, u64)>>,
    /// The device recording, if any, with the attempt that opened it.
    native: Mutex<Option<(u64, backend::Session)>>,
}

struct Control {
    state: RecorderState,
    generation: u64,
    /// Bumped by every start and cancel, so a late permission answer or device report for an
    /// earlier attempt is ignored.
    attempt: u64,
    elapsed: f64,
    /// (clock, elapsed) when recording last (re)started.
    anchor: Option<(f64, f64)>,
    /// Elapsed follows the headless clock of the starting thread, not the wall clock.
    virtual_clock: bool,
    fake: Option<FakeSession>,
    /// What an interruption saved, for `stop`.
    saved: Option<Recording>,
    /// The file the current attempt writes.
    path: PathBuf,
}

struct FakeSession {
    input: fake::Input,
    path: PathBuf,
    meter: Meter,
    /// Samples folded into the meter.
    metered: u64,
    unplug: Option<f64>,
}

/// Whether recorders use the device backend: in a window, and always in a browser (which has
/// no headless mode and no file system for a fake microphone's WAV).
fn devices() -> bool {
    cfg!(target_arch = "wasm32") || crate::video::playback_enabled()
}

/// Seconds since the first recorder looked, for elapsed time in a window.
fn wall_now() -> f64 {
    static ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    ORIGIN.get_or_init(Instant::now).elapsed().as_secs_f64()
}

thread_local! {
    /// Fake recorders following this thread's headless clock.
    static HEADLESS: std::cell::RefCell<Vec<Weak<Shared>>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// A headless runtime starts a frame: fake recorders started on this thread catch up to it,
/// so bound level meters move and an unplugged fake microphone interrupts on time.
pub(crate) fn headless_frame(_clock: f64) {
    let recorders: Vec<Arc<Shared>> = HEADLESS.with_borrow_mut(|h| {
        h.retain(|w| w.strong_count() > 0);
        h.iter().filter_map(Weak::upgrade).collect()
    });
    for r in recorders {
        r.follow();
    }
}

/// A new runtime on this thread starts its clock at 0: fake recordings keep their elapsed
/// time and go on from there.
pub(crate) fn headless_reset() {
    let recorders: Vec<Arc<Shared>> = HEADLESS.with_borrow(|h| h.iter().filter_map(Weak::upgrade).collect());
    for r in recorders {
        let mut c = r.ctl.lock().unwrap();
        c.advance();
        if c.anchor.is_some() {
            c.anchor = Some((0.0, c.elapsed));
        }
    }
}

impl Recorder {
    /// How often a recording publishes its level in a window. A `poll` that draws elapsed
    /// time can ask for frames at this rate while recording with `tx.wake_after`.
    pub const METER_INTERVAL: Duration = Duration::from_millis(33);

    /// A recorder writing `path`: WAV when it ends in `.wav`, otherwise AAC, at 44.1 kHz
    /// mono. The format decides the file's extension: AAC writes `.m4a` (or keeps `.mp4`) and
    /// WAV writes `.wav`, so `to_file("note")` writes `note.m4a`, and
    /// [`Recording::path`] is always the file written. Nothing happens until
    /// [`start`](Self::start); each start overwrites the file.
    pub fn to_file(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let wav = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("wav"));
        let format = if wav { RecordFormat::Wav } else { RecordFormat::Aac };
        let opts = Options { format, sample_rate: 44_100, channels: 1, fake: None };
        let ctl = Control {
            state: RecorderState::Idle,
            generation: 0,
            attempt: 0,
            elapsed: 0.0,
            anchor: None,
            virtual_clock: false,
            fake: None,
            saved: None,
            path: path.clone(),
        };
        let silent = Level::SILENT;
        Self {
            shared: Arc::new(Shared {
                path,
                opts: Mutex::new(opts),
                ctl: Mutex::new(ctl),
                level: AtomicU64::new(pack(silent)),
                faked: AtomicBool::new(false),
                live: Live::new(0.0),
                waker: Mutex::new(None),
                native: Mutex::new(None),
            }),
        }
    }
    /// The format for the next start, overriding the extension's. The file's extension
    /// follows it: `to_file("note.m4a").format(RecordFormat::Wav)` writes `note.wav`.
    pub fn format(self, format: RecordFormat) -> Self {
        self.shared.opts.lock().unwrap().format = format;
        self
    }
    /// Device sample rate for the next start (default 44 100 Hz).
    pub fn sample_rate(self, hz: u32) -> Self {
        self.shared.opts.lock().unwrap().sample_rate = hz.clamp(8_000, 192_000);
        self
    }
    /// Device channels for the next start: 1 (default) or 2.
    pub fn channels(self, channels: u16) -> Self {
        self.shared.opts.lock().unwrap().channels = channels.clamp(1, 2);
        self
    }
    /// Record from `mic` instead of a device, even in a window (where its level and elapsed
    /// time update when read). A fake microphone always writes 16-bit mono WAV at its input's
    /// rate: for an AAC recorder at `note.m4a` it writes `note.wav`, and [`Recording::path`]
    /// says so.
    pub fn fake(self, mic: FakeMicrophone) -> Self {
        self.shared.opts.lock().unwrap().fake = Some(mic);
        self
    }

    /// The file a device recording writes: the path given to [`to_file`](Self::to_file),
    /// with the format's extension. A fake microphone writes the `.wav` beside it.
    pub fn path(&self) -> PathBuf {
        with_format_extension(&self.shared.path, self.shared.opts.lock().unwrap().format)
    }

    /// Microphone access now. Headless: the installed [`FakeMicrophone`]'s (default
    /// `Granted`). Never asks.
    pub fn permission() -> MicPermission {
        if devices() { backend::permission() } else { FakeMicrophone::installed().permission }
    }
    /// Asks for microphone access if it's `Undetermined`, then calls `callback` with the
    /// answer, from any thread (at once when already decided, and always at once headless).
    /// macOS shows its prompt with the app's `NSMicrophoneUsageDescription`; a bundled app
    /// without one gets `Denied` rather than the crash macOS would cause.
    pub fn request_permission(callback: impl FnOnce(MicPermission) + Send + 'static) {
        if devices() { backend::request_permission(Box::new(callback)) } else { callback(FakeMicrophone::request_installed()) }
    }

    /// Starts recording, from `Idle`, `Stopped`, `Interrupted` or `Failed` (an interrupted
    /// recording's file is overwritten). Asks for permission first when it's `Undetermined`
    /// (the state is `Starting` meanwhile); without it the state becomes `Failed`.
    pub fn start(&self) {
        let shared = &self.shared;
        let fake = shared.fake_mic();
        let attempt = {
            let mut c = shared.ctl.lock().unwrap();
            if matches!(c.state, RecorderState::Starting | RecorderState::Recording | RecorderState::Paused) {
                return;
            }
            c.attempt += 1;
            c.elapsed = 0.0;
            c.anchor = None;
            c.fake = None;
            shared.faked.store(false, Ordering::Relaxed);
            c.saved = None;
            let format = if fake.is_some() { RecordFormat::Wav } else { shared.opts.lock().unwrap().format };
            c.path = with_format_extension(&shared.path, format);
            c.virtual_clock = fake.is_some() && !devices();
            c.set(RecorderState::Starting);
            c.attempt
        };
        shared.publish(Level::SILENT);
        if fake.is_none()
            && let Err(e) = backend::preflight()
        {
            shared.fail(attempt, e);
            return shared.settle();
        }
        let permission = match &fake {
            Some(mic) => mic.permission,
            None => backend::permission(),
        };
        match permission {
            MicPermission::Granted => shared.launch(attempt, fake),
            MicPermission::Denied => shared.fail(attempt, DENIED.into()),
            MicPermission::Restricted => shared.fail(attempt, RESTRICTED.into()),
            MicPermission::Undetermined => match fake {
                Some(_) => {
                    let answer = shared.resolve_fake_permission();
                    match answer {
                        MicPermission::Granted => shared.launch(attempt, shared.fake_mic()),
                        _ => shared.fail(attempt, denial(answer)),
                    }
                }
                None => {
                    let weak = Arc::downgrade(shared);
                    backend::request_permission(Box::new(move |answer| {
                        let Some(shared) = weak.upgrade() else { return };
                        match answer {
                            MicPermission::Granted => shared.launch(attempt, None),
                            _ => shared.fail(attempt, denial(answer)),
                        }
                        shared.settle();
                    }));
                }
            },
        }
        shared.settle();
    }

    /// Pauses a recording; elapsed time and the file stop growing.
    pub fn pause(&self) {
        let shared = &self.shared;
        {
            let mut c = shared.lock_following();
            if c.state != RecorderState::Recording {
                return;
            }
            c.anchor = None;
            c.set(RecorderState::Paused);
        }
        shared.publish(Level::SILENT);
        if let Some((_, s)) = &*shared.native.lock().unwrap() {
            backend::pause(s);
        }
        shared.settle();
    }

    /// Resumes a paused recording into the same file.
    pub fn resume(&self) {
        let shared = &self.shared;
        {
            let mut c = shared.ctl.lock().unwrap();
            if c.state != RecorderState::Paused {
                return;
            }
            c.anchor = c.now().map(|now| (now, c.elapsed));
            c.set(RecorderState::Recording);
        }
        if let Some((_, s)) = &*shared.native.lock().unwrap() {
            backend::resume(s);
        }
        shared.settle();
    }

    /// Stops and finalizes the file (blocking for the few milliseconds that takes), returning
    /// it. After an interruption, returns what was saved. Stopping while `Starting` abandons
    /// the start; stopping when not recording returns an error and changes nothing.
    pub fn stop(&self) -> Result<Recording, String> {
        let shared = &self.shared;
        let result = shared.stop();
        shared.settle();
        result
    }

    /// Stops and deletes the file of a recording in progress (or interrupted), back to
    /// `Idle`. Only a file this recording created is deleted: a stopped recording's file is
    /// the app's and stays, and cancelling a start that hasn't opened the input yet leaves an
    /// earlier file at the same path alone.
    pub fn cancel(&self) {
        let shared = &self.shared;
        let path = {
            let mut c = shared.ctl.lock().unwrap();
            if !matches!(c.state, RecorderState::Starting | RecorderState::Recording | RecorderState::Paused | RecorderState::Interrupted(_)) {
                return;
            }
            c.attempt += 1;
            c.anchor = None;
            c.set(RecorderState::Idle);
            // A device recording in progress deletes its own file, as it alone knows whether
            // it created one yet.
            c.fake.take().map(|f| f.path).or_else(|| c.saved.take().map(|s| s.path))
        };
        shared.publish(Level::SILENT);
        let session = shared.native.lock().unwrap().take();
        if let Some((_, session)) = session {
            backend::cancel(session);
        }
        if let Some(path) = path {
            let _ = std::fs::remove_file(path);
        }
        shared.settle();
    }

    pub fn state(&self) -> RecorderState {
        self.shared.lock_following().state.clone()
    }
    /// Whether audio is being captured now (`Recording`, not paused).
    pub fn is_recording(&self) -> bool {
        self.shared.lock_following().state == RecorderState::Recording
    }
    /// Seconds recorded, pauses excluded: 0 until recording starts, held while paused and
    /// after stopping. Live from the clock in a window; the virtual clock headless.
    pub fn elapsed(&self) -> f64 {
        self.shared.lock_following().elapsed
    }
    /// The smoothed input level, silent unless recording. Takes no lock in a window and never
    /// allocates, so it's fine to read every frame.
    pub fn level(&self) -> Level {
        if self.shared.faked.load(Ordering::Relaxed) {
            self.shared.follow();
        }
        unpack(self.shared.level.load(Ordering::Acquire))
    }
    /// [`level().meter()`](Level::meter) as a [`Live`] value, for `live_scale`, `live_rect`
    /// or `live_opacity`: a bound meter redraws at the meter rate while recording without
    /// re-running the view, and draws nothing while idle or paused.
    pub fn live_level(&self) -> Live<f32> {
        self.shared.live.clone()
    }
    /// Called, from any thread, when the state changes: started, paused, resumed, stopped,
    /// interrupted or failed. Hand it the app's `setup` waker so `poll` hears a permission
    /// answer or an unplugged microphone. Nothing calls it for elapsed time or levels.
    pub fn set_waker(&self, waker: crate::Waker) {
        *self.shared.waker.lock().unwrap() = Some((waker, 0));
        self.shared.settle();
    }
}

const DENIED: &str = "microphone access is denied: allow it in System Settings > Privacy & Security > Microphone";
const RESTRICTED: &str = "microphone access is restricted on this device";

fn denial(answer: MicPermission) -> String {
    if answer == MicPermission::Restricted { RESTRICTED.into() } else { DENIED.into() }
}

impl Control {
    fn set(&mut self, state: RecorderState) {
        if self.state != state {
            self.state = state;
            self.generation += 1;
        }
    }
    fn now(&self) -> Option<f64> {
        if self.virtual_clock { headless_clock() } else { Some(wall_now()) }
    }
    /// Elapsed follows the clock while recording. A clock behind the anchor (a new runtime on
    /// this thread) restarts from here.
    fn advance(&mut self) {
        if self.state != RecorderState::Recording {
            return;
        }
        let Some(now) = self.now() else { return };
        match self.anchor {
            Some((clock, elapsed)) if clock <= now => self.elapsed = elapsed + (now - clock),
            _ => self.anchor = Some((now, self.elapsed)),
        }
    }
}

impl Shared {
    /// The fake microphone a start would use: the recorder's own, or, headless, the thread's.
    fn fake_mic(&self) -> Option<FakeMicrophone> {
        let own = self.opts.lock().unwrap().fake.clone();
        own.or_else(|| (!devices()).then(FakeMicrophone::installed))
    }

    fn resolve_fake_permission(&self) -> MicPermission {
        let mut opts = self.opts.lock().unwrap();
        match &mut opts.fake {
            Some(mic) => mic.resolve(),
            None => FakeMicrophone::request_installed(),
        }
    }

    /// The control, caught up to the clock.
    fn lock_following(&self) -> std::sync::MutexGuard<'_, Control> {
        let fake = self.ctl.lock().unwrap().fake.is_some();
        if fake {
            self.follow();
        }
        let mut c = self.ctl.lock().unwrap();
        c.advance();
        c
    }

    fn publish(&self, level: Level) {
        self.level.store(pack(level), Ordering::Release);
        self.live.set(level.meter());
    }

    /// Tells the app's waker that the state changed. Call with no locks held.
    fn settle(&self) {
        let generation = self.ctl.lock().unwrap().generation;
        let waker = match &mut *self.waker.lock().unwrap() {
            Some((waker, heard)) if *heard != generation => {
                *heard = generation;
                Some(waker.clone())
            }
            _ => None,
        };
        if let Some(waker) = waker {
            waker();
        }
    }

    fn fail(&self, attempt: u64, error: String) {
        let mut c = self.ctl.lock().unwrap();
        if c.attempt == attempt && c.state == RecorderState::Starting {
            c.set(RecorderState::Failed(error));
        }
    }

    /// Permission is granted: open the input.
    fn launch(self: &Arc<Self>, attempt: u64, fake: Option<FakeMicrophone>) {
        if self.ctl.lock().unwrap().attempt != attempt {
            return;
        }
        match fake {
            Some(mic) => self.launch_fake(attempt, mic),
            None => {
                let opts = self.opts.lock().unwrap().clone();
                let path = self.ctl.lock().unwrap().path.clone();
                let mut native = self.native.lock().unwrap();
                match backend::start(self, attempt, &path, &opts) {
                    Ok(session) => *native = Some((attempt, session)),
                    Err(e) => self.fail(attempt, e),
                }
            }
        }
    }

    fn launch_fake(self: &Arc<Self>, attempt: u64, mic: FakeMicrophone) {
        let path = self.ctl.lock().unwrap().path.clone();
        let opened = mic.open().and_then(|input| {
            // The file exists from the start, as a device recording's does.
            fake::write_wav(&path, &input, 0)?;
            Ok(input)
        });
        let mut c = self.ctl.lock().unwrap();
        if c.attempt != attempt || c.state != RecorderState::Starting {
            return;
        }
        match opened {
            Ok(input) => {
                c.fake = Some(FakeSession { input, path, meter: Meter::default(), metered: 0, unplug: mic.unplug });
                self.faked.store(true, Ordering::Relaxed);
                c.anchor = c.now().map(|now| (now, 0.0));
                c.set(RecorderState::Recording);
                if c.virtual_clock {
                    HEADLESS.with_borrow_mut(|h| {
                        h.retain(|w| w.strong_count() > 0);
                        if !h.iter().any(|w| std::ptr::eq(w.as_ptr(), Arc::as_ptr(self))) {
                            h.push(Arc::downgrade(self));
                        }
                    });
                }
            }
            Err(e) => c.set(RecorderState::Failed(e)),
        }
    }

    /// A fake recording catches up to the clock: elapsed, levels, and an unplugged input.
    fn follow(&self) {
        let level = {
            let mut c = self.ctl.lock().unwrap();
            c.advance();
            if c.state != RecorderState::Recording {
                return;
            }
            let elapsed = c.elapsed;
            let Some(f) = c.fake.as_mut() else { return };
            if let Some(unplug) = f.unplug.filter(|t| elapsed + CLOCK_SLACK >= *t) {
                c.elapsed = unplug;
                c.anchor = None;
                let f = c.fake.take().unwrap();
                let frames = frames_at(unplug, f.input.rate);
                match fake::write_wav(&f.path, &f.input, frames) {
                    Ok(()) => {
                        c.saved = Some(Recording { path: f.path, duration: frames as f64 / f.input.rate as f64 });
                        c.set(RecorderState::Interrupted("the microphone went away".into()));
                    }
                    Err(e) => c.set(RecorderState::Failed(e)),
                }
                drop(c);
                self.publish(Level::SILENT);
                self.settle();
                return;
            }
            let rate = f.input.rate;
            let target = frames_at(elapsed, rate);
            let block = (rate / 100).max(1) as u64;
            // Only the last few seconds still show in the meter.
            f.metered = f.metered.max(target.saturating_sub(10 * rate as u64) / block * block);
            while f.metered + block <= target {
                let input = &f.input;
                f.meter.push_samples((f.metered..f.metered + block).map(|i| input.sample(i)), rate);
                f.metered += block;
            }
            f.meter.level()
        };
        self.publish(level);
    }

    fn stop(&self) -> Result<Recording, String> {
        let fake = self.ctl.lock().unwrap().fake.is_some();
        if fake {
            self.follow();
        }
        let mut c = self.ctl.lock().unwrap();
        c.advance();
        match c.state.clone() {
            RecorderState::Recording | RecorderState::Paused => {}
            RecorderState::Interrupted(_) => {
                let saved = c.saved.take().ok_or_else(|| "the interrupted recording wasn't saved".to_string());
                c.set(RecorderState::Stopped);
                return saved;
            }
            RecorderState::Starting => {
                c.attempt += 1;
                c.set(RecorderState::Idle);
                drop(c);
                if let Some((_, session)) = self.native.lock().unwrap().take() {
                    backend::cancel(session);
                }
                return Err("stopped before recording started".into());
            }
            RecorderState::Failed(e) => return Err(e),
            RecorderState::Idle | RecorderState::Stopped => return Err("not recording".into()),
        }
        c.anchor = None;
        let result = match c.fake.take() {
            Some(f) => {
                let frames = frames_at(c.elapsed, f.input.rate);
                fake::write_wav(&f.path, &f.input, frames).map(|()| Recording { path: f.path, duration: frames as f64 / f.input.rate as f64 })
            }
            None => {
                let path = c.path.clone();
                drop(c);
                let session = self.native.lock().unwrap().take();
                let stopped = match session {
                    Some((_, session)) => backend::stop(session).map(|duration| Recording { path, duration }),
                    None => Err("not recording".into()),
                };
                c = self.ctl.lock().unwrap();
                // The device may have gone away while this waited.
                if let RecorderState::Interrupted(_) = c.state
                    && let Some(saved) = c.saved.take()
                {
                    c.set(RecorderState::Stopped);
                    drop(c);
                    self.publish(Level::SILENT);
                    return Ok(saved);
                }
                stopped
            }
        };
        match &result {
            Ok(_) => c.set(RecorderState::Stopped),
            Err(e) => c.set(RecorderState::Failed(e.clone())),
        }
        drop(c);
        self.publish(Level::SILENT);
        result
    }

    // Reports from a device backend's thread.

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn began(&self, attempt: u64) {
        {
            let mut c = self.ctl.lock().unwrap();
            if c.attempt != attempt || c.state != RecorderState::Starting {
                return;
            }
            c.anchor = Some((wall_now(), 0.0));
            c.set(RecorderState::Recording);
        }
        self.settle();
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn failed(&self, attempt: u64, error: String) {
        self.fail(attempt, error);
        // Only this attempt's session: a late report from an earlier one leaves a newer alone.
        let session = {
            let mut native = self.native.lock().unwrap();
            if native.as_ref().is_some_and(|(a, _)| *a == attempt) { native.take() } else { None }
        };
        drop(session);
        self.settle();
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn interrupted(&self, attempt: u64, reason: String, duration: f64) {
        {
            let mut c = self.ctl.lock().unwrap();
            if c.attempt != attempt || !matches!(c.state, RecorderState::Recording | RecorderState::Paused) {
                return;
            }
            c.advance();
            c.anchor = None;
            c.saved = Some(Recording { path: c.path.clone(), duration });
            c.set(RecorderState::Interrupted(reason));
        }
        self.publish(Level::SILENT);
        self.settle();
    }

    /// A level from the device, published unless the recording moved on.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn metered(&self, attempt: u64, level: Level) {
        let c = self.ctl.lock().unwrap();
        if c.attempt == attempt && c.state == RecorderState::Recording {
            drop(c);
            self.publish(level);
        }
    }
}

/// The last handle is gone: a recording in progress stops and keeps its file. This never
/// locks (it has the only reference), so it can't deadlock wherever the last handle drops.
impl Drop for Shared {
    fn drop(&mut self) {
        let c = self.ctl.get_mut().unwrap_or_else(PoisonError::into_inner);
        c.advance();
        if let Some(f) = c.fake.take()
            && matches!(c.state, RecorderState::Recording | RecorderState::Paused)
        {
            let elapsed = f.unplug.map_or(c.elapsed, |t| c.elapsed.min(t));
            let _ = fake::write_wav(&f.path, &f.input, frames_at(elapsed, f.input.rate));
        }
        if let Some((_, session)) = self.native.get_mut().unwrap_or_else(PoisonError::into_inner).take() {
            backend::finish(session);
        }
    }
}

/// Clock sums drift by a few ulps (60 steps of 1/60 s fall just short of 1 s): treat elapsed
/// time this close to a boundary as on it, so frame rate never moves a sample or an unplug.
const CLOCK_SLACK: f64 = 1e-9;

/// Whole samples recorded after `seconds` at `rate`.
fn frames_at(seconds: f64, rate: u32) -> u64 {
    ((seconds + CLOCK_SLACK) * rate as f64).floor().max(0.0) as u64
}

/// `path` with an extension that holds `format`, which AVAudioRecorder picks the container by.
fn with_format_extension(path: &Path, format: RecordFormat) -> PathBuf {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    let (keep, want): (&[&str], &str) = match format {
        RecordFormat::Aac => (&["m4a", "mp4"], "m4a"),
        RecordFormat::Wav => (&["wav"], "wav"),
    };
    if ext.as_deref().is_some_and(|e| keep.contains(&e)) { path.to_path_buf() } else { path.with_extension(want) }
}

fn pack(level: Level) -> u64 {
    (u64::from(level.peak_db().to_bits()) << 32) | u64::from(level.rms_db().to_bits())
}

fn unpack(bits: u64) -> Level {
    Level::new(f32::from_bits((bits >> 32) as u32), f32::from_bits(bits as u32))
}

#[cfg(test)]
mod tests;
