//! `WindowHandle`: a window's native reference that outlives any one host, and the requests
//! made through it before the host exists.

use super::*;

pub(super) type Request = Box<dyn FnOnce(usize) -> WakeEvent + Send>;

#[derive(Default)]
pub(super) struct NativeWindow {
    pub(super) window: Mutex<Weak<Window>>,
    pub(super) number: AtomicU32,
    pub(super) ready: AtomicBool,
    /// What the host's meter last published.
    pub(super) stats: Arc<Mutex<SurfaceStats>>,
    /// The event loop and slot once the host exists, and the requests made before it did.
    pub(super) route: Mutex<(Option<(EventLoopProxy<WakeEvent>, usize)>, Vec<Request>)>,
}

/// A window's native reference, refreshed across App close/reopen without owning the window.
#[derive(Clone)]
pub struct WindowHandle {
    pub(super) native: Arc<NativeWindow>,
    pub(super) key: Arc<str>,
}
impl WindowHandle {
    pub(crate) fn new(key: &str) -> Self { Self { native: Arc::default(), key: key.into() } }

    /// Routes requests to the window's host, starting with those queued before it existed.
    pub(super) fn bind(&self, proxy: EventLoopProxy<WakeEvent>, slot: usize) {
        let mut route = self.native.route.lock().unwrap();
        for request in std::mem::take(&mut route.1) { let _ = proxy.send_event(request(slot)); }
        route.0 = Some((proxy, slot));
    }

    /// A headless run never binds a host; it takes the queued requests instead.
    pub(crate) fn take_headless(&self) -> Vec<WakeEvent> {
        let queued = std::mem::take(&mut self.native.route.lock().unwrap().1);
        queued.into_iter().map(|request| request(0)).collect()
    }

    pub(crate) fn headless_pending(&self) -> bool {
        !self.native.route.lock().unwrap().1.is_empty()
    }

    pub(super) fn send(&self, request: impl FnOnce(usize) -> WakeEvent + Send + 'static) {
        let mut route = self.native.route.lock().unwrap();
        match &route.0 {
            Some((proxy, slot)) => { let _ = proxy.send_event(request(*slot)); }
            None => route.1.push(Box::new(request)),
        }
    }

    /// The first frame has completed for the current native lifetime. For a launcher
    /// this confirms GPU completion of an alpha-zero frame before reveal. False after destruction.
    pub fn is_ready(&self) -> bool { self.window().is_ok() && self.native.ready.load(Ordering::Acquire) }
    pub fn key(&self) -> &str { &self.key }
    /// The window's frame counts: redraws, presents, failed drawable acquisitions, loop turns
    /// and pacing: current totals, and per-second rates updated once a second while it draws.
    pub fn surface_stats(&self) -> SurfaceStats { crate::frame::surface_stats::read(&self.native.stats) }
    pub(super) fn window(&self) -> Result<Arc<Window>, String> {
        self.native.window.lock().unwrap().upgrade().ok_or_else(|| "window is closed".into())
    }
    /// macOS CGWindowID for ScreenCaptureKit exclusion; None before registration or after destruction.
    pub fn window_number(&self) -> Option<u32> {
        let _window = self.window().ok()?;
        let number = self.native.number.load(Ordering::Acquire);
        (number != 0).then_some(number)
    }
    /// Where the window is on the desktop and whether it's maximized, minimized or full screen,
    /// to save and reopen it there. None while it's closed, and on Wayland, which hides window
    /// positions. Call it on the UI thread.
    pub fn placement(&self) -> Option<crate::windows::WindowPlacement> {
        let window = self.window().ok()?;
        let (outer, inner) = (window.outer_position().ok()?, window.inner_position().ok()?);
        let size = window.inner_size();
        Some(crate::windows::WindowPlacement {
            position: (outer.x, outer.y),
            content_position: (inner.x, inner.y),
            content_size: (size.width, size.height),
            scale: window.scale_factor(),
            maximized: window.is_maximized(),
            minimized: window.is_minimized().unwrap_or(false),
            full_screen: window.fullscreen().is_some(),
        })
    }
    pub fn set_click_through(&self, click_through: bool) -> Result<(), String> {
        self.window()?.set_cursor_hittest(!click_through).map_err(|e| e.to_string())
    }
    pub fn set_always_on_top(&self, on: bool) {
        self.send(move |slot| WakeEvent::Level(slot, on));
    }
    /// Physical desktop pixels; retained across recreation. Unsupported on Wayland.
    pub fn set_position(&self, position: (i32, i32)) {
        self.send(move |slot| WakeEvent::Position(slot, position));
    }
    pub fn place(&self, display: crate::DisplayTarget, placement: crate::OverlayPlacement) {
        let place = crate::windows::PlaceRequest::new(display, placement);
        self.send(move |slot| WakeEvent::Place(slot, place));
    }
    /// [`Tx::resize_window`] from outside a handler, on `tok::motion::DEFAULT`.
    pub fn resize(&self, size: (f32, f32)) {
        self.frame(crate::window::frame::FrameRequest { size: Some(size), animate: true, ..Default::default() });
    }
    /// [`Tx::resize_window_instant`] from outside a handler.
    pub fn resize_instant(&self, size: (f32, f32)) {
        self.frame(crate::window::frame::FrameRequest { size: Some(size), ..Default::default() });
    }
    /// [`Tx::center_window`] from outside a handler.
    pub fn center(&self) {
        self.frame(crate::window::frame::FrameRequest { center: true, ..Default::default() });
    }
    pub(super) fn frame(&self, request: crate::window::frame::FrameRequest) {
        self.send(move |slot| WakeEvent::Frame(slot, request));
    }
    /// Show/recreate, or hide while keeping state. A launcher takes keys and reveals cached
    /// pixels; an ordinary window shows without requesting focus.
    pub fn set_visible(&self, visible: bool) {
        let request = if visible { WindowRequest::Show } else { WindowRequest::Hide };
        self.send(move |slot| WakeEvent::Window(slot, request));
    }
    pub fn show(&self) { self.set_visible(true); }
    pub fn hide(&self) { self.set_visible(false); }
    /// Focus/activate an already-visible window, restoring it if minimized. Wayland requests attention.
    pub fn focus(&self) { self.send(WakeEvent::Focus); }
    /// Show, restore and activate, creating the window if needed (see `AppHandle::open`).
    /// A default close destroys its host; use App::keep_running to reopen.
    /// Windows added with `App::window` always reopen. Requests made before the loop runs
    /// apply once the window's host starts.
    pub fn open(&self) { self.send(|slot| WakeEvent::Window(slot, WindowRequest::Open(None))); }
    pub fn close(&self) { self.send(|slot| WakeEvent::Window(slot, WindowRequest::Close)); }
    /// Borrow a raw native handle while alive. Platform thread restrictions still apply;
    /// do not retain an extracted pointer beyond the callback.
    pub fn with_raw_handle<R>(&self, f: impl for<'a> FnOnce(winit::raw_window_handle::WindowHandle<'a>) -> R) -> Result<R, String> {
        let window = self.window()?;
        let handle = window.window_handle().map_err(|e| e.to_string())?;
        Ok(f(handle))
    }
}

#[cfg(target_os = "macos")]
pub(super) fn native_window_number(window: &Window) -> Option<u32> {
    // SAFETY: called on winit's event thread.
    u32::try_from(unsafe { crate::platform::macos::ns_window(window)?.windowNumber() }).ok().filter(|n| *n != 0)
}
#[cfg(not(target_os = "macos"))]
pub(super) fn native_window_number(_: &Window) -> Option<u32> { None }
