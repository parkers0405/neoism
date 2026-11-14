//! XI2 raw motion on a private connection. Idle overlays have no polling deadline.
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use x11rb::{connection::Connection, protocol::{Event, xinput::{ConnectionExt as _, Device, EventMask, XIEventMask}, randr::ConnectionExt as _, xproto::ConnectionExt as _}, rust_connection::RustConnection};

pub(crate) struct MouseWatch {
    connection: Arc<RustConnection>,
    observer: Arc<crate::input::pointer_observer::Observer>,
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    root: u32,
}
impl MouseWatch {
    pub fn new(wake: Option<crate::Waker>) -> Option<Self> {
        let (connection, screen) = x11rb::connect(None).ok()?;
        let root = connection.setup().roots.get(screen)?.root;
        connection.xinput_xi_query_version(2, 0).ok()?.reply().ok()?;
        connection.xinput_xi_select_events(root, &[EventMask { deviceid: Device::ALL_MASTER.into(), mask: vec![XIEventMask::RAW_MOTION] }]).ok()?.check().ok()?;
        connection.flush().ok()?;
        let connection = Arc::new(connection);
        let observer = Arc::new(crate::input::pointer_observer::Observer::new(wake));
        let running = Arc::new(AtomicBool::new(true));
        let (reader, sample, active) = (connection.clone(), observer.clone(), running.clone());
        let thread = std::thread::spawn(move || {
            while let Ok(event) = reader.wait_for_event() {
                if matches!(event, Event::XinputRawMotion(_))
                    && let Ok(cookie) = reader.query_pointer(root) && let Ok(p) = cookie.reply() && p.same_screen {
                    sample.observe((i32::from(p.root_x), i32::from(p.root_y)));
                }
            }
            active.store(false, Ordering::Release);
        });
        Some(Self { connection, observer, running, thread: Some(thread), root })
    }
    pub fn supported(&self) -> bool { self.running.load(Ordering::Acquire) }
    pub fn take(&self) -> Option<(i32, i32)> { self.observer.take() }
    pub fn geometry(&self, canvas: [i32; 4], follow: bool) {
        if !self.observer.needs_geometry(canvas, follow) { return; }
        let displays = if follow {
            self.connection.randr_get_monitors(self.root, true).ok().and_then(|r| r.reply().ok()).map(|r| {
                r.monitors.iter().map(|m| [i32::from(m.x), i32::from(m.y), i32::from(m.x) + i32::from(m.width), i32::from(m.y) + i32::from(m.height)]).collect()
            }).unwrap_or_default()
        } else { Vec::new() };
        self.observer.configure(canvas, displays, follow);
        if let Ok(cookie) = self.connection.query_pointer(self.root) && let Ok(p) = cookie.reply() && p.same_screen {
            self.observer.observe((i32::from(p.root_x), i32::from(p.root_y)));
        }
    }
}
impl Drop for MouseWatch {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        unsafe extern "C" { fn shutdown(fd: std::ffi::c_int, how: std::ffi::c_int) -> std::ffi::c_int; }
        // This private connection stays owned here until the reader exits. Shutdown wakes
        // wait_for_event without touching winit's display or waiting for another input event.
        unsafe { shutdown(self.connection.stream().as_raw_fd(), 2); }
        if let Some(thread) = self.thread.take() { let _ = thread.join(); }
    }
}
