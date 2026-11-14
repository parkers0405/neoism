//! Media playback: a cheap, cloneable [`Player`] handle for video drawn into a
//! [`NativeSurface`], or for audio with no element at all.
//!
//! The handle only records intent (play, pause, seek, rate, looping, muted).
//!
//! Video: each frame that draws the element reconciles that intent with the platform player on
//! the UI thread and pulls the frame for the frame's presentation time, so a paused or hidden
//! video costs nothing per frame. On macOS, AVPlayer + AVPlayerItemVideoOutput pull
//! IOSurface-backed `CVPixelBuffer`s for the display link's target time, zero-copy. Until a
//! video first plays, and after its player is parked, it shows a poster frame decoded on a
//! worker thread, with no player or decoder session held.
//!
//! Audio ([`Player::audio`]): no surface and no frames. In a window, edits apply on the main
//! thread at once, and a 20 Hz main-queue tick runs only while some clip plays, loads to play
//! or seeks, to notice the end, loop and loading. Nothing redraws for it: apps read
//! [`Player::time`] on their own frames (see [`Player::set_waker`]).
//!
//! Headless runs (virtual clock) never create a player and never make sound. Video decodes the
//! frame at its virtual time synchronously, so `--snap` and `--frames` are deterministic: time
//! starts on the first frame that draws the element after `play`, and advances by the runtime
//! clock times `rate`. Audio opens synchronously on the first read of its state, duration or
//! time (or on a frame once it plays), and its time follows the clock of the runtime stepping
//! on the thread that made it, from the first frame or edit after `play`: pausing, seeking,
//! rate, looping and the end (which wakes the app's waker) all as in a window, silently.
//!
//! Other platforms report [`PlayerState::Failed`] and draw a neutral placeholder for video.

#[cfg(all(target_os = "macos", feature = "ogg-audio"))]
mod foreign;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as backend;
#[cfg(all(test, target_os = "macos"))]
pub(crate) use macos::test_frames;
#[cfg(not(target_os = "macos"))]
mod unsupported;
#[cfg(not(target_os = "macos"))]
use unsupported as backend;

use crate::native_surface::{ByteFrame, ColorInfo, Format, NativeSurface};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use crate::time::Instant;

/// Set once a window event loop runs. Until then (headless runs, tests) no player is created.
static PLAYBACK: AtomicBool = AtomicBool::new(false);

/// Called by the windowed shell before its event loop starts.
pub(crate) fn enable_playback() {
    if !PLAYBACK.swap(true, Ordering::AcqRel) {
        backend::enable();
    }
}

pub(crate) fn playback_enabled() -> bool {
    PLAYBACK.load(Ordering::Acquire)
}

/// Headless: the clock of the runtime stepping on this thread, and the audio clips made here.
#[derive(Default)]
struct Headless {
    clock: Option<f64>,
    audio: Vec<Weak<Shared>>,
}

thread_local! {
    static HEADLESS: std::cell::RefCell<Headless> = std::cell::RefCell::default();
}

/// A headless runtime starts a frame at `clock`: audio clips made on this thread follow it,
/// and the app's waker hears one end.
pub(crate) fn headless_frame(clock: f64) {
    let clips: Vec<Arc<Shared>> = HEADLESS.with_borrow_mut(|h| {
        h.clock = Some(clock);
        h.audio.retain(|w| w.strong_count() > 0);
        h.audio.iter().filter_map(Weak::upgrade).collect()
    });
    for clip in clips {
        clip.follow_clock();
    }
}

/// A new runtime on this thread starts its clock at 0, replacing an earlier runtime's.
pub(crate) fn headless_reset() {
    HEADLESS.with_borrow_mut(|h| h.clock = Some(0.0));
}

/// Never panics, so destructors can ask (`None` while the thread's storage is torn down).
pub(crate) fn headless_clock() -> Option<f64> {
    HEADLESS.try_with(|h| h.try_borrow().ok().and_then(|h| h.clock)).ok().flatten()
}

/// Where media comes from. Strings containing `://` are URLs; anything else is a file path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaSource {
    File(PathBuf),
    Url(String),
}

impl From<&str> for MediaSource {
    fn from(s: &str) -> Self {
        if s.contains("://") { Self::Url(s.into()) } else { Self::File(s.into()) }
    }
}
impl From<String> for MediaSource {
    fn from(s: String) -> Self {
        if s.contains("://") { Self::Url(s) } else { Self::File(s.into()) }
    }
}
impl From<PathBuf> for MediaSource {
    fn from(path: PathBuf) -> Self {
        Self::File(path)
    }
}
impl From<&Path> for MediaSource {
    fn from(path: &Path) -> Self {
        Self::File(path.into())
    }
}

/// A media URL with HTTP request headers, for clips behind authentication. Build it with
/// [`Player::request`], then make the player with [`audio`](Self::audio) or
/// [`video`](Self::video). Every request the player makes for the clip sends the headers.
/// Header values never appear in `Debug` output or logs.
///
/// ```no_run
/// let token = "…";
/// let clip = rune::Player::request("https://example.com/media/clip.m4a")
///     .header("Authorization", format!("Bearer {token}"))
///     .audio();
/// clip.play();
/// ```
#[derive(Clone)]
pub struct MediaRequest {
    url: String,
    headers: Vec<(String, String)>,
}

impl std::fmt::Debug for MediaRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.headers.iter().map(|(name, _)| name.as_str()).collect();
        f.debug_struct("MediaRequest").field("url", &self.url).field("headers", &names).finish()
    }
}

impl MediaRequest {
    /// Adds a header sent with every request for the clip. A repeated name replaces the earlier
    /// value.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        let (name, value) = (name.into(), value.into());
        self.headers.retain(|(n, _)| !n.eq_ignore_ascii_case(&name));
        self.headers.push((name, value));
        self
    }
    /// An audio clip, as [`Player::audio`].
    pub fn audio(self) -> Player {
        Player::create(MediaSource::Url(self.url), Kind::Audio, self.headers)
    }
    /// A video, muted, as [`Player::url`].
    pub fn video(self) -> Player {
        Player::create(MediaSource::Url(self.url), Kind::Video, self.headers)
    }
}

/// Playback state. Video reports it as of the last frame that drew it; audio as of its last
/// tick or edit.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlayerState {
    /// Opening the source (and, for video, decoding its first frame).
    Loading,
    /// Loaded and paused.
    Ready,
    Playing,
    /// Reached the end without looping. `play` starts again from the beginning.
    Ended,
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Video,
    Audio,
}

/// A video or audio clip. Clones share one player; dropping the last clone (and, for video,
/// every element showing it) stops and frees it. Video starts muted; audio starts unmuted.
///
/// ```no_run
/// let clip = rune::Player::file("capture.mov");
/// clip.set_looping(true);
/// clip.play();
/// // In a view: size the element to the clip's aspect ratio once it is known.
/// let (w, h) = clip.natural_size().unwrap_or((16, 9));
/// let player = rune::el().w(480.0).h(480.0 * h as f32 / w as f32)
///     .video(&clip, rune::SurfaceFit::Fit);
///
/// let tap = rune::Player::audio("sounds/tap.m4a");
/// tap.play(); // audible: audio isn't muted
/// let progress = tap.time() / tap.duration().unwrap_or(1.0);
/// ```
#[derive(Clone)]
pub struct Player {
    surface: NativeSurface,
    shared: Arc<Shared>,
}

/// [`Player`], named for the element builder it pairs with ([`El::video`](crate::El::video)).
pub type Video = Player;

impl std::fmt::Debug for Player {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let c = self.shared.ctl.lock().unwrap();
        f.debug_struct("Player")
            .field("kind", &self.shared.kind)
            .field("source", &self.shared.source)
            .field("state", &c.state())
            .field("time", &c.time)
            .finish_non_exhaustive()
    }
}

impl Player {
    /// A video, muted.
    pub fn new(source: MediaSource) -> Self {
        Self::create(source, Kind::Video, Vec::new())
    }
    /// A local movie file (mp4, mov; H.264 or HEVC), muted.
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self::new(MediaSource::File(path.into()))
    }
    /// A video at an `http(s)://` or `file://` URL, muted.
    pub fn url(url: impl Into<String>) -> Self {
        Self::new(MediaSource::Url(url.into()))
    }
    /// An audio clip (mp3, m4a/AAC, wav, aiff, flac; Ogg Vorbis, Ogg Opus and WebM Opus with
    /// the default `ogg-audio` feature) from a path or URL, unmuted. It plays without any
    /// element, draws nothing and pulls no frames.
    pub fn audio(source: impl Into<MediaSource>) -> Self {
        Self::create(source.into(), Kind::Audio, Vec::new())
    }
    /// A URL to open with request headers: `Player::request(url).header(name, value).audio()`
    /// (or `.video()`). See [`MediaRequest`].
    pub fn request(url: impl Into<String>) -> MediaRequest {
        MediaRequest { url: url.into(), headers: Vec::new() }
    }

    fn create(source: MediaSource, kind: Kind, headers: Vec<(String, String)>) -> Self {
        let ctl = Control { muted: kind == Kind::Video, ..Control::default() };
        let shared = Arc::new(Shared {
            source,
            headers,
            kind,
            ctl: Mutex::new(ctl),
            wakes: Mutex::new(Vec::new()),
            waker: Mutex::new(None),
            notified: AtomicU64::new(0),
            native: Mutex::new(backend::Native::default()),
        });
        backend::created(&shared);
        if kind == Kind::Audio && !playback_enabled() {
            HEADLESS.with_borrow_mut(|h| h.audio.push(Arc::downgrade(&shared)));
        }
        Self { surface: NativeSurface::with_video(shared.clone()), shared }
    }

    pub fn source(&self) -> &MediaSource {
        &self.shared.source
    }
    /// Whether this is an audio-only player from [`Player::audio`].
    pub fn is_audio(&self) -> bool {
        self.shared.kind == Kind::Audio
    }

    /// Decode the first frame now, off the main thread, so the first frame that draws this clip
    /// shows it rather than an empty box: for the page an app expects to show next. Headless
    /// runs decode frames as they draw, so there it does nothing.
    pub fn preload(&self) {
        if playback_enabled() {
            backend::preload(&self.shared);
        }
    }

    /// Start or resume playback. After [`PlayerState::Ended`] it starts from the beginning.
    pub fn play(&self) {
        self.edit(|c| {
            if c.failed.is_some() {
                return;
            }
            if c.ended {
                c.ended = false;
                c.seek = Some(0.0);
                c.time = 0.0;
            }
            c.playing = true;
            c.anchor = None;
        });
    }
    pub fn pause(&self) {
        self.edit(|c| {
            c.playing = false;
            c.anchor = None;
        });
    }
    /// Jump to `seconds` (clamped to the clip). Keeps playing if it was playing.
    pub fn seek(&self, seconds: f64) {
        let seconds = if seconds.is_finite() { seconds.max(0.0) } else { 0.0 };
        self.edit(|c| {
            let seconds = c.duration.map_or(seconds, |d| seconds.min(d));
            c.seek = Some(seconds);
            c.time = seconds;
            c.ended = false;
            c.anchor = None;
        });
    }
    /// Playback speed while playing; 1 is normal. Negative and non-finite rates are ignored.
    pub fn set_rate(&self, rate: f32) {
        if rate.is_finite() && rate > 0.0 {
            self.edit(|c| {
                c.rate = rate;
                c.anchor = None;
            });
        }
    }
    pub fn set_looping(&self, looping: bool) {
        self.edit(|c| c.looping = looping);
    }
    /// Sound plays only while unmuted. Videos start muted, audio clips unmuted.
    pub fn set_muted(&self, muted: bool) {
        self.edit(|c| c.muted = muted);
    }
    /// Called, from any thread, when the state changes: loaded, ended, failed, or started
    /// again by a loop. Hand it the app's `setup` waker so `poll` sees an audio clip end
    /// without ticking. Nothing calls it for progress. If the state already changed (loaded, or
    /// failed at once on an unsupported format), it's called once right away.
    pub fn set_waker(&self, waker: crate::Waker) {
        *self.shared.waker.lock().unwrap() = Some((waker, 0));
        self.shared.settle(false);
    }

    pub fn state(&self) -> PlayerState {
        self.read().state()
    }
    /// Whether `play` is in effect (also true while loading after `play`).
    pub fn is_playing(&self) -> bool {
        self.shared.ctl.lock().unwrap().playing
    }
    /// Playback position in seconds. Video: as of the last drawn frame (or the last `seek`).
    /// Audio: read from the player when called on the main thread while it plays.
    pub fn time(&self) -> f64 {
        let time = self.read().time;
        backend::live_time(&self.shared).unwrap_or(time)
    }
    /// Length in seconds, once loaded.
    pub fn duration(&self) -> Option<f64> {
        self.read().duration
    }
    /// Display size in pixels (rotation applied), once a video loads. None for audio.
    pub fn natural_size(&self) -> Option<(u32, u32)> {
        self.shared.ctl.lock().unwrap().size
    }
    pub fn rate(&self) -> f32 {
        self.shared.ctl.lock().unwrap().rate
    }
    pub fn looping(&self) -> bool {
        self.shared.ctl.lock().unwrap().looping
    }
    pub fn muted(&self) -> bool {
        self.shared.ctl.lock().unwrap().muted
    }
    /// The surface video frames land in: for frame observers and draw counts.
    pub fn surface(&self) -> &NativeSurface {
        &self.surface
    }

    fn read(&self) -> MutexGuard<'_, Control> {
        if self.shared.kind == Kind::Audio && !playback_enabled() {
            self.shared.open_now();
            self.shared.follow_clock();
        }
        self.shared.ctl.lock().unwrap()
    }

    fn edit(&self, f: impl FnOnce(&mut Control)) {
        // Headless audio: fold in the time played up to now at the old rate, then restart the
        // clock from the edit.
        let clock = (self.shared.kind == Kind::Audio && !playback_enabled()).then(headless_clock).flatten();
        if clock.is_some() {
            self.shared.open_now();
        }
        {
            let mut c = self.shared.ctl.lock().unwrap();
            if let Some(now) = clock {
                c.advance_virtual(now);
            }
            f(&mut c);
            if let Some(now) = clock {
                c.advance_virtual(now);
            }
        }
        if clock.is_some() {
            self.shared.settle(false);
        }
        backend::apply(&self.shared);
        self.shared.wake_windows();
    }
}

/// The clock a pull samples: a window's next presentation, or a headless run's virtual time.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Clock {
    Display(Instant),
    Virtual(f64),
}

/// What one pull tells the runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Pulled {
    /// Keep drawing frames: playing, or waiting on a seek.
    pub active: bool,
    /// Changes when state, duration or size change, so views reading them re-run.
    pub generation: u64,
}

/// Folds one drawn video's `generation` into a frame's signature, which changes, re-running
/// views, only when some video's state does. Order-free, so paint order doesn't count.
pub(crate) fn fold(signature: u64, generation: u64, key: usize) -> u64 {
    signature.wrapping_add((generation ^ (key as u64).rotate_left(32)).wrapping_mul(0x9e37_79b9_7f4a_7c15).rotate_left(31))
}

/// [`fold`] for an element that isn't pulled this frame (out of sight, transparent): what it
/// last heard, without pulling a frame. Nothing for a surface with no video.
pub(crate) fn fold_unpulled(signature: u64, surface: &NativeSurface) -> u64 {
    match surface.video() {
        Some(video) => fold(signature, video.ctl.lock().unwrap().generation, surface.key()),
        None => signature,
    }
}

/// What `--tree` reports for a media element.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Snapshot {
    pub kind: Kind,
    pub state: PlayerState,
    pub time: f64,
    pub duration: Option<f64>,
    pub size: Option<(u32, u32)>,
    pub rate: f32,
    pub looping: bool,
    pub muted: bool,
}

/// Clip facts read once when it opens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Meta {
    pub duration: f64,
    /// None for audio.
    pub size: Option<(u32, u32)>,
    /// Seconds per frame, for snapping virtual time to whole frames.
    pub frame: f64,
}

pub(crate) struct Shared {
    source: MediaSource,
    /// HTTP request headers for a URL source. Never logged.
    headers: Vec<(String, String)>,
    kind: Kind,
    ctl: Mutex<Control>,
    wakes: Mutex<Vec<Weak<crate::live::Wake>>>,
    /// The app's waker and the generation it last heard about.
    waker: Mutex<Option<(crate::Waker, u64)>>,
    /// The generation windows last heard about.
    notified: AtomicU64,
    native: Mutex<backend::Native>,
}

impl Shared {
    pub(crate) fn snapshot(&self) -> Snapshot {
        let c = self.ctl.lock().unwrap();
        Snapshot {
            kind: self.kind,
            state: c.state(),
            time: c.time,
            duration: c.duration,
            size: c.size,
            rate: c.rate,
            looping: c.looping,
            muted: c.muted,
        }
    }

    /// Redraw every window that has drawn this player. Safe from any thread.
    fn wake_windows(&self) {
        let wakes: Vec<_> = self.wakes.lock().unwrap().iter().filter_map(Weak::upgrade).collect();
        for wake in wakes {
            wake.notify();
        }
    }

    /// Tell the app's waker (and, outside a frame, the windows) that the state changed.
    /// Call with no locks held.
    fn settle(&self, windows: bool) {
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
        if self.notified.swap(generation, Ordering::AcqRel) != generation && windows {
            self.wake_windows();
        }
    }

    fn register(&self, wake: &Arc<crate::live::Wake>) {
        let mut wakes = self.wakes.lock().unwrap();
        wakes.retain(|w| w.strong_count() > 0);
        if !wakes.iter().any(|w| std::ptr::eq(w.as_ptr(), Arc::as_ptr(wake))) {
            wakes.push(Arc::downgrade(wake));
        }
    }

    /// Headless: open the clip synchronously, once.
    fn open_now(&self) {
        let mut native = self.native.lock().unwrap();
        if !self.ctl.lock().unwrap().opened() {
            let meta = backend::probe(&mut native, &self.source, &self.headers, self.kind);
            self.ctl.lock().unwrap().open(meta);
            drop(native);
            self.settle(false);
        }
    }

    /// Headless audio: playback time follows the runtime clock of this thread, if one steps.
    fn follow_clock(&self) {
        if self.kind != Kind::Audio || playback_enabled() {
            return;
        }
        let Some(now) = headless_clock() else { return };
        if self.ctl.lock().unwrap().playing {
            self.open_now();
        }
        self.ctl.lock().unwrap().advance_virtual(now);
        self.settle(false);
    }

    /// Runs on the UI thread for each element that draws this player, once per frame.
    pub(crate) fn pull(self: &Arc<Self>, surface: &NativeSurface, clock: Clock, wake: &Arc<crate::live::Wake>) -> Pulled {
        self.register(wake);
        if self.kind == Kind::Audio {
            // Nothing to draw: the element only re-runs its view when the state changes.
            match clock {
                Clock::Virtual(_) => self.open_now(),
                Clock::Display(_) => backend::drive(self),
            }
            return Pulled { active: false, generation: self.ctl.lock().unwrap().generation };
        }
        let active = match clock {
            Clock::Virtual(now) => self.pull_virtual(surface, now),
            Clock::Display(target) => backend::pull(self, surface, target),
        };
        let pulled = {
            let mut c = self.ctl.lock().unwrap();
            if c.failed.is_some() && !c.placeholder {
                c.placeholder = true;
                push_placeholder(surface);
            }
            Pulled { active: active && c.failed.is_none(), generation: c.generation }
        };
        self.settle(false);
        pulled
    }

    /// Headless: decode the frame at the virtual playback time, synchronously.
    fn pull_virtual(&self, surface: &NativeSurface, now: f64) -> bool {
        self.open_now();
        let mut native = self.native.lock().unwrap();
        let (meta, frame, shown) = {
            let mut c = self.ctl.lock().unwrap();
            let Some(meta) = c.meta.filter(|_| c.failed.is_none()) else { return false };
            c.advance_virtual(now);
            (meta, meta.frame_index(c.time), c.shown)
        };
        if shown != Some(frame) {
            let decoded = backend::decode(&mut native, &self.source, &self.headers, meta.frame_time(frame));
            let mut c = self.ctl.lock().unwrap();
            c.shown = Some(frame);
            let id = c.next_frame_id();
            if let Err(e) = decoded.and_then(|bytes| surface.push_bytes(bytes, id)) {
                c.fail(e);
            }
        }
        self.ctl.lock().unwrap().playing
    }
}

impl Meta {
    pub(crate) fn frames(&self) -> i64 {
        ((self.duration / self.frame).round() as i64).max(1)
    }
    pub(crate) fn frame_index(&self, t: f64) -> i64 {
        ((t / self.frame + 1e-6).floor() as i64).clamp(0, self.frames() - 1)
    }
    /// The middle of frame `i`, so decoders never land on a neighbour's boundary.
    pub(crate) fn frame_time(&self, i: i64) -> f64 {
        ((i as f64 + 0.5) * self.frame).min(self.duration)
    }
}

#[derive(Debug)]
pub(crate) struct Control {
    // Intent, from the handle.
    pub playing: bool,
    pub rate: f32,
    pub looping: bool,
    pub muted: bool,
    pub seek: Option<f64>,
    // Reported by pulls and ticks.
    pub time: f64,
    pub duration: Option<f64>,
    pub size: Option<(u32, u32)>,
    pub meta: Option<Meta>,
    pub ended: bool,
    pub failed: Option<String>,
    placeholder: bool,
    generation: u64,
    frame_id: u64,
    /// Headless: (runtime clock, playback time) when playback last (re)started.
    anchor: Option<(f64, f64)>,
    /// Headless: the frame index on the surface.
    shown: Option<i64>,
}

impl Default for Control {
    fn default() -> Self {
        Self {
            playing: false,
            rate: 1.0,
            looping: false,
            muted: true,
            seek: None,
            time: 0.0,
            duration: None,
            size: None,
            meta: None,
            ended: false,
            failed: None,
            placeholder: false,
            generation: 0,
            frame_id: 0,
            anchor: None,
            shown: None,
        }
    }
}

impl Control {
    pub(crate) fn state(&self) -> PlayerState {
        if let Some(e) = &self.failed {
            PlayerState::Failed(e.clone())
        } else if self.meta.is_none() {
            PlayerState::Loading
        } else if self.playing {
            PlayerState::Playing
        } else if self.ended {
            PlayerState::Ended
        } else {
            PlayerState::Ready
        }
    }
    pub(crate) fn opened(&self) -> bool {
        self.meta.is_some() || self.failed.is_some()
    }
    pub(crate) fn open(&mut self, meta: Result<Meta, String>) {
        match meta {
            Ok(meta) => {
                self.meta = Some(meta);
                self.duration = Some(meta.duration);
                self.size = meta.size;
                self.time = self.time.min(meta.duration);
                self.seek = self.seek.map(|t| t.min(meta.duration));
                self.generation += 1;
            }
            Err(e) => self.fail(e),
        }
    }
    pub(crate) fn fail(&mut self, error: String) {
        if self.failed.is_none() {
            self.failed = Some(error);
            self.playing = false;
            self.generation += 1;
        }
    }
    pub(crate) fn next_frame_id(&mut self) -> u64 {
        self.frame_id += 1;
        self.frame_id
    }

    /// Headless clock: playback time follows the runtime clock from the first pull after
    /// `play`, `seek` or a rate change.
    pub(crate) fn advance_virtual(&mut self, now: f64) {
        let Some(meta) = self.meta else { return };
        if let Some(t) = self.seek.take() {
            self.time = t.min(meta.duration);
            self.anchor = None;
        }
        if !self.playing {
            return;
        }
        // A clock behind the anchor (another runtime on this thread) restarts from here.
        let (start_clock, start_time) = match self.anchor {
            Some((clock, time)) if clock <= now => (clock, time),
            _ => *self.anchor.insert((now, self.time)),
        };
        let t = start_time + (now - start_clock).max(0.0) * self.rate as f64;
        if t < meta.duration {
            self.time = t;
        } else if self.looping && meta.duration > 0.0 {
            self.time = t % meta.duration;
        } else {
            self.time = meta.duration;
            self.playing = false;
            self.ended = true;
            self.anchor = None;
            self.generation += 1;
        }
    }

    /// Display clock: fold a player position into the state, looping or ending at the end.
    /// Returns true when the player should restart from zero.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn observe(&mut self, time: f64, at_end: bool) -> bool {
        self.time = self.duration.map_or(time, |d| time.clamp(0.0, d));
        if !(self.playing && at_end) {
            return false;
        }
        if self.looping {
            self.time = 0.0;
            return true;
        }
        self.playing = false;
        self.ended = true;
        self.generation += 1;
        false
    }
}

/// A 16:9 mid-grey frame for videos that can't play.
fn push_placeholder(surface: &NativeSurface) {
    let (w, h) = (32, 18);
    let grey = [0x46u8, 0x46, 0x46, 0xff];
    let rgba: Vec<u8> = grey.iter().copied().cycle().take(w * h * 4).collect();
    let frame = ByteFrame {
        width: w as u32,
        height: h as u32,
        format: Format::Bgra,
        color: ColorInfo { matrix: crate::native_surface::Matrix::Bt709, transfer: crate::native_surface::Transfer::Srgb },
        planes: [rgba, Vec::new()],
    };
    let _ = surface.push_bytes(frame, u64::MAX);
}

#[cfg(test)]
mod tests;
