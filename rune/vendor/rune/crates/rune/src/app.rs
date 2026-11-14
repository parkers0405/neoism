//! The app's lifetime beside its window. `App` runs an `AppSpec` with or without a window and
//! decides what closing it means; `AppHandle` opens, focuses, closes and feeds the running app
//! from any thread.
//!
//! The state, scene and text live with the app, not the window: closing the window keeps them,
//! and reopening shows the same UI, settled. Closing hides the window where the platform can
//! (macOS, Windows, X11) and destroys it on Wayland, where a hidden toplevel doesn't exist;
//! the GPU device and renderer stay either way.
//!
//! ```ignore
//! let app = rune::App::new(spec).windowless().keep_running();
//! let handle = app.handle();
//! std::thread::spawn(move || {
//!     for request in requests() {
//!         match request {
//!             Request::Activate(token) => handle.activate(token),
//!             Request::Snapshot(s) => handle.update(move |ui, _| ui.snapshot = s),
//!             Request::Quit => handle.quit(),
//!         }
//!     }
//! });
//! app.run();
//! ```

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::el::Tx;
use crate::tooling::{AppSpec, Args};

#[path = "app/web.rs"]
mod web;
#[cfg(target_arch = "wasm32")]
pub(crate) use web::with_font;

type Update<S> = Box<dyn FnOnce(&mut S, &mut Tx) -> bool + Send>;
/// `App::on_window`'s hook, run after each native window creation.
pub(crate) type OnWindow = Box<dyn FnMut(&crate::WindowHandle)>;

/// `App::turn_scope`'s hooks.
pub(crate) struct TurnScope<S> {
    pub(crate) enter: fn(&mut S),
    pub(crate) leave: fn(&mut S),
}

impl<S> Clone for TurnScope<S> {
    fn clone(&self) -> Self { *self }
}

impl<S> Copy for TurnScope<S> {}

/// An `AppSpec` with its lifetime: by default it opens one window and quits when it closes,
/// like `rune::run`.
pub struct App<S: 'static> {
    pub(crate) spec: AppSpec<S>,
    pub(crate) chrome: crate::Chrome,
    pub(crate) titlebar: Option<crate::Titlebar>,
    init: Option<Box<dyn FnOnce() -> S>>,
    argv: Option<Vec<String>>,
    pub(crate) open: bool,
    pub(crate) release_gpu: bool,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) accessory: bool,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) standard_menus: bool,
    pub(crate) keep_running: bool,
    pub(crate) on_close: Option<fn(&mut S, &mut Tx)>,
    pub(crate) on_quit: Option<fn(&mut S, &mut Tx)>,
    pub(crate) turn_scope: Option<TurnScope<S>>,
    pub(crate) on_focus: Option<fn(&mut S, bool, &mut Tx)>,
    pub(crate) on_mods: Option<fn(&mut S, crate::Mods, &mut Tx)>,
    pub(crate) on_ime: Option<fn(&mut S, &crate::Ime, &mut Tx) -> bool>,
    pub(crate) on_reopen: Option<fn(&mut S, bool, &mut Tx) -> bool>,
    pub(crate) on_open_url: Option<crate::open_url::incoming::OnOpenUrl<S>>,
    pub(crate) shared: Arc<Shared<S>>,
    pub(crate) overlays: Vec<Box<dyn crate::overlay::PendingOverlay>>,
    pub(crate) windows: Vec<crate::AnyWindowSpec>,
    pub(crate) on_window: Option<OnWindow>,
    pub(crate) on_ready: Option<OnWindow>,
    pub(crate) on_frame_encoded: Option<crate::frame::encoded::OnFrameEncoded>,
    pub(crate) on_frame_drawn: Option<crate::frame::encoded::OnFrameDrawn<S>>,
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) same_tab_router: Option<Box<dyn Fn(&str) -> bool>>,
    pub(crate) macos_window: crate::MacWindowOptions,
    pub(crate) compositor: Option<Box<dyn crate::compose::Compositor<S>>>,
    pub(crate) window_options: crate::WindowOptions,
    pub(crate) canvas_id: Option<String>,
    pub(crate) full_viewport: bool,
    pub(crate) fit_content_height: bool,
    pub(crate) web_document: bool,
    /// `App::font_url`'s fonts, which a web app fetches before its first frame.
    #[cfg(target_arch = "wasm32")]
    pub(crate) font_urls: Vec<String>,
    pub(crate) float_on_open: bool,
    /// `App::open_inactive`: the first open leaves focus where it is.
    pub(crate) open_inactive: bool,
    /// `App::maximized`: the window opens maximized.
    pub(crate) maximized: bool,
    pub(crate) modifier_keys: bool,
    pub(crate) tab_visits_controls: bool,
    pub(crate) srgb_blend: bool,
    pub(crate) font_smoothing: crate::FontSmoothing,
    pub(crate) dev_stats: Option<fn() -> String>,
    pub(crate) playground: Option<crate::Playground<S>>,
}

impl<S: 'static> App<S> {
    pub fn new(spec: AppSpec<S>) -> Self {
        let shared: Arc<Shared<S>> = Arc::default();
        shared.shortcuts.named(spec.title);
        App {
            spec,
            chrome: crate::Chrome::DEFAULT,
            titlebar: None,
            init: None,
            argv: None,
            open: true,
            release_gpu: false,
            accessory: false,
            standard_menus: false,
            keep_running: false,
            on_close: None,
            on_quit: None,
            turn_scope: None,
            on_focus: None,
            on_mods: None,
            on_ime: None,
            on_reopen: None,
            on_open_url: None,
            shared,
            overlays: Vec::new(),
            windows: Vec::new(),
            on_window: None,
            on_ready: None,
            on_frame_encoded: None,
            on_frame_drawn: None,
            same_tab_router: None,
            macos_window: crate::MacWindowOptions::default(),
            compositor: None,
            window_options: crate::WindowOptions::default(),
            canvas_id: None,
            full_viewport: false,
            fit_content_height: false,
            web_document: false,
            #[cfg(target_arch = "wasm32")]
            font_urls: Vec::new(),
            float_on_open: false,
            open_inactive: false,
            maximized: false,
            modifier_keys: false,
            tab_visits_controls: false,
            srgb_blend: true,
            font_smoothing: crate::FontSmoothing::default(),
            dev_stats: None,
            playground: None,
        }
    }

    /// Composite this window's translucent fills, blurs, shadows, images, video and text in
    /// sRGB (gamma) space instead of linear light, matching CSS and Core Animation over dark
    /// backgrounds, as CoreText does. On by default. With a `Compositor`, `Frame::srgb_blend`
    /// says which encoding its passes must match.
    pub fn srgb_blend(mut self, enabled: bool) -> Self {
        self.srgb_blend = enabled;
        self
    }

    /// How this app's windows rasterise and place glyphs (see [`crate::FontSmoothing`]):
    /// `On` by default on macOS and in a Mac's browser, `Off` elsewhere (CoreGraphics' pipeline
    /// without font smoothing's dilation, as `-webkit-font-smoothing: antialiased` draws).
    /// `Tx::set_font_smoothing` switches it while running.
    pub fn font_smoothing(mut self, mode: crate::FontSmoothing) -> Self {
        self.font_smoothing = mode;
        self
    }

    /// Configure this window's frame. Overlays retain their own native behaviour.
    pub fn chrome(mut self, chrome: crate::Chrome) -> Self {
        self.chrome = chrome;
        self
    }

    /// Draw this window's titlebar in the view, `Titlebar::height` points tall, with the
    /// window's own controls over it (see [`crate::Titlebar`]): macOS keeps its traffic lights,
    /// Windows gets caption buttons Rune draws, and `Cx::chrome_insets` says what they take.
    /// Mark the band `.window_drag()`.
    pub fn titlebar(mut self, titlebar: crate::Titlebar) -> Self {
        self.titlebar = Some(titlebar);
        self
    }

    /// Native options for this app's window (size limits, resizability, ...). `WindowSpec::from_app`
    /// starts from them, and `WindowSpec::options` replaces them.
    pub fn window_options(mut self, options: crate::WindowOptions) -> Self {
        self.window_options = options;
        self
    }

    /// Size the window's height to its content, as SwiftUI's `.windowResizability(.contentSize)`
    /// does: the root lays out at its natural height for the window's width, the window opens
    /// at that height and follows it when the content changes, on the change's spring like
    /// `Tx::resize_window`. Give the root no fixed or `fill` height. Users still set the width.
    pub fn fit_content_height(mut self) -> Self {
        self.fit_content_height = true;
        self
    }

    /// Ask a tiling window manager to float the window when it opens, with no window rule.
    /// On Linux the window maps at a fixed size (its minimum equals its maximum), which
    /// sway, Hyprland, niri and i3 float; once its first frame is shown, the window's own
    /// minimum and maximum apply and the user can resize it. Ignored elsewhere.
    pub fn float_on_open(mut self) -> Self {
        self.float_on_open = true;
        self
    }

    /// The window's first open shows it without taking focus or activating the app, so the
    /// window the user works in keeps its keys and a press it holds. A later `open` or `focus`
    /// activates it. macOS and Windows; elsewhere the window manager decides.
    pub fn open_inactive(mut self) -> Self {
        self.open_inactive = true;
        self
    }

    /// Open the window maximized, as a window saved maximized reopens. The size it opens at
    /// stays what restoring it returns to.
    pub fn maximized(mut self, maximized: bool) -> Self {
        self.maximized = maximized;
        self
    }

    /// Send modifier keys pressed and released on their own (`Key::Modifier`, with the side)
    /// to `on_key` and the focused input's key handler, for hold-to-talk on Fn or Right Option,
    /// or a Shift tap. Off by default, since handlers written without it may act on any key.
    pub fn modifier_keys(mut self) -> Self {
        self.modifier_keys = true;
        self
    }

    /// Tab and Shift-Tab visit every focusable control, buttons included, whatever the system's
    /// keyboard navigation setting. Off (the default), Tab follows macOS: with Keyboard
    /// navigation off it visits only text fields and lists.
    pub fn tab_visits_controls(mut self, on: bool) -> Self {
        self.tab_visits_controls = on;
        self
    }

    /// Native and portable options together, including `WindowOptions::launcher_panel()`.
    pub fn configured_options(mut self, options: crate::ConfiguredWindowOptions) -> Self {
        self.window_options = options.options;
        self.macos_window = options.native;
        self
    }

    /// Native window policy, also retained by a single-window App.
    pub fn macos_window(mut self, options: crate::MacWindowOptions) -> Self {
        self.macos_window = options;
        self
    }

    /// Called once per native focus transition, including while hidden or occluded.
    /// Headless scripts deliver the same callback with `{"focus": false}`.
    pub fn on_focus(mut self, f: fn(&mut S, bool, &mut Tx)) -> Self {
        self.on_focus = Some(f);
        self
    }

    /// Hears the modifier keys held down (Shift, Control, Alt or Option, the logo key) each time
    /// they change, wherever focus is and whether or not anything takes the key, as a web page's
    /// window-level `keydown`/`keyup` for `Alt` hear it; and none held once the window loses
    /// focus (a `blur`). For a reveal while a modifier is held: Desktop's Home shows every
    /// session's worktree and branch while Alt is down. Return nothing: it can't take the key
    /// from anyone, and [`Tx::live_only`] says the view reads nothing that changed.
    ///
    /// ```ignore
    /// app.on_modifiers(|s: &mut App, mods, _| s.alt_held = mods.alt)
    /// ```
    pub fn on_modifiers(mut self, f: fn(&mut S, crate::Mods, &mut Tx)) -> Self {
        self.on_mods = Some(f);
        self
    }

    /// Hear the input method for an editor the app draws itself, whose caret is marked with
    /// [`El::ime_cursor`](crate::El::ime_cursor): `Ime::Preedit` with its cursor range (draw it
    /// at the caret, not yet in the text; empty ends it), `Ime::Commit` (insert it) and
    /// `Ime::Enabled`/`Disabled`. Return true when the editor took the event. A focused
    /// `text_input` takes them instead, but a composition this hook has open ends here even
    /// after focus moved.
    ///
    /// Typing reaches the editor once: a plain character arrives as a key, `Key::Text` in
    /// `on_key`, and `Ime::Commit` carries only what an input method composed (⌥E E's `é`, a
    /// CJK conversion, the emoji picker, dictation), as for `text_input`. While a composition
    /// is open, Return and Escape go to the input method, not `on_key`. Native windows only.
    pub fn on_ime(mut self, f: fn(&mut S, &crate::Ime, &mut Tx) -> bool) -> Self {
        self.on_ime = Some(f);
        self
    }

    /// Adds a transparent overlay with its own state, hidden until `OverlayHandle::show`.
    /// Bare specs preserve primary-display edge placement and never request key focus;
    /// `OverlaySpec::options` selects displays, placement and native input policy.
    /// Uses this app's fonts. Headless runs don't draw overlays.
    pub fn overlay<T: 'static>(&mut self, spec: impl Into<crate::ConfiguredOverlay<T>>, state: T) -> crate::overlay::OverlayHandle<T> {
        let (overlay, handle) = crate::overlay::Overlay::new(spec.into(), state, &self.spec);
        self.overlays.push(Box::new(overlay));
        handle
    }

    /// Adds an ordinary window with its own state type to this app's event loop, such as a
    /// settings window beside the main one. `spec.options` applies (`visible: false` creates it
    /// hidden; `App::windowless` on its app defers creation), and either handle opens it later.
    /// Focus, input, clocks and redraws stay with each window; quitting, the Dock icon and
    /// accessory mode stay with the process.
    ///
    /// Closing it hides it and runs its `on_close`, like `keep_running`: reopening shows the
    /// same state (Wayland destroys and recreates the window). It never keeps the app running
    /// alone: the app still ends with its own window, and `Tx::quit` from either quits both.
    /// Headless tools select it with `--window KEY`; otherwise `--scene` and `--size` are left
    /// to the primary window.
    pub fn window<T: 'static>(&mut self, spec: crate::WindowSpec<T>) -> (AppHandle<T>, crate::WindowHandle) {
        let (app, window) = (spec.app.handle(), crate::WindowHandle::new(&spec.key));
        self.windows.push(crate::AnyWindowSpec::secondary(spec, window.clone()));
        (app, window)
    }

    /// Build the first state with `f` instead of `AppSpec::init`, so it can capture what the
    /// host already made (a channel, a handle to its runtime). Scenes still apply on top.
    pub fn init(mut self, f: impl FnOnce() -> S + 'static) -> Self {
        self.init = Some(Box::new(f));
        self
    }

    /// Read Rune's flags from `argv` (without the program name) instead of the process's
    /// arguments. An app that owns its command line passes only what it means for Rune, often
    /// nothing.
    pub fn args(mut self, argv: impl IntoIterator<Item = String>) -> Self {
        self.argv = Some(argv.into_iter().collect());
        self
    }

    /// Start without a window; `AppHandle::open` or `activate` shows it. Implies `keep_running`.
    pub fn windowless(mut self) -> Self {
        self.open = false;
        self.keep_running = true;
        self
    }

    /// Closing the window keeps the app, its state and its event loop running until
    /// `AppHandle::quit` or `Tx::quit`.
    pub fn keep_running(mut self) -> Self {
        self.keep_running = true;
        self
    }

    /// Closing the window also releases the GPU device and renderer (and destroys the window
    /// everywhere), so an app idling without a window holds no UI GPU memory. Reopening
    /// recreates them, which costs a device creation.
    pub fn release_gpu_when_closed(mut self) -> Self {
        self.release_gpu = true;
        self
    }

    /// Draws with `backend` on macOS. Without a call, a Mac app draws with Rune's native Metal
    /// backend (the `metal-backend` feature, on by default), so `GpuBackend::Wgpu` opts out;
    /// `GpuBackend::Metal` needs the feature. `RUNE_GPU_BACKEND=wgpu` or `=metal` overrides it
    /// without a rebuild. It applies to every GPU the process makes from here on, and a
    /// `prewarm_gpu` already running on the other backend starts over on this one; an app that
    /// opts out calls `rune::prewarm_gpu_on(GpuBackend::Wgpu)` first in `main` instead of
    /// `prewarm_gpu()` to avoid the restart. An iOS app, which embeds a `LayerHost` instead,
    /// calls `rune::prewarm_gpu_on`. Does nothing elsewhere.
    pub fn gpu_backend(self, backend: crate::GpuBackend) -> Self {
        crate::gpu::backend::choose(backend);
        self
    }

    /// macOS: run as an accessory (menu-bar) app: no Dock icon, no default menu bar, and
    /// launching doesn't activate the app or take focus. The app installs its own menu if it
    /// wants one (`standard_menus` gives it Close, Minimize and Edit), and may change the
    /// activation policy later. No effect on other platforms.
    pub fn accessory(mut self) -> Self {
        self.accessory = true;
        self
    }

    /// macOS: add the standard menu items every Mac app has to the menu bar of this process
    /// (hidden for an accessory app, whose key equivalents still work): an Edit menu (Undo ⌘Z,
    /// Redo ⇧⌘Z, Cut, Copy, Paste, Select All) and Window › Minimize ⌘M and Close ⌘W, each
    /// sent to the first responder. AppKit windows in the process (an open panel, an updater)
    /// get their editing and close keys back. In a Rune window every one of these keys still
    /// reaches text fields, `on_key` and shortcuts first, once, exactly as without the menu;
    /// a ⌘W or ⌘M nothing takes then closes or minimizes a window that has that title-bar
    /// button, as a click on it would (`on_close` and `keep_running` apply). No Quit item.
    /// Existing menus and custom `MenuItem`s are kept; items go into menus of the same title.
    /// Installed once per process when the first window of an app with it starts. A regular
    /// (non-accessory) app whose menu bar is still winit's default gets these items without
    /// asking. No effect on other platforms.
    pub fn standard_menus(mut self) -> Self {
        self.standard_menus = true;
        self
    }

    /// Runs whenever the window closes (close button, `Tx::close_window`, `AppHandle::close`,
    /// macOS Quit from the menu, Cmd-Q or the Dock), before it goes: flush drafts, stop previews.
    /// Its `Tx::quit` quits.
    pub fn on_close(mut self, f: fn(&mut S, &mut Tx)) -> Self {
        self.on_close = Some(f);
        self
    }

    /// Runs once in every open window when the app starts to quit, before any window closes
    /// for it: `Tx::quit` or `AppHandle::quit` from any window, the last window closing (in the
    /// windows still open), macOS Quit (⌘Q, the Dock, logout, restart and shutdown), and a
    /// Windows logoff or shutdown (`WM_QUERYENDSESSION`, or `WM_ENDSESSION` when no query came
    /// first), as Electron's `before-quit` and `session-end` do between them. Save what must
    /// survive the quit here, synchronously: a Windows session may end the process once the
    /// hooks returned. From then on [`crate::quitting`] holds, so an `on_close` that follows
    /// (macOS, the end of a Windows session) can tell the app ending from its window closing
    /// alone. A Windows shutdown another app refuses clears it, and a later quit runs this again.
    /// What it asks of its `Tx` beyond state is moot once the loop ends. Native windows only.
    pub fn on_quit(mut self, f: fn(&mut S, &mut Tx)) -> Self {
        self.on_quit = Some(f);
        self
    }

    /// Runs `enter` before each turn of the window's event handling that can reach its state
    /// (an event, a wake, a frame, a hook), and `leave` after it, so state the app's windows
    /// share can move into the state of the window whose turn it is and back. The windows of
    /// one event loop take turns, never two at once. Native windows only.
    pub fn turn_scope(mut self, enter: fn(&mut S), leave: fn(&mut S)) -> Self {
        self.turn_scope = Some(TurnScope { enter, leave });
        self
    }

    /// macOS: runs on the main thread when the app is reopened while it runs: its Dock icon
    /// clicked, or opened again from Finder, Spotlight or `open`, also in an accessory app.
    /// `has_visible_windows` says whether any Rune window that reopening could bring back is on
    /// screen and not minimized (launcher panels and overlays don't count). Return true when the
    /// hook handled it, which skips Rune's default; false lets the default run. When no
    /// such window shows, the default shows and activates a minimized window, else the window Rune
    /// hid last (closed with `keep_running`, or hidden), else the main window. It never brings
    /// back a launcher or other nonactivating panel, a window that closed for good, or anything
    /// in an accessory app. With several windows, every window's hook runs, in window order,
    /// and any true skips the default. Windows: runs when the app is launched again without a
    /// link while it runs, once `rune::single_instance` made it the app's one process; the
    /// default then also brings the front window forward, restored. Never called on other
    /// platforms. Headless scripts deliver it with `{"reopen": true}`.
    pub fn on_reopen(mut self, f: fn(&mut S, bool, &mut Tx) -> bool) -> Self {
        self.on_reopen = Some(f);
        self
    }

    /// macOS: runs on the main thread with each link the system opens with this app
    /// (`myapp://...`, from `open`, a browser or another app), including the one that launched
    /// it, which runs before the window's first frame. Each runs in arrival order as an
    /// `AppHandle::update`, so a window it opens through a handle opens in the same turn. The
    /// string is passed through as received; it comes from outside the app, so parse and
    /// validate it before acting on it. Bringing a window forward is up to the app
    /// (`AppHandle::open`). The app's bundle must declare the scheme (`CFBundleURLTypes`, see
    /// the README). With several windows whose apps have the hook, the first still running
    /// receives links.
    ///
    /// Windows: the links among the launch arguments (`scheme://…`), as Windows passes a link to
    /// the process it starts, and with `rune::single_instance` the links of every later launch,
    /// which that launch hands to this process instead of opening a window. The app registers its
    /// scheme itself (`HKEY_CURRENT_USER\Software\Classes\<scheme>`, usually at install). Never
    /// called on other platforms (Linux passes links to a new process as arguments).
    pub fn on_open_url(mut self, f: fn(&mut S, &str, &mut Tx)) -> Self {
        self.on_open_url = Some(f);
        self
    }

    /// On web, routes [`Tx::open_url_same_tab`] links inside a single-page app: `route` gets the
    /// absolute URL of each link on the page's own origin and returns true when it handled it
    /// (`history.pushState` and a render), or false to let the browser load it with
    /// `location.assign`. Links to other origins always load. Ignored on native.
    pub fn same_tab_router(mut self, route: impl Fn(&str) -> bool + 'static) -> Self {
        self.same_tab_router = Some(Box::new(route));
        self
    }

    /// Runs on the UI thread each time the native window is created (again after a Wayland
    /// close or a GPU release): set its icon or native styles through
    /// `WindowHandle::with_raw_handle`.
    pub fn on_window(mut self, f: impl FnMut(&crate::WindowHandle) + 'static) -> Self {
        self.on_window = Some(Box::new(f));
        self
    }

    /// First-frame readiness, once per native creation. Launcher panels call this while
    /// hidden, after GPU completion of its cached surface frame. A window that isn't on screen
    /// (covered, occluded, hidden or on another Space) doesn't wait to draw: this runs at its
    /// first frame skipped for that, or a second after it opens with no frame at all.
    /// Not called by headless tooling. May queue a show through the supplied handle.
    pub fn on_ready(mut self, f: impl FnMut(&crate::WindowHandle) + 'static) -> Self {
        self.on_ready = Some(Box::new(f));
        self
    }

    /// Runs on the UI thread after each frame of the app's window, saying what it came to
    /// ([`FrameOutcome`](crate::FrameOutcome)): `Drawn` once it was encoded, submitted and
    /// presented, the first frame during creation and a hidden launcher's frames included;
    /// `Unchanged` for a frame only a timer asked for that drew what was on screen; `Skipped`
    /// for one a covered, minimised or occluded window, or a drawable that never came, kept
    /// from drawing. For logging frame timing: pair [`FrameEncoded::at`](crate::FrameEncoded::at)
    /// with `Tx::input_time` for input latency. It changes nothing about when or what the window
    /// draws; without it, each frame checks an `Option`. A single-window headless run reports
    /// every frame on its virtual clock as `Drawn`, with wall-clock instants and `refresh_hz` at
    /// its `--fps`, so an app's own meter reads its build and encode costs there too. Overlays,
    /// multi-window headless runs and the web never call it.
    pub fn on_frame_encoded(mut self, f: impl FnMut(&crate::FrameEncoded) + 'static) -> Self {
        self.on_frame_encoded = Some(Box::new(f));
        self
    }

    /// Runs on the UI thread after each `Drawn` frame of the app's window, with the runtime as
    /// that frame presented it (`Runtime::elements`, `Runtime::state`), for an app that watches
    /// its own motion. Pair it with [`last_input`](crate::last_input) to tell motion the user
    /// caused from motion they didn't. Like `on_frame_encoded`, it changes nothing about when
    /// or what the window draws, and only desktop windows call it.
    pub fn on_frame_drawn(mut self, f: impl FnMut(&crate::Runtime<S>) + 'static) -> Self {
        self.on_frame_drawn = Some(Box::new(f));
        self
    }

    /// Draw every frame of the app's window (and headless renders) through `compositor`
    /// instead of Rune's default pass (see `compose`).
    pub fn compositor(mut self, compositor: impl crate::compose::Compositor<S>) -> Self {
        self.compositor = Some(Box::new(compositor));
        self
    }

    /// A handle for other threads; requests made before `run` apply once it starts.
    pub fn handle(&self) -> AppHandle<S> {
        AppHandle {
            shared: self.shared.clone(),
        }
    }

    /// The app's own counters (memory, caches, queues), shown beside the window's frame stats
    /// in a native trace's summary. Keep it cheap. For example, with the `procwatch`
    /// crate: `.dev_stats(|| procwatch::snapshot().map_or_else(String::new, |s| format!("{} MB, {} fds", s.footprint >> 20, s.fds)))`.
    pub fn dev_stats(mut self, stats: fn() -> String) -> Self {
        self.dev_stats = Some(stats);
        self
    }

    /// A storybook in this window for auditing its scenes, stories and overlays: ⌘⇧D, a bar
    /// along the bottom, or the View menu's Playground item opens it over the app. Shown in debug
    /// builds unless `RUNE_PLAYGROUND=0`; `--playground [NAME]` opens it at launch, headless
    /// too, where it otherwise never appears, so snapshots and goldens don't change.
    pub fn playground(mut self, playground: crate::Playground<S>) -> Self {
        if playground.is_enabled() && crate::playground::offered_by_env() {
            let item = crate::MenuItem::new("Playground", |_: &mut S, tx: &mut Tx| tx.toggle_playground())
                .id("rune.playground")
                .menu("View");
            let item = match playground.shortcut_key() { Some(shortcut) => item.key_equivalent(shortcut.clone()), None => item };
            self = self.menu_item(item);
        }
        self.playground = Some(playground);
        self
    }

    /// Runs until the app quits. Agent-tooling arguments (`--snap`, `--tree`, ...) render
    /// headless instead: the window requests are ignored there, and updates apply each frame.
    /// Panics when the app can't start, such as without a display; `try_run` returns that error.
    pub fn run(self) {
        if let Err(error) = self.try_run() { panic!("{error}"); }
    }

    /// `run`, returning an error instead of panicking when the event loop can't start (no
    /// display over ssh or in CI, no Wayland or X11 server) or fails.
    #[cfg(target_arch = "wasm32")]
    pub fn try_run(mut self) -> Result<(), String> {
        if let Some(argv) = self.argv.take() {
            let _ = Args::parse_from(argv);
        }
        crate::platform::web::run(self)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn try_run(mut self) -> Result<(), String> {
        let args = match self.argv.take() {
            Some(argv) => Args::parse_from(argv),
            None => Args::parse(),
        };
        if args.headless() {
            crate::render::discard_prewarm();
            crate::tooling::try_headless(self, args)
        } else {
            crate::window::run(self, args)
        }
    }
}

impl<S: 'static> App<S> {
    pub(crate) fn resolve_args(&mut self, inherited: Args) -> Args {
        self.argv.take().map_or(inherited, Args::parse_from)
    }

    /// On web: the canvas sits inline in the page (`canvas_id` without `full_viewport`, or with
    /// `fit_content_height`) rather than covering the viewport.
    #[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
    pub(crate) fn web_inline(&self) -> bool {
        self.canvas_id.is_some() && (self.fit_content_height || !self.full_viewport)
    }

    pub(crate) fn into_parts(self, scene: Option<&str>) -> (AppSpec<S>, S, Arc<Shared<S>>) {
        let state = crate::tooling::perf_warn::launch::init(|| match self.init {
            Some(init) => crate::tooling::with_scene(&self.spec, init(), scene),
            None => crate::tooling::initial_state(&self.spec, scene),
        });
        (self.spec, state, self.shared)
    }
}

/// Opens, focuses, closes and feeds a running `App` from any thread. Every call wakes the event
/// loop; nothing polls. Window requests collapse to the latest.
pub struct AppHandle<S: 'static> {
    pub(crate) shared: Arc<Shared<S>>,
}

impl<S> Clone for AppHandle<S> {
    fn clone(&self) -> Self {
        AppHandle {
            shared: self.shared.clone(),
        }
    }
}

impl<S: 'static> AppHandle<S> {
    /// Wake the event loop so `AppSpec::poll` runs, also while the window is hidden; for a
    /// background thread that queued work for `poll` to drain.
    pub fn wake(&self) {
        self.request(|q| q.woken = true);
    }

    /// Show the window (creating it if needed), restore it if minimized, and activate the app
    /// with it in front, including the first open of a window that doesn't exist yet. A
    /// nonactivating panel takes keys without activating. The window open at launch doesn't
    /// activate the app beyond what the platform does at launch.
    pub fn open(&self) {
        self.request(|q| { q.window = Some(WindowRequest::Open(None)); q.minimize = false; });
    }

    /// Show or recreate the window without requesting focus. A minimized window stays minimized;
    /// open() also restores and activates it. Window-manager policy still applies.
    pub fn show(&self) {
        self.request(|q| q.window = Some(WindowRequest::Show));
    }

    /// Hide the window, retaining its host and state. Does not invoke the close callback.
    pub fn hide(&self) {
        self.request(|q| { q.window = Some(WindowRequest::Hide); q.minimize = false; });
    }

    /// Focus/activate an already-visible window, restoring it if minimized.
    /// On Wayland this requests attention; use activate(token) for compositor-authorized focus.
    pub fn focus(&self) {
        self.request(|q| { q.focus = true; q.minimize = false; });
    }

    /// Set the outer position in physical desktop pixels (negative coordinates are valid).
    /// Retained across recreation. Wayland does not support application window placement.
    pub fn set_position(&self, position: (i32, i32)) {
        self.request(|q| { q.position = Some(position); q.place = None; });
    }

    /// Place in display-local points. UnderPointer samples the pointer at this call;
    /// disconnected displays fall back to primary when the UI thread applies the request.
    /// Wayland does not permit application placement.
    pub fn place(&self, display: crate::DisplayTarget, placement: crate::OverlayPlacement) {
        let place = crate::windows::PlaceRequest::new(display, placement);
        self.request(|q| { q.place = Some(place); q.position = None; });
    }

    /// Show and focus the window with an activation token another process forwarded
    /// (`XDG_ACTIVATION_TOKEN` on Wayland, `DESKTOP_STARTUP_ID` on X11), so the compositor
    /// lets it take focus. Where tokens don't exist this is `open`.
    pub fn activate(&self, token: impl Into<String>) {
        let token = token.into();
        self.request(|q| { q.window = Some(WindowRequest::Open(Some(token).filter(|t| !t.is_empty()))); q.minimize = false; });
    }

    /// Close the window as the close button would (`App::on_close` runs; without
    /// `keep_running` the app quits).
    pub fn close(&self) {
        self.request(|q| { q.window = Some(WindowRequest::Close); q.minimize = false; });
    }

    /// Minimize the window, as `Tx::minimize` does from a handler. Does nothing for a hidden
    /// window, a panel or launcher, or one that can't minimize. After a `show` it minimizes
    /// the shown window; a later `open`, `focus`, `hide` or `close` cancels it, and it cancels
    /// an earlier `focus`.
    pub fn minimize(&self) {
        self.request(|q| { q.minimize = true; q.focus = false; });
    }

    /// Quit the app: every open window's `App::on_quit` runs, the window closes without
    /// `on_close`, and `run` returns.
    ///
    /// On web this unmounts the app: its loop stops, its GPU device is destroyed, its listeners
    /// come off the page, the body style and title it changed go back, and the canvas Rune made
    /// is removed, while one passed in with [`App::canvas_id`] stays, restyled as it was, for
    /// its owner. Another app may start straight away; it mounts once this one has gone. An
    /// app quit before it mounts never does.
    pub fn quit(&self) {
        self.shared.quitting.store(true, Ordering::Release);
        self.request(|q| q.quit = true);
    }

    /// macOS: show or hide the app's Dock icon (and app menu) while it runs, switching between
    /// the regular and accessory activation policies. Does nothing elsewhere.
    pub fn set_dock_icon(&self, visible: bool) {
        self.request(|q| q.dock = Some(visible));
    }

    /// Whether the window is on screen (shown, not hidden or closed). Minimized and occluded
    /// windows count as shown.
    pub fn is_visible(&self) -> bool {
        self.shared.visible.load(Ordering::Acquire)
    }

    /// Why the window couldn't be created the last time it was asked for (no display, no GPU
    /// adapter or device), until a later creation succeeds. The app keeps running windowless.
    pub fn window_error(&self) -> Option<String> {
        self.shared.window_error.lock().unwrap().clone()
    }

    /// Change the state from this thread: `f` runs on the UI thread, in call order, even while no
    /// window shows, and the next frame commits it with the motion its `Tx` asks for.
    pub fn update(&self, f: impl FnOnce(&mut S, &mut Tx) + Send + 'static) {
        self.update_if(move |s, tx| { f(s, tx); true });
    }

    /// `update` for a timer or feed that often finds nothing new: `f` returns whether it changed
    /// state the view reads, as `AppSpec::poll` does. On false the view doesn't re-run and the
    /// window doesn't redraw (a hidden one doesn't refresh), while what `f` asked of its `Tx`
    /// still happens: clipboard, close and quit, menus, `wake_after`, and scrolls, which take
    /// a frame. Several updates drained together commit once if any returned true.
    ///
    /// Returning false after changing state the view reads leaves the screen stale until the
    /// next change.
    pub fn update_if(&self, f: impl FnOnce(&mut S, &mut Tx) -> bool + Send + 'static) {
        self.request(|q| q.updates.push(Box::new(f)));
    }

    fn request(&self, f: impl FnOnce(&mut Requests<S>)) {
        f(&mut self.shared.requests.lock().unwrap());
        self.shared.wake();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WindowRequest {
    /// Show and focus, with an activation token when one was forwarded.
    Open(Option<String>),
    Show,
    Hide,
    Close,
}

pub(crate) struct Requests<S> {
    pub window: Option<WindowRequest>,
    pub dock: Option<bool>,
    pub focus: bool,
    /// `AppHandle::minimize`, after `window`. It and `focus` are last-wins.
    pub minimize: bool,
    pub position: Option<(i32, i32)>,
    pub place: Option<crate::windows::PlaceRequest>,
    pub quit: bool,
    /// `AppHandle::wake` asked for `poll`.
    pub woken: bool,
    pub updates: Vec<Update<S>>,
}

impl<S> Requests<S> {
    /// Nothing but updates: no wake for `poll`, no window, focus or geometry request.
    pub(crate) fn only_updates(&self) -> bool {
        !self.woken && self.window.is_none() && !self.focus && !self.minimize && self.position.is_none() && self.place.is_none()
    }
}

impl<S> Default for Requests<S> {
    fn default() -> Self {
        Requests {
            window: None,
            dock: None,
            focus: false,
            minimize: false,
            position: None,
            place: None,
            quit: false,
            woken: false,
            updates: Vec::new(),
        }
    }
}

pub(crate) struct Shared<S> {
    pub(crate) menus: crate::native_menu::Hub<S>,
    pub(crate) shortcuts: crate::shortcuts::Hub<S>,
    requests: Mutex<Requests<S>>,
    wake: Mutex<Option<crate::runtime::Waker>>,
    /// Published by the window host for `AppHandle::is_visible` / `window_error`.
    pub(crate) visible: AtomicBool,
    /// `AppHandle::quit` was called, or the web app is ending.
    pub(crate) quitting: AtomicBool,
    pub(crate) window_error: Mutex<Option<String>>,
}

impl<S> Default for Shared<S> {
    fn default() -> Self {
        Shared {
            menus: crate::native_menu::Hub::default(),
            shortcuts: crate::shortcuts::Hub::default(),
            requests: Mutex::default(),
            wake: Mutex::new(None),
            visible: AtomicBool::new(false),
            quitting: AtomicBool::new(false),
            window_error: Mutex::new(None),
        }
    }
}

impl<S> Shared<S> {
    /// Installs the event loop's wake-up and wakes it once for requests made before it existed.
    pub fn connect(&self, wake: crate::runtime::Waker) {
        *self.wake.lock().unwrap() = Some(wake);
        self.wake();
    }

    pub fn wake(&self) {
        let wake = self.wake.lock().unwrap().clone();
        if let Some(wake) = wake {
            wake();
        }
    }

    pub fn take(&self) -> Requests<S> {
        std::mem::take(&mut self.requests.lock().unwrap())
    }

    /// The queued updates alone; the other requests stay.
    pub(crate) fn take_updates(&self) -> Vec<Update<S>> {
        std::mem::take(&mut self.requests.lock().unwrap().updates)
    }

    /// Runs the queued updates with `run`, then takes the other requests, so a window an update
    /// opens (or shows, focuses, places, quits) applies in this turn. Updates the updates queue
    /// wait for the next turn, which their wake brings.
    pub(crate) fn drain(&self, mut run: impl FnMut(Update<S>)) -> Requests<S> {
        for f in self.take_updates() { run(f); }
        let mut requests = self.requests.lock().unwrap();
        let later = std::mem::take(&mut requests.updates);
        std::mem::replace(&mut *requests, Requests { updates: later, ..Requests::default() })
    }

    /// Queues `f` for the next `take`, as `AppHandle::update_if` does, without waking the loop.
    pub(crate) fn queue_update(&self, f: Update<S>) {
        self.requests.lock().unwrap().updates.push(f);
    }

    /// A request a `--desktop` run applies to the window (dock and quit aside) waits for `take`.
    pub(crate) fn window_pending(&self) -> bool {
        let r = self.requests.lock().unwrap();
        r.window.is_some() || r.focus || r.minimize || r.position.is_some() || r.place.is_some() || !r.updates.is_empty()
    }

    pub fn take_place(&self) -> Option<crate::windows::PlaceRequest> {
        self.requests.lock().unwrap().place.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::el::{El, Shell, el};

    use crate::runtime::{Cx, Env, Runtime};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn spec() -> AppSpec<u32> {
        AppSpec::new("test", (100.0, 100.0), || 0, |_: &u32, _: &mut Cx<u32>| el())
    }

    #[test]
    fn a_named_web_canvas_sits_inline_unless_it_covers_the_viewport() {
        assert!(!App::new(spec()).web_inline(), "Rune's own canvas covers the viewport");
        assert!(!App::new(spec()).full_viewport().web_inline());
        assert!(App::new(spec()).canvas_id("view").web_inline(), "canvas_id alone stays inline");
        assert!(App::new(spec()).canvas_id("view").fit_content_height().web_inline());
        assert!(!App::new(spec()).canvas_id("route").full_viewport().web_inline());
        assert!(App::new(spec()).canvas_id("view").full_viewport().fit_content_height().web_inline(),
            "sizing to content wins over covering the viewport");
    }

    #[test]
    fn window_panel_placement_and_pixel_position_are_last_request_wins() {
        let app = App::new(spec());
        let handle = app.handle();
        handle.set_position((10, 20));
        handle.place(crate::DisplayTarget::Primary, crate::OverlayPlacement::FullScreen);
        let q = app.shared.take();
        assert!(q.position.is_none());
        assert_eq!(q.place.unwrap().placement, crate::OverlayPlacement::FullScreen);
        handle.place(crate::DisplayTarget::Primary, crate::OverlayPlacement::FullScreen);
        handle.set_position((-10, 20));
        let q = app.shared.take();
        assert!(q.place.is_none());
        assert_eq!(q.position, Some((-10, 20)));
    }

    #[test]
    fn window_panel_focus_is_local_deduplicated_and_runs_without_frames() {
        let mut rt = Runtime::new(Vec::new(), |_: &Vec<bool>, _: &mut Cx<Vec<bool>>| el(), (100.0, 100.0), Env::default());
        rt.on_focus = Some(|seen, focused, tx| { seen.push(focused); if !focused { tx.close_window(); } });
        let mut other = Runtime::new(Vec::new(), |_: &Vec<bool>, _: &mut Cx<Vec<bool>>| el(), (100.0, 100.0), Env::default());
        other.on_focus = rt.on_focus;
        rt.window_focus(true);
        rt.window_focus(true);
        rt.window_focus(false);
        rt.window_focus(false);
        assert_eq!(rt.state, [true, false]);
        assert_eq!(rt.shell_request, Some(Shell::Close));
        assert!(other.state.is_empty());
        other.window_focus(false);
        assert_eq!(other.state, [false]);
        other.on_focus = None;
        other.frame(0.0);
        assert!(!other.stale());
        other.window_focus(true);
        assert!(!other.stale(), "focus on a committed view without on_window_focus asks for no extra frame");
    }

    #[test]
    fn handles_report_the_window_the_host_publishes() {
        let app = App::new(spec());
        let handle = app.handle();
        assert!(!handle.is_visible() && handle.window_error().is_none());
        app.shared.visible.store(true, Ordering::Release);
        *app.shared.window_error.lock().unwrap() = Some("no GPU adapter".into());
        assert!(handle.is_visible());
        assert_eq!(handle.window_error().as_deref(), Some("no GPU adapter"));
    }

    #[test]
    fn handles_cross_threads_whatever_the_state() {
        fn send_sync<T: Send + Sync + Clone>() {}
        send_sync::<AppHandle<std::rc::Rc<()>>>();
    }

    #[test]
    fn init_closures_replace_the_spec_init_and_scenes_apply_on_top() {
        let mut spec = spec();
        spec.scenes = &[("big", |n| *n += 100)];
        let captured = 7;
        let (_, state, _) = App::new(spec).init(move || captured).into_parts(Some("big"));
        assert_eq!(state, 107);
        let (_, state, _) = App::new(self::spec()).into_parts(None);
        assert_eq!(state, 0);
    }

    #[test]
    fn handle_requests_queue_before_the_loop_and_wake_it_after() {
        let app = App::new(spec()).windowless();
        assert!(!app.open && app.keep_running);
        let handle = app.handle();
        let other = handle.clone();
        std::thread::spawn(move || {
            other.update(|n, _| *n += 1);
            other.activate("token");
        })
        .join()
        .unwrap();
        handle.update(|n, _| *n *= 10);
        handle.close();
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = wakes.clone();
        app.shared.connect(Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }));
        assert_eq!(
            wakes.load(Ordering::SeqCst),
            1,
            "connecting wakes once for queued requests"
        );
        let requests = app.shared.take();
        assert_eq!(
            requests.window,
            Some(WindowRequest::Close),
            "window requests collapse to the latest"
        );
        assert!(!requests.quit);
        let mut n = 0;
        for f in requests.updates {
            f(&mut n, &mut Tx::default());
        }
        assert_eq!(n, 10, "updates apply in call order");
        handle.activate("");
        assert_eq!(wakes.load(Ordering::SeqCst), 2);
        assert_eq!(
            app.shared.take().window,
            Some(WindowRequest::Open(None)),
            "an empty token is no token"
        );
    }

    #[test]
    fn a_window_an_update_opens_applies_in_the_same_drain() {
        let app = App::new(spec()).windowless();
        let handle = app.handle();
        let inner = handle.clone();
        handle.update(move |n, _| {
            *n += 1;
            inner.open();
            inner.update(|n, _| *n += 10);
        });
        let mut n = 0;
        let requests = app.shared.drain(|f| { f(&mut n, &mut Tx::default()); });
        assert_eq!(n, 1);
        assert_eq!(requests.window, Some(WindowRequest::Open(None)), "the open applies this turn");
        let next = app.shared.take();
        assert_eq!(next.updates.len(), 1, "an update's own update waits for the next turn");
        assert!(next.window.is_none());
    }

    #[test]
    fn tray_requests_keep_show_focus_and_geometry_together() {
        let app = App::new(spec()).windowless();
        let handle = app.handle();
        std::thread::spawn(move || {
            handle.set_position((10, 20));
            handle.hide();
            handle.set_position((-300, 40));
            handle.show();
            handle.focus();
        }).join().unwrap();
        let requests = app.shared.take();
        assert_eq!(requests.window, Some(WindowRequest::Show), "focus must not overwrite show");
        assert!(requests.focus);
        assert_eq!(requests.position, Some((-300, 40)));
        let drained = app.shared.take();
        assert!(!drained.focus && drained.window.is_none() && drained.position.is_none());
    }

    #[test]
    fn updates_commit_on_the_next_frame_and_carry_shell_requests() {
        fn view(n: &u32, _: &mut Cx<u32>) -> El {
            crate::el::row().child(el().key("box").size(10.0 + *n as f32, 10.0))
        }
        let mut rt = Runtime::new(
            0u32,
            view,
            (200.0, 200.0),
            Env {
                system_dark: true,
                scale: 1.0,
            },
        );
        rt.frame(0.0);
        rt.update(|n, tx| {
            *n = 30;
            tx.instant();
        });
        assert!(rt.shell_request.is_none());
        rt.frame(1.0 / 120.0);
        let b = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("box"))
            .unwrap();
        assert_eq!(b.rect[2], 40.0, "the update's instant commit lands at once");
        rt.update(|_, tx| tx.close_window());
        rt.update(|_, tx| tx.quit());
        rt.update(|_, tx| tx.close_window());
        assert_eq!(rt.shell_request.take(), Some(Shell::Quit), "quit outranks close");
    }

    #[test]
    fn minimize_reaches_the_shell_from_handlers_updates_and_handles() {
        let mut rt = Runtime::new(0u32, |_: &u32, _: &mut Cx<u32>| el(), (100.0, 100.0), Env::default());
        rt.on_focus = Some(|_, focused, tx| if !focused { tx.minimize(); });
        rt.window_focus(true);
        assert!(!rt.minimize_request);
        rt.window_focus(false);
        assert!(rt.minimize_request && rt.shell_request.is_none(), "a handler's minimize is latched for the shell");
        rt.ignore_minimize("a test");
        assert!(!rt.minimize_request, "a shell that can't minimize takes the request");
        rt.update(|_, tx| tx.minimize());
        assert!(rt.minimize_request);
        let app = App::new(spec());
        app.handle().minimize();
        let requests = app.shared.take();
        assert!(requests.minimize && requests.window.is_none());
    }

    #[test]
    fn a_handles_minimize_and_focus_are_last_wins_and_hide_close_or_open_cancel_it() {
        let app = App::new(spec());
        let h = app.handle();
        let after = |calls: &dyn Fn()| { calls(); let r = app.shared.take(); (r.window, r.focus, r.minimize) };
        assert_eq!(after(&|| { h.focus(); h.minimize(); }), (None, false, true), "focus then minimize stays minimized");
        assert_eq!(after(&|| { h.minimize(); h.focus(); }), (None, true, false));
        assert_eq!(after(&|| { h.show(); h.minimize(); }), (Some(WindowRequest::Show), false, true), "shows, then minimizes");
        assert_eq!(after(&|| { h.minimize(); h.show(); }), (Some(WindowRequest::Show), false, true));
        assert_eq!(after(&|| { h.minimize(); h.hide(); }), (Some(WindowRequest::Hide), false, false));
        assert_eq!(after(&|| { h.minimize(); h.close(); }), (Some(WindowRequest::Close), false, false));
        assert_eq!(after(&|| { h.minimize(); h.open(); }), (Some(WindowRequest::Open(None)), false, false), "open restores");
    }

    #[test]
    fn a_standard_close_or_minimize_key_acts_only_when_no_handler_takes_it() {
        use crate::runtime::Unhandled;
        use crate::{Key, KeyEvent, Mods, PhysicalInput, PhysicalKeyCode as Code};
        let cmd = Mods { cmd: true, ..Mods::default() };
        let press = |rt: &mut Runtime<u32>, code, c, then| {
            rt.physical_then(PhysicalInput::Key { key: code, mods: cmd, pressed: true, repeat: false },
                Some(KeyEvent { key: Key::Command(c), mods: cmd }), None, Some(then));
            rt.frame(0.0);
        };
        let mut rt = Runtime::new(0u32, |_: &u32, _: &mut Cx<u32>| el(), (100.0, 100.0), Env::default());
        press(&mut rt, Code::KeyW, 'w', Unhandled::Close);
        assert_eq!(rt.shell_request.take(), Some(Shell::Close), "an unhandled ⌘W closes as the button would");
        press(&mut rt, Code::KeyM, 'm', Unhandled::Minimize);
        assert!(std::mem::take(&mut rt.minimize_request) && rt.shell_request.is_none());
        rt.on_key = Some(|n, ev, _| { *n += 1; ev.is_command('w') });
        press(&mut rt, Code::KeyW, 'w', Unhandled::Close);
        assert_eq!((rt.state, rt.shell_request), (1, None), "the app's own ⌘W wins, once");
        press(&mut rt, Code::KeyM, 'm', Unhandled::Minimize);
        assert_eq!(rt.state, 2, "the app saw ⌘M first");
        assert!(rt.minimize_request, "and didn't take it");
    }

    thread_local! { static BUILDS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) }; }

    fn counted(n: &u32, _: &mut Cx<u32>) -> El {
        BUILDS.set(BUILDS.get() + 1);
        crate::el::row().child(el().key("box").size(10.0 + *n as f32, 10.0))
    }

    fn drain(rt: &mut Runtime<u32>, app: &App<u32>) -> bool {
        let mut updated = false;
        for f in app.shared.take().updates { updated |= rt.update_if(f); }
        updated
    }

    #[test]
    fn updates_that_change_nothing_build_nothing() {
        let app = App::new(AppSpec::new("t", (100.0, 100.0), || 0, counted));
        let handle = app.handle();
        let mut rt = Runtime::new(0u32, counted, (100.0, 100.0), Env::default());
        rt.frame(0.0);
        let (builds, commits) = (BUILDS.get(), rt.stats.commits);
        for _ in 0..100 {
            handle.update_if(|_, _| false);
            assert!(!drain(&mut rt, &app));
            assert!(!rt.stale(), "nothing to redraw");
            rt.frame(1.0 / 120.0);
        }
        assert_eq!((BUILDS.get() - builds, rt.stats.commits - commits), (0, 0));

        handle.update_if(|_, _| false);
        handle.update_if(|n, tx| { *n = 5; tx.instant(); true });
        handle.update_if(|_, _| false);
        assert!(drain(&mut rt, &app), "one change in the batch commits it");
        rt.frame(1.0 / 120.0);
        rt.frame(1.0 / 120.0);
        assert_eq!(BUILDS.get() - builds, 1, "once");
        assert_eq!(rt.elements().into_iter().find(|e| e.key.as_deref() == Some("box")).unwrap().rect[2], 15.0);

        handle.update(|_, _| {});
        assert!(drain(&mut rt, &app), "`update` always counts as a change");
        rt.frame(1.0 / 120.0);
        assert_eq!(BUILDS.get() - builds, 2);
    }

    #[test]
    fn an_update_that_changes_nothing_still_delivers_its_requests() {
        let mut rt = Runtime::new(0u32, counted, (100.0, 100.0), Env::default());
        rt.frame(0.0);
        assert!(!rt.update_if(|_, tx| { tx.copy("hello"); tx.wake_after(std::time::Duration::from_millis(500)); false }));
        assert!(!rt.stale());
        assert_eq!(rt.copy_request.as_deref(), Some("hello"));
        assert_eq!(rt.wake_deadline(), None, "left for the next frame, as `update` leaves it");
        rt.schedule_pending_wakes();
        assert_eq!(rt.wake_deadline(), Some(rt.clock + 0.5), "or scheduled by a host that runs none");
        assert!(!rt.update_if(|_, tx| { tx.quit(); false }));
        assert_eq!(rt.shell_request, Some(Shell::Quit));
    }

    /// A `wake_after(ZERO)` from an update that changed nothing, in a batch that still draws,
    /// asks for the frame after, as `update`'s does: the frame doesn't spend it before polling.
    #[test]
    fn an_unchanging_updates_next_frame_timer_survives_a_frame() {
        let run = |changed: bool| {
            let mut rt = Runtime::new(0u32, counted, (100.0, 100.0), Env::default());
            rt.frame(0.0);
            rt.update_if(move |_, tx| { tx.wake_after(std::time::Duration::ZERO); changed });
            rt.update(|n, _| *n += 1);
            rt.frame(1.0);
            rt.wake_deadline()
        };
        assert!(run(true).is_some());
        assert_eq!(run(false), run(true));
    }

    #[test]
    fn a_hidden_window_drains_its_inbox_on_wake_and_shows_it_when_reopened() {
        #[derive(Default)]
        struct Lock { inbox: Option<std::sync::mpsc::Receiver<u32>>, locked: u32 }
        fn view(s: &Lock, _: &mut Cx<Lock>) -> El {
            crate::el::row().child(el().key("box").size(10.0 + s.locked as f32, 10.0))
        }
        let mut rt = Runtime::new(Lock::default(), view, (200.0, 200.0), Env { system_dark: true, scale: 1.0 });
        rt.poll = Some(|s: &mut Lock, _, tx: &mut Tx| {
            let Some(n) = s.inbox.as_ref().and_then(|inbox| inbox.try_iter().last()) else { return false };
            s.locked = n;
            tx.wake_after(std::time::Duration::from_secs(60));
            true
        });
        rt.wall_start = Some(std::time::Instant::now());
        let (send, inbox) = std::sync::mpsc::channel();
        rt.state.inbox = Some(inbox);
        rt.frame(0.0);
        let width = |rt: &Runtime<Lock>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("box")).unwrap().rect[2];
        assert_eq!(width(&rt), 10.0);

        // Hidden from here: something queues a lock and wakes the loop.
        send.send(30).unwrap();
        assert!(rt.poll_hidden(), "a hidden launcher renders when poll reports a change");
        assert!(!rt.poll_hidden());
        assert_eq!(rt.state.locked, 30, "poll drained the inbox without a frame");
        assert_eq!(width(&rt), 10.0, "nothing laid out, animated or presented while hidden");
        assert!(rt.wake_deadline().is_some(), "poll's own timers still schedule while hidden");

        // `show` lands everything on its first frame.
        rt.settle_all();
        rt.frame(1.0 / 60.0);
        assert_eq!(width(&rt), 40.0);
    }

    /// An update's `wake_after`, delivered while no frame draws (hidden, covered, asleep or
    /// minimised), arms its timer on the next hidden poll.
    #[test]
    fn an_update_arriving_without_frames_arms_its_timer() {
        let mut rt = Runtime::new(0_u32, |_: &u32, _: &mut Cx<u32>| el(), (100.0, 100.0), Env { system_dark: true, scale: 1.0 });
        rt.poll = Some(|polls: &mut u32, _, _: &mut Tx| { *polls += 1; false });
        rt.wall_start = Some(std::time::Instant::now());
        rt.frame(0.0);
        assert_eq!(rt.wake_deadline(), None);
        rt.update(|_, tx| tx.wake_after(std::time::Duration::from_secs(5)));
        rt.poll_hidden();
        let deadline = rt.wake_deadline().expect("the update's timer is armed without a frame");
        assert!((4.9..5.5).contains(&deadline), "it counts from the update: {deadline}");
        rt.frame(0.0);
        assert_eq!(rt.wake_deadline(), Some(deadline), "and the next frame doesn't arm it twice");
    }

    /// A hidden host's due timer runs `poll` and spends the timer without building the view,
    /// poll or not. The view waits, dirty, for the frame that shows the host.
    #[test]
    fn a_hidden_timer_polls_without_building_the_view() {
        use std::time::{Duration, Instant};
        for with_poll in [true, false] {
            let mut rt = Runtime::new(0_u32, |_: &u32, _: &mut Cx<u32>| el(), (100.0, 100.0), Env { system_dark: true, scale: 1.0 });
            if with_poll { rt.poll = Some(|polls: &mut u32, _, _: &mut Tx| { *polls += 1; true }); }
            let t0 = Instant::now();
            rt.wall_start = Some(t0);
            rt.frame(0.0);
            let commits = rt.stats.commits;
            rt.update(|_, tx| tx.wake_after(Duration::from_millis(10)));
            assert_eq!(rt.hidden_wake(), with_poll);
            assert!(rt.wake_deadline().is_some());
            rt.wall_start = Some(t0 - Duration::from_secs(1));
            assert_eq!(rt.hidden_wake(), with_poll);
            assert_eq!(rt.wake_deadline(), None, "the due timer is spent, so it doesn't wake the host again");
            assert_eq!(rt.stats.commits, commits, "no view or layout while hidden");
            assert!(rt.needs_commit());
            rt.frame(0.0);
            assert_eq!(rt.stats.commits, commits + 1, "the frame that shows it builds once");
        }
    }
}
