//! What a headless run was asked to do, for the app to read (`tooling::headless_run`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicPtr, Ordering};

use crate::edit::KeyEvent;

/// The headless run this process is rendering: its script, timing and output. One per run,
/// shared by every window, so a multi-window run reads the same timeline from each.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct HeadlessRun {
    pub(crate) script: Option<PathBuf>,
    pub(crate) events: Vec<TimedEvent>,
    pub(crate) scale: f32,
    pub(crate) fps: f64,
    pub(crate) size: (f32, f32),
    pub(crate) start: f64,
    pub(crate) frames: usize,
    pub(crate) end: f64,
    pub(crate) desktop: Option<PathBuf>,
    pub(crate) windows: std::sync::Arc<std::sync::Mutex<Vec<WindowFrame>>>,
}

/// Where a `--desktop` run drew one window from time `t` on.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub struct WindowFrame {
    pub t: f64,
    pub key: String,
    /// x, y, width, height in output points (script coordinates).
    pub frame: [f32; 4],
    pub visible: bool,
    /// The key window, which script keys reach.
    pub focused: bool,
}

impl HeadlessRun {
    /// The `--desktop` backdrop, when the run composites every window over it.
    pub fn desktop(&self) -> Option<&Path> { self.desktop.as_deref() }

    /// A `--desktop` run's window frames so far: each window at the start, then one entry
    /// whenever one moves, resizes, shows, hides or takes keys. Empty otherwise.
    pub fn window_frames(&self) -> Vec<WindowFrame> { self.windows.lock().unwrap().clone() }

    /// Each window as it stands at time `t` in a `--desktop` run, in the order the app made them.
    pub fn windows_at(&self, t: f64) -> Vec<WindowFrame> {
        let frames = self.windows.lock().unwrap();
        let mut out: Vec<WindowFrame> = Vec::new();
        for f in frames.iter().filter(|f| f.t <= t + 1e-9) {
            match out.iter_mut().find(|o| o.key == f.key) {
                Some(o) => *o = f.clone(),
                None => out.push(f.clone()),
            }
        }
        out
    }

    /// The `--script` file.
    pub fn script(&self) -> Option<&Path> { self.script.as_deref() }

    /// The script's events in time order, expanded as Rune feeds them: a `tap` is a move, a
    /// down and an up; `type` is one key per character at its `every` interval.
    pub fn events(&self) -> &[TimedEvent] { &self.events }

    /// The script's `mark` events, in time order: (name, seconds).
    pub fn marks(&self) -> impl Iterator<Item = (&str, f64)> {
        self.events.iter().filter_map(|e| match &e.event { ScriptEvent::Mark(name) => Some((name.as_str(), e.t)), _ => None })
    }

    /// The time of the first mark called `name`.
    pub fn mark(&self, name: &str) -> Option<f64> { self.marks().find(|(n, _)| *n == name).map(|(_, t)| t) }

    /// The time of the script's last event (marks included), or 0 without one.
    pub fn script_end(&self) -> f64 { self.events.last().map_or(0.0, |e| e.t) }

    /// Device pixels per point (`--scale`, default 2).
    pub fn scale(&self) -> f32 { self.scale }

    /// Frames per virtual second (`--fps`, default 60).
    pub fn fps(&self) -> f64 { self.fps }

    /// The rendered window's viewport in points (`--size`, or the window's own size); the whole
    /// output with `--desktop`.
    pub fn size(&self) -> (f32, f32) { self.size }

    /// Seconds of virtual time at the first dumped frame (`--start`, marks resolved).
    pub fn start(&self) -> f64 { self.start }

    /// Frames written to `--video` or `--out` (0 for a `--snap` or `--tree` run).
    pub fn frames(&self) -> usize { self.frames }

    /// Seconds of virtual time at the last frame the run renders.
    pub fn end(&self) -> f64 { self.end }
}

/// A script event at its time in seconds.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct TimedEvent {
    pub t: f64,
    pub event: ScriptEvent,
}

/// One input (or marker) a script feeds the app.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum ScriptEvent {
    /// `{"type": "mark", "name": …}`: a named point in time with no input effect.
    Mark(String),
    /// A logical key; `repeat` is a held key's auto-repeat.
    Key { key: KeyEvent, repeat: bool },
    /// `{"type": "key-up"}`: a logical key's release.
    KeyUp(KeyEvent),
    Paste(String),
    /// A paste with HTML or RTF beside the text (`{"paste": {"text": …, "html": …}}`).
    PasteContent(crate::ClipboardContent),
    /// A registered global shortcut chord. Only its handler runs; it never reaches `on_key`.
    Shortcut(crate::Shortcut),
    /// A hardware key or modifier change (`{"physical": …}`).
    Physical(crate::PhysicalInput),
    Pointer { action: PointerAction, at: ScriptPoint },
    /// The window gaining (true) or losing focus.
    Focus(bool),
    /// `{"reopen": true}`: the app reopened, as a click on its macOS Dock icon does.
    Reopen,
    /// `{"open_url": …}`: a link the system opened with the app (`App::on_open_url`).
    OpenUrl(String),
    /// `{"type": "resize", "dw": …, "dh": …, "dur": …}`: a person resizing the window, one step
    /// a frame, (dw, dh) points from its size when the resize began.
    Resize { dw: f32, dh: f32 },
    /// A native menu item fired by name.
    Menu(String),
    /// A pop-up menu's picked item id, or None when the script dismisses it.
    PopupMenu(Option<String>),
    /// `{"type": "file-dialog", "path": …}`: the file chosen in the open file dialog, or None
    /// for `{"type": "file-dialog-cancel"}`.
    FileDialog(Option<std::path::PathBuf>),
    /// `{"drag_files": …}`: a file drag from another app moved over the window.
    DragFiles { paths: Vec<std::path::PathBuf>, at: ScriptPoint },
    /// `{"drop_files": …}`: files dropped on the window.
    DropFiles { paths: Vec<std::path::PathBuf>, at: ScriptPoint },
    /// `{"drag_files": "leave"}`: the file drag left the window.
    DragFilesLeave,
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PointerAction {
    Move,
    Down,
    Up,
    SecondaryDown,
    SecondaryUp,
    /// A wheel or trackpad delta in points.
    Scroll(f32),
    /// A trackpad release velocity in points per second.
    Fling(f32),
    /// A wheel or trackpad delta (dx, dy) in points with a horizontal part.
    ScrollXy(f32, f32),
    /// A release velocity (vx, vy) in points per second with a horizontal part.
    FlingXy(f32, f32),
    /// `{"type": "wheel"}`: one wheel delta with its modifiers and phase.
    Wheel(crate::WheelEvent),
    /// `{"type": "magnify"}`: a trackpad pinch.
    Magnify(crate::MagnifyEvent),
    /// `{"type": "rotate"}`: a two-finger rotation.
    Rotate(crate::RotateEvent),
    /// `{"type": "smart-magnify"}`: a two-finger double tap.
    SmartMagnify,
    /// A press of a button besides the primary and secondary (`"button": "middle"`).
    ButtonDown(crate::PointerButton),
    ButtonUp(crate::PointerButton),
}

/// Where a pointer event lands, as the script wrote it.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub enum ScriptPoint {
    /// Points from the window's top-left.
    At(f32, f32),
    /// The centre of the element with this key (or path suffix), found when the event runs.
    Element(String),
    /// An offset from the last pointer-down position (drags, the release of a tap).
    FromDown(f32, f32),
}

static RUN: AtomicPtr<HeadlessRun> = AtomicPtr::new(std::ptr::null_mut());

/// The headless run this process is rendering, or None in a window. Set before the app's
/// state is created, so `init`, `setup`, views and handlers can read it; a `run_windows`
/// factory runs too early to see it.
pub fn headless_run() -> Option<&'static HeadlessRun> {
    // SAFETY: only `publish` stores, and it stores leaked boxes that are never freed.
    unsafe { RUN.load(Ordering::Acquire).as_ref() }
}

/// Makes `run` the process's headless run. Each run leaks its small description so earlier
/// references stay valid; a process renders one run, or a few in tests.
pub(crate) fn publish(run: HeadlessRun) {
    RUN.store(Box::into_raw(Box::new(run)), Ordering::Release);
}
