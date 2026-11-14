//! X11 input SHAPE regions. The visual surface stays intact; only pointer ownership changes.

use x11rb::{connection::Connection, protocol::{shape::{ConnectionExt as _, SK, SO}, xproto::{ConnectionExt as _, ClipOrdering, ConfigureWindowAux, Rectangle, StackMode}}, rust_connection::RustConnection};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

thread_local! {
    static SHOWN: std::cell::RefCell<Vec<(u32, crate::OverlayLevel)>> = const { std::cell::RefCell::new(Vec::new()) };
}

pub(crate) struct InputWindow {
    watch: std::cell::RefCell<Option<crate::platform::x11::pointer::MouseWatch>>,
    wake: Option<crate::Waker>,
    visible: std::cell::Cell<bool>,
    follow: std::cell::Cell<bool>,
    input: std::cell::Cell<crate::OverlayInput>,
    connection: RustConnection,
    root: u32,
    window: u32,
    last: Option<Vec<Rectangle>>,
}

impl InputWindow {
    pub fn show(&self, level: crate::OverlayLevel) {
        self.visible.set(true);
        self.refresh_watch();
        let windows = SHOWN.with(|shown| {
            let mut shown = shown.borrow_mut();
            shown.retain(|(id, _)| *id != self.window);
            shown.push((self.window, level));
            shown.clone()
        });
        for level in [crate::OverlayLevel::Status, crate::OverlayLevel::AboveStatus, crate::OverlayLevel::AboveOverlays] {
            for (id, at) in &windows {
                if *at == level { let _ = self.connection.configure_window(*id, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE)); }
            }
        }
        let _ = self.connection.flush();
    }
    pub fn hide(&self) { self.visible.set(false); self.watch.borrow_mut().take(); SHOWN.with(|shown| shown.borrow_mut().retain(|(id, _)| *id != self.window)); }
    pub fn new(window: &winit::window::Window, wake: Option<crate::Waker>, input: crate::OverlayInput, follow: bool) -> Option<Self> {
        let id = match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Xlib(w) => u32::try_from(w.window).ok()?,
            RawWindowHandle::Xcb(w) => w.window.get(),
            _ => return None,
        };
        let (connection, screen) = x11rb::connect(None).ok()?;
        let version = connection.shape_query_version().ok()?.reply().ok()?;
        if (version.major_version, version.minor_version) < (1, 1) { return None; }
        let root = connection.setup().roots.get(screen)?.root;
        Some(Self { connection, root, window: id, last: None, watch: std::cell::RefCell::default(), wake, visible: std::cell::Cell::new(false), follow: std::cell::Cell::new(follow), input: std::cell::Cell::new(input) })
    }
    pub fn global_pointer(&self) -> crate::OverlaySupport {
        if self.input.get() != crate::OverlayInput::Regions && !self.follow.get() { return crate::OverlaySupport::Supported; }
        if self.watch.borrow().as_ref().is_some_and(|w| w.supported()) { crate::OverlaySupport::Supported } else { crate::OverlaySupport::Failed }
    }
    pub fn set_follow_pointer(&self, follow: bool) { self.follow.set(follow); self.refresh_watch(); }
    fn refresh_watch(&self) {
        if self.visible.get() && (self.input.get() == crate::OverlayInput::Regions || self.follow.get()) {
            if self.watch.borrow().is_none() { *self.watch.borrow_mut() = crate::platform::x11::pointer::MouseWatch::new(self.wake.clone()); }
            self.update_geometry();
        } else { self.watch.borrow_mut().take(); }
    }
    pub fn update_geometry(&self) {
        let watch = self.watch.borrow();
        let Some(watch) = watch.as_ref() else { return };
        if let Some(p) = self.connection.translate_coordinates(self.window, self.root, 0, 0).ok().and_then(|r| r.reply().ok())
            && let Some(g) = self.connection.get_geometry(self.window).ok().and_then(|r| r.reply().ok()) {
            watch.geometry([i32::from(p.dst_x), i32::from(p.dst_y), i32::from(p.dst_x) + i32::from(g.width), i32::from(p.dst_y) + i32::from(g.height)], self.follow.get());
        }
    }
    pub fn set_input(&mut self, input: crate::OverlayInput) {
        self.input.set(input);
        self.refresh_watch();
        self.last = None;
        match input {
            crate::OverlayInput::All => { let _ = self.connection.shape_mask(SO::SET, SK::INPUT, self.window, 0, 0, x11rb::NONE).ok().and_then(|r| r.check().ok()); }
            crate::OverlayInput::None => self.regions(&[], 1.0),
            _ => {}
        }
    }
    pub fn take_pointer(&self, scale: f32) -> Option<((f32, f32), (i32, i32))> {
        let desktop = self.watch.borrow().as_ref()?.take()?;
        let p = self.connection.translate_coordinates(self.window, self.root, 0, 0).ok()?.reply().ok()?;
        Some((((desktop.0 - i32::from(p.dst_x)) as f32 / scale, (desktop.1 - i32::from(p.dst_y)) as f32 / scale), desktop))
    }
    pub fn regions(&mut self, rects: &[[f32; 4]], scale: f32) {
        let rects: Vec<_> = rects.iter().filter_map(|r| {
            let x = (r[0] * scale).floor().max(0.0).min(i16::MAX as f32) as i16;
            let y = (r[1] * scale).floor().max(0.0).min(i16::MAX as f32) as i16;
            let width = ((r[2] * scale).ceil() - f32::from(x)).clamp(0.0, u16::MAX as f32) as u16;
            let height = ((r[3] * scale).ceil() - f32::from(y)).clamp(0.0, u16::MAX as f32) as u16;
            (width > 0 && height > 0).then_some(Rectangle { x, y, width, height })
        }).collect();
        if self.last.as_ref().is_some_and(|last| last.len() == rects.len() && last.iter().zip(&rects).all(|(a, b)|
            (a.x, a.y, a.width, a.height) == (b.x, b.y, b.width, b.height))) { return; }
        // X11 retains its implicit button grab through release even if input shape changes.
        if self.connection.shape_rectangles(SO::SET, SK::INPUT, ClipOrdering::UNSORTED, self.window, 0, 0, &rects)
            .ok().and_then(|request| request.check().ok()).is_some() {
            // This connection is separate from wgpu's. Wait for the shape to be applied
            // before presenting pixels, rather than racing two independent X11 streams.
            self.last = Some(rects);
        }
    }
}

impl Drop for InputWindow { fn drop(&mut self) { self.hide(); } }

pub(crate) fn pointer_position() -> Option<(i32, i32)> {
    let (connection, screen) = x11rb::connect(None).ok()?;
    let root = connection.setup().roots.get(screen)?.root;
    let p = connection.query_pointer(root).ok()?.reply().ok()?;
    p.same_screen.then_some((i32::from(p.root_x), i32::from(p.root_y)))
}

pub(crate) fn output_crtc(output: u32) -> Option<u32> {
    use x11rb::protocol::randr::ConnectionExt as _;
    let (connection, _) = x11rb::connect(None).ok()?;
    let crtc = connection.randr_get_output_info(output, x11rb::CURRENT_TIME).ok()?.reply().ok()?.crtc;
    (crtc != 0).then_some(crtc)
}
