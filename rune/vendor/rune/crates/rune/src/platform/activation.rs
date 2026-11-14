//! Focusing an already-mapped Wayland window with a forwarded XDG activation token. winit 0.30
//! uses a token only while creating a window (`focus_window` is a no-op on Wayland), so this
//! binds `xdg_activation_v1` on winit's own display connection, in a private queue, and
//! activates winit's surface.

use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use wayland_backend::client::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;
use winit::window::Window;

struct State;

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<XdgActivationV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &XdgActivationV1,
        _: <XdgActivationV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// One window's activation binding, made on first use and kept with the window.
pub(crate) struct Activator {
    queue: EventQueue<State>,
    activation: XdgActivationV1,
    surface: WlSurface,
}

impl Activator {
    /// None off Wayland or when the compositor lacks `xdg_activation_v1`.
    pub fn new(window: &Window) -> Option<Activator> {
        let (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(handle)) = (
            window.display_handle().ok()?.as_raw(),
            window.window_handle().ok()?.as_raw(),
        ) else {
            return None;
        };
        // SAFETY: winit owns this `wl_display` and keeps it alive for the event loop, which
        // outlives every window; a foreign backend never closes it.
        let backend = unsafe { Backend::from_foreign_display(display.display.as_ptr().cast()) };
        let conn = Connection::from_backend(backend);
        let (globals, queue) = registry_queue_init::<State>(&conn).ok()?;
        let activation = globals.bind::<XdgActivationV1, _, _>(&queue.handle(), 1..=1, ()).ok()?;
        // SAFETY: the pointer is the live `wl_surface` proxy winit created for this window, and
        // the activator is dropped before the window.
        let id = unsafe { ObjectId::from_ptr(WlSurface::interface(), handle.surface.as_ptr().cast()) }.ok()?;
        let surface = WlSurface::from_id(&conn, id).ok()?;
        Some(Activator {
            queue,
            activation,
            surface,
        })
    }

    /// Asks the compositor to focus the window. It decides: an expired or foreign token is
    /// ignored, or only marks the window urgent.
    pub fn activate(&mut self, token: &str) {
        self.activation.activate(token.to_owned(), &self.surface);
        let _ = self.queue.flush();
    }
}

impl Drop for Activator {
    fn drop(&mut self) {
        self.activation.destroy();
        let _ = self.queue.flush();
    }
}
