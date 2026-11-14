//! Window descriptions shared by the native shell and deterministic headless selection.
use crate::{App, AppSpec};

#[derive(Clone, Debug)]
pub(crate) struct PlaceRequest {
    pub display: crate::DisplayTarget,
    pub placement: crate::OverlayPlacement,
    pub pointer: Option<(i32, i32)>,
    /// The caller's target, before sampling the pointer; headless runs resolve this.
    pub requested: crate::DisplayTarget,
}

impl PlaceRequest {
    pub fn new(display: crate::DisplayTarget, placement: crate::OverlayPlacement) -> Self {
        #[allow(unused_mut)] // Only desktop hosts can sample a pointer's display.
        let mut request = Self { requested: display.clone(), display, placement, pointer: None };
        if request.display == crate::DisplayTarget::UnderPointer {
            #[cfg(target_os = "macos")]
            { request.display = crate::platform::macos::pointer_display().map_or(crate::DisplayTarget::Primary,
                |id| crate::DisplayTarget::Native(crate::NativeDisplayId::MacOS(id))); }
            #[cfg(target_os = "windows")]
            { request.pointer = crate::platform::win32::pointer_position(); }
            #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
            { request.pointer = crate::platform::x11::overlay::pointer_position(); }
        }
        request
    }

    pub fn at(size: (f32, f32)) -> crate::overlay::Placement {
        crate::overlay::Placement { size, edge: crate::ScreenEdge::Top, margin: 0.0, align: crate::Align::Center, inset: 0.0 }
    }
}

/// Process-level settings applied once, before constructing the native event loop.
#[derive(Clone, Copy, Debug, Default)]
pub struct RunOptions {
    /// macOS: no Dock icon/default menu and no activation at launch. Ignored elsewhere.
    pub accessory: bool,
}

/// A display in desktop physical-pixel coordinates (origins may be negative).
/// Headless runs provide one deterministic virtual display.
#[derive(Clone, Debug, PartialEq)]
pub struct Display {
    /// Native identity for overlay targeting; absent for the headless virtual display.
    pub id: Option<crate::DisplayId>,
    pub name: Option<String>,
    pub position: (i32, i32),
    pub size: (u32, u32),
    pub scale: f64,
    pub primary: bool,
}

impl Display {
    pub fn native_id(&self) -> Option<crate::NativeDisplayId> { self.id.as_ref().and_then(|id| id.native_id()) }

    /// (x, y, width, height) in the native global, top-left coordinate space. macOS uses
    /// CGDisplayBounds points; Windows/X11 use desktop pixels; Wayland uses compositor
    /// logical coordinates. Keep the origin in this space; scale only local offsets.
    pub fn native_bounds(&self) -> [f64; 4] {
        #[cfg(target_os = "macos")]
        if let Some(crate::NativeDisplayId::MacOS(id)) = self.native_id() { return crate::platform::macos::display_bounds(id); }
        let divisor = if matches!(self.native_id(), Some(crate::NativeDisplayId::Wayland(_))) { self.scale } else { 1.0 };
        [f64::from(self.position.0), f64::from(self.position.1), f64::from(self.size.0) / divisor, f64::from(self.size.1) / divisor]
    }
}

/// A window's minimum content size when neither `WindowOptions` nor `AppSpec` sets one.
pub(crate) const DEFAULT_MIN_SIZE: (f32, f32) = (420.0, 400.0);

#[derive(Clone, Debug)]
pub struct WindowOptions {
    pub transparent: bool,
    pub borderless: bool,
    /// Clicks, and so hover, pass to whatever is below.
    pub click_through: bool,
    pub always_on_top: bool,
    pub visible: bool,
    /// Physical desktop coordinates, not points. Use a display's position unchanged.
    pub position: Option<(i32, i32)>,
    /// Physical content size; overrides AppSpec's logical size when set.
    pub physical_size: Option<(u32, u32)>,
    /// Logical minimum; None inherits AppSpec's minimum, Some((0, 0)) removes the floor.
    /// Requested sizes (`Tx::resize_window`) stay inside the minimum and maximum.
    pub min_size: Option<(f32, f32)>,
    /// Logical maximum content size. None leaves it unconstrained.
    pub max_size: Option<(f32, f32)>,
    /// Whether the user can resize the window. Programmatic sizing is unaffected. A window
    /// that isn't resizable doesn't maximize either: its maximize (zoom) button is disabled.
    pub resizable: bool,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            transparent: false, borderless: false, click_through: false,
            always_on_top: false, visible: true, position: None, physical_size: None,
            min_size: None, max_size: None, resizable: true,
        }
    }
}

impl WindowOptions {
    /// A hidden, retained launcher. On macOS, prepares its first frame at alpha zero;
    /// show/open takes keys without activating the app. Use `on_ready` to register a shortcut.
    /// Other platforms use an ordinary initially hidden window.
    pub fn launcher_panel() -> ConfiguredWindowOptions {
        let mut native = MacWindowOptions::default().nonactivating_panel(true);
        native.launcher = true;
        Self { visible: false, borderless: true, transparent: true, resizable: false,
            min_size: Some((0.0, 0.0)), ..Default::default() }.macos(native)
    }
    /// Optional AppKit settings without adding fields to existing WindowOptions literals.
    /// Ignored on other platforms.
    pub fn macos(self, native: MacWindowOptions) -> ConfiguredWindowOptions {
        ConfiguredWindowOptions { options: self, native }
    }
    /// One transparent, borderless, topmost, click-through overlay covering this display.
    pub fn overlay(display: &Display) -> Self {
        Self {
            transparent: true, borderless: true, click_through: true, always_on_top: true,
            visible: true, position: Some(display.position), physical_size: Some(display.size),
            min_size: Some((0.0, 0.0)), max_size: None, resizable: true,
        }
    }

    /// The window's minimum and maximum content sizes, with `AppSpec::min_size` as the
    /// fallback minimum.
    pub(crate) fn size_limits(&self, app_min: Option<(f32, f32)>) -> ((f32, f32), Option<(f32, f32)>) {
        let mut min = self.min_size.or(app_min).unwrap_or(DEFAULT_MIN_SIZE);
        if let Some(max) = self.max_size { min = (min.0.min(max.0), min.1.min(max.1)); }
        (min, self.max_size)
    }

    /// The minimum a window that fits its content height opens with: the content sets the
    /// height, so the default minimum height doesn't apply.
    pub(crate) fn fit_min_size(&self, app_min: Option<(f32, f32)>) -> Option<(f32, f32)> {
        if app_min.is_none() && self.min_size.is_none() { Some((DEFAULT_MIN_SIZE.0, 0.0)) } else { app_min }
    }

    pub(crate) fn attributes(&self, title: &str, size: (f32, f32), app_min: Option<(f32, f32)>, visible: bool) -> winit::window::WindowAttributes {
        use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
        use winit::window::{Window, WindowButtons, WindowLevel};
        let (min, _) = self.size_limits(app_min);
        let buttons = if self.resizable { WindowButtons::all() } else { WindowButtons::all() - WindowButtons::MAXIMIZE };
        let mut attrs = Window::default_attributes()
            .with_title(title)
            .with_inner_size(LogicalSize::new(size.0, size.1))
            .with_min_inner_size(LogicalSize::new(min.0, min.1))
            .with_resizable(self.resizable)
            .with_enabled_buttons(buttons)
            .with_transparent(self.transparent)
            .with_decorations(!self.borderless)
            .with_visible(visible)
            .with_window_level(if self.always_on_top { WindowLevel::AlwaysOnTop } else { WindowLevel::Normal });
        if let Some((w, h)) = self.max_size { attrs = attrs.with_max_inner_size(LogicalSize::new(w, h)); }
        if let Some((x, y)) = self.position { attrs = attrs.with_position(PhysicalPosition::new(x, y)); }
        if let Some((w, h)) = self.physical_size { attrs = attrs.with_inner_size(PhysicalSize::new(w, h)); }
        attrs
    }
}

/// AppKit properties for an ordinary window, reapplied on native recreation.
/// Omitted properties keep winit's defaults.
#[derive(Clone, Debug, Default)]
pub struct MacWindowOptions {
    pub(crate) shadow: Option<bool>,
    pub(crate) level: Option<isize>,
    pub(crate) collection_behavior: Option<usize>,
    pub(crate) exclude_from_capture: Option<bool>,
    pub(crate) nonactivating_panel: bool,
    pub(crate) launcher: bool,
    pub(crate) corner_radius: Option<f32>,
    pub(crate) corner_curve_circular: bool,
}

impl MacWindowOptions {
    /// Use a key-taking NSPanel. Open/focus never activates the app or its other windows.
    pub fn nonactivating_panel(mut self, enabled: bool) -> Self {
        self.nonactivating_panel = enabled;
        if !enabled { self.launcher = false; }
        self
    }
    pub fn shadow(mut self, enabled: bool) -> Self { self.shadow = Some(enabled); self }
    /// AppKit NSWindow.Level value (normal 0, floating 3, status 25).
    pub fn level(mut self, level: isize) -> Self { self.level = Some(level); self }
    /// AppKit NSWindow.CollectionBehavior bit mask. Replaces the existing mask.
    pub fn collection_behavior(mut self, bits: usize) -> Self { self.collection_behavior = Some(bits); self }
    /// Requests NSWindowSharingNone; capture clients should also filter window_number().
    /// false restores NSWindowSharingReadOnly. This is not a capture guarantee. A nonactivating
    /// panel applies it only while shown (see `OverlayOptions::exclude_from_capture`).
    pub fn exclude_from_capture(mut self, exclude: bool) -> Self { self.exclude_from_capture = Some(exclude); self }
    /// Rounds a transparent, borderless window or panel to `radius` points with a continuous
    /// (squircle) curve. The window's root layer clips everything it shows, Rune's drawing and
    /// native material views alike, so the root view needn't clip itself to the radius. A
    /// border drawn inside the edge is still the app's. The system shadow, when on, follows
    /// the rounded shape. Ignored for opaque or bordered windows, where AppKit owns the frame,
    /// and on other platforms.
    pub fn corner_radius(mut self, radius: f32) -> Self { self.corner_radius = Some(radius.max(0.0)); self }
    /// Use a circular corner arc instead of the continuous curve for `corner_radius`.
    pub fn corner_curve_circular(mut self) -> Self { self.corner_curve_circular = true; self }
}

/// Ordinary options plus native builders, accepted by WindowSpec::configured_options.
#[derive(Clone, Debug)]
pub struct ConfiguredWindowOptions {
    pub(crate) options: WindowOptions,
    pub(crate) native: MacWindowOptions,
}

impl From<WindowOptions> for ConfiguredWindowOptions {
    fn from(options: WindowOptions) -> Self { Self { options, native: MacWindowOptions::default() } }
}

impl ConfiguredWindowOptions {
    /// Amends the AppKit settings in place, keeping what a preset already set:
    /// `WindowOptions::launcher_panel().macos(|native| native.corner_radius(16.0))` stays a
    /// launcher. Ignored on other platforms.
    pub fn macos(mut self, amend: impl FnOnce(MacWindowOptions) -> MacWindowOptions) -> Self {
        self.native = amend(self.native);
        self
    }
}

/// One independent retained tree and app state. Share Live handles or application services
/// between states; events, focus, animation clocks and redraws remain window-local.
pub struct WindowSpec<S: 'static> {
    pub key: String,
    pub app: App<S>,
    pub options: WindowOptions,
    /// Runs on the event thread after native creation. Not called by headless tooling.
    pub on_created: Option<Box<dyn FnOnce(crate::window::WindowHandle)>>,
}

impl<S> WindowSpec<S> {
    pub fn new(key: impl Into<String>, app: AppSpec<S>) -> Self {
        Self::from_app(key, App::new(app))
    }
    /// Preserve AppHandle, windowless, keep-running and GPU-release behavior for this window.
    /// Closing a default window removes only it; quitting any app exits the whole event loop.
    /// RunOptions controls factory display discovery and tooling/window selection. App::args
    /// replaces the selected window's arguments (including native/headless mode). Isolate a
    /// host CLI before factory discovery with run_windows_with_args(options, [], factory).
    /// App::accessory is process-level: use run_windows_with(RunOptions, factory) instead.
    /// AppHandle::set_dock_icon remains global.
    pub fn from_app(key: impl Into<String>, app: App<S>) -> Self {
        Self { key: key.into(), options: app.window_options.clone(), app, on_created: None }
    }
    /// Configure this window's frame, including headless traffic-light placement.
    pub fn chrome(mut self, chrome: crate::Chrome) -> Self { self.app = self.app.chrome(chrome); self }
    /// This window draws its own titlebar, like `App::titlebar`.
    pub fn titlebar(mut self, titlebar: crate::Titlebar) -> Self { self.app = self.app.titlebar(titlebar); self }
    /// Composite this window's translucent fills, blurs, shadows, images and video in sRGB
    /// (gamma) space instead of linear light, matching CSS and Core Animation. With a
    /// `Compositor`, `Frame::srgb_blend` says which encoding its passes must match.
    pub fn srgb_blend(mut self, enabled: bool) -> Self { self.app = self.app.srgb_blend(enabled); self }
    /// This window's font smoothing (`App::font_smoothing`).
    pub fn font_smoothing(mut self, mode: crate::FontSmoothing) -> Self { self.app = self.app.font_smoothing(mode); self }
    pub fn on_focus(mut self, f: fn(&mut S, bool, &mut crate::Tx)) -> Self { self.app = self.app.on_focus(f); self }
    pub fn on_modifiers(mut self, f: fn(&mut S, crate::Mods, &mut crate::Tx)) -> Self { self.app = self.app.on_modifiers(f); self }
    /// This window's editor's input method events, like `App::on_ime`.
    pub fn on_ime(mut self, f: fn(&mut S, &crate::Ime, &mut crate::Tx) -> bool) -> Self { self.app = self.app.on_ime(f); self }
    /// This window's playground, like `App::playground`.
    pub fn playground(mut self, playground: crate::Playground<S>) -> Self { self.app = self.app.playground(playground); self }
    /// Runs in this window when the app starts to quit, like `App::on_quit`.
    pub fn on_quit(mut self, f: fn(&mut S, &mut crate::Tx)) -> Self { self.app = self.app.on_quit(f); self }
    /// Runs when the app is reopened, like `App::on_reopen`.
    pub fn on_reopen(mut self, f: fn(&mut S, bool, &mut crate::Tx) -> bool) -> Self { self.app = self.app.on_reopen(f); self }
    /// Receives links opened with the app, like `App::on_open_url`.
    pub fn on_open_url(mut self, f: fn(&mut S, &str, &mut crate::Tx)) -> Self { self.app = self.app.on_open_url(f); self }
    /// Send `Key::Modifier` presses and releases to this window, like `App::modifier_keys`.
    pub fn modifier_keys(mut self) -> Self { self.app = self.app.modifier_keys(); self }
    /// Tab visits every focusable control in this window, like `App::tab_visits_controls`.
    pub fn tab_visits_controls(mut self, on: bool) -> Self { self.app = self.app.tab_visits_controls(on); self }
    /// Called on the event thread after the first frame is ready, once per native creation.
    pub fn on_ready(mut self, f: impl FnMut(&crate::WindowHandle) + 'static) -> Self {
        self.app = self.app.on_ready(f); self
    }
    /// Reports each of this window's frames, like `App::on_frame_encoded`. A spec made with
    /// `WindowSpec::from_app` keeps its app's hook unless this replaces it; windows `App::window`
    /// adds have their own.
    pub fn on_frame_encoded(mut self, f: impl FnMut(&crate::FrameEncoded) + 'static) -> Self {
        self.app = self.app.on_frame_encoded(f); self
    }
    pub fn options(mut self, options: WindowOptions) -> Self {
        self.options = options;
        self.app.macos_window = MacWindowOptions::default();
        self
    }
    pub fn configured_options(mut self, options: ConfiguredWindowOptions) -> Self {
        let ConfiguredWindowOptions { options, native } = options;
        self.options = options;
        self.app.macos_window = native;
        self
    }
    pub fn state(mut self, state: S) -> Self { self.app = self.app.init(move || state); self }
    pub fn on_created(mut self, callback: impl FnOnce(crate::window::WindowHandle) + 'static) -> Self {
        self.on_created = Some(Box::new(callback));
        self
    }
}

impl<S: 'static> WindowSpec<S> {
    /// Erase the state type, so one factory can return windows of different state types.
    pub fn boxed(self) -> AnyWindowSpec { self.into() }
    /// Open this window without taking focus from the window that has it, such as a window a
    /// tab is dragged into while the press stays with its strip. A later `open` or `focus`
    /// activates it.
    pub fn inactive(mut self) -> Self { self.app = self.app.open_inactive(); self }
    /// Open this window maximized, like `App::maximized`.
    pub fn maximized(mut self, maximized: bool) -> Self { self.app = self.app.maximized(maximized); self }
}

thread_local! {
    /// Windows [`open_window`] asked for, which the event loop starts at the end of its turn.
    static OPENED: std::cell::RefCell<Vec<AnyWindowSpec>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Opens another window in the running app's event loop, with its own state, retained tree,
/// focus and clocks, as the windows a `run_windows` factory returns do: a second document
/// window, or a tab dragged out of its strip. Call it on the UI thread (a handler, an update,
/// `on_ready`); the window starts at the end of the turn. It keeps the app running as the
/// factory's windows do, so the app ends when the last of them closes. Its key must differ
/// from every open window's; a window with a key already open isn't opened. The handle places,
/// focuses and closes it, and its requests wait for it to start. Headless runs open nothing.
pub fn open_window<S: 'static>(spec: WindowSpec<S>) -> crate::WindowHandle {
    let handle = crate::WindowHandle::new(&spec.key);
    if !crate::is_headless() {
        let opened = AnyWindowSpec(Box::new(Erased { spec, handle: Some(handle.clone()), secondary: false }));
        OPENED.with(|o| o.borrow_mut().push(opened));
    }
    handle
}

/// The windows `open_window` queued since the last call.
pub(crate) fn take_opened() -> Vec<AnyWindowSpec> {
    OPENED.with(|o| std::mem::take(&mut *o.borrow_mut()))
}

/// A window's place on the desktop, in physical pixels: what to save to reopen it there.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WindowPlacement {
    /// The frame's top-left, the space `WindowOptions::position` takes.
    pub position: (i32, i32),
    /// The content's top-left.
    pub content_position: (i32, i32),
    /// The content's size, the space `WindowOptions::physical_size` takes.
    pub content_size: (u32, u32),
    /// Physical pixels per point on the window's display.
    pub scale: f64,
    pub maximized: bool,
    pub minimized: bool,
    pub full_screen: bool,
}

/// A `WindowSpec` of any state type, for factories that mix them:
/// `vec![review.boxed(), settings.into()]`.
pub struct AnyWindowSpec(Box<dyn ErasedWindow>);

impl AnyWindowSpec {
    pub fn key(&self) -> &str { self.0.key() }

    /// A window `App::window` added: it closes by hiding and never keeps the loop running alone.
    pub(crate) fn secondary<T: 'static>(mut spec: WindowSpec<T>, handle: crate::WindowHandle) -> Self {
        spec.app.keep_running = true;
        Self(Box::new(Erased { spec, handle: Some(handle), secondary: true }))
    }

    pub(crate) fn resolve_args(&mut self, inherited: crate::tooling::Args) -> crate::tooling::Args { self.0.resolve_args(inherited) }

    /// Render this window headless; `scale` converts a physical size option to points.
    pub(crate) fn headless(self, args: crate::tooling::Args, scale: f32) { self.0.headless(args, scale) }

    /// This window as one member of a `--desktop` composite; `selected` takes `--scene`.
    pub(crate) fn desktop(self, args: &crate::tooling::Args, selected: bool, scale: f32) -> Box<dyn crate::desktop::Member> {
        self.0.desktop(args, selected, scale)
    }

    pub(crate) fn start(self, inherited: crate::tooling::Args, cx: crate::window::Start<'_>) -> Option<Box<dyn crate::window::AnyHost>> {
        self.0.start(inherited, cx)
    }
}

impl<S: 'static> From<WindowSpec<S>> for AnyWindowSpec {
    fn from(spec: WindowSpec<S>) -> Self { Self(Box::new(Erased { spec, handle: None, secondary: false })) }
}

trait ErasedWindow {
    fn key(&self) -> &str;
    fn take_windows(&mut self) -> Vec<AnyWindowSpec>;
    fn resolve_args(&mut self, inherited: crate::tooling::Args) -> crate::tooling::Args;
    fn headless(self: Box<Self>, args: crate::tooling::Args, scale: f32);
    fn desktop(self: Box<Self>, args: &crate::tooling::Args, selected: bool, scale: f32) -> Box<dyn crate::desktop::Member>;
    fn start(self: Box<Self>, inherited: crate::tooling::Args, cx: crate::window::Start<'_>) -> Option<Box<dyn crate::window::AnyHost>>;
}

struct Erased<S: 'static> {
    spec: WindowSpec<S>,
    handle: Option<crate::WindowHandle>,
    secondary: bool,
}

impl<S: 'static> ErasedWindow for Erased<S> {
    fn key(&self) -> &str { &self.spec.key }
    fn take_windows(&mut self) -> Vec<AnyWindowSpec> { std::mem::take(&mut self.spec.app.windows) }
    fn resolve_args(&mut self, inherited: crate::tooling::Args) -> crate::tooling::Args { self.spec.app.resolve_args(inherited) }
    fn headless(self: Box<Self>, args: crate::tooling::Args, scale: f32) {
        let WindowSpec { mut app, options, .. } = self.spec;
        if let Some((w, h)) = options.physical_size { app.spec.size = (w as f32 / scale, h as f32 / scale); }
        let transparent = options.transparent;
        app.window_options = options;
        crate::tooling::headless_configured(app, args, transparent);
    }
    fn desktop(self: Box<Self>, args: &crate::tooling::Args, selected: bool, scale: f32) -> Box<dyn crate::desktop::Member> {
        let Erased { spec, handle, secondary } = *self;
        Box::new(crate::desktop::Window::new(spec, handle, args, selected, !secondary, scale))
    }
    fn start(self: Box<Self>, mut inherited: crate::tooling::Args, cx: crate::window::Start<'_>) -> Option<Box<dyn crate::window::AnyHost>> {
        // A secondary window is not what --scene or --size describe unless --window selects it.
        if self.secondary && inherited.window.as_deref() != Some(self.spec.key.as_str()) {
            inherited.scene = None;
            inherited.size = None;
        }
        crate::window::start(self.spec, self.handle, !self.secondary, inherited, cx)
    }
}

/// Each window followed by those its app added with `App::window`, depth first.
pub(crate) fn flatten(windows: impl IntoIterator<Item = AnyWindowSpec>) -> Vec<AnyWindowSpec> {
    let mut out = Vec::new();
    for mut window in windows {
        let children = window.0.take_windows();
        out.push(window);
        out.extend(flatten(children));
    }
    out
}

pub(crate) fn validate(windows: &[AnyWindowSpec]) -> Result<(), String> {
    let mut keys = std::collections::HashSet::new();
    for window in windows {
        if window.key().is_empty() || !keys.insert(window.key()) { return Err(format!("empty or duplicate window key: {:?}", window.key())); }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::dpi::{LogicalSize, Size};

    #[test]
    fn launcher_preset_is_hidden_and_preserves_ordinary_defaults() {
        let preset = WindowOptions::launcher_panel();
        assert!(preset.native.nonactivating_panel && preset.native.launcher);
        assert!(!preset.options.visible && preset.options.borderless && preset.options.transparent);
        assert!(!MacWindowOptions::default().nonactivating_panel(true).launcher);
        assert!(!preset.native.nonactivating_panel(false).launcher);
        assert!(WindowOptions::default().visible);
        let spec = AppSpec::new("launcher", (300.0, 100.0), || (), |_, _| crate::el());
        let app = App::new(spec).configured_options(WindowOptions::launcher_panel());
        assert!(app.macos_window.launcher);
        assert!(!app.window_options.visible);
        let window = WindowSpec::from_app("launcher", app);
        assert!(!window.options.visible && window.options.transparent);
    }

    #[test]
    fn amending_a_preset_keeps_its_launcher_behaviour() {
        let preset = WindowOptions::launcher_panel().macos(|native| native.corner_radius(16.0).shadow(true));
        assert!(preset.native.launcher && preset.native.nonactivating_panel);
        assert_eq!((preset.native.corner_radius, preset.native.shadow), (Some(16.0), Some(true)));
        assert!(!preset.options.visible && preset.options.transparent);
        let spec = AppSpec::new("launcher", (300.0, 100.0), || (), |_, _| crate::el());
        let app = App::new(spec).configured_options(preset);
        assert!(app.macos_window.launcher);
        assert_eq!(app.macos_window.corner_radius, Some(16.0));
    }

    #[test]
    fn app_windows_follow_their_app_and_share_its_key_space() {
        let mut main = App::new(AppSpec::new("main", (100.0, 100.0), || 0u32, |_, _| crate::el()));
        let settings = || WindowSpec::new("settings", AppSpec::new("settings", (100.0, 100.0), String::new, |_, _| crate::el()));
        let (_, handle) = main.window(settings());
        assert_eq!(handle.key(), "settings");
        let other = WindowSpec::new("other", AppSpec::new("other", (100.0, 100.0), || (), |_, _| crate::el()));
        let flat = flatten([WindowSpec::from_app("main", main).boxed(), other.into()]);
        assert_eq!(flat.iter().map(AnyWindowSpec::key).collect::<Vec<_>>(), ["main", "settings", "other"]);
        assert!(validate(&flat).is_ok());
        let mut clash = App::new(AppSpec::new("main", (100.0, 100.0), || 0u32, |_, _| crate::el()));
        clash.window(settings());
        assert!(validate(&flatten([WindowSpec::from_app("settings", clash).boxed()])).is_err());
    }

    #[test]
    fn default_window_options_remain_inferable_at_existing_call_sites() {
        // This fails to compile if options becomes generic over Into<...>.
        let _configure: fn(WindowSpec<()>) -> WindowSpec<()> = |spec| spec.options(Default::default());
    }

    #[test]
    fn explicit_empty_args_never_read_the_host_cli() {
        let options = RunOptions { accessory: true };
        let copied = options;
        assert!(options.accessory && copied.accessory);
        let args = crate::tooling::window_args(Some(vec![]), || panic!("must not inspect host --trace/--snap flags"));
        assert!(!args.headless());
        assert!(args.scene.is_none() && args.size.is_none());
        let inherited = crate::tooling::window_args(None, || crate::tooling::Args::parse_from(
            vec!["--trace".into(), "out.jsonl".into()]));
        assert!(inherited.headless());
    }

    #[test]
    fn window_app_args_override_inherited_scene_and_size() {
        fn spec() -> AppSpec<()> { AppSpec::new("test", (100.0, 100.0), || (), |_, _| crate::el()) }
        let inherited = crate::tooling::Args::parse_from(vec!["--scene".into(), "host-scene".into()]);
        let mut window = WindowSpec::from_app("pill", App::new(spec()).args(vec!["--size".into(), "320x80".into()]));
        let resolved = window.app.resolve_args(inherited.clone());
        assert_eq!(resolved.size, Some((320.0, 80.0)));
        assert!(resolved.scene.is_none());
        let mut plain = WindowSpec::new("plain", spec());
        assert_eq!(plain.app.resolve_args(inherited).scene.as_deref(), Some("host-scene"));
        let trace = crate::tooling::Args::parse_from(vec!["--trace".into(), "host.jsonl".into()]);
        let mut isolated = WindowSpec::from_app("isolated", App::new(spec()).args(vec![]));
        assert!(!isolated.app.resolve_args(trace).headless());
        let mut traced = WindowSpec::from_app("traced", App::new(spec()).args(vec!["--trace".into(), "window.jsonl".into()]));
        assert!(traced.app.resolve_args(crate::tooling::Args::default()).headless());
    }

    #[test]
    fn compact_window_overrides_the_legacy_minimum_without_changing_defaults() {
        let defaults = WindowOptions::default().attributes("app", (560.0, 440.0), None, true);
        assert_eq!(defaults.min_inner_size, Some(Size::Logical(LogicalSize::new(420.0, 400.0))));
        assert!(defaults.resizable && defaults.decorations && defaults.visible);
        assert_eq!(defaults.max_inner_size, None);

        let compact = WindowOptions {
            min_size: Some((280.0, 80.0)), max_size: Some((320.0, 120.0)),
            resizable: false, borderless: true, ..Default::default()
        }.attributes("compact", (300.0, 100.0), Some((860.0, 560.0)), false);
        assert_eq!(compact.inner_size, Some(Size::Logical(LogicalSize::new(300.0, 100.0))));
        assert_eq!(compact.min_inner_size, Some(Size::Logical(LogicalSize::new(280.0, 80.0))));
        assert_eq!(compact.max_inner_size, Some(Size::Logical(LogicalSize::new(320.0, 120.0))));
        assert!(!compact.resizable && !compact.decorations && !compact.visible);
    }

    #[test]
    fn maximum_alone_caps_the_inherited_minimum_per_axis() {
        let options = WindowOptions { max_size: Some((320.0, 120.0)), ..Default::default() };
        for (app_min, expected) in [
            (None, (320.0, 120.0)),
            (Some((860.0, 80.0)), (320.0, 80.0)),
        ] {
            let attrs = options.attributes("compact", (300.0, 100.0), app_min, true);
            assert_eq!(attrs.min_inner_size, Some(Size::Logical(LogicalSize::new(expected.0, expected.1))));
            assert_eq!(attrs.max_inner_size, Some(Size::Logical(LogicalSize::new(320.0, 120.0))));
        }
    }

    #[test]
    fn default_options_inherit_the_app_minimum() {
        let attrs = WindowOptions::default().attributes("app", (900.0, 600.0), Some((860.0, 560.0)), true);
        assert_eq!(attrs.min_inner_size, Some(Size::Logical(LogicalSize::new(860.0, 560.0))));
    }

    #[test]
    fn a_window_that_fits_its_content_has_no_default_minimum_height() {
        let fit = WindowOptions::default().fit_min_size(None);
        assert_eq!(WindowOptions::default().size_limits(fit), ((420.0, 0.0), None));
        assert_eq!(WindowOptions::default().fit_min_size(Some((300.0, 200.0))), Some((300.0, 200.0)));
        let options = WindowOptions { min_size: Some((300.0, 500.0)), ..Default::default() };
        assert_eq!(options.size_limits(options.fit_min_size(None)), ((300.0, 500.0), None));
    }
}
