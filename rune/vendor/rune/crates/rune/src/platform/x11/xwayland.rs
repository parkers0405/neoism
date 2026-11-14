//! An overlay on XWayland, for a Wayland compositor without layer-shell (GNOME's Mutter): an
//! override-redirect ARGB window of the notification type on its own libxcb connection. Like a
//! layer surface it is mapped while it lives and destroyed when hidden. An overlay that takes no
//! input gets an empty input shape, so it never takes a click. One whose host set
//! `OverlayOptions::xwayland_input` takes the pointer in its input shape (all of it, or its hit
//! regions) and reads its own events; being override-redirect, it never takes keyboard focus.
//! The rest of the app stays on Wayland.

use std::num::NonZeroU32;
use std::os::fd::AsRawFd;
use std::ptr::NonNull;

use winit::raw_window_handle::{RawDisplayHandle, RawWindowHandle, XcbDisplayHandle, XcbWindowHandle};
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ClipOrdering, ColormapAlloc, ConfigureWindowAux, ConnectionExt as _, CreateWindowAux,
    EventMask, KeyButMask, PropMode, Rectangle, StackMode, VisualClass, WindowClass,
};
use x11rb::wrapper::ConnectionExt as _;
use x11rb::xcb_ffi::XCBConnection;

use crate::overlay::Placement;
use crate::platform::layer_shell::{Input, Watch};
use crate::runtime::Waker;
use crate::{DisplayTarget, OverlayInput, OverlayOptions};

/// The events a window that takes input asks for; one that takes none asks for nothing.
fn events(input: OverlayInput) -> EventMask {
    if input == OverlayInput::None { return EventMask::NO_EVENT; }
    EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION | EventMask::ENTER_WINDOW | EventMask::LEAVE_WINDOW
}

pub(crate) struct XwaylandSurface {
    /// Dropped first: its thread stops polling the connection's socket within one timeout.
    watch: Option<Watch>,
    connection: XCBConnection,
    screen: usize,
    root: u32,
    window: u32,
    colormap: u32,
    scale: f32,
    input: OverlayInput,
    /// The input shape's rectangles (pixels) last set for `OverlayInput::Regions`.
    regions: Option<Vec<(i16, i16, u16, u16)>>,
    size: (u16, u16),
}

impl XwaylandSurface {
    /// `None` without an X server (`DISPLAY`), a 32-bit visual, or the SHAPE extension the
    /// input region needs. `input` is the overlay's resolved input; anything but `None` asks
    /// for pointer events, and `wake` turns the event loop when they arrive.
    pub fn new(name: &str, at: Placement, options: &OverlayOptions, input: OverlayInput, wake: Option<Waker>) -> Option<Self> {
        std::env::var_os("DISPLAY").filter(|display| !display.is_empty())?;
        let (connection, screen) = XCBConnection::connect(None).ok()?;
        let version = connection.shape_query_version().ok()?.reply().ok()?;
        if (version.major_version, version.minor_version) < (1, 1) { return None; }
        let root_screen = connection.setup().roots.get(screen)?;
        let root = root_screen.root;
        let visual = root_screen.allowed_depths.iter().filter(|d| d.depth == 32)
            .flat_map(|d| d.visuals.iter()).find(|v| v.class == VisualClass::TRUE_COLOR)?.visual_id;
        let scale = xft_scale(&connection, root);
        let colormap = connection.generate_id().ok()?;
        connection.create_colormap(ColormapAlloc::NONE, colormap, root, visual).ok()?.check().ok()?;
        let window = connection.generate_id().ok()?;
        let mut this = Self { watch: None, connection, screen, root, window: 0, colormap, scale, input: OverlayInput::None, regions: None, size: (1, 1) };
        let (x, y, w, h) = this.rect(at, options)?;
        this.size = (w, h);
        let aux = CreateWindowAux::new().override_redirect(1).background_pixel(0).border_pixel(0).colormap(colormap)
            .event_mask(events(input));
        this.connection.create_window(32, window, root, x, y, w, h, 0, WindowClass::INPUT_OUTPUT, visual, &aux).ok()?.check().ok()?;
        this.window = window;
        let atom = |name: &[u8]| Some(this.connection.intern_atom(false, name).ok()?.reply().ok()?.atom);
        let (kind, notification) = (atom(b"_NET_WM_WINDOW_TYPE")?, atom(b"_NET_WM_WINDOW_TYPE_NOTIFICATION")?);
        let c = &this.connection;
        c.change_property32(PropMode::REPLACE, window, kind, AtomEnum::ATOM, &[notification]).ok()?;
        c.change_property8(PropMode::REPLACE, window, AtomEnum::WM_NAME, AtomEnum::STRING, name.as_bytes()).ok()?;
        let id = crate::tooling::app_id().unwrap_or(name);
        let class = [id.as_bytes(), b"\0", id.as_bytes(), b"\0"].concat();
        c.change_property8(PropMode::REPLACE, window, AtomEnum::WM_CLASS, AtomEnum::STRING, &class).ok()?;
        this.shape(input)?;
        let c = &this.connection;
        c.map_window(window).ok()?;
        c.configure_window(window, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE)).ok()?;
        c.flush().ok()?;
        this.input = input;
        if input != OverlayInput::None { this.watch = wake.map(|wake| Watch::spawn(this.connection.as_raw_fd(), wake)); }
        Some(this)
    }

    /// The input shape for `input`: the whole window, nothing, or (for regions) nothing until
    /// `regions` sets them.
    fn shape(&mut self, input: OverlayInput) -> Option<()> {
        self.regions = None;
        let c = &self.connection;
        match input {
            OverlayInput::All => c.shape_mask(SO::SET, SK::INPUT, self.window, 0, 0, x11rb::NONE).ok()?.check().ok(),
            _ => c.shape_rectangles(SO::SET, SK::INPUT, ClipOrdering::UNSORTED, self.window, 0, 0, &[]).ok()?.check().ok(),
        }
    }

    /// Changes what the window takes. Turning input on needs a watch made with the surface, so
    /// an overlay made without input stays click-through until it is made again.
    pub fn set_input(&mut self, input: OverlayInput) {
        let input = if self.watch.is_none() { OverlayInput::None } else { input };
        if input == self.input { return; }
        self.input = input;
        let _ = self.connection.change_window_attributes(self.window, &ChangeWindowAttributesAux::new().event_mask(events(input)));
        let _ = self.shape(input);
        let _ = self.connection.flush();
    }

    /// Sets the input shape to the overlay's hit regions (points) while it takes input in
    /// regions; checked, so the shape applies before the frame's pixels present.
    pub fn regions(&mut self, rects: &[[f32; 4]]) {
        if self.input != OverlayInput::Regions { return; }
        let scale = self.scale;
        let rects: Vec<_> = rects.iter().filter_map(|r| {
            let x = (r[0] * scale).floor().clamp(0.0, f32::from(i16::MAX)) as i16;
            let y = (r[1] * scale).floor().clamp(0.0, f32::from(i16::MAX)) as i16;
            let width = ((r[2] * scale).ceil() - f32::from(x)).clamp(0.0, f32::from(u16::MAX)) as u16;
            let height = ((r[3] * scale).ceil() - f32::from(y)).clamp(0.0, f32::from(u16::MAX)) as u16;
            (width > 0 && height > 0).then_some(Rectangle { x, y, width, height })
        }).collect();
        let key: Vec<_> = rects.iter().map(|r| (r.x, r.y, r.width, r.height)).collect();
        if self.regions.as_ref() == Some(&key) { return; }
        if self.connection.shape_rectangles(SO::SET, SK::INPUT, ClipOrdering::UNSORTED, self.window, 0, 0, &rects)
            .ok().and_then(|request| request.check().ok()).is_some() {
            self.regions = Some(key);
        }
    }

    /// The desktop's scale (`Xft.dpi` / 96), which XWayland's native scaling sets.
    pub fn scale(&self) -> f32 { self.scale }

    pub fn place(&mut self, at: Placement, options: &OverlayOptions) {
        let Some((x, y, w, h)) = self.rect(at, options) else { return };
        self.size = (w, h);
        let aux = ConfigureWindowAux::new().x(i32::from(x)).y(i32::from(y)).width(u32::from(w)).height(u32::from(h)).stack_mode(StackMode::ABOVE);
        let _ = self.connection.configure_window(self.window, &aux);
        let _ = self.connection.flush();
    }

    /// What the server sent since the last call, as overlay input in points. A window that
    /// takes no input asks for no events, so only errors arrive and are dropped.
    pub fn dispatch(&mut self) -> Vec<Input> {
        let mut input = Vec::new();
        while let Ok(Some(event)) = self.connection.poll_for_event() {
            if self.input != OverlayInput::None { pointer_input(event, self.scale, self.size, &mut input); }
        }
        if let Some(watch) = &self.watch { watch.turned(); }
        input
    }

    pub fn raw_handles(&self) -> Option<(RawDisplayHandle, RawWindowHandle)> {
        let display = XcbDisplayHandle::new(NonNull::new(self.connection.get_raw_xcb_connection()), i32::try_from(self.screen).ok()?);
        let window = XcbWindowHandle::new(NonZeroU32::new(self.window)?);
        Some((RawDisplayHandle::Xcb(display), RawWindowHandle::Xcb(window)))
    }

    /// The window's pixels on the target monitor: the primary unless the pointer's is asked for.
    fn rect(&self, at: Placement, options: &OverlayOptions) -> Option<(i16, i16, u16, u16)> {
        let monitors = self.connection.randr_get_monitors(self.root, true).ok()?.reply().ok()?.monitors;
        let bounds: Vec<_> = monitors.iter().map(|m| {
            let (x, y) = (f64::from(m.x), f64::from(m.y));
            [x, y, x + f64::from(m.width), y + f64::from(m.height)]
        }).collect();
        let primary = monitors.iter().position(|m| m.primary);
        let pointer = (options.display == DisplayTarget::UnderPointer).then(|| {
            let p = self.connection.query_pointer(self.root).ok()?.reply().ok()?;
            p.same_screen.then_some((f64::from(p.root_x), f64::from(p.root_y)))
        }).flatten();
        let monitor = &monitors[crate::overlay::options::choose_display(&bounds, primary, None, pointer)?];
        let scale = self.scale;
        let area = (f32::from(monitor.width) / scale, f32::from(monitor.height) / scale);
        let r = crate::overlay::options::local_rect(at, options.placement, area);
        let px = |v: f32| (v * scale).round();
        let x = (f32::from(monitor.x) + px(r[0])).clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
        let y = (f32::from(monitor.y) + px(r[1])).clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
        let size = |v: f32| px(v).clamp(1.0, f32::from(u16::MAX)) as u16;
        Some((x, y, size(r[2]), size(r[3])))
    }
}

impl Drop for XwaylandSurface {
    fn drop(&mut self) {
        if self.window != 0 { let _ = self.connection.destroy_window(self.window); }
        let _ = self.connection.free_colormap(self.colormap);
        let _ = self.connection.flush();
    }
}

/// One X event as overlay input: positions in points, the primary button as `Button` and the
/// secondary as `Secondary`. While a button is held the window keeps the pointer (X11's
/// implicit grab), so leaving then is ignored and a release outside the window leaves.
fn pointer_input(event: Event, scale: f32, size: (u16, u16), input: &mut Vec<Input>) {
    let at = |x: i16, y: i16| Input::Move(f32::from(x) / scale, f32::from(y) / scale);
    let (e, down) = match event {
        Event::MotionNotify(e) => return input.push(at(e.event_x, e.event_y)),
        Event::EnterNotify(e) => return input.push(at(e.event_x, e.event_y)),
        Event::LeaveNotify(e) => {
            let held = KeyButMask::BUTTON1 | KeyButMask::BUTTON2 | KeyButMask::BUTTON3;
            if !e.state.intersects(held) { input.push(Input::Leave); }
            return;
        }
        Event::ButtonPress(e) => (e, true),
        Event::ButtonRelease(e) => (e, false),
        _ => return,
    };
    let button = match e.detail {
        1 => Input::Button(down),
        3 => Input::Secondary(down),
        _ => return,
    };
    input.push(at(e.event_x, e.event_y));
    input.push(button);
    let inside = (0..i32::from(size.0)).contains(&i32::from(e.event_x)) && (0..i32::from(size.1)).contains(&i32::from(e.event_y));
    if !down && !inside { input.push(Input::Leave); }
}

/// `Xft.dpi` from the root window's resource database, as a scale; 1 when unset.
fn xft_scale(connection: &XCBConnection, root: u32) -> f32 {
    let Some(reply) = connection.get_property(false, root, AtomEnum::RESOURCE_MANAGER, AtomEnum::STRING, 0, 1 << 16)
        .ok().and_then(|cookie| cookie.reply().ok()) else { return 1.0 };
    String::from_utf8_lossy(&reply.value).lines()
        .find_map(|line| line.strip_prefix("Xft.dpi:")?.trim().parse::<f32>().ok())
        .map_or(1.0, |dpi| (dpi / 96.0).clamp(1.0, 4.0))
}

#[cfg(test)]
#[path = "xwayland_tests.rs"]
mod tests;
