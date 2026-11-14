//! Placement and input policy for native overlays. Coordinates inside a display are points.

use crate::{Align, Cx, El, OverlaySpec, ScreenEdge};

/// An opaque display identity, valid for this event-loop session. Obtain it from `Display::id`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayId(pub(crate) winit::monitor::MonitorHandle, Option<NativeDisplayId>);

/// Native display identity. Windows handles retain their full pointer width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeDisplayId {
    MacOS(u32), Windows(isize),
    /// winit identifies X11 monitors by RandR CRTC. Output IDs can also be targeted below.
    X11Crtc(u32), X11Output(u32),
    /// Registry-global wl_output name, not a client-local protocol object ID.
    Wayland(u32),
}

impl DisplayId {
    pub fn native_id(&self) -> Option<NativeDisplayId> { self.1 }

    pub(crate) fn from_monitor(monitor: winit::monitor::MonitorHandle, _wayland: bool) -> Self {
        #[cfg(target_os = "macos")]
        let native = { use winit::platform::macos::MonitorHandleExtMacOS; Some(NativeDisplayId::MacOS(monitor.native_id())) };
        #[cfg(target_os = "windows")]
        let native = { use winit::platform::windows::MonitorHandleExtWindows; Some(NativeDisplayId::Windows(monitor.hmonitor())) };
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        let native = Some(if _wayland {
            use winit::platform::wayland::MonitorHandleExtWayland;
            NativeDisplayId::Wayland(monitor.native_id())
        } else {
            use winit::platform::x11::MonitorHandleExtX11;
            NativeDisplayId::X11Crtc(monitor.native_id())
        });
        #[cfg(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))]
        let native = None;
        Self(monitor, native)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum DisplayTarget {
    #[default]
    Primary,
    UnderPointer,
    Id(DisplayId),
    /// Resolve a native capture/display ID; disconnected IDs fall back to primary.
    Native(NativeDisplayId),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OverlayPlacement {
    Edge { edge: ScreenEdge, margin: f32, align: Align, inset: f32 },
    /// Points from the selected display's top-left, not desktop pixels. The rect isn't kept
    /// on that display, so on [`DisplayTarget::Primary`] it's a global rect that may sit on
    /// any display: the space [`El::on_screen_layout`] reports in, so an overlay can open
    /// exactly over an element of a window. On macOS one point space spans displays of any
    /// scale, and the overlay draws at the backing scale of the display it lands on.
    /// Elsewhere a rect off the primary display is exact only where displays share a scale.
    Rect { x: f32, y: f32, width: f32, height: f32 },
    FullScreen,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlayLevel {
    #[default]
    Status,
    AboveStatus,
    /// Above Rune's status and picker overlays (macOS level 27).
    AboveOverlays,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyPolicy {
    #[default]
    None,
    /// Permit `OverlayHandle::request_keys`; showing alone never requests focus.
    OnDemand,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlayInput {
    /// Preserve `OverlaySpec::interactive` (whole surface or complete pass-through).
    #[default]
    Legacy,
    /// Pass every click through, and hear no hover. Following remains available when enabled;
    /// use Regions without `.hit()` elements for a hover-only canvas.
    None,
    /// Only elements marked `El::hit()` intercept the pointer.
    Regions,
    All,
}

impl OverlayInput {
    pub(crate) fn resolved(self, interactive: bool) -> Self {
        match self { Self::Legacy => if interactive { Self::All } else { Self::None }, other => other }
    }
}

/// An overlay shows its elements' [`Cursor`](crate::Cursor)s while the pointer is over a hit
/// area (with [`OverlayInput::Regions`], a hit region). On macOS that works for a non-key
/// overlay only while its app is frontmost, unless `rune` is built with the
/// `macos-background-cursor` feature, which uses private API (an App Store review risk).
#[derive(Clone, Debug, Default)]
pub struct OverlayOptions {
    /// None preserves the spec's edge, margin, alignment and inset.
    pub placement: Option<OverlayPlacement>,
    pub display: DisplayTarget,
    /// Re-resolve UnderPointer while visible. Unsupported on Wayland.
    pub follow_pointer: bool,
    pub level: OverlayLevel,
    /// Native best effort, not a security boundary. Use the window number in capture filters.
    /// Applies only while shown, including the exit fade. On macOS, hidden panels order out
    /// before restoring sharing, even when prewarmed: alpha zero and a sharing reset alone
    /// must not leave an excluded window on every Space. Exclusion resumes before ordering in.
    /// `OverlayHandle::set_exclude_from_capture` changes it later.
    pub exclude_from_capture: bool,
    pub keys: KeyPolicy,
    pub input: OverlayInput,
    /// Create and warm a hidden native host on the event thread at startup. Desktop hosts
    /// retain their window and GPU surface across hide/show (unless
    /// `ConfiguredOverlay::release_gpu_when_hidden`); Wayland must recreate on hide.
    /// macOS orders the panel in at alpha zero only for a scoped prewarm, then orders it out.
    /// Initial preparation settles the view. Later reveals refresh content, geometry and
    /// live values while preserving motion elapsed since each hidden commit. Hidden motion
    /// catches up on poll/reveal without driving a frame loop.
    pub prewarm: bool,
    /// Wayland without layer-shell (GNOME): lets an overlay that takes pointer input show on
    /// XWayland, as an override-redirect window that takes clicks in its input shape and never
    /// keyboard focus. Off by default: connecting starts XWayland on GNOME, so a host opts in
    /// explicitly, having checked that it already runs. `KeyPolicy::OnDemand` overlays still
    /// can't use it. Read when the surface is made, at each show on Wayland;
    /// `OverlayHandle::set_xwayland_input` changes it.
    pub xwayland_input: bool,
}

/// Keeps existing exhaustive `OverlaySpec` literals source compatible.
pub struct ConfiguredOverlay<T: 'static> {
    pub spec: OverlaySpec<T>,
    pub options: OverlayOptions,
    pub on_key: Option<fn(&mut T, &crate::KeyEvent, &mut crate::Tx) -> bool>,
    pub on_created: Option<Box<dyn FnMut(OverlayCreated)>>,
    pub(crate) poll: Option<fn(&mut T, f64, &mut crate::Tx) -> bool>,
    pub(crate) on_focus: Option<fn(&mut T, bool, &mut crate::Tx)>,
    pub(crate) compositor: Option<Box<dyn crate::compose::Compositor<T>>>,
    pub(crate) setup: Option<fn(&mut T, crate::Waker)>,
    pub(crate) srgb_blend: bool,
    pub(crate) macos_level: Option<isize>,
    pub(crate) modifier_keys: bool,
    pub(crate) animate_reveal: bool,
    pub(crate) release_after: Option<std::time::Duration>,
}

#[derive(Clone, Copy, Debug)]
pub struct OverlayCreated {
    /// The actual macOS panel CGWindowID, available before its first show.
    pub window_number: Option<u32>,
    pub capabilities: OverlayCapabilities,
}

impl<T: 'static> ConfiguredOverlay<T> {
    pub fn on_focus(mut self, f: fn(&mut T, bool, &mut crate::Tx)) -> Self { self.on_focus = Some(f); self }
    /// Composite this overlay's translucent fills, blurs, shadows, images, video and text in
    /// sRGB (gamma) space instead of linear light, matching CSS and Core Animation (on by
    /// default). With a `Compositor`, `Frame::srgb_blend` says which encoding its passes must
    /// match.
    pub fn srgb_blend(mut self, enabled: bool) -> Self { self.srgb_blend = enabled; self }
    /// Called before each frame; true means state changed, not that another frame is due.
    /// Defaults to the app's poll when its state type matches this overlay's. Other state
    /// types have no inherited poll. Use `Tx::wake_after` for the next timed update;
    /// deadlines also run while hidden, like the window host, without drawing.
    pub fn poll(mut self, f: fn(&mut T, f64, &mut crate::Tx) -> bool) -> Self { self.poll = Some(f); self }
    /// Draw every rendered overlay frame, including hidden prewarm frames, through this
    /// compositor. `Frame::draw_ui` keeps the overlay's transparent clear. Runtime-only
    /// hidden timer updates do not render or call the compositor.
    pub fn compositor(mut self, compositor: impl crate::compose::Compositor<T>) -> Self {
        self.compositor = Some(Box::new(compositor));
        self
    }
    pub fn on_key(mut self, f: fn(&mut T, &crate::KeyEvent, &mut crate::Tx) -> bool) -> Self { self.on_key = Some(f); self }
    /// Like `App::modifier_keys`: send modifier keys pressed and released on their own
    /// (`Key::Modifier`) to `on_key` and the focused input. Off by default.
    pub fn modifier_keys(mut self) -> Self { self.modifier_keys = true; self }
    /// Like `AppSpec::setup`: called once with a waker other threads use when new data arrives
    /// (an `ImageCache` decode, a channel). It runs on the event thread when the event loop
    /// starts, before the overlay's first frame and before updates queued through its handle.
    /// Waking a shown overlay draws a frame, which polls; a hidden one runs a frame without
    /// drawing. Headless runs don't create overlays, so they don't call it.
    pub fn setup(mut self, f: fn(&mut T, crate::Waker)) -> Self { self.setup = Some(f); self }
    /// Called on the event thread after native creation, before the first show, and again for
    /// each window made later (Windows and macOS make a new one for each show with
    /// `release_gpu_when_hidden`), so a window number in a capture filter stays current.
    /// Use `prewarm: true` to create at startup rather than waiting for `show()`.
    pub fn on_created(mut self, f: impl FnMut(OverlayCreated) + 'static) -> Self { self.on_created = Some(Box::new(f)); self }
    /// Showing doesn't settle motion: what the state started while hidden plays from the
    /// first visible frame, such as a notification stack's first card entering. By default
    /// an unretained overlay shows settled, at its target.
    pub fn animate_reveal(mut self) -> Self { self.animate_reveal = true; self }
    /// While hidden, the overlay holds no GPU surface, swapchain or renderer. Each show makes the
    /// surface, and each hide drops it. Away from macOS, the GPU device and renderer then wait
    /// five minutes for the next surface (a window's or overlay's) before they go, so an overlay
    /// shown often (a HUD on each dictation) keeps one device rather than making one per show
    /// (hex#1268). On Windows and macOS the window comes and goes with the surface, and a
    /// prewarmed one makes nothing at startup, so a hidden overlay there has no window at all
    /// (NVIDIA's Instant Replay took a hidden display-sized window for a game on Windows, and
    /// macOS 27's Mission Control lost its Space thumbnails to hidden screen-sized panels). On X11
    /// a prewarmed one makes only its window at startup and keeps it. A show then waits for the
    /// surface (a few milliseconds), and with no device to take, for a new device and its
    /// pipelines too (over half a second on some Windows PCs). For overlays that show rarely and
    /// cover a display, such as a drawing layer or a picker: hidden, their swapchains hold tens
    /// of megabytes. By default a prewarmed overlay makes its surface at startup and keeps it,
    /// so it shows at once.
    pub fn release_gpu_when_hidden(mut self) -> Self { self.release_after = Some(std::time::Duration::ZERO); self }
    /// `release_gpu_when_hidden`, once the overlay has stayed hidden for `after`: shows in
    /// a burst (a HUD across a few dictations) keep their surface and show at once, and an app
    /// at rest gives the device back. A prewarmed one still makes its surface only to show.
    /// macOS ignores it: a prewarmed panel retains its pixels at alpha zero.
    pub fn release_gpu_after(mut self, after: std::time::Duration) -> Self { self.release_after = Some(after); self }
    /// The macOS window level (`NSWindow.Level`, e.g. 3 for `.floating`), in place of
    /// `OverlayOptions::level`; `OverlayHandle::set_macos_level` changes it later. At any
    /// level a macOS overlay goes exactly where it's placed, over the menu bar too.
    pub fn macos_level(mut self, level: isize) -> Self { self.macos_level = Some(level); self }
}

impl<T: 'static> OverlaySpec<T> {
    pub fn new(name: &'static str, size: (f32, f32), view: fn(&T, &mut Cx<T>) -> El) -> Self {
        Self { name, size, edge: ScreenEdge::Top, margin: 0.0, align: Align::Center, inset: 0.0, interactive: false, view }
    }

    pub fn options(self, options: OverlayOptions) -> ConfiguredOverlay<T> {
        ConfiguredOverlay { spec: self, options, on_key: None, on_created: None, poll: None, on_focus: None, compositor: None, setup: None, srgb_blend: true, macos_level: None, modifier_keys: false, animate_reveal: false, release_after: None }
    }
}

impl<T: 'static> From<OverlaySpec<T>> for ConfiguredOverlay<T> {
    fn from(spec: OverlaySpec<T>) -> Self { spec.options(OverlayOptions::default()) }
}

/// Native capability, distinct from whether an overlay could be created.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlaySupport {
    #[default]
    Unknown,
    Supported,
    /// The OS accepted a capture-exclusion request; capture clients may still need a filter.
    Requested,
    Unsupported,
    Failed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OverlayCapabilities {
    pub capture_exclusion: OverlaySupport,
    pub nonactivating_keys: OverlaySupport,
    pub global_pointer: OverlaySupport,
}

/// Bounds in logical points, local to a display. Also shared by native placement tests.
pub(crate) fn local_rect(at: crate::overlay::Placement, placement: Option<OverlayPlacement>, area: (f32, f32)) -> [f32; 4] {
    let (edge, margin, align, inset) = match placement {
        Some(OverlayPlacement::FullScreen) => return [0.0, 0.0, area.0, area.1],
        Some(OverlayPlacement::Rect { x, y, width, height }) => return [x, y, width.max(1.0), height.max(1.0)],
        Some(OverlayPlacement::Edge { edge, margin, align, inset }) => (edge, margin, align, inset),
        None => (at.edge, at.margin, at.align, at.inset),
    };
    let (w, h) = (at.size.0.max(1.0), at.size.1.max(1.0));
    let x = match align { Align::Start => inset, Align::Center => (area.0 - w) / 2.0, Align::End => area.0 - w - inset };
    let y = match edge { ScreenEdge::Top => margin, ScreenEdge::Bottom => area.1 - h - margin };
    [x, y, w, h]
}

pub(crate) fn contains(rect: [f32; 4], x: f32, y: f32) -> bool {
    x >= rect[0] && y >= rect[1] && x < rect[2] && y < rect[3]
}

/// Bounds and pointer share one native coordinate space. Never rescale desktop origins.
pub(crate) fn choose_display(bounds: &[[f64; 4]], primary: Option<usize>, requested: Option<usize>, pointer: Option<(f64, f64)>) -> Option<usize> {
    requested.filter(|i| *i < bounds.len()).or_else(|| pointer.and_then(|(x, y)| {
        bounds.iter().position(|r| x >= r[0] && y >= r[1] && x < r[2] && y < r[3])
    })).or(primary.filter(|i| *i < bounds.len())).or_else(|| (!bounds.is_empty()).then_some(0))
}

/// The scale of the display holding most of `frame` (`[x, y, w, h]`), as AppKit picks a
/// window's backing scale; `None` when it's on none. `displays` are `(frame, scale)` in the
/// same space.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn scale_under(frame: [f64; 4], displays: &[([f64; 4], f64)]) -> Option<f64> {
    let overlap = |d: [f64; 4]| {
        let w = (frame[0] + frame[2]).min(d[0] + d[2]) - frame[0].max(d[0]);
        let h = (frame[1] + frame[3]).min(d[1] + d[3]) - frame[1].max(d[1]);
        w.max(0.0) * h.max(0.0)
    };
    displays.iter().map(|&(d, scale)| (overlap(d), scale)).filter(|(area, _)| *area > 0.0)
        .max_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, scale)| scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_takes_the_scale_of_the_display_holding_most_of_it() {
        // A 2x laptop with a 1x monitor to its right, in one global space.
        let displays = [([0.0, 0.0, 1512.0, 982.0], 2.0), ([1512.0, 0.0, 1920.0, 1080.0], 1.0)];
        assert_eq!(scale_under([100.0, 100.0, 400.0, 300.0], &displays), Some(2.0));
        assert_eq!(scale_under([1400.0, 100.0, 400.0, 300.0], &displays), Some(1.0), "mostly on the monitor");
        assert_eq!(scale_under([1400.0, 100.0, 200.0, 300.0], &displays), Some(2.0), "mostly on the laptop");
        assert_eq!(scale_under([-900.0, 0.0, 400.0, 300.0], &displays), None, "on no display");
    }

    #[test]
    fn display_selection_handles_negative_origins_shared_edges_and_disconnects() {
        // Native pixels, so a 2x display can have a larger rectangle than the primary.
        let bounds = [[-3840.0, -200.0, 0.0, 1960.0], [0.0, 0.0, 1920.0, 1080.0]];
        assert_eq!(choose_display(&bounds, Some(1), None, Some((-100.0, 0.0))), Some(0));
        assert_eq!(choose_display(&bounds, Some(1), None, Some((0.0, 0.0))), Some(1));
        assert_eq!(choose_display(&bounds, Some(1), Some(0), None), Some(0));
        assert_eq!(choose_display(&bounds, Some(1), Some(4), None), Some(1));
        assert_eq!(choose_display(&bounds, None, None, None), Some(0));
        assert_eq!(choose_display(&[], None, None, None), None);
    }

    #[test]
    fn edge_and_rect_use_display_local_points() {
        let at = crate::overlay::Placement { size: (300.0, 80.0), edge: ScreenEdge::Bottom, margin: 24.0, align: Align::End, inset: 12.0 };
        assert_eq!(local_rect(at, None, (960.0, 540.0)), [648.0, 436.0, 300.0, 80.0]);
        assert_eq!(local_rect(at, Some(OverlayPlacement::FullScreen), (960.0, 540.0)), [0.0, 0.0, 960.0, 540.0]);
        assert_eq!(local_rect(at, Some(OverlayPlacement::Rect { x: -10.0, y: 20.0, width: 100.0, height: 50.0 }), (960.0, 540.0)), [-10.0, 20.0, 100.0, 50.0]);
        // A 2x display left of primary: scale local offsets, never the desktop origin.
        let r = local_rect(at, None, (960.0, 540.0));
        assert_eq!((-1920 + (r[0] * 2.0) as i32, -100 + (r[1] * 2.0) as i32), (-624, 772));
    }
}
