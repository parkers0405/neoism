//! Where a file drag from another app is over a window. winit 0.30 reports file drags without
//! a position on every platform (macOS has no `draggingUpdated:`, and Windows and X11 drop
//! the point they're given), and no pointer moves arrive while the drag lasts. The host
//! samples the system pointer instead, when the drag enters, each frame while it's over the
//! window, and when it drops.
use winit::window::Window;

#[derive(Default)]
pub(crate) struct DropPointer {
    /// A connection kept for the drag, so sampling 60 times a second doesn't reconnect.
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    x11: Option<(x11rb::rust_connection::RustConnection, u32)>,
}

impl DropPointer {
    pub(crate) fn new() -> Self {
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        return Self {
            x11: x11rb::connect(None).ok().and_then(|(c, screen)| {
                use x11rb::connection::Connection as _;
                let root = c.setup().roots.get(screen)?.root;
                Some((c, root))
            }),
        };
        #[allow(unreachable_code)]
        Self::default()
    }

    /// The pointer over `window`'s content, in points from its top-left, when the platform
    /// can say.
    pub(crate) fn sample(&self, window: &Window) -> Option<(f32, f32)> {
        #[cfg(target_os = "macos")]
        {
            let view = crate::platform::macos::content_view(window)?;
            let ns = view.window()?;
            // SAFETY: hosts run on AppKit's main thread.
            let p = view.convertPoint_fromView(unsafe { ns.mouseLocationOutsideOfEventStream() }, None);
            let y = if view.isFlipped() { p.y } else { view.bounds().size.height - p.y };
            return Some((p.x as f32, y as f32));
        }
        #[cfg(target_os = "windows")]
        {
            let (x, y) = crate::platform::win32::pointer_position()?;
            return screen_to_window(window, (x, y));
        }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        {
            use x11rb::protocol::xproto::ConnectionExt as _;
            let (c, root) = self.x11.as_ref()?;
            let p = c.query_pointer(*root).ok()?.reply().ok()?;
            return p.same_screen.then(|| screen_to_window(window, (i32::from(p.root_x), i32::from(p.root_y)))).flatten();
        }
        #[allow(unreachable_code)]
        { let _ = window; None }
    }
}

/// A screen point in physical pixels, in `window`'s content in points. `None` when the window
/// can't say where it is (Wayland, including an X11 connection under XWayland).
#[cfg(any(target_os = "windows", all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
fn screen_to_window(window: &Window, (x, y): (i32, i32)) -> Option<(f32, f32)> {
    let origin = window.inner_position().ok()?;
    let scale = window.scale_factor() as f32;
    Some(((x - origin.x) as f32 / scale, (y - origin.y) as f32 / scale))
}
