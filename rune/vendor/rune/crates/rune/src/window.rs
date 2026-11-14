//! One event loop, independent window lifetime hosts. App/AppHandle and multi-window
//! factories share the same input, rendering, activation and close/reopen paths.

use std::sync::{Arc, Mutex, Weak, atomic::{AtomicBool, AtomicU32, Ordering}};
use std::{cell::RefCell, rc::Rc};
use std::time::Duration;
use crate::time::Instant;

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize, PhysicalPosition};
use winit::event::{ElementState, Ime as WIme, MouseButton, StartCause, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{Theme, Window, WindowId, WindowLevel};
use winit::raw_window_handle::HasWindowHandle;

use crate::app::{Shared, WindowRequest};
use crate::compose::{Compositor, Frame};
use crate::frame::clock::FrameClock;
use crate::edit::Ime;
use crate::el::{Shell, Tx};
use crate::render::{Gpu, Renderer};
use crate::runtime::{Env, Runtime, TitlebarAction, Waker};
use crate::frame::surface_stats::{Pacing, SurfaceMeter, SurfaceStats};
use crate::tooling::{AppSpec, Args, Chrome, runtime_for};
use crate::windows::{AnyWindowSpec, Display, WindowOptions, WindowSpec};

#[cfg(target_os = "macos")]
mod launch_drawables;
#[cfg(target_os = "macos")]
use launch_drawables::LaunchDrawables;
mod chrome;
use chrome::{frame_chrome, place_chrome, sync_chrome, titlebar_double_click};
mod create;
mod native;
use native::Native;
mod win;
use win::{FirstFrame, Win};
mod draw;
mod redraw;
use redraw::{Redraw, RedrawEffect, RedrawEvent};
#[cfg(target_os = "windows")]
use redraw::{Steady, SteadyEvent};
mod presented;
pub(crate) use presented::Presented;
use presented::composing;
mod platform_sync;
use platform_sync::sync_platform;
mod shell_requests;
mod keys;
pub(crate) use keys::{key_base, map_key_event, map_mods};
mod any_host;
mod app_events;
mod event_loop;
mod events;
mod file_drop;
pub(crate) mod frame;
mod handle;
pub use handle::WindowHandle;
use handle::native_window_number;
mod lifecycle;
#[cfg(target_os = "macos")]
mod mac;
mod pacing;
pub(crate) use pacing::{
    HiddenPace, Missed, TimerFrame, OCCLUDED_MAX, OCCLUDED_RETRY, TIMEOUT_RETRY, TIMER_FRAME,
    acquire, acquire_checked, timer_deadline, timer_wants_frame,
};
#[cfg(test)]
pub(crate) use pacing::{HIDDEN_REFRESH, TIMER_FRAME_STALL};
#[cfg(test)]
use pacing::wall_wake_deadline;
mod stepping;
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
mod float_on_open;
#[cfg(not(target_arch = "wasm32"))]
pub use event_loop::run;
pub(crate) use event_loop::run_windows;
#[cfg(test)]
#[path = "window/covered_tests.rs"]
mod covered;
#[cfg(all(test, target_os = "macos"))]
#[path = "window/occluded_surface_tests.rs"]
mod occluded_surface;
#[cfg(test)]
mod tests;

pub(crate) enum WakeEvent {
    DisplayTick(usize),
    App(usize), Live(usize), Window(usize, WindowRequest), Focus(usize),
    Position(usize, (i32, i32)), Level(usize, bool),
    Place(usize, crate::windows::PlaceRequest),
    Frame(usize, crate::window::frame::FrameRequest),
    /// The app was reopened (macOS: its Dock icon clicked; Windows: launched again while it
    /// runs, see `single_instance`); for `Windows`, not one host.
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
    Reopen,
    /// The system asks the app to quit (`crate::quit`): every window's `App::on_quit` runs;
    /// with `true` the session ends now and the windows close.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    Quit(bool),
    /// An assistive client asked something of a surface's accessibility tree (`crate::a11y`):
    /// the host answers at the end of the turn this wakes.
    #[cfg_attr(not(feature = "accessibility"), allow(dead_code))]
    A11y(usize),
    /// A native menu's modal loop holds the event loop: run its idle turn (`Windows::turn`).
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    Turn,
}

/// What `Windows` needs of a host, whatever its state type: an app window or an overlay.
pub(crate) trait AnyHost {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, _cause: StartCause) {}
    fn wake_deadline(&self) -> Option<Instant> { None }
    fn resumed(&mut self, event_loop: &ActiveEventLoop);
    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: WakeEvent);
    fn owns(&self, id: WindowId) -> bool;
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent);
    /// Draws what has no redraw event (a layer surface); true while it needs more frames.
    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) -> bool { false }
    /// The turn ends: the next frame it wants is asked for, or a paint put off runs (`redraw`).
    fn turn_ends(&mut self, _event_loop: &ActiveEventLoop, _now: Instant) {}
    fn finished(&self) -> bool { false }
    /// The OS is ending the app without Rune asking: run close hooks before the hosts drop.
    fn terminate(&mut self) {}
    /// The app starts to quit: runs `App::on_quit`, unless the window already closed.
    fn quit_hook(&mut self) {}
    /// Ends a tracking pop-up menu and answers waiting ones `None`, before the host drops or
    /// the loop exits.
    fn cancel_menus(&mut self) {}
    /// False for hosts that must not keep the loop running alone (overlays).
    fn keeps_loop(&self) -> bool { true }
    /// An app window's key; overlays have none.
    fn key(&self) -> Option<&str> { None }
    /// How reopening the app sees it; overlays never come back.
    fn reopen_candidate(&self) -> crate::platform::reopen::Candidate { Default::default() }
    /// Runs `App::on_reopen` as an update; true when it handled the reopen.
    fn reopen_hook(&mut self, _event_loop: &ActiveEventLoop, _has_visible_windows: bool) -> bool { false }
    /// Applies what was asked of it through its handle since the last wake, now rather than
    /// when that wake arrives.
    fn apply_pending(&mut self, _event_loop: &ActiveEventLoop) {}
    /// Shows, restores and activates it as the default reopen.
    fn reopen(&mut self, _event_loop: &ActiveEventLoop) {}
}

/// Builds every host, attaching its shortcut and menu guards, before any resumes: a window's
/// first updates drain on resume and may register on a later window.
fn start_all<T, H>(items: Vec<T>, mut build: impl FnMut(usize, T) -> Option<H>, resume: impl FnMut(&mut H)) -> Vec<Option<H>> {
    let mut hosts: Vec<_> = items.into_iter().enumerate().map(|(slot, item)| build(slot, item)).collect();
    hosts.iter_mut().flatten().for_each(resume);
    hosts
}

type Factory = Box<dyn FnOnce(&[Display]) -> Vec<AnyWindowSpec>>;

struct Windows {
    factory: Option<Factory>,
    args: Args,
    proxy: EventLoopProxy<WakeEvent>,
    hosts: Vec<Option<Box<dyn AnyHost>>>,
    // Weak pool preserves release_gpu_when_closed when the last GPU owner closes.
    gpu: Rc<RefCell<Weak<Gpu>>>,
}
impl Windows {
    /// Starts the windows `crate::open_window` asked for this turn, in fresh slots, each like
    /// a window the factory returned but without `--scene` or `--size`, which describe the
    /// launch's windows.
    fn open_requested(&mut self, event_loop: &ActiveEventLoop) {
        if self.factory.is_some() { return; }
        loop {
            let specs = crate::windows::flatten(crate::windows::take_opened());
            if specs.is_empty() { return; }
            let wayland = is_wayland(event_loop);
            for spec in specs {
                if self.hosts.iter().flatten().any(|h| h.key() == Some(spec.key())) {
                    eprintln!("rune: a window keyed {:?} is already open", spec.key());
                    continue;
                }
                let mut args = self.args.clone();
                (args.scene, args.size) = (None, None);
                let mut overlays = Vec::new();
                let slot = self.hosts.len();
                let host = spec.start(args, Start { proxy: self.proxy.clone(), slot, gpu: self.gpu.clone(), wayland, traced: false, overlays: &mut overlays });
                self.hosts.push(host);
                if let Some(Some(host)) = self.hosts.get_mut(slot) { host.resumed(event_loop); }
                self.connect_overlays(overlays, event_loop);
            }
        }
    }

    fn connect_overlays(&mut self, overlays: Vec<Box<dyn crate::overlay::PendingOverlay>>, event_loop: &ActiveEventLoop) {
        for mut overlay in overlays {
            let (slot, proxy) = (self.hosts.len(), self.proxy.clone());
            let frame_proxy = proxy.clone();
            overlay.connect(Arc::new(move || { let _ = proxy.send_event(WakeEvent::App(slot)); }),
                Arc::new(move || { let _ = frame_proxy.send_event(WakeEvent::DisplayTick(slot)); }), self.gpu.clone());
            overlay.resumed(event_loop);
            self.hosts.push(Some(overlay));
        }
    }

    fn cleanup(&mut self, event_loop: &ActiveEventLoop) {
        if !event_loop.exiting() { self.open_requested(event_loop); }
        for host in &mut self.hosts {
            if let Some(h) = host.as_mut().filter(|h| h.finished()) {
                h.cancel_menus();
                *host = None;
            }
        }
        // Another window's quit: a menu still tracking would hold the loop open.
        if event_loop.exiting() {
            for host in self.hosts.iter_mut().flatten() { host.cancel_menus(); }
        }
        if !self.hosts.iter().flatten().any(|h| h.keeps_loop()) {
            // The app ends with its last main window: windows still open close as their close
            // button would, once the quit's hooks ran. A quit already asked to exit and skips
            // their close hooks.
            if !event_loop.exiting() {
                self.quit(event_loop, false);
                for host in self.hosts.iter_mut().flatten() { host.terminate(); }
            }
            event_loop.exit();
        }
    }
}

/// What a window needs from the shell to start, whatever its state type.
pub(crate) struct Start<'a> {
    proxy: EventLoopProxy<WakeEvent>,
    slot: usize,
    gpu: Rc<RefCell<Weak<Gpu>>>,
    wayland: bool,
    /// Whether this window records `--native` traces.
    traced: bool,
    overlays: &'a mut Vec<Box<dyn crate::overlay::PendingOverlay>>,
}

/// Starts one window's host, or renders it headless when its own arguments ask for that.
pub(crate) fn start<S: 'static>(mut spec: WindowSpec<S>, handle: Option<WindowHandle>, keeps_loop: bool, mut inherited: Args, cx: Start<'_>) -> Option<Box<dyn AnyHost>> {
    if inherited.native && !cx.traced {
        inherited.trace = None;
        inherited.frames = None;
    }
    let args = spec.app.resolve_args(inherited);
    if args.headless() {
        if let Some((w, h)) = spec.options.physical_size {
            let scale = args.scale.unwrap_or(2.0);
            spec.app.spec.size = (w as f32 / scale, h as f32 / scale);
        }
        let transparent = spec.options.transparent;
        spec.app.window_options = spec.options;
        crate::tooling::headless_configured(spec.app, args, transparent);
        return None;
    }
    crate::video::enable_playback();
    cx.overlays.append(&mut spec.app.overlays);
    let handle = handle.unwrap_or_else(|| WindowHandle::new(&spec.key));
    let open_urls = crate::open_url::incoming::listen(&spec.app);
    let mut host = Host::new(spec, handle, args, cx.proxy, cx.slot, cx.gpu, cx.wayland);
    (host.keeps_loop, host._open_urls) = (keeps_loop, open_urls);
    Some(Box::new(host))
}

impl ApplicationHandler<WakeEvent> for Windows {
    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        let _handling = crate::quit::handling();
        for host in self.hosts.iter_mut().flatten() { host.new_events(event_loop, cause); }
        // Before resumed there are no hosts to clean up.
        if self.factory.is_none() { self.cleanup(event_loop); }
    }
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let _handling = crate::quit::handling();
        let Some(factory) = self.factory.take() else { return };
        let primary = event_loop.primary_monitor();
        let displays: Vec<_> = event_loop.available_monitors().map(|m| Display {
            id: Some(crate::DisplayId::from_monitor(m.clone(), is_wayland(event_loop))),
            name: m.name(), position: (m.position().x, m.position().y),
            size: (m.size().width, m.size().height), scale: m.scale_factor(), primary: primary.as_ref() == Some(&m),
        }).collect();
        let specs = crate::windows::flatten(factory(&displays));
        crate::windows::validate(&specs).expect("window keys");
        let trace_window = self.args.window.clone().or_else(|| specs.first().map(|s| s.key().to_owned()));
        let mut overlays = Vec::new();
        let wayland = is_wayland(event_loop);
        self.hosts = start_all(specs, |slot, spec| {
            let traced = trace_window.as_deref() == Some(spec.key());
            spec.start(self.args.clone(), Start { proxy: self.proxy.clone(), slot, gpu: self.gpu.clone(), wayland, traced, overlays: &mut overlays })
        }, |host| host.resumed(event_loop));
        self.connect_overlays(overlays, event_loop);
        #[cfg(target_os = "macos")]
        {
            let proxy = self.proxy.clone();
            crate::platform::reopen::install(move || proxy.send_event(WakeEvent::Reopen).is_ok());
        }
        #[cfg(target_os = "windows")]
        {
            let proxy = self.proxy.clone();
            crate::open_url::instance::on_relaunch(move || proxy.send_event(WakeEvent::Reopen).is_ok());
        }
        let proxy = self.proxy.clone();
        crate::quit::install(move |ending| proxy.send_event(WakeEvent::Quit(ending)).is_ok());
        crate::open_url::incoming::settle();
        // A window opened at launch took it; a GPU kept for a later one would sit idle until then.
        crate::render::discard_prewarm();
        self.cleanup(event_loop);
    }
    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: WakeEvent) {
        let _handling = crate::quit::handling();
        let slot = match &event {
            WakeEvent::DisplayTick(i) | WakeEvent::App(i) | WakeEvent::Live(i) | WakeEvent::Window(i, _) | WakeEvent::Focus(i)
                | WakeEvent::Position(i, _) | WakeEvent::Level(i, _) | WakeEvent::Place(i, _) | WakeEvent::Frame(i, _)
                | WakeEvent::A11y(i) => *i,
            WakeEvent::Reopen => {
                self.reopen(event_loop);
                self.cleanup(event_loop);
                return;
            }
            WakeEvent::Quit(ending) => {
                self.quit(event_loop, *ending);
                self.cleanup(event_loop);
                return;
            }
            WakeEvent::Turn => { #[cfg(not(target_arch = "wasm32"))] self.turn(event_loop); return; }
        };
        if let Some(Some(host)) = self.hosts.get_mut(slot) {
            host.user_event(event_loop, event);
        }
        self.cleanup(event_loop);
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let _handling = crate::quit::handling();
        if let Some(host) = self.hosts.iter_mut().flatten().find(|h| h.owns(id)) {
            let _input = crate::input::time::arrived(&event);
            host.window_event(event_loop, id, event);
        }
        self.cleanup(event_loop);
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let _handling = crate::quit::handling();
        crate::cursor::os::make_pending(event_loop);
        let mut animating = false;
        for host in self.hosts.iter_mut().flatten() {
            animating |= host.about_to_wait(event_loop);
        }
        // The frames windows want next (`redraw`), and on Windows the paints the system keeps
        // putting off behind another window's.
        let now = Instant::now();
        for host in self.hosts.iter_mut().flatten() { host.turn_ends(event_loop, now); }
        // Layer surfaces have no redraw events: keep turning while one animates (its vsync
        // paces the loop). Hosts whose only motion is an ambient loop sleep until its next step.
        let deadline = self.hosts.iter().flatten().filter_map(|host| host.wake_deadline()).min();
        event_loop.set_control_flow(if animating { ControlFlow::Poll }
            else { deadline.map_or(ControlFlow::Wait, |at| ControlFlow::WaitUntil(crate::time::for_winit(at))) });
    }
    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        let _handling = crate::quit::handling();
        // Every quit ends here, so the windows still open run `App::on_quit` first, unless the
        // quit's hooks already ran (a Windows logoff, the last window closing).
        self.quit(event_loop, false);
        // Rune didn't ask to exit, so the OS is ending the app: macOS Quit, Cmd-Q, the Dock's
        // Quit, logout. Windows close as their close button would, before the clipboard stops.
        if !event_loop.exiting() {
            for host in self.hosts.iter_mut().flatten() { host.terminate(); }
        }
        for host in self.hosts.iter_mut().flatten() { host.cancel_menus(); }
        // Surfaces go while winit's display connection is still open (layer surfaces live on it).
        self.hosts.clear();
        crate::clipboard::shutdown();
        crate::quit::send(crate::quit::Event::Exited);
    }
}

use file_drop::{DROP_SAMPLE, FileDrop, FileStep};

struct Host<S: 'static> {
    /// `App::chrome`, as the app set it (`chrome::frame_chrome` lays a titlebar over it).
    chrome: Chrome,
    /// `App::titlebar`, unless the window is borderless.
    titlebar: Option<crate::Titlebar>,
    options: WindowOptions,
    placement: Option<crate::windows::PlaceRequest>,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    macos_window: crate::MacWindowOptions,
    native_handle: WindowHandle,
    on_created: Option<Box<dyn FnOnce(WindowHandle)>>,
    on_window: Option<crate::app::OnWindow>,
    on_ready: Option<crate::app::OnWindow>,
    frame_reports: crate::frame::encoded::FrameReports<S>,
    finished: bool,
    /// False for windows `App::window` added: they live as long as the loop, never keep it.
    keeps_loop: bool,
    spec: AppSpec<S>,
    args: Args,
    keep_running: bool,
    on_close: Option<fn(&mut S, &mut Tx)>,
    on_quit: Option<fn(&mut S, &mut Tx)>,
    /// `App::turn_scope`, and whether a turn is in it now.
    turn_scope: Option<crate::app::TurnScope<S>>,
    turn: any_host::Turn,
    on_reopen: Option<fn(&mut S, bool, &mut Tx) -> bool>,
    /// Its place among the windows receiving links (`App::on_open_url`).
    _open_urls: Option<crate::open_url::incoming::Listening>,
    /// When it last hid, in `reopen::stamp` order.
    hidden_at: Option<u64>,
    release_gpu: bool,
    open_at_start: bool,
    /// `App::float_on_open`: each window maps at a fixed size, then takes its own limits.
    #[cfg_attr(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))), allow(dead_code))]
    float_on_open: bool,
    /// `App::open_inactive`: the first open leaves focus where it is.
    open_inactive: bool,
    /// `App::maximized`.
    open_maximized: bool,
    shared: Arc<Shared<S>>,
    shortcuts: crate::shortcuts::Guard,
    menus: crate::native_menu::Guard<S>,
    popup: crate::popup_menu::Presenter,
    file_dialog: crate::file_dialog::Presenter,
    rt: Runtime<S>,
    /// Before `win` and `gpu`: fields drop in order, and the trace waits for the window's last
    /// present times.
    trace: Option<crate::tooling::Trace>,
    trace_start: Option<Instant>,
    trace_frames: usize,
    meter: SurfaceMeter,
    win: Option<Win>,
    gpu: Option<(Arc<Gpu>, Renderer)>,
    gpu_pool: Rc<RefCell<Weak<Gpu>>>,
    /// A window has shown before: the next one opens settled.
    shown: bool,
    started: bool,
    clock: FrameClock,
    idle: bool,
    mods: ModifiersState,
    control_click: bool,
    files: FileDrop,
    clipboard: crate::clipboard::Client,
    secure_input: crate::input::secure_input::SecureInput,
    ime_allowed: bool,
    comp: Option<Box<dyn Compositor<S>>>,
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    wayland: bool,
    /// The window's frame moving to where a handler asked (`Tx::resize_window`), top-down
    /// screen points on the host's frame clock. macOS only; elsewhere frames change at once.
    frame_anim: Option<crate::window::frame::FrameAnim>,
    /// The window origin `frame_anim` last set, to notice a drag mid-flight.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    frame_origin: Option<[f32; 2]>,
    /// The content size a resize request jumps to, so its Resized event lays out like a step
    /// of a resize animation rather than a user's resize.
    frame_jump: Option<(f32, f32)>,
    /// Present times of every window this host opens while tracing.
    #[cfg(target_os = "macos")]
    present_log: Option<Arc<crate::frame::present_time::metal::Log>>,
    #[cfg(target_os = "macos")]
    frame_wake: Waker,
}

impl<S: 'static> Host<S> {
    fn new(window_spec: WindowSpec<S>, native_handle: WindowHandle, args: Args, proxy: EventLoopProxy<WakeEvent>, slot: usize, gpu_pool: Rc<RefCell<Weak<Gpu>>>, wayland: bool) -> Self {
        let WindowSpec { mut app, options, key: _, on_created } = window_spec;
        let mut args = app.resolve_args(args);
        let on_window = app.on_window.take();
        let on_ready = app.on_ready.take();
        let frame_reports = crate::frame::encoded::FrameReports::new(app.on_frame_encoded.take(), Some(native_handle.key())).inspecting(app.on_frame_drawn.take());
        let macos_window = std::mem::take(&mut app.macos_window);
        // Before the menu guard, so custom items in Edit or Window join these menus.
        #[cfg(target_os = "macos")]
        crate::native_menu::install_standard_menus(app.standard_menus);
        native_handle.bind(proxy.clone(), slot);
        let live_proxy = proxy.clone();
        #[cfg(target_os = "macos")]
        let frame_proxy = proxy.clone();
        let waker: Waker = Arc::new(move || {
            let _ = proxy.send_event(WakeEvent::App(slot));
        });
        let (open, keep_running, on_close, release_gpu) = (app.open, app.keep_running, app.on_close, app.release_gpu);
        let (on_reopen, on_quit, turn_scope) = (app.on_reopen, app.on_quit, app.turn_scope);
        #[cfg(target_os = "macos")]
        let present_log = args.trace.as_ref().map(|_| Arc::default());
        let mut app = app;
        let comp = app.compositor.take();
        let (on_focus, on_ime) = (app.on_focus, app.on_ime);
        let titlebar = app.titlebar.filter(|_| !options.borderless);
        let chrome = app.chrome;
        let (srgb_blend, font_smoothing) = (app.srgb_blend, app.font_smoothing);
        let modifier_keys = app.modifier_keys;
        let tab_visits_controls = app.tab_visits_controls;
        let fit_height = app.fit_content_height;
        let float_on_open = app.float_on_open;
        let (open_inactive, open_maximized) = (app.open_inactive, app.maximized);
        let app_stats = app.dev_stats;
        let playground = app.playground.take();
        let (mut spec, state, shared) = app.into_parts(args.scene.as_deref());
        let mut size = args.size.unwrap_or(spec.size);
        // The window reports the system appearance and scale when it opens.
        let env = Env::new(args.theme.as_deref() != Some("light"), 1.0);
        let mut rt = runtime_for(&spec, state, size, env, setup_waker(&shared));
        crate::playground::attach(&mut rt, playground, &spec, args.playground.clone(), args.variant.clone(), true);
        if fit_height {
            spec.min_size = options.fit_min_size(spec.min_size);
            // Opens at the content's height; the first frame corrects any difference its
            // real scale makes.
            rt.fit_height = true;
            let (min, max) = options.size_limits(spec.min_size);
            size = crate::window::frame::clamp_size((size.0, rt.content_height()), min, max);
            rt.set_viewport(size, env.scale);
            spec.size = size;
            args.size = None;
        }
        rt.on_focus = on_focus;
        rt.app_ime.hook = on_ime;
        rt.set_modifier_keys(modifier_keys);
        rt.set_srgb_blend(srgb_blend); rt.set_font_smoothing(font_smoothing);
        rt.set_system_reduced_motion(crate::platform::preferences::reduced_motion());
        rt.set_full_keyboard_access(crate::platform::preferences::full_keyboard_access());
        rt.set_tab_visits_controls(tab_visits_controls);
        crate::platform::accent::watch();
        rt.set_system_accent(crate::system_accent());
        crate::platform::scroll_bars::watch();
        rt.set_system_scroll_bars_state(crate::platform::scroll_bars::current().with_override(args.scroll_bars));
        rt.trace_channels = args.trace.as_ref().map(|_| Default::default());
        rt.set_live_waker(Arc::new(move || { let _ = live_proxy.send_event(WakeEvent::Live(slot)); }));
        rt.set_live_visible(false);
        rt.set_os_drag(crate::drag_out::SUPPORTED);
        rt.transparent = options.transparent;
        rt.track_marks = comp.is_some();
        rt.wall_start = Some(Instant::now());
        rt.open_urls_natively = args.script.is_none() && args.trace.is_none() && args.frames.is_none();
        rt.record_events = false;
        let meter = SurfaceMeter::new(native_handle.key(), native_handle.native.stats.clone());
        rt.surface_stats = Some(meter.shared());
        rt.app_stats = app_stats;
        let mut clipboard = crate::clipboard::Client::default();
        clipboard.connect(waker.clone());
        shared.connect(waker.clone());
        let popup = crate::popup_menu::Presenter::new(waker.clone());
        let file_dialog = crate::file_dialog::Presenter::new(waker.clone());
        let shortcuts = crate::shortcuts::Guard::attach(&shared, waker, false, wayland);
        let menus = crate::native_menu::Guard::attach(&shared, false);
        Host {
            chrome,
            titlebar,
            options,
            placement: None,
            macos_window,
            native_handle,
            on_created,
            on_window,
            on_ready,
            frame_reports,
            finished: false,
            keeps_loop: true,
            spec,
            args,
            keep_running,
            on_close,
            on_quit,
            turn_scope,
            turn: any_host::Turn::Between,
            on_reopen,
            _open_urls: None,
            hidden_at: None,
            release_gpu,
            open_at_start: open,
            float_on_open,
            open_inactive,
            open_maximized,
            shared,
            shortcuts,
            menus,
            popup,
            file_dialog,
            rt,
            win: None,
            gpu: None,
            gpu_pool,
            shown: false,
            started: false,
            clock: FrameClock::new(Instant::now()),
            idle: true,
            mods: ModifiersState::empty(),
            control_click: false,
            files: FileDrop::default(),
            clipboard,
            secure_input: Default::default(),
            ime_allowed: false,
            comp,
            wayland,
            frame_anim: None,
            frame_origin: None,
            frame_jump: None,
            trace: None,
            trace_start: None,
            trace_frames: 0,
            meter,
            #[cfg(target_os = "macos")]
            present_log,
            #[cfg(target_os = "macos")]
            frame_wake: Arc::new(move || { let _ = frame_proxy.send_event(WakeEvent::DisplayTick(slot)); }),
        }
    }
}

/// `setup`'s waker: `AppHandle::wake`, so a drain that also holds updates that changed nothing
/// still polls and redraws for it.
fn setup_waker<S: 'static>(shared: &Arc<crate::app::Shared<S>>) -> Waker {
    let handle = crate::AppHandle { shared: shared.clone() };
    Arc::new(move || handle.wake())
}

/// What a shown window that can't draw runs for an update, a wake, a due timer or a redraw:
/// covered (an occlusion event, or a drawable refused as occluded), minimised, or waiting out
/// a missed frame. `poll`, due timers and queued input run now, as for a hidden window
/// (`Runtime::hidden_wake`); the view and layout wait, dirty, for the frame that uncovers it.
/// Uncovering settles first, so that frame builds once and paints motion where it would have
/// ended, as Core Animation's time-based motion has by the time a covered window shows again.
pub(crate) fn covered_step<S: 'static>(rt: &mut Runtime<S>) { rt.hidden_wake(); }

/// Whether a drain of other threads' requests asks the window for a frame. A hidden launcher
/// renders a change into its cached frame (paced); a shown launcher redraws for a change or
/// as it shows; any other shown window redraws, unless only updates that changed nothing
/// (`quiet`) woke it.
fn drain_frame(launcher: bool, visible: bool, was_shown: bool, changed: bool, quiet: bool) -> bool {
    if launcher && !visible { return changed; }
    !quiet && if launcher { changed || was_shown && visible } else { visible }
}

/// A vsynced sRGB (or non-sRGB when `srgb_blend` is set) configuration for the surface, and the
/// first alpha mode it supports that composites translucent pixels.
pub(crate) fn surface_config(surface: &wgpu::Surface, adapter: &wgpu::Adapter, (width, height): (u32, u32), srgb_blend: bool)
    -> Option<(wgpu::SurfaceConfiguration, Option<wgpu::CompositeAlphaMode>)> {
    let mut config = surface.get_default_config(adapter, width.max(1), height.max(1))?;
    let caps = surface.get_capabilities(adapter);
    let srgb = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(config.format);
    config.format = if srgb_blend {
        let non_srgb = srgb.remove_srgb_suffix();
        if caps.formats.contains(&non_srgb) {
            non_srgb
        } else {
            config.view_formats = vec![non_srgb];
            srgb
        }
    } else {
        srgb
    };
    // Backdrop copies read the drawable mid-frame (`ActivePass::split`: backdrop blur, effect
    // backdrops, rune-fx blend modes). On Metal it costs `framebufferOnly`, but without it they
    // quietly fall back.
    config.usage |= caps.usages & wgpu::TextureUsages::COPY_SRC;
    config.present_mode = wgpu::PresentMode::AutoVsync;
    config.desired_maximum_frame_latency = 2;
    Some((config, crate::render::translucency::mode(surface, adapter)))
}

/// Configures `surface`, returning wgpu's error instead of panicking in its default handler,
/// so a surface the driver refuses (an overlay's XWayland window on NVIDIA, hex#979) costs
/// only its owner. The message names the configuration for logs.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn configure(surface: &wgpu::Surface, device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> Result<(), String> {
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
    surface.configure(device, config);
    let error = pollster::block_on(internal.pop()).or(pollster::block_on(validation.pop()));
    match error {
        None => Ok(()),
        Some(error) => {
            let text = error.to_string();
            let cause = text.lines().map(str::trim).filter(|line| !line.is_empty() && *line != "Caused by:").collect::<Vec<_>>().join(": ");
            Err(format!("{cause} ({}x{}, {:?}, {:?}, {:?})", config.width, config.height, config.format, config.alpha_mode, config.present_mode))
        }
    }
}

/// The browser reports surface errors asynchronously; there is nothing to catch here.
#[cfg(target_arch = "wasm32")]
pub(crate) fn configure(surface: &wgpu::Surface, device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> Result<(), String> {
    surface.configure(device, config);
    Ok(())
}

/// Follows a blend switch (`Tx::set_srgb_blend`) on a configured surface: reconfigures it for
/// `rt`'s mode at its size and alpha mode, rebuilds `renderer` when its target format changes,
/// and rebuilds the `alpha` pass (when there is one) for the new format. Returns false when the
/// surface can't be configured.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn reblend<S: 'static>(surface: &wgpu::Surface, gpu: &Gpu, config: &mut wgpu::SurfaceConfiguration,
    renderer: &mut Renderer, alpha: &mut Option<crate::render::surface_alpha::AlphaSurface>, rt: &mut Runtime<S>) -> bool {
    let srgb = rt.srgb_blend();
    let Some((next, _)) = surface_config(surface, &gpu.adapter, (config.width, config.height), srgb) else { return false };
    if (config.format, &config.view_formats) != (next.format, &next.view_formats) {
        (config.format, config.view_formats) = (next.format, next.view_formats);
        if let Err(error) = configure(surface, &gpu.device, config) {
            eprintln!("rune: could not reconfigure a surface for a blend switch: {error}");
            return false;
        }
    }
    let target = if srgb { config.format.remove_srgb_suffix() } else { config.format };
    rebuild_renderer(&gpu.device, renderer, target, rt);
    if alpha.is_some() {
        *alpha = Some(crate::render::surface_alpha::AlphaSurface::new(&gpu.device, target, (config.width, config.height)));
    }
    true
}

/// Switches `renderer` to `target` when its format changed, keeping its atlases, layer targets
/// and extra pipelines, and sets its blend mode to `rt`'s.
pub(crate) fn rebuild_renderer<S: 'static>(device: &wgpu::Device, renderer: &mut Renderer, target: wgpu::TextureFormat, rt: &mut Runtime<S>) {
    renderer.set_format(device, target);
    renderer.set_srgb_blend(rt.srgb_blend());
    // Before the first frame, so its glyph masks go to the GPU fill when the renderer has one.
    renderer.attach_text(&mut rt.text.atlas);
}

pub(crate) fn is_wayland(event_loop: &ActiveEventLoop) -> bool {
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    return winit::platform::wayland::ActiveEventLoopExtWayland::is_wayland(event_loop);
    #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
    {
        let _ = event_loop;
        false
    }
}

/// A window's content top-left in points from the primary display's top-left (see
/// `El::on_screen_layout`), the space an overlay's `OverlayPlacement::Rect` on
/// `DisplayTarget::Primary` is in. Elsewhere than macOS that's desktop pixels from the
/// primary's origin over the window's scale, as `overlay::position` converts the other way.
/// Wayland hides it.
pub(crate) fn screen_origin(window: &Window) -> Option<(f32, f32)> {
    #[cfg(target_os = "macos")]
    return crate::platform::macos::content_origin(window);
    #[cfg(not(target_os = "macos"))]
    {
        let p = window.inner_position().ok()?;
        let o = window.primary_monitor().map_or(winit::dpi::PhysicalPosition::new(0, 0), |m| m.position());
        let scale = window.scale_factor();
        Some(((f64::from(p.x - o.x) / scale) as f32, (f64::from(p.y - o.y) / scale) as f32))
    }
}

/// Tells `rt` where `window`'s content is on screen: in points for `El::on_screen_layout`
/// ([`screen_origin`]), and in desktop pixels for `Drag::screen_point`. True when that moved
/// an `El::on_screen_layout` element.
pub(crate) fn sync_screen_origin<S: 'static>(rt: &mut Runtime<S>, window: &Window) -> bool {
    rt.set_window_pixels(window.inner_position().ok().map(|p| (p.x, p.y)));
    screen_origin(window).is_some_and(|o| rt.set_window_origin(o))
}

/// The scale an overlay draws at. On macOS, read from the panel AppKit shows, and before it's
/// ordered in from the display holding most of its frame, where it will appear.
pub(crate) fn backing_scale(window: &Window) -> f32 {
    #[cfg(target_os = "macos")]
    if let Some(scale) = crate::platform::macos::backing_scale(window) { return scale; }
    window.scale_factor() as f32
}

/// Native titlebar behaviour the framework owns: traffic-light placement, drag regions and the
/// double-click action. Elsewhere only the portable parts apply.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn uses_drag_regions(chrome: Chrome, borderless: bool, panel: bool) -> bool {
    chrome.drag_regions && (borderless || chrome.full_size_content) && !panel
}

pub(crate) fn pointer_button(button: MouseButton) -> crate::PointerButton {
    match button {
        MouseButton::Left => crate::PointerButton::Primary,
        MouseButton::Right => crate::PointerButton::Secondary,
        MouseButton::Middle => crate::PointerButton::Middle,
        MouseButton::Back => crate::PointerButton::Back,
        MouseButton::Forward => crate::PointerButton::Forward,
        MouseButton::Other(n) => crate::PointerButton::Other(n),
    }
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn gesture_phase(phase: TouchPhase) -> crate::GesturePhase {
    match phase {
        TouchPhase::Started => crate::GesturePhase::Began,
        TouchPhase::Moved => crate::GesturePhase::Moved,
        TouchPhase::Ended | TouchPhase::Cancelled => crate::GesturePhase::Ended,
    }
}

/// Hands a winit touch (a finger, or a pen where the system reports it as one) to the runtime
/// in window points, timed on a clock that starts with the first touch.
pub(crate) fn touch<S: 'static>(rt: &mut crate::Runtime<S>, touch: &winit::event::Touch, scale: f32) {
    static START: std::sync::OnceLock<crate::time::Instant> = std::sync::OnceLock::new();
    let phase = match touch.phase {
        TouchPhase::Started => crate::TouchPhase::Started,
        TouchPhase::Moved => crate::TouchPhase::Moved,
        TouchPhase::Ended => crate::TouchPhase::Ended,
        TouchPhase::Cancelled => crate::TouchPhase::Cancelled,
    };
    let time = START.get_or_init(crate::time::Instant::now).elapsed().as_secs_f64();
    rt.touch(touch.id, phase, touch.location.x as f32 / scale, touch.location.y as f32 / scale, time);
}

impl<S: 'static> Host<S> {
    /// Tells the runtime where the window's content is on screen, drawing a frame when that
    /// moves an `El::on_screen_layout` element.
    fn sync_origin(&mut self) {
        let Some(w) = &self.win else { return };
        if sync_screen_origin(&mut self.rt, &w.window) {
            w.request_redraw();
        }
    }

    /// Publishes what `AppHandle::is_visible` reports.
    fn publish(&self) {
        let visible = !self.finished && self.win.as_ref().is_some_and(|w| w.visible);
        self.shared.visible.store(visible, Ordering::Release);
    }
}
