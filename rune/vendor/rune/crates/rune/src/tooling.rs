//! Agent tooling: headless deterministic rendering, synthetic input scripts, element-tree dumps,
//! motion-lint traces and frame strips. Same renderer as the window, fixed virtual clock.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::compose::Compositor;
use crate::edit::{Key, KeyEvent, Mods};
use crate::el::{El, Tx};
use crate::render::write_png;
use crate::runtime::{Cx, Env, Runtime, Waker};
use crate::text::TextSystem;

pub use crate::tooling::headless_run::{HeadlessRun, PointerAction, ScriptEvent, ScriptPoint, TimedEvent, WindowFrame, headless_run};

pub struct AppSpec<S: 'static> {
    pub title: &'static str,
    pub size: (f32, f32),
    /// The smallest window the user can resize to, in points (None: 420×400).
    pub min_size: Option<(f32, f32)>,
    pub init: fn() -> S,
    pub view: fn(&S, &mut Cx<S>) -> El,
    /// Named states for snapshots: each modifies the initial state.
    pub scenes: &'static [(&'static str, fn(&mut S))],
    /// Font files replacing the built-in Inter (empty keeps it), and the sans/mono family names
    /// (CSS lists; `ui-monospace` stands for [`Family::SystemMono`](crate::Family::SystemMono)'s
    /// families, so mono spans can be SF Mono).
    pub fonts: &'static [&'static [u8]],
    pub families: (&'static str, &'static str),
    /// Called once with a waker other threads use when new data arrives.
    pub setup: Option<fn(&mut S, Waker)>,
    /// Called at the start of every frame with seconds since start (virtual when headless);
    /// true when state changed.
    pub poll: Option<fn(&mut S, f64, &mut Tx) -> bool>,
    /// Keys unhandled by the focused control, or every key while nothing has focus
    /// (`Tx::focused` tells which); true consumes the key. The rune README's "Key routing"
    /// lists the whole order.
    pub on_key: Option<fn(&mut S, &KeyEvent, &mut Tx) -> bool>,
}

impl<S: 'static> AppSpec<S> {
    /// The required parts; everything else at its default. Literals that set a few more fields
    /// end with `..AppSpec::new(..)`, so new fields don't break them.
    pub fn new(title: &'static str, size: (f32, f32), init: fn() -> S, view: fn(&S, &mut Cx<S>) -> El) -> Self {
        AppSpec {
            title,
            size,
            min_size: None,
            init,
            view,
            scenes: &[],
            fonts: &[],
            families: ("Inter", "Inter"),
            setup: None,
            poll: None,
            on_key: None,
        }
    }
}

/// Window frame options. Elements marked `.window_drag()` move the window on every platform;
/// a double press there zooms (macOS follows System Settings' titlebar double-click action;
/// elsewhere it toggles maximised).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chrome {
    /// macOS: the content view fills the window under a transparent, title-less titlebar (the
    /// app draws its own titlebar area). Off: a standard titled window. Other platforms keep
    /// their native frame either way.
    pub full_size_content: bool,
    /// macOS: the close button's top-left, in points from the window's top-left, kept there
    /// across resizes and full screen (Electron's `trafficLightPosition`). `None` leaves
    /// AppKit's placement. The three buttons keep AppKit's own size and spacing.
    pub traffic_lights: Option<(f32, f32)>,
    /// macOS: only `.window_drag()` regions move the window, so content under a full-size
    /// titlebar stays clickable (Electron's app regions). Off: AppKit's titlebar band drags too.
    /// Applies only to this window's content view when borderless or full-size-content.
    /// Ordinary titled windows and NSPanel overlays keep their native behaviour.
    pub drag_regions: bool,
}

impl Chrome {
    pub const DEFAULT: Chrome = Chrome { full_size_content: true, traffic_lights: None, drag_regions: false };
    /// Headless `--chrome` overlay and checks: macOS 26 draws 14 pt buttons on a 23 pt pitch.
    pub const TRAFFIC_LIGHT: f32 = 14.0;
    pub const TRAFFIC_PITCH: f32 = 23.0;

    /// Where macOS will draw the three buttons (x, y, w, h in points), assuming macOS 26 metrics
    /// and AppKit's default origin (7, 6) when unpositioned.
    pub fn traffic_light_rects(&self) -> Option<[[f32; 4]; 3]> {
        if !self.full_size_content && self.traffic_lights.is_none() {
            return None;
        }
        let (x, y) = self.traffic_lights.unwrap_or((7.0, 6.0));
        Some(std::array::from_fn(|i| [x + i as f32 * Self::TRAFFIC_PITCH, y, Self::TRAFFIC_LIGHT, Self::TRAFFIC_LIGHT]))
    }

    /// The trailing edge of [`Chrome::traffic_light_rects`]' zoom button, or 0 without them:
    /// what [`Cx::titlebar_inset`](crate::Cx::titlebar_inset) reports in a headless run.
    pub fn titlebar_inset(&self) -> f32 {
        self.traffic_light_rects().map_or(0.0, |r| r[2][0] + r[2][2])
    }
}

impl Default for Chrome {
    fn default() -> Self {
        Chrome::DEFAULT
    }
}

static EXTRA: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
static APP_ID: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

/// The application id the window reports to the desktop (Wayland `app_id`, X11 `WM_CLASS`), so
/// compositors match it to the app's `.desktop` entry, icon and window rules. Call before `run`.
pub fn set_app_id(id: &'static str) {
    let _ = APP_ID.set(id);
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn app_id() -> Option<&'static str> {
    APP_ID.get().copied()
}

/// Command-line arguments Rune didn't recognise, in order, for the app to interpret. Ready
/// before any window: called before `run` (to pick the file a window opens, say), it reads
/// Rune's flags from the process's arguments as `run` will, `--help` and a malformed flag
/// included. The first read wins, so an app that passes `App::args` reads this after `run`
/// starts.
pub fn extra_args() -> &'static [String] {
    if EXTRA.get().is_none() { Args::parse(); }
    EXTRA.get().map_or(&[], |v| v.as_slice())
}

pub(crate) fn runtime_for<S: 'static>(spec: &AppSpec<S>, state: S, size: (f32, f32), env: Env, waker: Waker) -> Runtime<S> {
    let text = TextSystem::for_window(spec.fonts, spec.families);
    let mut rt = Runtime::with_text(state, spec.view, size, env, text);
    rt.poll = spec.poll;
    rt.on_key = spec.on_key;
    rt.window_title.start(spec.title);
    rt.set_live_waker(waker.clone());
    #[cfg(feature = "hot")]
    if crate::hot::enabled() {
        rt.hot = Some(crate::hot::Session::connect(waker.clone()));
    }
    if let Some(f) = spec.setup {
        f(&mut rt.state, waker);
    }
    rt
}

#[path = "tooling/args.rs"]
mod args;
#[path = "tooling/crop.rs"]
mod crop;
#[path = "tooling/eased_input.rs"]
mod eased_input;
#[path = "tooling/headless.rs"]
mod headless;
#[path = "tooling/html.rs"]
pub(crate) mod html;
#[path = "tooling/key_names.rs"]
mod key_names;
#[path = "tooling/script.rs"]
mod script;
#[path = "tooling/strip.rs"]
mod strip;
#[path = "tooling/trace.rs"]
mod trace;

pub use args::{Args, is_headless};
pub(crate) use args::*;
pub use headless::headless;
pub(crate) use headless::*;
pub(crate) use key_names::{key_named, script_key_base, script_mods};
use script::parse_script;
pub(crate) use strip::*;
pub(crate) use trace::{FrameTiming, Trace, tree_json};
use trace::r2;

/// Opens the app's window and quits when it closes (`App` for other lifetimes), or renders
/// headless for the tooling arguments.
pub fn run<S: 'static>(spec: AppSpec<S>) {
    #[cfg(feature = "hot")]
    crate::hot::run(spec, Args::parse());
    #[cfg(not(feature = "hot"))]
    crate::app::App::new(spec).run();
}

/// Create independent windows from a display-aware factory. Headless tools invoke the same
/// factory with one virtual display and select one retained tree using `--window KEY`.
/// Windows may have different state types: return `AnyWindowSpec`s (`WindowSpec::boxed`).
pub fn run_windows<I: IntoIterator<Item: Into<crate::AnyWindowSpec>>>(factory: impl FnOnce(&[crate::Display]) -> I + 'static) {
    run_windows_with(crate::RunOptions::default(), factory);
}

/// Run a window factory with process-level options, such as macOS accessory activation.
pub fn run_windows_with<I: IntoIterator<Item: Into<crate::AnyWindowSpec>>>(options: crate::RunOptions, factory: impl FnOnce(&[crate::Display]) -> I + 'static) {
    started(try_run_windows_with(options, factory));
}

/// `run_windows_with`, returning an error instead of panicking when the event loop can't start
/// (no display over ssh or in CI, no Wayland or X11 server) or fails.
pub fn try_run_windows_with<I: IntoIterator<Item: Into<crate::AnyWindowSpec>>>(options: crate::RunOptions, factory: impl FnOnce(&[crate::Display]) -> I + 'static) -> Result<(), String> {
    run_window_factory(options, window_args(None, Args::parse), erase(factory))
}

/// Run a window factory with explicit Rune arguments, without reading the host CLI.
/// Pass `[]` to isolate an embedding application's flags before display discovery.
pub fn run_windows_with_args<I: IntoIterator<Item: Into<crate::AnyWindowSpec>>>(options: crate::RunOptions, args: impl IntoIterator<Item = String>, factory: impl FnOnce(&[crate::Display]) -> I + 'static) {
    started(try_run_windows_with_args(options, args, factory));
}

/// `run_windows_with_args`, returning an error instead of panicking when the event loop can't
/// start or fails.
pub fn try_run_windows_with_args<I: IntoIterator<Item: Into<crate::AnyWindowSpec>>>(options: crate::RunOptions, args: impl IntoIterator<Item = String>, factory: impl FnOnce(&[crate::Display]) -> I + 'static) -> Result<(), String> {
    run_window_factory(options, window_args(Some(args.into_iter().collect()), Args::parse), erase(factory))
}

fn started(result: Result<(), String>) {
    if let Err(error) = result { panic!("{error}"); }
}

type Factory = Box<dyn FnOnce(&[crate::Display]) -> Vec<crate::AnyWindowSpec>>;

fn erase<I: IntoIterator<Item: Into<crate::AnyWindowSpec>>>(factory: impl FnOnce(&[crate::Display]) -> I + 'static) -> Factory {
    Box::new(move |displays| factory(displays).into_iter().map(Into::into).collect())
}

pub(crate) fn window_args(args: Option<Vec<String>>, process: impl FnOnce() -> Args) -> Args {
    args.map_or_else(process, Args::parse_from)
}

fn run_window_factory(options: crate::RunOptions, args: Args, factory: Factory) -> Result<(), String> {
    if args.headless() {
        crate::render::discard_prewarm();
        let displays = desktop(&args, read_script(&args).as_ref()).displays();
        headless_window(factory(&displays), &displays, args, options)
    } else {
        crate::window::run_windows(factory, args, options)
    }
}

/// Lists the windows, or renders the one `--window KEY` selects (the first by default) with its
/// own state. Windows that apps added with `App::window` are selectable too.
fn headless_window(windows: Vec<crate::AnyWindowSpec>, displays: &[crate::Display], args: Args, options: crate::RunOptions) -> Result<(), String> {
    let mut windows = crate::windows::flatten(windows);
    crate::windows::validate(&windows).unwrap_or_else(|e| die(&e));
    if args.list_windows {
        for window in windows { println!("{}", window.key()); }
        return Ok(());
    }
    let index = match args.window.as_deref() {
        Some(key) => windows.iter().position(|w| w.key() == key).unwrap_or_else(|| die(&format!("no window {key:?}; use --list-windows"))),
        None if !windows.is_empty() => 0,
        None => die("the app created no windows"),
    };
    if args.desktop.is_some() {
        crate::tooling::interaction::gate::headless();
        crate::desktop::run(windows, index, args, options.accessory);
        return Ok(());
    }
    let mut window = windows.remove(index);
    let args = window.resolve_args(args);
    if !args.headless() {
        return crate::window::run_windows(Box::new(move |_| vec![window]), args, options);
    }
    let scale = displays.iter().find(|d| d.primary).unwrap_or(&displays[0]).scale;
    window.headless(args, scale as f32);
    Ok(())
}

/// `run` with a compositor drawing every frame (see `compose`); `App::compositor` for other lifetimes.
pub fn run_with<S: 'static>(spec: AppSpec<S>, compositor: impl Compositor<S>) {
    crate::app::App::new(spec).compositor(compositor).run();
}

pub fn initial_state<S: 'static>(spec: &AppSpec<S>, scene: Option<&str>) -> S {
    with_scene(spec, (spec.init)(), scene)
}

/// `state` with the named scene applied.
pub(crate) fn with_scene<S: 'static>(spec: &AppSpec<S>, mut s: S, scene: Option<&str>) -> S {
    if let Some(name) = scene {
        let Some((_, f)) = spec.scenes.iter().find(|(n, _)| *n == name) else {
            let names: Vec<_> = spec.scenes.iter().map(|(n, _)| *n).collect();
            die(&format!("no scene {name:?}; scenes: {}", names.join(", ")));
        };
        f(&mut s);
    }
    s
}

// -------------------------------------------------------------------------------------------
// Scripts

#[derive(Clone, Debug)]
pub(crate) enum Pos {
    Xy(f32, f32),
    Key(String),
    /// Relative to the last pointer-down position.
    Rel(f32, f32),
}

#[derive(Clone, Debug)]
pub(crate) enum Kind {
    /// A named time with no input effect.
    Mark(String),
    Focus(bool),
    /// The app reopened (macOS: its Dock icon clicked).
    Reopen,
    /// A link the system opened with the app (`App::on_open_url`).
    OpenUrl(String),
    Menu(String),
    /// Answers the open pop-up menu: an item's id, or `None` to dismiss it.
    PopupMenu(Option<String>),
    /// Answers the open file dialog: the chosen path, or `None` to cancel it.
    FileDialog(Option<PathBuf>),
    Physical(crate::PhysicalInput),
    /// A registered global shortcut, delivered only to its handler.
    Shortcut(crate::Shortcut),
    Move,
    Down,
    Up,
    SecondaryDown,
    SecondaryUp,
    /// The key and whether it is an auto-repeat.
    Key(KeyEvent, bool),
    /// A key press with its physical code (`{"type": "key", "code": …}`), as a native host sends it.
    PhysicalKey(crate::PhysicalInput, KeyEvent, Option<char>),
    Paste(crate::ClipboardContent),
    /// A wheel delta (dx, dy) in points.
    Wheel(f32, f32),
    /// A release velocity (vx, vy) in points per second.
    Fling(f32, f32),
    /// A file drag from another app (`drag_files`, `drop_files`).
    Files(FileStep),
    /// `{"type": "wheel"}`: one delta with modifiers and a phase, at the pointer.
    WheelEvent(crate::WheelEvent),
    Magnify(crate::MagnifyEvent),
    Rotate(crate::RotateEvent),
    SmartMagnify,
    /// A button besides the primary and secondary (`"button": "middle"`).
    ButtonDown(crate::PointerButton),
    ButtonUp(crate::PointerButton),
    KeyUp(KeyEvent),
    /// A person dragging the window's corner, (dw, dh) from the size where (0, 0) started.
    Resize(f32, f32),
}

impl Kind {
    /// A press that later `Rel` positions are relative to.
    pub(crate) fn is_down(&self) -> bool { matches!(self, Kind::Down | Kind::SecondaryDown | Kind::ButtonDown(_)) }
    pub(crate) fn is_up(&self) -> bool { matches!(self, Kind::Up | Kind::SecondaryUp | Kind::ButtonUp(_)) }
    /// Without a position it lands where the pointer is, not at the last press.
    pub(crate) fn at_pointer(&self) -> bool {
        matches!(self, Kind::Wheel(..) | Kind::Fling(..) | Kind::WheelEvent(_) | Kind::Magnify(_) | Kind::Rotate(_) | Kind::SmartMagnify)
    }
}

/// The canvas input of a script (`canvas_input.rs`) at window point (x, y); false for any
/// other kind.
pub(crate) fn canvas_input<S: 'static>(rt: &mut Runtime<S>, kind: &Kind, (x, y): (f32, f32)) -> bool {
    match kind {
        Kind::WheelEvent(ev) => rt.wheel_event(ev.at(x, y)),
        Kind::Magnify(ev) => rt.magnify_event(ev.at(x, y)),
        Kind::Rotate(ev) => rt.rotate_event(ev.at(x, y)),
        Kind::SmartMagnify => rt.smart_magnify(x, y),
        Kind::ButtonDown(b) => rt.button_down(x, y, *b),
        Kind::ButtonUp(b) => rt.button_up(x, y, *b),
        Kind::KeyUp(k) => { rt.key_up(k.clone()); }
        Kind::PhysicalKey(event, k, base) => rt.physical_key_input(*event, k.clone(), *base),
        _ => return false,
    }
    true
}

#[derive(Clone, Debug)]
pub(crate) enum FileStep {
    Hover(Vec<PathBuf>),
    Drop(Vec<PathBuf>),
    Leave,
}

/// A scripted wheel delta: vertical-only ones take the plain vertical path, unless an
/// `on_wheel` handler may take them.
pub(crate) fn wheel<S: 'static>(rt: &mut Runtime<S>, (x, y): (f32, f32), dx: f32, dy: f32) {
    if dx == 0.0 && !rt.hears_wheel() { rt.wheel(x, y, dy) } else { rt.wheel_xy(x, y, dx, dy) }
}

fn script_phase(e: &Value) -> crate::GesturePhase {
    match e["phase"].as_str().unwrap_or("moved") {
        "began" => crate::GesturePhase::Began,
        "moved" => crate::GesturePhase::Moved,
        "ended" => crate::GesturePhase::Ended,
        other => die(&format!("unknown gesture phase {other}; use began, moved or ended")),
    }
}

/// A scripted release velocity: vertical-only ones take the plain vertical path.
pub(crate) fn fling<S: 'static>(rt: &mut Runtime<S>, (x, y): (f32, f32), vx: f32, vy: f32) {
    if vx == 0.0 { rt.fling(x, y, vy) } else { rt.fling_xy(x, y, vx, vy) }
}

#[derive(Clone, Debug)]
pub(crate) struct Ev {
    pub t: f64,
    pub kind: Kind,
    pub pos: Pos,
}

impl Ev {
    /// The event as apps read it through `headless_run`.
    pub(crate) fn public(&self) -> TimedEvent {
        let at = || match &self.pos {
            Pos::Xy(x, y) => ScriptPoint::At(*x, *y),
            Pos::Key(key) => ScriptPoint::Element(key.clone()),
            Pos::Rel(dx, dy) => ScriptPoint::FromDown(*dx, *dy),
        };
        let pointer = |action| ScriptEvent::Pointer { action, at: at() };
        let event = match &self.kind {
            Kind::Mark(name) => ScriptEvent::Mark(name.clone()),
            Kind::Focus(focused) => ScriptEvent::Focus(*focused),
            Kind::Reopen => ScriptEvent::Reopen,
            Kind::OpenUrl(url) => ScriptEvent::OpenUrl(url.clone()),
            Kind::Menu(item) => ScriptEvent::Menu(item.clone()),
            Kind::PopupMenu(picked) => ScriptEvent::PopupMenu(picked.clone()),
            Kind::FileDialog(path) => ScriptEvent::FileDialog(path.clone()),
            Kind::Physical(input) => ScriptEvent::Physical(*input),
            Kind::Shortcut(chord) => ScriptEvent::Shortcut(chord.clone()),
            Kind::Move => pointer(PointerAction::Move),
            Kind::Down => pointer(PointerAction::Down),
            Kind::Up => pointer(PointerAction::Up),
            Kind::SecondaryDown => pointer(PointerAction::SecondaryDown),
            Kind::SecondaryUp => pointer(PointerAction::SecondaryUp),
            Kind::Key(key, repeat) => ScriptEvent::Key { key: key.clone(), repeat: *repeat },
            Kind::PhysicalKey(event, key, _) => ScriptEvent::Key {
                key: key.clone(), repeat: matches!(event, crate::PhysicalInput::Key { repeat: true, .. }),
            },
            Kind::Paste(content) if content.is_rich() => ScriptEvent::PasteContent(content.clone()),
            Kind::Paste(content) => ScriptEvent::Paste(content.text().to_owned()),
            Kind::Wheel(0.0, dy) => pointer(PointerAction::Scroll(*dy)),
            Kind::Wheel(dx, dy) => pointer(PointerAction::ScrollXy(*dx, *dy)),
            Kind::Fling(0.0, v) => pointer(PointerAction::Fling(*v)),
            Kind::Fling(vx, vy) => pointer(PointerAction::FlingXy(*vx, *vy)),
            Kind::Files(FileStep::Hover(paths)) => ScriptEvent::DragFiles { paths: paths.clone(), at: at() },
            Kind::Files(FileStep::Drop(paths)) => ScriptEvent::DropFiles { paths: paths.clone(), at: at() },
            Kind::Files(FileStep::Leave) => ScriptEvent::DragFilesLeave,
            Kind::WheelEvent(ev) => pointer(PointerAction::Wheel(*ev)),
            Kind::Magnify(ev) => pointer(PointerAction::Magnify(*ev)),
            Kind::Rotate(ev) => pointer(PointerAction::Rotate(*ev)),
            Kind::SmartMagnify => pointer(PointerAction::SmartMagnify),
            Kind::ButtonDown(b) => pointer(PointerAction::ButtonDown(*b)),
            Kind::ButtonUp(b) => pointer(PointerAction::ButtonUp(*b)),
            Kind::KeyUp(key) => ScriptEvent::KeyUp(key.clone()),
            Kind::Resize(dw, dh) => ScriptEvent::Resize { dw: *dw, dh: *dh },
        };
        TimedEvent { t: self.t, event }
    }
}

pub(crate) fn read_script(args: &Args) -> Option<Value> {
    args.script.as_ref().map(|path| {
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| die(&format!("{}: {e}", path.display())));
        serde_json::from_str(&text).unwrap_or_else(|e| die(&format!("{}: {e}", path.display())))
    })
}

pub(crate) fn load_script(script: &Value) -> Vec<Ev> {
    let list = match script { Value::Object(o) => o.get("events").cloned().unwrap_or_else(|| json!([])), v => v.clone() };
    parse_script(list)
}

/// The script's displays and pointer, which headless `place()` resolves against. Without
/// scripted displays, a `--desktop` run has one display the size of its output: `--size`, else
/// the backdrop's pixels at `--scale`.
pub(crate) fn desktop(args: &Args, script: Option<&Value>) -> crate::desktop::displays::VirtualDesktop {
    let scale = f64::from(args.scale.unwrap_or(2.0));
    let size = args.desktop.as_ref().map(|path| match args.size {
        Some((w, h)) => (f64::from(w), f64::from(h)),
        None => {
            let (w, h) = ::image::image_dimensions(path).unwrap_or_else(|e| die(&format!("{}: {e}", path.display())));
            (f64::from(w) / scale, f64::from(h) / scale)
        }
    });
    crate::desktop::displays::VirtualDesktop::from_script_sized(script, scale, size).unwrap_or_else(|e| die(&e))
}

/// Feeds a script's file drag step to `rt`, at `at` in window points.
pub(crate) fn file_input<S: 'static>(rt: &mut Runtime<S>, step: &FileStep, at: (f32, f32)) {
    match step {
        FileStep::Hover(paths) => rt.drag_files(paths.clone(), Some(at)),
        FileStep::Drop(paths) => rt.drop_files(paths.clone(), Some(at)),
        FileStep::Leave => rt.drag_files_leave(),
    }
}

#[cfg(test)]
include!("tooling/parser_tests.rs");

pub(crate) mod headless_run;
pub(crate) mod interaction;
pub(crate) mod perf_warn;
pub(crate) mod record;
#[cfg(test)]
pub(crate) mod test_alloc;
#[cfg(test)]
#[path = "tooling/window_tests.rs"]
mod window_tests;

#[cfg(test)]
#[path = "tooling/tests.rs"]
mod tests;
