//! A Wayland layer-shell surface for overlays: `zwlr_layer_shell_v1` on winit's own display
//! connection, in a private event queue with its own seat. The surface reserves no space;
//! input regions and the rendered buffer share a commit. Global pointer observation and
//! capture exclusion are unavailable. wgpu draws through the raw `wl_surface`.

use std::os::fd::{AsRawFd, RawFd};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use winit::raw_window_handle::{RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle};
use wayland_backend::client::Backend;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_surface::{self, WlSurface};
use wayland_client::protocol::{wl_output::{self, WlOutput}, wl_seat::{self, WlSeat}, wl_pointer::{self, WlPointer}, wl_keyboard::{self, WlKeyboard}, wl_touch::{self, WlTouch}};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{Shape, WpCursorShapeDeviceV1};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1};

use crate::el::Align;
use crate::overlay::{Placement, ScreenEdge};
use crate::runtime::Waker;
use crate::{OverlayInput, OverlayOptions, OverlayPlacement};

pub(crate) enum Input {
    Move(f32, f32), Leave, Button(bool), Secondary(bool), Key(crate::KeyEvent), KeyUp(crate::KeyEvent),
    Physical(crate::PhysicalInput, Option<crate::KeyEvent>),
}

#[derive(Default)]
struct State {
    /// The size the compositor asked for in its latest configure (0 means ours).
    configured: Option<(u32, u32)>,
    scale: Option<i32>,
    closed: bool,
    input: Vec<Input>,
    surface: Option<WlSurface>,
    pointer_inside: bool,
    /// The serial of the latest button event over the surface, until `dispatch` records it.
    keyboard_focus: bool,
    want_pointer: bool,
    want_keys: bool,
    capabilities: Option<wl_seat::Capability>,
    pointer: Option<WlPointer>,
    /// A finger on the surface acts as the primary button, so a tap is a click.
    touch: Option<WlTouch>,
    touch_id: Option<i32>,
    /// `wp_cursor_shape_manager_v1` when the compositor has it, and its device for `pointer`.
    shapes: Option<WpCursorShapeManagerV1>,
    shape: Option<WpCursorShapeDeviceV1>,
    /// The serial of the pointer's enter while it is over the surface, and the cursor set since.
    entered: Option<u32>,
    shown: Option<crate::el::Cursor>,
    keyboard: Option<WlKeyboard>,
    xkb: Option<xkbcommon::xkb::State>,
}

impl Dispatch<WlOutput, ()> for State {
    fn event(_: &mut Self, _: &WlOutput, _: wl_output::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlSeat, ()> for State {
    fn event(state: &mut Self, seat: &WlSeat, event: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: wayland_client::WEnum::Value(caps) } = event {
            state.capabilities = Some(caps);
            state.update_seat(seat, qh);
        }
    }
}
impl State {
    fn update_seat(&mut self, seat: &WlSeat, qh: &QueueHandle<Self>) {
        let caps = self.capabilities.unwrap_or_else(wl_seat::Capability::empty);
        let pointer = self.want_pointer && caps.contains(wl_seat::Capability::Pointer);
        let keyboard = self.want_keys && caps.contains(wl_seat::Capability::Keyboard);
        if pointer && self.pointer.is_none() {
            let p = seat.get_pointer(qh, ());
            self.shape = self.shapes.as_ref().map(|shapes| shapes.get_pointer(&p, qh, ()));
            self.pointer = Some(p);
        }
        let touch = self.want_pointer && caps.contains(wl_seat::Capability::Touch);
        if touch && self.touch.is_none() { self.touch = Some(seat.get_touch(qh, ())); }
        if keyboard && self.keyboard.is_none() { self.keyboard = Some(seat.get_keyboard(qh, ())); }
        if !pointer && let Some(p) = self.pointer.take() {
            if let Some(shape) = self.shape.take() { shape.destroy(); }
            if p.version() >= 3 { p.release(); }
            self.pointer_inside = false;
            self.entered = None;
        }
        if !touch && let Some(t) = self.touch.take() { if t.version() >= 3 { t.release(); } self.touch_id = None; }
        if !keyboard && let Some(k) = self.keyboard.take() { if k.version() >= 3 { k.release(); } self.xkb = None; self.keyboard_focus = false; }
    }
}
impl Dispatch<WlPointer, ()> for State {
    fn event(state: &mut Self, _: &WlPointer, event: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_pointer::Event::Enter { serial, surface, surface_x, surface_y } => {
                state.pointer_inside = state.surface.as_ref() == Some(&surface);
                // Until the surface sets a cursor, the pointer keeps the one it arrived with.
                state.entered = state.pointer_inside.then_some(serial);
                state.shown = None;
                if state.pointer_inside { state.input.push(Input::Move(surface_x as f32, surface_y as f32)); }
            }
            wl_pointer::Event::Motion { surface_x, surface_y, .. } if state.pointer_inside => state.input.push(Input::Move(surface_x as f32, surface_y as f32)),
            wl_pointer::Event::Leave { surface, .. } if state.surface.as_ref() == Some(&surface) => {
                state.pointer_inside = false;
                state.entered = None;
                state.input.push(Input::Leave);
            }
            wl_pointer::Event::Button { button: 0x110, state: wayland_client::WEnum::Value(button), .. } if state.pointer_inside => {
                let pressed = button == wl_pointer::ButtonState::Pressed;
                state.input.push(Input::Button(pressed));
                if pressed && let Some(xkb) = &state.xkb {
                    state.input.push(Input::Physical(crate::PhysicalInput::Modifiers(key_mods(xkb)), None));
                }
            }
            // BTN_RIGHT: the secondary gesture, as a window's right button is.
            wl_pointer::Event::Button { button: 0x111, state: wayland_client::WEnum::Value(button), .. } if state.pointer_inside => {
                state.input.push(Input::Secondary(button == wl_pointer::ButtonState::Pressed));
            }
            _ => {}
        }
    }
}
impl Dispatch<WlTouch, ()> for State {
    fn event(state: &mut Self, _: &WlTouch, event: wl_touch::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_touch::Event::Down { surface, id, x, y, .. } if state.touch_id.is_none() && state.surface.as_ref() == Some(&surface) => {
                state.touch_id = Some(id);
                state.input.push(Input::Move(x as f32, y as f32));
                state.input.push(Input::Button(true));
            }
            wl_touch::Event::Motion { id, x, y, .. } if state.touch_id == Some(id) => state.input.push(Input::Move(x as f32, y as f32)),
            wl_touch::Event::Up { id, .. } if state.touch_id == Some(id) => {
                state.touch_id = None;
                state.input.push(Input::Button(false));
                state.input.push(Input::Leave);
            }
            // The compositor took the touch (a system gesture): leave before releasing so it isn't a click.
            wl_touch::Event::Cancel if state.touch_id.is_some() => {
                state.touch_id = None;
                state.input.push(Input::Leave);
                state.input.push(Input::Button(false));
            }
            _ => {}
        }
    }
}
impl Dispatch<WlKeyboard, ()> for State {
    fn event(state: &mut Self, _: &WlKeyboard, event: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use xkbcommon::xkb;
        match event {
            wl_keyboard::Event::Enter { surface, .. } => state.keyboard_focus = state.surface.as_ref() == Some(&surface),
            wl_keyboard::Event::Leave { surface, .. } if state.surface.as_ref() == Some(&surface) => {
                state.keyboard_focus = false;
                state.input.push(Input::Physical(crate::PhysicalInput::FocusLost, None));
            }
            wl_keyboard::Event::Keymap { format: wayland_client::WEnum::Value(wl_keyboard::KeymapFormat::XkbV1), fd, size } => {
                use std::io::Read;
                let mut map = String::new();
                if size <= 16 * 1024 * 1024 && std::fs::File::from(fd).take(u64::from(size)).read_to_string(&mut map).is_ok() {
                    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
                    state.xkb = xkb::Keymap::new_from_string(&context, map.trim_end_matches('\0').to_owned(), xkb::KEYMAP_FORMAT_TEXT_V1, xkb::COMPILE_NO_FLAGS).map(|map| xkb::State::new(&map));
                }
            }
            wl_keyboard::Event::Modifiers { mods_depressed, mods_latched, mods_locked, group, .. } => {
                if let Some(xkb) = &mut state.xkb {
                    xkb.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
                    state.input.push(Input::Physical(crate::PhysicalInput::Modifiers(key_mods(xkb)), None));
                }
            }
            wl_keyboard::Event::Key { key, state: wayland_client::WEnum::Value(pressed), .. } if state.keyboard_focus => {
                use winit::platform::scancode::PhysicalKeyExtScancode;
                if let Some(xkb) = &state.xkb {
                    let pressed = pressed == wl_keyboard::KeyState::Pressed;
                    let fallback = key_event(xkb, key);
                    if let winit::keyboard::PhysicalKey::Code(key) = crate::PhysicalKeyCode::from_scancode(key) {
                        state.input.push(Input::Physical(crate::PhysicalInput::Key { key, mods: key_mods(xkb), pressed, repeat: false }, fallback));
                    } else if let Some(key) = fallback {
                        state.input.push(if pressed { Input::Key(key) } else { Input::KeyUp(key) });
                    }
                }
            }
            _ => {}
        }
    }
}

fn key_mods(state: &xkbcommon::xkb::State) -> crate::Mods {
    use xkbcommon::xkb;
    let active = |name: &str| state.mod_name_is_active(name, xkb::STATE_MODS_EFFECTIVE);
    crate::Mods { shift: active(xkb::MOD_NAME_SHIFT), ctrl: active(xkb::MOD_NAME_CTRL), cmd: active(xkb::MOD_NAME_CTRL), alt: active(xkb::MOD_NAME_ALT), logo: active(xkb::MOD_NAME_LOGO) }
}

fn key_event(state: &xkbcommon::xkb::State, key: u32) -> Option<crate::KeyEvent> {
    use xkbcommon::xkb;
    let code = xkb::Keycode::new(key + 8);
    let symbol = state.key_get_one_sym(code).raw();
    let mods = key_mods(state);
    let key = match named_keysym(symbol) {
        Some(key) => key,
        None => {
            let text = state.key_get_utf8(code);
            if mods.cmd { crate::Key::Command(char::from_u32(xkb::keysym_to_utf32(state.key_get_one_sym(code)))?.to_ascii_lowercase()) }
            else if !text.is_empty() && !text.chars().any(char::is_control) { crate::Key::Text(text) } else { return None; }
        }
    };
    Some(crate::KeyEvent { key, mods })
}

/// The named key an XKB keysym is, or None for one that types (or nothing Rune names).
fn named_keysym(symbol: u32) -> Option<crate::Key> {
    Some(match symbol {
        0xff1b => crate::Key::Escape, 0xff0d => crate::Key::Enter, 0xff08 => crate::Key::Backspace,
        0xffff => crate::Key::Delete, 0xff09 | 0xfe20 => crate::Key::Tab,
        0xff50 => crate::Key::Home, 0xff57 => crate::Key::End, 0xff55 => crate::Key::PageUp, 0xff56 => crate::Key::PageDown,
        0xff51 => crate::Key::Left, 0xff52 => crate::Key::Up, 0xff53 => crate::Key::Right, 0xff54 => crate::Key::Down,
        0xffbe..=0xffd5 => crate::Key::Function((symbol - 0xffbe + 1) as u8),
        0xff67 => crate::Key::ContextMenu,
        0xff63 => crate::Key::Insert, 0xff61 | 0xff15 => crate::Key::PrintScreen,
        0xff14 => crate::Key::ScrollLock, 0xff13 | 0xff6b => crate::Key::Pause,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::named_keysym;
    use crate::Key;

    #[test]
    fn special_keysyms_are_named_keys() {
        for (symbol, key) in [
            (0xff14, Key::ScrollLock), (0xff13, Key::Pause), (0xff6b, Key::Pause), (0xff67, Key::ContextMenu),
            (0xff61, Key::PrintScreen), (0xff15, Key::PrintScreen), (0xff63, Key::Insert),
            (0xffbe, Key::Function(1)), (0xffd5, Key::Function(24)), (0xff55, Key::PageUp),
        ] {
            assert_eq!(named_keysym(symbol), Some(key), "keysym {symbol:#x}");
        }
        assert_eq!(named_keysym(0x61), None);
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlCompositor, ()> for State {
    fn event(_: &mut Self, _: &WlCompositor, _: <WlCompositor as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlRegion, ()> for State {
    fn event(_: &mut Self, _: &WlRegion, _: <WlRegion as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ZwlrLayerShellV1, ()> for State {
    fn event(_: &mut Self, _: &ZwlrLayerShellV1, _: <ZwlrLayerShellV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WpCursorShapeManagerV1, ()> for State {
    fn event(_: &mut Self, _: &WpCursorShapeManagerV1, _: <WpCursorShapeManagerV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WpCursorShapeDeviceV1, ()> for State {
    fn event(_: &mut Self, _: &WpCursorShapeDeviceV1, _: <WpCursorShapeDeviceV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

/// The cursor-shape-v1 shape for `c`, as `os_cursor` maps winit's. An image is the arrow.
fn shape(c: crate::el::Cursor) -> Shape {
    use crate::el::Cursor;
    match c {
        Cursor::Default | Cursor::Hidden | Cursor::Image(_) => Shape::Default,
        Cursor::Pointer => Shape::Pointer,
        Cursor::Text => Shape::Text,
        Cursor::Grab => Shape::Grab,
        Cursor::Grabbing => Shape::Grabbing,
        Cursor::NotAllowed => Shape::NotAllowed,
        Cursor::Move => Shape::Move,
        Cursor::ResizeColumn => Shape::ColResize,
        Cursor::ResizeRow => Shape::RowResize,
        Cursor::Crosshair => Shape::Crosshair,
    }
}
impl Dispatch<WlSurface, ()> for State {
    fn event(state: &mut Self, _: &WlSurface, event: wl_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_surface::Event::PreferredBufferScale { factor } = event {
            state.scale = Some(factor.max(1));
        }
    }
}
impl Dispatch<ZwlrLayerSurfaceV1, ()> for State {
    fn event(state: &mut Self, layer: &ZwlrLayerSurfaceV1, event: zwlr_layer_surface_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                layer.ack_configure(serial);
                state.configured = Some((width, height));
            }
            zwlr_layer_surface_v1::Event::Closed => state.closed = true,
            _ => {}
        }
    }
}

/// Wakes the event loop when the compositor writes to the display. winit turns its loop only
/// for its own queue's events, so input for this surface's queue (a click on an idle overlay)
/// would otherwise wait for an unrelated wake. An XWayland overlay watches its X connection
/// the same way.
pub(crate) struct Watch {
    stop: Arc<AtomicBool>,
    /// How many times the overlay has dispatched, so the watcher waits for the loop to read
    /// what woke it instead of spinning on a socket that stays readable until then.
    turns: Arc<(Mutex<u64>, Condvar)>,
}

impl Watch {
    pub(crate) fn spawn(fd: RawFd, wake: Waker) -> Watch {
        let stop = Arc::new(AtomicBool::new(false));
        let turns = Arc::new((Mutex::new(0u64), Condvar::new()));
        let (stopped, seen) = (stop.clone(), turns.clone());
        let _ = std::thread::Builder::new().name("rune-layer-watch".into()).spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let mut poll = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
                // SAFETY: one pollfd for a display socket. winit's outlives the event loop; an
                // XWayland surface drops its watch before closing its connection, so at worst a
                // reused descriptor wakes the loop once more before this thread stops.
                if unsafe { libc::poll(&mut poll, 1, 250) } <= 0 { continue; }
                let (count, turned) = &*seen;
                let before = *count.lock().unwrap();
                wake();
                let _ = turned.wait_timeout_while(count.lock().unwrap(), Duration::from_millis(100), |n| *n == before);
            }
        });
        Watch { stop, turns }
    }

    pub(crate) fn turned(&self) {
        let (count, turned) = &*self.turns;
        *count.lock().unwrap() += 1;
        turned.notify_all();
    }
}

impl Drop for Watch {
    /// The thread notices within one poll timeout; joining would stall hiding.
    fn drop(&mut self) { self.stop.store(true, Ordering::Relaxed); }
}

/// What the overlay needs to know after the compositor talked to the surface.
pub(crate) struct Changes {
    pub size: Option<(u32, u32)>,
    pub input: Vec<Input>,
    pub scale: Option<i32>,
    pub closed: bool,
}

pub(crate) struct LayerSurface {
    registry: WlRegistry,
    seat_global: Option<(u32, u32)>,
    regions: std::cell::RefCell<Option<Vec<[i32; 4]>>>,
    compositor: WlCompositor,
    output: Option<WlOutput>,
    seat: Option<WlSeat>,
    options: OverlayOptions,
    queue: EventQueue<State>,
    state: State,
    layer: ZwlrLayerSurfaceV1,
    surface: WlSurface,
    display: NonNull<std::ffi::c_void>,
    watch: Option<Watch>,
}

impl LayerSurface {
    /// None when the display isn't Wayland or the compositor lacks layer-shell.
    pub fn new(display: RawDisplayHandle, namespace: &str, at: Placement, interactive: bool, options: &OverlayOptions, output_id: Option<u32>, wake: Option<Waker>) -> Option<LayerSurface> {
        let RawDisplayHandle::Wayland(display) = display else { return None };
        // SAFETY: winit owns this `wl_display` for the life of the event loop, which outlives
        // every overlay; a foreign backend never closes it.
        let backend = unsafe { Backend::from_foreign_display(display.display.as_ptr().cast()) };
        let conn = Connection::from_backend(backend);
        let (globals, queue) = registry_queue_init::<State>(&conn).ok()?;
        let qh = queue.handle();
        let compositor: WlCompositor = globals.bind(&qh, 1..=6, ()).ok()?;
        let shell: ZwlrLayerShellV1 = globals.bind(&qh, 1..=4, ()).ok()?;
        let output = output_id.and_then(|id| {
            globals.contents().with_list(|list| list.iter().find(|g| g.name == id && g.interface == "wl_output").map(|g| {
                globals.registry().bind::<WlOutput, _, _>(g.name, g.version.min(4), &qh, ())
            }))
        });
        let want_pointer = options.input.resolved(interactive) != OverlayInput::None;
        let want_keys = options.keys == crate::KeyPolicy::OnDemand;
        let registry = globals.registry().clone();
        let seat_global = globals.contents().with_list(|list| list.iter().find(|g| g.interface == "wl_seat").map(|g| (g.name, g.version.min(8))));
        let seat = if want_pointer || want_keys { seat_global.map(|(name, version)| registry.bind::<WlSeat, _, _>(name, version, &qh, ())) } else { None };
        let surface = compositor.create_surface(&qh, ());
        let region = compositor.create_region(&qh, ());
        surface.set_input_region(Some(&region));
        region.destroy();
        // Preserve existing overlay-layer semantics. There is no layer above Overlay;
        // relative ordering of peers is compositor policy, unlike AppKit's numeric levels.
        let layer = shell.get_layer_surface(&surface, output.as_ref(), Layer::Overlay, namespace.to_owned(), &qh, ());
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_exclusive_zone(0);
        let shapes = globals.bind::<WpCursorShapeManagerV1, _, _>(&qh, 1..=1, ()).ok();
        let state = State { surface: Some(surface.clone()), want_pointer, want_keys, shapes, ..State::default() };
        let watch = wake.map(|wake| Watch::spawn(conn.backend().poll_fd().as_raw_fd(), wake));
        let mut this = LayerSurface { registry, seat_global, regions: std::cell::RefCell::new(Some(Vec::new())), compositor, output, seat, options: options.clone(), queue, state, layer, surface, display: display.display, watch };
        this.place(at);
        this.surface.commit();
        while this.state.configured.is_none() && !this.state.closed {
            this.queue.blocking_dispatch(&mut this.state).ok()?;
        }
        (!this.state.closed).then_some(this)
    }

    pub fn place(&mut self, at: Placement) {
        match self.options.placement {
            Some(OverlayPlacement::FullScreen) => {
                self.layer.set_size(0, 0);
                self.layer.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
                self.layer.set_margin(0, 0, 0, 0);
                return;
            }
            Some(OverlayPlacement::Rect { x, y, width, height }) => {
                self.layer.set_size(width.ceil().max(1.0) as u32, height.ceil().max(1.0) as u32);
                self.layer.set_anchor(Anchor::Top | Anchor::Left);
                self.layer.set_margin(y.round() as i32, 0, 0, x.round() as i32);
                return;
            }
            _ => {}
        }
        let at = match self.options.placement {
            Some(OverlayPlacement::Edge { edge, margin, align, inset }) => Placement { size: at.size, edge, margin, align, inset },
            _ => at,
        };
        let (margin, inset) = (at.margin.round() as i32, at.inset.round() as i32);
        let (vertical, top, bottom) = match at.edge {
            ScreenEdge::Top => (Anchor::Top, margin, 0),
            ScreenEdge::Bottom => (Anchor::Bottom, 0, margin),
        };
        let (horizontal, left, right) = match at.align {
            Align::Start => (Anchor::Left, inset, 0),
            Align::Center => (Anchor::empty(), 0, 0),
            Align::End => (Anchor::Right, 0, inset),
        };
        self.layer.set_size(at.size.0.ceil() as u32, at.size.1.ceil() as u32);
        self.layer.set_anchor(vertical | horizontal);
        self.layer.set_margin(top, right, bottom, left);
    }

    pub fn configured_size(&self) -> Option<(u32, u32)> { self.state.configured }

    /// Sets the surface's pending input region; true when it changed, so it waits for a commit.
    pub fn input_regions(&self, regions: &[[f32; 4]], interactive: bool) -> bool {
        let next = match self.options.input.resolved(interactive) {
            OverlayInput::All => None,
            OverlayInput::Regions => Some(regions.iter().filter_map(|r| {
                let (x, y) = (r[0].floor() as i32, r[1].floor() as i32);
                let (w, h) = (r[2].ceil() as i32 - x, r[3].ceil() as i32 - y);
                (w > 0 && h > 0).then_some([x, y, w, h])
            }).collect()),
            _ => Some(Vec::new()),
        };
        if *self.regions.borrow() == next { return false; }
        if let Some(rects) = &next {
            let region = self.compositor.create_region(&self.queue.handle(), ());
            for &[x, y, w, h] in rects { region.add(x, y, w, h); }
            self.surface.set_input_region(Some(&region));
            region.destroy();
        } else {
            self.surface.set_input_region(None);
        }
        *self.regions.borrow_mut() = next;
        // No commit here: a drawn frame's presentation commits input and pixels together, and a
        // frame that draws nothing commits it alone (`Overlay::unchanged_frame`).
        true
    }

    pub fn set_input(&mut self, input: OverlayInput) {
        self.options.input = input;
        self.state.want_pointer = input != OverlayInput::None;
        if self.seat.is_none() && self.state.want_pointer {
            self.seat = self.seat_global.map(|(name, version)| self.registry.bind(name, version, &self.queue.handle(), ()));
        }
        if let Some(seat) = &self.seat { self.state.update_seat(seat, &self.queue.handle()); }
    }

    pub fn request_keys(&self) -> bool {
        if self.layer.version() < 4 { return false; }
        self.layer.set_keyboard_interactivity(KeyboardInteractivity::OnDemand);
        self.surface.commit();
        let _ = self.queue.flush();
        true
    }

    pub fn supports_keys(&self) -> bool { self.layer.version() >= 4 && self.seat.is_some() }

    /// Shows `want` while the pointer is over the surface, once per change. Without
    /// cursor-shape-v1 only `Cursor::Hidden` applies; others keep the cursor the pointer
    /// arrived with, such as a terminal's I-beam.
    pub fn set_cursor(&mut self, want: crate::el::Cursor) {
        let (Some(serial), Some(pointer)) = (self.state.entered, &self.state.pointer) else { return };
        if self.state.shown == Some(want) { return; }
        match (want, &self.state.shape) {
            (crate::el::Cursor::Hidden, _) => pointer.set_cursor(serial, None, 0, 0),
            (want, Some(device)) => device.set_shape(serial, shape(want)),
            (_, None) => return,
        }
        self.state.shown = Some(want);
        let _ = self.queue.flush();
    }

    /// Commit role state (placement) without a new buffer; wgpu's present commits buffers.
    pub fn commit(&mut self) {
        self.surface.commit();
        let _ = self.queue.flush();
    }

    pub fn set_buffer_scale(&mut self, scale: i32) {
        if self.surface.version() >= 3 {
            self.surface.set_buffer_scale(scale);
        }
    }

    /// Handles what the compositor sent since the last call (winit reads the socket).
    pub fn dispatch(&mut self) -> Changes {
        self.turn();
        Changes { size: self.state.configured.take(), input: std::mem::take(&mut self.state.input), scale: self.state.scale.take(), closed: self.state.closed }
    }

    /// Handles what the compositor sent to a hidden surface and drops its input, keeping size
    /// and scale for the next `dispatch`; true once the compositor closed it. libwayland frees
    /// an undispatched event without closing its fd, so a queue left alone leaks every
    /// `wl_keyboard.keymap` (each virtual keyboard a paste tool makes sends a few).
    pub fn drain(&mut self) -> bool {
        self.turn();
        self.state.input.clear();
        self.state.closed
    }

    fn turn(&mut self) {
        let _ = self.queue.dispatch_pending(&mut self.state);
        let _ = self.queue.flush();
        if let Some(watch) = &self.watch { watch.turned(); }
    }

    pub fn raw_handles(&self) -> (RawDisplayHandle, RawWindowHandle) {
        let surface = NonNull::new(self.surface.id().as_ptr().cast()).expect("live wl_surface");
        (
            RawDisplayHandle::Wayland(WaylandDisplayHandle::new(self.display)),
            RawWindowHandle::Wayland(WaylandWindowHandle::new(surface)),
        )
    }
}

impl Drop for LayerSurface {
    fn drop(&mut self) {
        if let Some(shape) = self.state.shape.take() { shape.destroy(); }
        if let Some(shapes) = self.state.shapes.take() { shapes.destroy(); }
        if let Some(pointer) = self.state.pointer.take() && pointer.version() >= 3 { pointer.release(); }
        if let Some(touch) = self.state.touch.take() && touch.version() >= 3 { touch.release(); }
        if let Some(keyboard) = self.state.keyboard.take() && keyboard.version() >= 3 { keyboard.release(); }
        if let Some(seat) = self.seat.take() && seat.version() >= 5 { seat.release(); }
        if let Some(output) = self.output.take() && output.version() >= 3 { output.release(); }
        self.layer.destroy();
        self.surface.destroy();
        let _ = self.queue.flush();
    }
}
