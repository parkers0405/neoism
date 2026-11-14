//! Overlays: surfaces beside the app's window (a recording HUD, meeting capsules). Each has
//! its own state type and `Runtime`, is fed from any thread through an `OverlayHandle`, and is
//! drawn transparent above other apps. Bare specs preserve edge placement on the primary
//! display and never request keyboard focus. `OverlaySpec::options` adds display targeting,
//! full-display/rect placement, regional input and explicit nonactivating keyboard policy.
//!
//! - Wayland: layer-shell (`layer_shell.rs`) with output-local input regions and a private
//!   seat queue. Hidden means destroyed. No global pointer observation or capture exclusion.
//!   Without layer-shell (GNOME), an overlay that takes no input shows on XWayland instead
//!   (`x11/xwayland.rs`): an override-redirect ARGB window with an empty input shape. One that
//!   takes the pointer does too when its host opts in (`OverlayOptions::xwayland_input`).
//! - X11: an override-redirect notification window, with an empty input shape unless
//!   interactive.
//! - macOS: winit's view moved into a non-activating `NSPanel` at status level on every space,
//!   observing pointer movement even through transparent regions, ordered front without key
//!   focus unless explicitly requested. Its window number is available for capture filters.
//! - Windows: a topmost tool window with `WS_EX_NOACTIVATE` (plus `WS_EX_TRANSPARENT` and
//!   `WS_EX_LAYERED` unless interactive), shown with `SW_SHOWNOACTIVATE`.
//! - With `release_gpu_when_hidden`, hidden means destroyed on Windows and macOS too.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use crate::time::Instant;

use winit::dpi::LogicalSize;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId, WindowLevel};

use crate::el::{Align, El, Tx};
use crate::compose::{Compositor, Frame};
use crate::frame::clock::FrameClock;
use crate::render::{Gpu, Renderer};
use crate::runtime::{Cx, Env, Runtime, Waker};
use crate::text::TextSystem;
use crate::tooling::AppSpec;
use crate::window::{AnyHost, WakeEvent};
use crate::{ConfiguredOverlay, DisplayTarget, KeyPolicy, OverlayInput, OverlayOptions, OverlayPlacement, OverlaySupport};

#[path = "overlay/draw.rs"]
mod draw;
#[path = "overlay/frame.rs"]
mod frame;
use frame::{idle_after, render_frame, step};
#[path = "overlay/placement.rs"]
mod placement;
#[path = "overlay/xwayland.rs"]
mod xwayland;
#[cfg(not(target_os = "macos"))]
pub(crate) use placement::monitor_at;
use placement::{monitor, position};
mod requests;
use requests::{Requests, Update};
#[path = "overlay/any_host.rs"]
mod any_host;
#[path = "overlay/input.rs"]
mod input;
pub(crate) mod options;

/// The screen edge an overlay sits on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScreenEdge {
    #[default]
    Top,
    Bottom,
}

pub struct OverlaySpec<T: 'static> {
    /// The window title, or the layer-shell namespace on Wayland.
    pub name: &'static str,
    /// In points.
    pub size: (f32, f32),
    pub edge: ScreenEdge,
    /// Distance from the edge, in points.
    pub margin: f32,
    /// Along the edge: centred, or at the start (left) or end (right) `inset` points in.
    pub align: Align,
    pub inset: f32,
    /// Clicks reach the view (meeting capsule buttons); otherwise they pass through.
    pub interactive: bool,
    /// The overlay's view. Its root should have no fill: whatever it leaves clear stays
    /// transparent.
    pub view: fn(&T, &mut Cx<T>) -> El,
}

/// Whether the platform could show the overlay, known after its first `show`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    Unknown,
    Available,
    /// Wayland without layer-shell, or its window, surface or show failed (`OverlayHandle::failure`).
    Unavailable,
}

/// Where an overlay sits and how big it is, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Placement {
    pub size: (f32, f32),
    pub edge: ScreenEdge,
    pub margin: f32,
    pub align: Align,
    pub inset: f32,
}

struct Shared<T> {
    native: Mutex<(Option<u32>, crate::OverlayCapabilities)>,
    requests: Mutex<Requests<T>>,
    availability: AtomicU8,
    /// Why the platform could not create or show the overlay, while it can't.
    failure: Mutex<Option<String>>,
    /// What draws the overlay: backend, adapter and alpha mode, once its surface exists.
    surface: Mutex<Option<String>>,
    /// The event loop's wake-up for this overlay; empty until the loop runs.
    wake: Mutex<Option<Waker>>,
    /// What the overlay's meter last published.
    stats: Arc<Mutex<crate::SurfaceStats>>,
}

impl<T> Shared<T> {
    fn wake(&self) {
        let wake = self.wake.lock().unwrap().clone();
        if let Some(wake) = wake {
            wake();
        }
    }
}

/// Shows, hides, moves and feeds an overlay from any thread; every call wakes the event loop.
pub struct OverlayHandle<T: 'static> {
    shared: Arc<Shared<T>>,
}

impl<T> Clone for OverlayHandle<T> {
    fn clone(&self) -> Self {
        OverlayHandle { shared: self.shared.clone() }
    }
}

impl<T: 'static> OverlayHandle<T> {
    /// Takes effect on the next event-loop turn, or after the current owned drag releases.
    pub fn set_input(&self, input: OverlayInput) { self.request(|q| q.input = Some(input)); }
    pub fn set_follow_pointer(&self, follow: bool) { self.request(|q| q.follow_pointer = Some(follow)); }
    /// Moves a shown [`DisplayTarget::UnderPointer`] overlay to the display under the pointer
    /// when two calls in a row find it on the same other display; at once, without taking
    /// focus. A caller's own pace
    /// (a HUD's few checks a second) instead of `follow_pointer`'s pointer watch. Windows and
    /// X11; elsewhere it does nothing.
    pub fn refollow_pointer(&self) { self.request(|q| q.refollow = true); }
    /// None restores the spec's legacy edge placement.
    pub fn set_placement(&self, placement: Option<OverlayPlacement>) { self.request(|q| q.placement = Some(placement)); }
    pub fn set_display(&self, display: DisplayTarget) { self.request(|q| q.display = Some(display)); }

    /// Request keyboard focus without application activation, where supported.
    pub fn request_keys(&self) { self.request(|q| q.request_keys = true); }

    /// macOS panel CGWindowID for an explicit ScreenCaptureKit exclusion filter. Available
    /// at native creation (see `on_created`/`prewarm`); a new one with each window made.
    pub fn window_number(&self) -> Option<u32> { self.shared.native.lock().unwrap().0 }

    /// Result of the native capture-exclusion request. Linux reports Unsupported.
    pub fn capture_exclusion(&self) -> OverlaySupport { self.capabilities().capture_exclusion }
    pub fn capabilities(&self) -> crate::OverlayCapabilities { self.shared.native.lock().unwrap().1 }
    /// Changes `OverlayOptions::exclude_from_capture` on the live window. It applies only while
    /// the overlay is shown; set while hidden, it takes effect at the next show, so there is no
    /// need to toggle it around show and hide.
    pub fn set_exclude_from_capture(&self, exclude: bool) { self.request(|q| q.exclude_from_capture = Some(exclude)); }
    /// Changes `OverlayOptions::xwayland_input`. It applies when the surface is next made, so
    /// a call before `show` in the same turn covers that show.
    pub fn set_xwayland_input(&self, allow: bool) { self.request(|q| q.xwayland_input = Some(allow)); }

    /// The overlay's frame counts: redraws, presents, failed drawable acquisitions, loop turns
    /// and pacing: current totals, and per-second rates updated once a second while it draws.
    pub fn surface_stats(&self) -> crate::SurfaceStats { crate::frame::surface_stats::read(&self.shared.stats) }

    /// Reveal the overlay. Prewarmed macOS overlays refresh hidden changes before reveal,
    /// preserving motion elapsed since each commit, even when show arrives in a later turn.
    /// Unretained hosts show settled, unless built with `ConfiguredOverlay::animate_reveal`.
    pub fn show(&self) {
        self.request(|q| { q.visible = Some(true); q.fade = None; });
    }

    pub fn hide(&self) {
        self.request(|q| { q.visible = Some(false); q.fade = None; });
    }

    /// Like `show`, fading the window's alpha in from zero over `duration` on macOS
    /// (AppKit's default ease). Elsewhere the overlay appears at once.
    pub fn fade_in(&self, duration: Duration) {
        self.request(|q| { q.visible = Some(true); q.fade = Some(duration); });
    }

    /// Like `hide`, fading the window's alpha out over `duration` on macOS and Windows before
    /// it goes. Input stops at once, but the overlay keeps drawing until the fade ends, so
    /// motion started in the same turn (an exit transition, a poll stepping its own animation)
    /// plays out during it, and frames still queued for the screen reach it. A GPU surface
    /// given back while hidden goes only once the fade ends. Showing again cancels the fade,
    /// and that motion carries on from where it is. Elsewhere the overlay hides at once.
    pub fn fade_out(&self, duration: Duration) {
        self.request(|q| { q.visible = Some(false); q.fade = Some(duration); });
    }

    /// Change the macOS window level (`NSWindow.Level`, e.g. 3 for `.floating`), shown or not.
    /// Overrides `OverlayOptions::level`; other platforms ignore it.
    pub fn set_macos_level(&self, level: isize) {
        self.request(|q| q.macos_level = Some(level));
    }

    pub fn set_edge(&self, edge: ScreenEdge) {
        self.request(|q| q.edge = Some(edge));
    }

    /// Moves the overlay to `edge`, `margin` points in.
    pub fn place(&self, edge: ScreenEdge, margin: f32) {
        self.request(|q| {
            q.edge = Some(edge);
            q.margin = Some(margin);
        });
    }

    /// Moves the overlay along its edge: centred, or `inset` points from the start or end.
    pub fn set_align(&self, align: Align, inset: f32) {
        self.request(|q| q.align = Some((align, inset)));
    }

    /// Resizes the overlay (points), keeping its edge and alignment.
    pub fn resize(&self, size: (f32, f32)) {
        self.request(|q| q.size = Some(size));
    }

    /// Change the overlay's state on the UI thread, in call order, shown or not.
    /// `Tx::wake_after` invokes the configured poll at its deadline, including while hidden.
    /// A hidden overlay's view doesn't run for it: the frame that shows the overlay builds it
    /// once, with the latest state. A prewarmed macOS overlay commits hidden changes at most
    /// every 250 ms to keep its retained frame close.
    pub fn update(&self, f: impl FnOnce(&mut T, &mut Tx) + Send + 'static) {
        self.update_if(move |s, tx| { f(s, tx); true });
    }

    /// `update` for a timer or feed that often finds nothing new: `f` returns whether it changed
    /// state the view reads, as `OverlayOptions::poll` does. On false the view doesn't re-run
    /// and the overlay neither draws nor refreshes while hidden, while what `f` asked of its
    /// `Tx` still happens: clipboard, menus, `wake_after`, and scrolls, which take a frame.
    ///
    /// Returning false after changing state the view reads leaves the screen stale until the
    /// next change.
    pub fn update_if(&self, f: impl FnOnce(&mut T, &mut Tx) -> bool + Send + 'static) {
        self.request(|q| q.updates.push(Box::new(f)));
    }

    pub fn availability(&self) -> Availability {
        match self.shared.availability.load(Ordering::Acquire) {
            1 => Availability::Available,
            2 => Availability::Unavailable,
            _ => Availability::Unknown,
        }
    }

    /// Why the overlay is `Unavailable`: the window, its GPU surface, or showing it failed.
    pub fn failure(&self) -> Option<String> {
        self.shared.failure.lock().unwrap().clone()
    }

    /// What draws the overlay, such as `Dx12 on Intel(R) UHD Graphics (IntegratedGpu),
    /// PreMultiplied`, once its surface exists; for logs.
    pub fn surface_description(&self) -> Option<String> {
        self.shared.surface.lock().unwrap().clone()
    }

    fn request(&self, f: impl FnOnce(&mut Requests<T>)) {
        f(&mut self.shared.requests.lock().unwrap());
        self.shared.wake();
    }
}

/// An overlay before the event loop runs: the shell gives it a wake-up and the GPU pool.
pub(crate) trait PendingOverlay: AnyHost {
    fn connect(&mut self, wake: Waker, frame_wake: Waker, gpu: Rc<RefCell<Weak<Gpu>>>);
}

enum Backing {
    Window(Arc<Window>),
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    Layer(crate::platform::layer_shell::LayerSurface),
    /// Wayland without layer-shell: an X11 window on XWayland, for overlays that take no input
    /// or whose host opted in to pointer input there.
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    Xwayland(crate::platform::x11::xwayland::XwaylandSurface),
}

/// A shown overlay. The wgpu surface drops before what it draws into.
struct Live {
    /// The OS accessibility adapter for this surface.
    a11y: Option<crate::a11y::Adapter>,
    /// Presents plain frames off the event loop's thread; joined before the surface drops.
    #[cfg(target_os = "macos")]
    present: Option<crate::frame::present_worker::PresentWorker>,
    #[cfg(target_os = "macos")]
    pacing: Option<crate::platform::display_link::DisplayLink>,
    /// `El::system_material` views behind the Metal layer.
    #[cfg(target_os = "macos")]
    materials: crate::system_material::macos::Materials,
    /// Fn presses, which winit doesn't send, while the overlay wants modifier keys.
    #[cfg(target_os = "macos")]
    fn_key: Option<crate::input::fn_key::FnKey>,
    alpha: Option<crate::render::surface_alpha::AlphaSurface>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    backing: Backing,
    scale: f32,
}

pub(crate) struct Overlay<T: 'static> {
    on_created: Option<Box<dyn FnMut(crate::OverlayCreated)>>,
    /// `on_created` heard of the current native window; a released one is announced again.
    announced: bool,
    setup: Option<fn(&mut T, Waker)>,
    pending_input: Option<OverlayInput>,
    visible: bool,
    animate_reveal: bool, // `ConfiguredOverlay::animate_reveal`
    /// When a fade-out started by `OverlayHandle::fade_out` ends. Until then the hidden
    /// overlay takes no input but keeps drawing, and isn't treated as occluded.
    fading_out: Option<Instant>,
    /// How long the fade-out ending at `fading_out` lasts, for its alpha (Windows).
    #[cfg(target_os = "windows")]
    fade_length: Duration,
    options: OverlayOptions,
    canvas_size: (f32, f32),
    name: &'static str,
    at: Placement,
    /// Where `place_window` last put the window, to carry the pointer across a move.
    origin: Option<winit::dpi::PhysicalPosition<i32>>,
    interactive: bool,
    /// `ConfiguredOverlay::macos_level` or the last `set_macos_level`.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    macos_level: Option<isize>,
    shared: Arc<Shared<T>>,
    rt: Runtime<T>,
    clipboard: crate::clipboard::Client,
    popup: crate::popup_menu::Presenter,
    compositor: Option<Box<dyn Compositor<T>>>,
    /// The non-activating panel holding winit's view (macOS), kept with the window. Declared
    /// before the window so the view goes back to it first.
    #[cfg(target_os = "macos")]
    panel: Option<crate::platform::macos::Panel>,
    #[cfg(target_os = "windows")]
    native: Option<crate::platform::win32::OverlayWindow>,
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    x11: Option<crate::platform::x11::overlay::InputWindow>,
    live: Option<Live>,
    /// The native window while it has no GPU surface: just made, or hidden with
    /// `release_gpu_when_hidden`. Windows and X11 only.
    parked: Option<Backing>,
    /// `ConfiguredOverlay::release_gpu_when_hidden` (zero) or `release_gpu_after`.
    release_after: Option<Duration>,
    /// When a hidden overlay with `release_after` gives its GPU surface back.
    release_at: Option<Instant>,
    cursor: Option<crate::cursor::os::OsCursor>,
    mods: winit::keyboard::ModifiersState,
    renderer: Option<Renderer>,
    gpu: Option<Arc<Gpu>>,
    gpu_pool: Rc<RefCell<Weak<Gpu>>>,
    dark: bool,
    dirty: bool,
    idle: bool,
    /// After the last frame the runtime asked for no other; only the compositor animates.
    runtime_idle: bool,
    quiet: frame::Quiet,
    clock: FrameClock,
    /// The panel is shown but none of it is on screen (covered, or the displays asleep).
    #[cfg(target_os = "macos")]
    occluded: bool,
    /// This loop turn already read the panel's occlusion (`sync_occlusion`).
    #[cfg(target_os = "macos")]
    occlusion_read: bool,
    /// When to try a frame again that had no drawable, or the next frame of motion with no
    /// display link to pace it.
    retry: Option<Instant>,
    /// The next occluded re-check's wait, backing off while the surface stays occluded.
    occluded_wait: Duration,
    /// When a hidden retained overlay commits the changes it hasn't yet.
    hidden_pace: crate::window::HiddenPace,
    #[cfg(target_os = "macos")]
    frame_wake: Option<Waker>,
    meter: crate::frame::surface_stats::SurfaceMeter,
    /// The display `refollow_pointer` last found the pointer on, away from the overlay's.
    refollow_away: Option<winit::monitor::MonitorHandle>,
}

impl<T: 'static> Overlay<T> {
    pub(crate) fn new<S>(configured: impl Into<ConfiguredOverlay<T>>, state: T, app: &AppSpec<S>) -> (Self, OverlayHandle<T>) {
        let ConfiguredOverlay { spec, options, on_key, on_created, poll, on_focus, compositor, setup, srgb_blend, macos_level, modifier_keys, animate_reveal, release_after } = configured.into();
        let shared = Arc::new(Shared { native: Mutex::default(), requests: Mutex::default(), availability: AtomicU8::new(0), failure: Mutex::default(), surface: Mutex::default(), wake: Mutex::new(None), stats: Arc::default() });
        let meter = crate::frame::surface_stats::SurfaceMeter::new(spec.name, shared.stats.clone());
        let text = TextSystem::for_window(app.fonts, app.families);
        let mut rt = Runtime::with_text(state, spec.view, spec.size, Env::default(), text);
        rt.set_srgb_blend(srgb_blend);
        rt.set_system_reduced_motion(crate::platform::preferences::reduced_motion());
        crate::platform::accent::watch();
        rt.set_system_accent(crate::system_accent());
        crate::platform::scroll_bars::watch();
        rt.set_system_scroll_bars_state(crate::platform::scroll_bars::current());
        rt.wall_start = Some(Instant::now());
        rt.native_overlay = true;
        rt.set_os_drag(crate::drag_out::SUPPORTED);
        rt.on_key = on_key;
        rt.set_modifier_keys(modifier_keys);
        rt.on_focus = on_focus;
        rt.set_live_visible(false);
        rt.surface_stats = Some(meter.shared());
        // App and overlay may use different state types. Downcast the function pointer
        // once, without a per-frame adapter or changing App::overlay's generic API.
        rt.poll = poll.or_else(|| app.poll.and_then(|f| {
            (&f as &dyn std::any::Any).downcast_ref::<fn(&mut T, f64, &mut Tx) -> bool>().copied()
        }));
        let overlay = Overlay {
            on_created,
            announced: false,
            setup,
            pending_input: None,
            visible: false,
            animate_reveal,
            fading_out: None,
            #[cfg(target_os = "windows")]
            fade_length: Duration::ZERO,
            options,
            canvas_size: spec.size,
            name: spec.name,
            at: Placement { size: spec.size, edge: spec.edge, margin: spec.margin, align: spec.align, inset: spec.inset },
            origin: None,
            interactive: spec.interactive,
            macos_level,
            shared: shared.clone(),
            rt,
            clipboard: Default::default(),
            popup: Default::default(),
            compositor,
            live: None,
            parked: None,
            release_after,
            release_at: None,
            cursor: None,
            #[cfg(target_os = "macos")]
            panel: None,
            #[cfg(target_os = "windows")]
            native: None,
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            x11: None,
            mods: Default::default(),
            renderer: None,
            gpu: None,
            gpu_pool: Rc::default(),
            dark: true,
            dirty: true,
            idle: true,
            runtime_idle: false,
            quiet: Default::default(),
            clock: FrameClock::new(Instant::now()),
            #[cfg(target_os = "macos")]
            occluded: false,
            #[cfg(target_os = "macos")]
            occlusion_read: false,
            retry: None,
            occluded_wait: crate::window::OCCLUDED_RETRY,
            hidden_pace: Default::default(),
            #[cfg(target_os = "macos")]
            frame_wake: None,
            meter,
            refollow_away: None,
        };
        (overlay, OverlayHandle { shared })
    }

    fn set_availability(&self, a: Availability) {
        let v = match a {
            Availability::Unknown => 0,
            Availability::Available => 1,
            Availability::Unavailable => 2,
        };
        self.shared.availability.store(v, Ordering::Release);
    }

    /// Marks the overlay `Unavailable` and keeps why, for `OverlayHandle::failure`.
    fn fail(&self, reason: String) {
        eprintln!("rune: overlay {} can't show: {reason}", self.name);
        *self.shared.failure.lock().unwrap() = Some(reason);
        self.set_availability(Availability::Unavailable);
    }

    fn show_fading(&mut self, event_loop: &ActiveEventLoop, fade: Duration) {
        if self.visible { return; }
        // Hidden, it missed the redraws a new accent asks for (`accent::watch`).
        self.rt.set_system_dark(crate::platform::preferences::dark().unwrap_or(self.dark));
        self.accent_changed(crate::system_accent());
        self.scroll_bars_changed(crate::platform::scroll_bars::current());
        // Shown again mid fade-out: the window is still up with current pixels, and its motion
        // carries on rather than settling.
        let fading = self.fading_out.take().is_some();
        let size = self.at.size;
        self.resolve_size(event_loop);
        if self.live.is_none() {
            self.prepare(event_loop, true);
            if self.shared.failure.lock().unwrap().is_some() { return; }
        }
        if self.live.is_none() { return; }
        self.resume_live();
        if size != self.at.size { self.resized(event_loop); }
        if self.live.is_none() { return; }
        if !fading { self.settle_before_reveal(); }
        if let Some(Backing::Window(window)) = self.live.as_ref().map(|l| &l.backing) {
            let window = window.clone();
            self.place_window(event_loop, &window);
            if let Some(gpu) = self.gpu.clone() { self.rescale(&gpu, crate::window::backing_scale(&window)); }
            if self.live.is_none() { return; }
            // Retained alpha-zero hosts already have pixels. A hidden update must reach
            // the surface before exposing it, rather than flashing the old frame.
            if !fading { self.refresh_for_reveal_at(Instant::now()); }
            self.show_window(&window, fade);
        }
        self.visible = true;
        self.release_at = None;
        self.hidden_pace.reset();
        self.occluded_wait = crate::window::OCCLUDED_RETRY;
        // Shown onto sleeping displays or under another window, the panel stays occluded.
        #[cfg(target_os = "macos")]
        {
            self.occluded = false;
            self.occlusion_read = false;
            self.sync_occlusion();
            if !self.occluded { self.ensure_pacing(); }
        }
        if !self.retained() { self.dirty = true; }
        self.idle &= !(self.rt.animating || self.compositor.as_ref().is_some_and(|c| c.animating()));
    }

    fn accent_changed(&mut self, accent: crate::SystemAccent) {
        if accent == self.rt.system_accent() { return; }
        self.rt.set_system_accent(accent);
        self.dirty = true;
    }

    fn scroll_bars_changed(&mut self, bars: crate::platform::scroll_bars::SystemScrollBars) {
        if bars == self.rt.system_scroll_bars_state() { return; }
        self.rt.set_system_scroll_bars_state(bars);
        self.dirty = true;
    }

    fn theme_changed(&mut self, dark: bool) {
        self.dark = dark;
        self.rt.set_system_dark(dark);
        self.dirty = true;
    }

    /// Makes the overlay's native window and GPU surface: at startup for a prewarmed overlay
    /// (`showing` false), or for the show. A prewarmed `release_gpu_when_hidden` overlay makes
    /// only its window at startup and parks it; its surface, and the GPU device for the first,
    /// wait for the show.
    fn prepare(&mut self, event_loop: &ActiveEventLoop, showing: bool) {
        if self.live.is_some() { return; }
        let surface = showing || !self.defers_gpu();
        let wayland = crate::window::is_wayland(event_loop);
        // Only an X11 window waits parked for its surface. A Wayland overlay's window is its
        // surface, and a Windows or macOS one that gives its GPU back is made for each show
        // (`park`): both wait.
        let parks = cfg!(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))) && !wayland;
        if !surface && !parks { return; }
        self.resolve_size(event_loop);
        let made = if wayland {
            self.layer(event_loop).or_else(|| self.xwayland())
        } else if self.parked.is_some() || self.window(event_loop).is_some() {
            // macOS made the surface with its panel; elsewhere the window waits in `parked`.
            match self.live.take() {
                Some(live) => Some(live),
                None if surface => self.unpark(),
                None => return self.ready(showing),
            }
        } else {
            None
        };
        match made {
            Some(live) => {
                self.live = Some(live);
                // An earlier attempt's failure no longer holds.
                self.shared.failure.lock().unwrap().take();
                // The surface was made before the panel was adopted and placed; its display's
                // scale is known now, before `warm` draws.
                if let Some(Backing::Window(window)) = self.live.as_ref().map(|l| &l.backing) && let Some(gpu) = self.gpu.clone() {
                    let scale = crate::window::backing_scale(window);
                    self.rescale(&gpu, scale);
                    if self.shared.failure.lock().unwrap().is_some() { return; }
                }
                self.ready(showing);
            }
            None => if self.shared.failure.lock().unwrap().is_none() {
                self.fail("the platform could not create its window or surface".into());
            } else {
                self.set_availability(Availability::Unavailable);
            },
        }
    }

    /// The window exists, with its surface or parked: the overlay can show. A prewarmed one
    /// warms what it has (parked, only the view); one that makes its surface for each show
    /// draws its own first frame instead.
    fn ready(&mut self, showing: bool) {
        self.set_availability(Availability::Available);
        if !self.animate_reveal { self.rt.settle_all(); }
        self.dirty = true;
        self.idle = true;
        if self.options.prewarm && !(showing && self.defers_gpu()) { self.warm(); }
        if !self.announced && let Some(created) = &mut self.on_created {
            self.announced = true;
            let (window_number, capabilities) = *self.shared.native.lock().unwrap();
            created(crate::OverlayCreated { window_number, capabilities });
        }
    }

    // A retained host can be prepared while hidden. Resume sampling before warm/reveal so
    // hidden publications reach that first visible frame, without waking hidden hosts.
    fn resume_live(&mut self) {
        self.rt.set_live_visible(true);
        if self.rt.live_pending() { self.dirty = true; }
    }

    fn settle_before_reveal(&mut self) {
        // Initial retained pixels were settled by warm(). Leaving a settle flag pending here
        // would snap changes arriving after that frame, in the same turn as show().
        if !self.retained() && !self.animate_reveal { self.rt.settle_all(); }
    }

    /// macOS keeps a prewarmed host's alpha-zero surface, and its pixels, while hidden, unless
    /// it gives its GPU back.
    fn retained(&self) -> bool { cfg!(target_os = "macos") && self.options.prewarm && !self.defers_gpu() }

    fn refresh_for_reveal_at(&mut self, now: Instant) {
        if self.retained() && (self.dirty || self.rt.animating) {
            self.commit_at(now);
            self.warm_frame();
        }
    }

    fn hide(&mut self) { self.hide_fading(Duration::ZERO) }

    /// Shown, or fading out: frames still draw.
    fn drawing(&self) -> bool { self.visible || self.fading_out.is_some() }

    /// A hidden overlay draws nothing more: its display link, timer frames and live values stop.
    fn stop_drawing(&mut self) {
        self.fading_out = None;
        #[cfg(target_os = "macos")]
        if let Some(live) = &mut self.live && let Some(pacing) = &mut live.pacing { pacing.stop(); }
        self.rt.set_live_visible(false);
        self.retry = None;
        self.quiet = Default::default();
    }

    /// Finishes native hiding too: AppKit's completion may stall while displays sleep. On
    /// Windows, steps the window's alpha until then.
    fn end_fade_out(&mut self, now: Instant) {
        let Some(end) = self.fading_out else { return };
        if end <= now { return self.hide(); }
        #[cfg(target_os = "windows")]
        if let Some(native) = &self.native { native.fade(fade_alpha(end, self.fade_length, now)); }
    }

    fn hide_fading(&mut self, fade: Duration) {
        // A hide need not report a blur, and a hidden overlay hears none: what it held is
        // released, after any Fn change the monitor saw first.
        #[cfg(target_os = "macos")]
        self.fn_key();
        if self.options.keys == KeyPolicy::OnDemand { self.rt.keys_lost(); }
        let was_shown = self.visible || self.fading_out.is_some();
        // Only macOS and Windows fade the window; elsewhere it goes at once, and so does drawing.
        let fading = cfg!(any(target_os = "macos", target_os = "windows")) && !fade.is_zero() && (self.visible || self.fading_out.is_some());
        if fading {
            self.fading_out = Some(Instant::now() + fade);
            #[cfg(target_os = "windows")]
            { self.fade_length = fade; }
        } else {
            self.stop_drawing();
        }
        self.visible = false;
        self.popup.cancel(&mut self.rt);
        self.rt.cancel_pointer();
        self.apply_cursor();
        self.hidden_input();
        let Some(live) = &self.live else {
            if was_shown { self.hidden_release(Instant::now()); }
            return;
        };
        match &live.backing {
            Backing::Window(window) => {
                #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
                if let Some(x11) = &self.x11 { x11.hide(); }
                // A fading window stays up; `end_fade_out` steps its alpha, then hides it.
                #[cfg(target_os = "windows")]
                {
                    let _ = window;
                    if let Some(native) = &mut self.native {
                        if fading { native.fade(255) } else { native.hide() }
                    }
                }
                #[cfg(target_os = "macos")]
                if let Some(panel) = &self.panel {
                    panel.hide(fade);
                }
                #[cfg(not(target_os = "windows"))]
                window.set_visible(false);
            }
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            Backing::Layer(_) | Backing::Xwayland(_) => { self.live.take(); }
        }
        if was_shown { self.hidden_release(Instant::now()); }
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    fn layer(&mut self, event_loop: &ActiveEventLoop) -> Option<Live> {
        use winit::raw_window_handle::HasDisplayHandle;
        use winit::platform::wayland::MonitorHandleExtWayland;
        let display = event_loop.display_handle().ok()?.as_raw();
        let monitor = monitor(event_loop, &self.options.display);
        let scale = monitor.as_ref().map_or(1.0, |m| m.scale_factor()).ceil() as i32;
        // With no target, the compositor chooses an output; global pointer querying is forbidden.
        let output = match &self.options.display {
            DisplayTarget::Id(id) if event_loop.available_monitors().any(|m| m == id.0) => Some(id.0.native_id()),
            DisplayTarget::Native(crate::NativeDisplayId::Wayland(id)) if event_loop.available_monitors().any(|m| m.native_id() == *id) => Some(*id),
            _ => None,
        };
        let wake = self.shared.wake.lock().unwrap().clone();
        let mut layer = crate::platform::layer_shell::LayerSurface::new(display, self.name, self.at, self.interactive, &self.options, output, wake)?;
        layer.set_buffer_scale(scale);
        if let Some((w, h)) = layer.configured_size() {
            if w > 0 { self.at.size.0 = w as f32; }
            if h > 0 { self.at.size.1 = h as f32; }
        }
        self.shared.native.lock().unwrap().1 = crate::OverlayCapabilities {
            capture_exclusion: OverlaySupport::Unsupported, global_pointer: OverlaySupport::Unsupported,
            nonactivating_keys: if layer.supports_keys() { OverlaySupport::Supported } else { OverlaySupport::Unsupported },
        };
        self.attach(Backing::Layer(layer), scale as f32)
    }

    #[cfg(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
    fn layer(&mut self, _event_loop: &ActiveEventLoop) -> Option<Live> {
        None
    }

    /// Makes the native window: on macOS with its GPU surface (`live`), which binds winit's view
    /// before the view moves into the panel; elsewhere without one (`parked`).
    fn window(&mut self, event_loop: &ActiveEventLoop) -> Option<()> {
        let mut attrs = Window::default_attributes()
            .with_title(self.name)
            .with_inner_size(LogicalSize::new(self.at.size.0 as f64, self.at.size.1 as f64))
            .with_decorations(false)
            .with_resizable(false)
            .with_transparent(true)
            .with_active(false)
            .with_visible(false)
            .with_window_level(WindowLevel::AlwaysOnTop);
        if let Some(p) = position(event_loop, self.at, &self.options) {
            attrs = attrs.with_position(p);
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        {
            use winit::platform::x11::{WindowAttributesExtX11, WindowType};
            attrs = attrs.with_override_redirect(true).with_x11_window_type(vec![WindowType::Notification]);
            if let Some(id) = crate::tooling::app_id() {
                attrs = attrs.with_name(id, id);
            }
        }
        #[cfg(target_os = "windows")]
        {
            use winit::platform::windows::WindowAttributesExtWindows;
            // DirectComposition draws the overlay (`Gpu::instance`); without a redirection bitmap
            // nothing under it paints the window black.
            attrs = attrs.with_skip_taskbar(true).with_undecorated_shadow(false).with_no_redirection_bitmap(true);
        }
        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attrs = attrs.with_has_shadow(false);
        }
        let window = match event_loop.create_window(attrs) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                self.fail(format!("could not create its window: {error}"));
                return None;
            }
        };
        #[cfg(target_os = "macos")]
        crate::native_menu::adopt_standard_view();
        self.dark = window.theme().map_or_else(|| crate::platform::preferences::desktop_dark(&window).unwrap_or(true), |t| t == winit::window::Theme::Dark);
        self.rt.set_system_dark(self.dark);
        if self.cursor.is_none() { self.cursor = Some(crate::cursor::os::OsCursor::new(event_loop)); }
        // The surface first: on macOS it binds winit's view and its layer, which then move
        // into the panel.
        #[cfg(target_os = "macos")]
        let live = self.attach(Backing::Window(window.clone()), crate::window::backing_scale(&window))?;
        #[cfg(target_os = "macos")]
        {
            let wake = self.shared.wake.lock().unwrap().clone();
            self.panel = Some(crate::platform::macos::Panel::adopt(&window, self.at, self.interactive, &self.options, wake)?);
            let panel = self.panel.as_ref().unwrap();
            if let Some(level) = self.macos_level { panel.set_level(level); }
            let native = (Some(panel.window_number()), crate::OverlayCapabilities {
                capture_exclusion: panel.capture_exclusion(), nonactivating_keys: OverlaySupport::Supported, global_pointer: OverlaySupport::Supported,
            });
            *self.shared.native.lock().unwrap() = native;
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = window.set_cursor_hittest(self.options.input.resolved(self.interactive) == OverlayInput::All);
            #[cfg(all(unix, not(target_os = "ios"), not(target_os = "android")))]
            {
                self.x11 = crate::platform::x11::overlay::InputWindow::new(&window, self.shared.wake.lock().unwrap().clone(), self.options.input.resolved(self.interactive), self.options.follow_pointer);
                if self.options.input == OverlayInput::Regions && self.x11.is_none() { return None; }
                self.shared.native.lock().unwrap().1 = crate::OverlayCapabilities {
                    capture_exclusion: OverlaySupport::Unsupported, nonactivating_keys: OverlaySupport::Unsupported, global_pointer: OverlaySupport::Supported,
                };
            }
            #[cfg(target_os = "windows")]
            {
                let wake = self.shared.wake.lock().unwrap().clone();
                match crate::platform::win32::OverlayWindow::new(&window, self.interactive, &self.options, wake) {
                    Ok(native) => self.native = Some(native),
                    Err(error) => {
                        self.fail(error);
                        return None;
                    }
                }
                self.shared.native.lock().unwrap().1 = crate::OverlayCapabilities {
                    capture_exclusion: self.native.as_ref().unwrap().capture_exclusion(), nonactivating_keys: OverlaySupport::Unsupported, global_pointer: OverlaySupport::Supported,
                };
            }
        }
        self.place_window(event_loop, &window);
        #[cfg(target_os = "macos")]
        { self.live = Some(live); }
        #[cfg(not(target_os = "macos"))]
        { self.parked = Some(Backing::Window(window)); }
        Some(())
    }

    /// The parked window's GPU surface, and the device for the first surface. Failing, the
    /// window stays parked for the next show to try again.
    fn unpark(&mut self) -> Option<Live> {
        let Some(Backing::Window(window)) = self.parked.take() else { return None };
        let live = self.attach(Backing::Window(window.clone()), crate::window::backing_scale(&window));
        if live.is_none() { self.parked = Some(Backing::Window(window)); }
        live
    }

    /// `release_gpu_when_hidden` or `release_gpu_after`. macOS honours only the first: a
    /// prewarmed panel with `release_gpu_after` (a HUD) retains its pixels.
    fn defers_gpu(&self) -> bool {
        if cfg!(target_os = "macos") { self.release_after == Some(Duration::ZERO) } else { self.release_after.is_some() }
    }

    /// Just hidden: gives the GPU surface back now, or schedules it for `release_gpu_after`. A
    /// fade-out keeps it until the fade ends (`end_fade_out` hides again), so the exit plays
    /// out on the window and its last frames reach the screen first.
    fn hidden_release(&mut self, now: Instant) {
        if !self.defers_gpu() || self.fading_out.is_some() { return; }
        match self.release_after {
            Some(after) if !after.is_zero() => self.release_at = Some(now + after),
            _ => self.park(),
        }
    }

    /// `release_gpu_after`'s time came while the overlay stayed hidden.
    pub(super) fn release_due(&mut self, now: Instant) {
        if self.release_at.is_some_and(|at| at <= now) && !self.drawing() {
            self.release_at = None;
            self.park();
        }
    }

    /// `release_gpu_when_hidden`: the hidden overlay gives back its surface, swapchain and
    /// renderer, and its hold on the GPU device, which waits a while for the next surface
    /// (`render::park_gpu`) once no other surface draws with it. The
    /// window stays parked on X11 only. On Windows a hidden topmost window, often
    /// display-sized, in a process that draws with DXGI is enough for game capture tools such
    /// as NVIDIA's Instant Replay to take the app for the running game; on macOS 27 hidden
    /// screen-sized panels that join every Space empty Mission Control's Space thumbnails.
    /// There the window goes too, and the next show makes a new one.
    fn park(&mut self) {
        if !self.defers_gpu() || !matches!(self.live.as_ref().map(|l| &l.backing), Some(Backing::Window(_))) { return; }
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        self.release();
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            let Some(Live { alpha, surface, backing, .. }) = self.live.take() else { return };
            // The surface goes before the device that made it and the window it draws into.
            drop(alpha);
            drop(surface);
            self.parked = Some(backing);
        }
        // The device goes to the next surface rather than with this one (hex#1268); one another
        // surface still draws with stays there.
        if let (Some(gpu), Some(renderer)) = (self.gpu.take(), self.renderer.take()) && let Ok(gpu) = Arc::try_unwrap(gpu) {
            crate::render::park_gpu(gpu, renderer);
        }
        *self.shared.surface.lock().unwrap() = None;
    }

    /// Moving the window under a still pointer (`place`, `set_placement`, a resize that keeps
    /// the far edge) keeps the runtime's pointer and a held drag in step, so a drag that moves
    /// its own overlay reports pointer travel. Hosts that place asynchronously catch up on the
    /// next pointer event.
    fn place_window(&mut self, event_loop: &ActiveEventLoop, window: &Window) {
        self.position_window(event_loop, window);
        // From the window itself: `window()` places it before it's live.
        self.sync_origin(window);
        let Ok(origin) = window.outer_position() else { return };
        if let Some(before) = self.origin.replace(origin) && before != origin {
            let scale = window.scale_factor();
            let points = |a: i32, b: i32| (f64::from(a - b) / scale) as f32;
            self.rt.window_moved(points(origin.x, before.x), points(origin.y, before.y));
        }
    }

    /// Tells the runtime where the overlay's content is on screen (see `El::on_screen_layout`).
    fn sync_origin(&mut self, window: &Window) {
        self.rt.set_window_pixels(window.inner_position().ok().map(|p| (p.x, p.y)));
        if let Some(origin) = crate::window::screen_origin(window) { self.origin_moved(origin); }
    }

    fn sync_live_origin(&mut self) {
        let Some(Backing::Window(window)) = self.live.as_ref().map(|l| &l.backing) else { return };
        let window = window.clone();
        self.sync_origin(&window);
    }

    /// Hidden too, the moved rects wait in the queue and mark the overlay dirty, so a retained
    /// reveal steps once (`refresh_for_reveal_at`) and delivers them; only a drawing overlay
    /// turns the loop for them now.
    fn origin_moved(&mut self, origin: (f32, f32)) {
        if !self.rt.set_window_origin(origin) { return; }
        self.dirty = true;
        if self.drawing() { self.idle = false; }
    }

    fn position_window(&self, event_loop: &ActiveEventLoop, window: &Window) {
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel {
            panel.place(self.at, &self.options);
            return;
        }
        if let Some(p) = position(event_loop, self.at, &self.options) {
            window.set_outer_position(p);
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(x11) = &self.x11 { x11.update_geometry(); }
    }

    /// Puts the window on screen without focusing it or activating the app.
    fn show_window(&mut self, window: &Window, fade: Duration) {
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.panel {
            panel.show(fade);
            return;
        }
        let _ = fade;
        #[cfg(target_os = "windows")]
        {
            let _ = window;
            if let Some(native) = &mut self.native {
                match native.show() {
                    Ok(()) => if self.shared.failure.lock().unwrap().take().is_some() { self.set_availability(Availability::Available) },
                    Err(error) => self.fail(error),
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        window.set_visible(true);
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        if let Some(x11) = &self.x11 { x11.show(self.options.level); }
    }

    fn resized(&mut self, event_loop: &ActiveEventLoop) {
        // A parked window takes the size now; its surface is made at it.
        if let Some(Backing::Window(w)) = &self.parked {
            let w = w.clone();
            let _ = w.request_inner_size(LogicalSize::new(self.at.size.0 as f64, self.at.size.1 as f64));
            return self.place_window(event_loop, &w);
        }
        let Some(live) = self.live.as_mut() else { return };
        match &mut live.backing {
            Backing::Window(w) => {
                let _ = w.request_inner_size(LogicalSize::new(self.at.size.0 as f64, self.at.size.1 as f64));
            }
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            Backing::Layer(layer) => {
                layer.place(self.at);
                layer.commit();
            }
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            Backing::Xwayland(surface) => surface.place(self.at, &self.options),
        }
        if let Some(gpu) = self.gpu.clone() {
            let scale = live.scale;
            live.scale = 0.0;
            self.rescale(&gpu, scale);
        }
        if let Some(Backing::Window(w)) = self.live.as_ref().map(|l| &l.backing) {
            let w = w.clone();
            self.place_window(event_loop, &w);
            if let Some(gpu) = self.gpu.clone() { self.rescale(&gpu, crate::window::backing_scale(&w)); }
        }
    }

    fn resolve_size(&mut self, event_loop: &ActiveEventLoop) {
        let at = Placement { size: self.canvas_size, ..self.at };
        #[cfg(target_os = "macos")]
        if let Some(rect) = self.panel.as_ref().map_or_else(|| crate::platform::macos::overlay_rect(at, &self.options), |panel| panel.rect(at, &self.options)) {
            self.at.size = (rect.size.width as f32, rect.size.height as f32);
            return;
        }
        if let Some(monitor) = monitor(event_loop, &self.options.display) {
            let area = monitor.size().to_logical::<f32>(monitor.scale_factor());
            let rect = crate::overlay::options::local_rect(at, self.options.placement, (area.width, area.height));
            self.at.size = (rect[2], rect[3]);
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
    fn follow_pointer(&mut self, event_loop: &ActiveEventLoop, point: (i32, i32)) {
        if !self.options.follow_pointer { return; }
        self.move_to_pointer_display(event_loop, point);
    }

    /// The pointer in desktop pixels, read now, where an overlay may follow it.
    #[cfg(target_os = "windows")]
    fn sample_pointer(_: &ActiveEventLoop) -> Option<(i32, i32)> {
        crate::platform::win32::pointer_position()
    }

    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    fn sample_pointer(event_loop: &ActiveEventLoop) -> Option<(i32, i32)> {
        (!crate::window::is_wayland(event_loop)).then(crate::platform::x11::overlay::pointer_position).flatten()
    }

    #[cfg(not(any(target_os = "windows", all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))))]
    fn sample_pointer(_: &ActiveEventLoop) -> Option<(i32, i32)> {
        None
    }

    /// Moves a window-backed `UnderPointer` overlay to the display containing `point` when it
    /// isn't there.
    /// The display containing `point` when a window-backed `UnderPointer` overlay isn't on it.
    fn pointer_display_away(&self, event_loop: &ActiveEventLoop, point: (i32, i32)) -> Option<winit::monitor::MonitorHandle> {
        if self.options.display != DisplayTarget::UnderPointer { return None; }
        let Some(Backing::Window(window)) = self.live.as_ref().map(|l| &l.backing) else { return None };
        event_loop.available_monitors().find(|m| {
            let (p, s) = (m.position(), m.size());
            point.0 >= p.x && point.1 >= p.y && i64::from(point.0) < i64::from(p.x) + i64::from(s.width)
                && i64::from(point.1) < i64::from(p.y) + i64::from(s.height)
        }).filter(|target| window.current_monitor().as_ref() != Some(target))
    }

    /// `refollow_pointer`'s check: the overlay moves once two checks in a row find the pointer
    /// on the same other display, so a pointer crossing back and forth over a display's edge
    /// doesn't flip it between them.
    fn refollow(&mut self, event_loop: &ActiveEventLoop, point: (i32, i32)) {
        let away = self.pointer_display_away(event_loop, point);
        if away.is_some() && away == self.refollow_away {
            self.refollow_away = None;
            self.move_to_pointer_display(event_loop, point);
        } else {
            self.refollow_away = away;
        }
    }

    fn move_to_pointer_display(&mut self, event_loop: &ActiveEventLoop, point: (i32, i32)) {
        if let Some(target) = self.pointer_display_away(event_loop, point) {
            // Pin the sampled target across size and position resolution, then resume following.
            self.options.display = DisplayTarget::Id(crate::DisplayId::from_monitor(target, crate::window::is_wayland(event_loop)));
            self.resolve_size(event_loop);
            self.resized(event_loop);
            self.options.display = DisplayTarget::UnderPointer;
        }
    }
}

#[path = "overlay/surface.rs"]
mod surface;

#[cfg(test)]
#[path = "overlay/tests.rs"]
mod tests;

/// A Windows overlay's alpha (255 opaque) at `now`, fading out linearly over `length` until `end`.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn fade_alpha(end: Instant, length: Duration, now: Instant) -> u8 {
    if length.is_zero() { return 0; }
    let left = end.saturating_duration_since(now).as_secs_f64() / length.as_secs_f64();
    (left.clamp(0.0, 1.0) * 255.0).round() as u8
}
