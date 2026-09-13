//! Opt-in Hyprland regression, run with NEOISM_LIVE_KEYBOARD_TEST=1 and
//! `--ignored --nocapture`. It exercises the production sender with Fcitx active.
//!
//! Run only after review, with NEOISM_LIVE_KEYBOARD_TEST=1 and --ignored --nocapture.
//! Manually focus the dedicated window if necessary; this test never focuses a
//! window, launches a browser, or dispatches a Hyprland command. Do not type or
//! switch focus during injection. Like any compositor input API, focus checking
//! and injection are not atomic; a compositor focus change can race a check.

use anyhow::{bail, ensure, Context, Result};
use enigo::Key;
use std::{
    collections::HashSet,
    fs::File,
    io::Write,
    os::fd::{AsFd, AsRawFd, FromRawFd},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};
use wayland_client::{
    protocol::{
        wl_buffer, wl_callback, wl_compositor, wl_keyboard, wl_registry, wl_seat, wl_shm,
        wl_shm_pool, wl_surface,
    },
    Connection, Dispatch, EventQueue, QueueHandle, WEnum,
};
use wayland_protocols::wp::text_input::zv3::client::{
    zwp_text_input_manager_v3, zwp_text_input_v3,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};
use xkbcommon::xkb;

const APP_ID: &str = "neoism-keyboard-regression";
const URL: &str = "https://example.invalid/neoism?q=keyboard&n=42#test";
const UNICODE: &str = "Grüße — café Ελληνικά 日本語 e\u{301} אבג";
const TIMEOUT: Duration = Duration::from_secs(20);
type Reply = mpsc::Sender<std::result::Result<(), String>>;

#[derive(Default)]
struct Receiver {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm: Option<xdg_wm_base::XdgWmBase>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    seat: Option<wl_seat::WlSeat>,
    text_manager: Option<zwp_text_input_manager_v3::ZwpTextInputManagerV3>,
    text_input: Option<zwp_text_input_v3::ZwpTextInputV3>,
    text_entered: bool,
    text_enabled: bool,
    enable_ack: bool,
    ime_done: bool,
    pending_commit: String,
    seats: usize,
    surface: Option<wl_surface::WlSurface>,
    buffer: Option<wl_buffer::WlBuffer>,
    configured: bool,
    focused: bool,
    entered: bool,
    closed: bool,
    error: Option<String>,
    xkb: Option<xkb::State>,
    masks: (u32, u32, u32, u32),
    held: HashSet<u32>,
    text: String,
    // Ctrl+l and Return are semantic receiver events, not inferred sender calls.
    events: Vec<(String, u32)>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Receiver {
    fn event(
        s: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        q: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_compositor" => {
                    s.compositor = Some(registry.bind(name, version.min(4), q, ()))
                }
                "wl_shm" => s.shm = Some(registry.bind(name, 1, q, ())),
                "xdg_wm_base" => s.wm = Some(registry.bind(name, 1, q, ())),
                "zwp_text_input_manager_v3" => {
                    s.text_manager = Some(registry.bind(name, 1, q, ()));
                }
                "wl_seat" => {
                    s.seats += 1;
                    s.seat = Some(registry.bind(name, version.min(5), q, ()));
                }
                _ => {}
            }
            if s.text_input.is_none() {
                if let (Some(manager), Some(seat)) = (&s.text_manager, &s.seat) {
                    s.text_input = Some(manager.get_text_input(seat, q, ()));
                }
            }
        }
    }
}
impl Receiver {
    fn enable_text_input(&mut self, connection: &Connection, q: &QueueHandle<Self>) {
        // Both enter events must name our surface. In particular this callback
        // is requested AFTER wl_keyboard.enter, even if text_input.enter came first.
        if self.focused && self.text_entered && !self.text_enabled {
            let input = self.text_input.as_ref().unwrap();
            input.enable();
            input.commit(); // serial 1; intentionally no surrounding-text updates.
            self.text_enabled = true;
            connection.display().sync(q, ());
        }
    }
}
impl Dispatch<wl_callback::WlCallback, ()> for Receiver {
    fn event(
        s: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        s.enable_ack = s.focused && s.text_entered && s.text_enabled;
    }
}
impl Dispatch<zwp_text_input_v3::ZwpTextInputV3, ()> for Receiver {
    fn event(
        s: &mut Self,
        _: &zwp_text_input_v3::ZwpTextInputV3,
        event: zwp_text_input_v3::Event,
        _: &(),
        connection: &Connection,
        q: &QueueHandle<Self>,
    ) {
        match event {
            zwp_text_input_v3::Event::Enter { surface } => {
                s.text_entered = s.surface.as_ref() == Some(&surface);
                s.enable_text_input(connection, q);
            }
            zwp_text_input_v3::Event::Leave { .. } => {
                s.text_entered = false;
                s.error = Some("Text-input focus lost; injection cancelled".into());
            }
            zwp_text_input_v3::Event::CommitString { text } => {
                s.pending_commit
                    .push_str(text.as_deref().unwrap_or_default());
            }
            zwp_text_input_v3::Event::Done { serial } => {
                if !s.focused || !s.text_entered || !s.text_enabled || serial != 1 {
                    s.error =
                        Some(format!("Unexpected text-input Done (serial {serial})"));
                    return;
                }
                // Done delivers an IME edit batch, not an enable acknowledgement.
                // Fcitx need not send one until it commits actual text.
                s.ime_done = true;
                s.text.push_str(&std::mem::take(&mut s.pending_commit));
            }
            zwp_text_input_v3::Event::DeleteSurroundingText {
                before_length,
                after_length,
            } if before_length != 0 || after_length != 0 => {
                s.error = Some("Unexpected IME surrounding-text deletion".into());
            }
            _ => {}
        }
    }
}
wayland_client::delegate_noop!(Receiver: ignore zwp_text_input_manager_v3::ZwpTextInputManagerV3);

impl Dispatch<wl_seat::WlSeat, ()> for Receiver {
    fn event(
        s: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        q: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            if caps.contains(wl_seat::Capability::Keyboard) && s.keyboard.is_none() {
                s.keyboard = Some(seat.get_keyboard(q, ()));
            }
        }
    }
}
impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Receiver {
    fn event(
        _: &mut Self,
        wm: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm.pong(serial);
        }
    }
}
impl Dispatch<xdg_surface::XdgSurface, ()> for Receiver {
    fn event(
        s: &mut Self,
        xdg: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg.ack_configure(serial);
            if !s.configured {
                let surface = s.surface.as_ref().unwrap();
                surface.attach(s.buffer.as_ref(), 0, 0);
                surface.damage(0, 0, 480, 160);
                surface.commit();
                s.configured = true;
            }
        }
    }
}
impl Dispatch<xdg_toplevel::XdgToplevel, ()> for Receiver {
    fn event(
        s: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Close = event {
            s.closed = true;
            s.focused = false;
        }
    }
}
impl Dispatch<wl_callback::WlCallback, Reply> for Receiver {
    fn event(
        s: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        reply: &Reply,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Runs after preceding receiver events, not from a stale sender-side flag.
        let result = if s.focused
            && s.text_entered
            && s.enable_ack
            && !s.closed
            && s.error.is_none()
            && s.seats == 1
        {
            Ok(())
        } else {
            Err("Dedicated test surface lost focus or receiver failed".into())
        };
        let _ = reply.send(result);
    }
}
impl Dispatch<wl_keyboard::WlKeyboard, ()> for Receiver {
    fn event(
        s: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        connection: &Connection,
        q: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Keymap { format, fd, size } => {
                let result = (|| -> Result<xkb::State> {
                    ensure!(
                        format == WEnum::Value(wl_keyboard::KeymapFormat::XkbV1),
                        "Non-XKB keymap"
                    );
                    ensure!(size > 0 && size <= 16 * 1024 * 1024, "Invalid keymap size");
                    // FDs can share an open-file offset with Fcitx/compositor;
                    // the production loader uses pread and never consumes it.
                    let map = super::super::shortcuts::read_map(&File::from(fd), size)?;
                    let mut state = xkb::State::new(&map);
                    let (d, l, k, g) = s.masks;
                    state.update_mask(d, l, k, 0, 0, g);
                    Ok(state)
                })();
                match result {
                    Ok(state) => s.xkb = Some(state),
                    Err(e) => s.error = Some(e.to_string()),
                }
            }
            wl_keyboard::Event::Enter { surface, keys, .. } => {
                s.focused = s.surface.as_ref() == Some(&surface);
                s.entered |= s.focused;
                s.enable_text_input(connection, q);
                if s.focused && !keys.is_empty() {
                    s.held.extend(keys.chunks_exact(4).map(|key|u32::from_ne_bytes(key.try_into().unwrap())));
                    eprintln!("Waiting for initially held keys to release: {:?}",s.held);
                }
            }
            wl_keyboard::Event::Leave { surface, .. }
                if s.surface.as_ref() == Some(&surface) =>
            {
                s.focused = false;
                s.error =
                    Some("Test surface lost keyboard focus; injection cancelled".into());
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                s.masks = (mods_depressed, mods_latched, mods_locked, group);
                if let Some(state) = &mut s.xkb {
                    state.update_mask(
                        mods_depressed,
                        mods_latched,
                        mods_locked,
                        0,
                        0,
                        group,
                    );
                }
            }
            wl_keyboard::Event::Key {
                key,
                state: WEnum::Value(direction),
                ..
            } => {
                if !s.focused {
                    s.error = Some("Key received without own surface focus".into());
                    return;
                }
                // Wayland modifier events are authoritative. Do NOT also update_key:
                // that would double-count modifier transitions on a Wayland client.
                let Some(state) = &s.xkb else {
                    s.error = Some("Key before keymap".into());
                    return;
                };
                if direction == wl_keyboard::KeyState::Released {
                    if !s.held.remove(&key) {
                        s.error = Some(format!("Unpaired release: {key}"));
                    }
                    return;
                }
                if !s.held.insert(key) {
                    s.error = Some(format!("Duplicate press: {key}"));
                }
                let code = xkb::Keycode::new(key + 8);
                let sym = state.key_get_one_sym(code).raw();
                let mods = state.serialize_mods(xkb::STATE_MODS_EFFECTIVE);
                let ctrl = state
                    .mod_name_is_active(xkb::MOD_NAME_CTRL, xkb::STATE_MODS_EFFECTIVE);
                match sym {
                    0xffe3 | 0xffe4 => {} // Control itself has no text.
                    0x6c if ctrl => {
                        if !s.text.is_empty() {
                            s.error = Some(
                                "Ctrl+l arrived after text instead of before it".into(),
                            );
                        }
                        let index = state.get_keymap().mod_get_index(xkb::MOD_NAME_CTRL);
                        if index >= 32 || mods != 1u32.checked_shl(index).unwrap_or(0) {
                            s.error = Some("Ctrl+l had extra modifiers".into());
                        }
                        s.events.push(("Ctrl+l".into(), mods));
                    }
                    0xff0d => {
                        s.events.push((format!("Return:{}", s.text), mods));
                        s.text.clear();
                    }
                    _ => {
                        let text = state.key_get_utf8(code);
                        if mods != 0 || text.is_empty() {
                            s.error = Some(format!(
                                "Unexpected symbol/modifiers: {sym:#x}/{mods:#x}"
                            ));
                        }
                        s.text.push_str(&text);
                    }
                }
            }
            _ => {}
        }
    }
}
wayland_client::delegate_noop!(Receiver: ignore wl_compositor::WlCompositor);
wayland_client::delegate_noop!(Receiver: ignore wl_shm::WlShm);
wayland_client::delegate_noop!(Receiver: ignore wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(Receiver: ignore wl_buffer::WlBuffer);
wayland_client::delegate_noop!(Receiver: ignore wl_surface::WlSurface);

// Never use blocking_dispatch/roundtrip on the receiver: even initial discovery
// and focus acquisition must time out when the compositor stops answering.
fn pump(
    connection: &Connection,
    queue: &mut EventQueue<Receiver>,
    s: &mut Receiver,
) -> Result<()> {
    queue.dispatch_pending(s)?;
    if let Err(e) = connection.flush() {
        if !matches!(&e, wayland_client::backend::WaylandError::Io(io) if io.kind() == std::io::ErrorKind::WouldBlock)
        {
            return Err(e.into());
        }
    }
    if let Some(read) = queue.prepare_read() {
        let mut fd = libc::pollfd {
            fd: read.connection_fd().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one initialized pollfd, valid for the duration of poll.
        let n = unsafe { libc::poll(&mut fd, 1, 20) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() != std::io::ErrorKind::Interrupted {
                return Err(e.into());
            }
        } else if n > 0 {
            ensure!(
                fd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) == 0,
                "Wayland disconnected"
            );
            read.read()?;
        }
    }
    queue.dispatch_pending(s)?;
    ensure!(!s.closed, "Test window closed");
    if let Some(e) = &s.error {
        bail!("{e}");
    }
    Ok(())
}

struct Window {
    connection: Connection,
    top: xdg_toplevel::XdgToplevel,
    xdg: xdg_surface::XdgSurface,
    surface: wl_surface::WlSurface,
    buffer: wl_buffer::WlBuffer,
    text_input: zwp_text_input_v3::ZwpTextInputV3,
    _file: File,
    cancelled: Arc<AtomicBool>,
}
impl Drop for Window {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.text_input.disable();
        self.text_input.commit();
        self.text_input.destroy();
        self.top.destroy();
        self.xdg.destroy();
        self.surface.destroy();
        self.buffer.destroy();
        let _ = self.connection.flush();
    }
}

#[test]
#[ignore = "live keyboard injection: review first, then explicitly opt in and focus the dedicated window"]
fn hyprland_production_keyboard_roundtrip() -> Result<()> {
    ensure!(
        std::env::var("NEOISM_LIVE_KEYBOARD_TEST").as_deref() == Ok("1"),
        "Set NEOISM_LIVE_KEYBOARD_TEST=1 explicitly"
    );
    ensure!(
        std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some(),
        "Requires Hyprland"
    );
    let connection = Connection::connect_to_env()?;
    let mut queue = connection.new_event_queue::<Receiver>();
    let q = queue.handle();
    connection.display().get_registry(&q, ());
    let mut s = Receiver::default();
    let discovery = Instant::now() + TIMEOUT;
    while s.compositor.is_none()
        || s.shm.is_none()
        || s.wm.is_none()
        || s.keyboard.is_none()
        || s.text_input.is_none()
    {
        ensure!(
            Instant::now() < discovery,
            "Wayland globals/keyboard timed out"
        );
        pump(&connection, &mut queue, &mut s)?;
    }
    // An anonymous, initialized XRGB buffer: no filesystem litter or unrelated UI.
    let name = std::ffi::CString::new(APP_ID)?;
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    ensure!(fd >= 0, "memfd_create: {}", std::io::Error::last_os_error());
    // SAFETY: memfd_create returned a new owned descriptor.
    let mut file = unsafe { File::from_raw_fd(fd) };
    file.write_all(&[0x38, 0x28, 0x18, 0xff].repeat(480 * 160))?;
    let pool = s
        .shm
        .as_ref()
        .unwrap()
        .create_pool(file.as_fd(), 480 * 160 * 4, &q, ());
    let buffer =
        pool.create_buffer(0, 480, 160, 480 * 4, wl_shm::Format::Xrgb8888, &q, ());
    pool.destroy();
    let surface = s.compositor.as_ref().unwrap().create_surface(&q, ());
    let xdg = s.wm.as_ref().unwrap().get_xdg_surface(&surface, &q, ());
    let top = xdg.get_toplevel(&q, ());
    top.set_app_id(APP_ID.into());
    top.set_title(format!(
        "Neoism keyboard regression ({}) — focus here; do not type",
        std::process::id()
    ));
    let cancelled = Arc::new(AtomicBool::new(false));
    let _window = Window {
        connection: connection.clone(),
        top,
        xdg,
        surface: surface.clone(),
        buffer: buffer.clone(),
        text_input: s.text_input.as_ref().unwrap().clone(),
        _file: file,
        cancelled: cancelled.clone(),
    };
    s.surface = Some(surface.clone());
    s.buffer = Some(buffer);
    surface.commit();
    eprintln!(
        "Focus the dedicated {APP_ID} window within 20 seconds. No automatic refocusing."
    );
    let focus_deadline = Instant::now() + TIMEOUT;
    while !s.configured
        || !s.entered
        || !s.focused
        || s.xkb.is_none()
        || !s.enable_ack
        || !s.held.is_empty()
    {
        ensure!(
            Instant::now() < focus_deadline,
            "Focus/IME activation timed out: keyboard={}, text_input={}, enable_ack={}, ime_done={}; requires a running text-input-v3 IME (Fcitx)",
            s.focused, s.text_entered, s.enable_ack, s.ime_done
        );
        pump(&connection, &mut queue, &mut s)?;
    }
    let num_lock = s
        .xkb
        .as_ref()
        .unwrap()
        .get_keymap()
        .mod_get_index(xkb::MOD_NAME_NUM);
    let allowed_locked = 1u32.checked_shl(num_lock).unwrap_or(0);
    ensure!(
        s.seats == 1 && s.masks.0 == 0 && s.masks.1 == 0 && s.masks.2 & !allowed_locked == 0,
        "Requires one seat and no held/latched modifiers; only initial NumLock is allowed"
    );
    ensure!(
        s.text.is_empty() && s.pending_commit.is_empty(),
        "Unexpected text during IME activation"
    );

    let (guards_tx, guards_rx) = mpsc::channel::<Reply>();
    let (done_tx, done_rx) = mpsc::channel();
    let (observed_tx, observed_rx) = mpsc::channel::<(usize, &'static str, Reply)>();
    let mut pending_observation = None;
    let deadline = Instant::now() + Duration::from_secs(60);
    // Cancel guarded sends on receiver failure. Let production finish paired
    // releases and bounded cleanup rather than terminating a transaction.
    let sender = std::thread::spawn(move || {
        let result = (|| -> Result<()> {
            let check = || -> Result<()> {
                ensure!(
                    !cancelled.load(Ordering::SeqCst) && Instant::now() < deadline,
                    "Live test cancelled/timed out"
                );
                let (tx, rx) = mpsc::channel();
                guards_tx.send(tx)?;
                rx.recv_timeout(Duration::from_secs(2))?
                    .map_err(anyhow::Error::msg)?;
                ensure!(
                    !cancelled.load(Ordering::SeqCst) && Instant::now() < deadline,
                    "Live test cancelled/timed out"
                );
                Ok(())
            };
            let observed = |events, text| -> Result<()> {
                let (tx, rx) = mpsc::channel();
                observed_tx.send((events, text, tx))?;
                rx.recv_timeout(Duration::from_secs(5))
                    .context("IME/keyboard receiver did not reach expected text/chord checkpoint")?
                    .map_err(anyhow::Error::msg)
            };
            for (i, text) in [URL, UNICODE].into_iter().enumerate() {
                check()?;
                super::send_keys(&[Key::Control, Key::Unicode('l')], &check)?;
                observed(2 * i + 1, "")?;
                check()?;
                super::send(text, &check)?;
                // IME is another client: sender/receiver display.sync alone is
                // not a barrier for its commit_string/Done or forwarded keys.
                observed(2 * i + 1, text)?;
                check()?;
                super::send_keys(&[Key::Return], &check)?;
                observed(2 * i + 2, "")?;
            }
            check()?; // Receiver barrier after the last production cleanup.
            Ok(())
        })();
        let _ = done_tx.send(result.map_err(|e| format!("{e:#}")));
    });
    loop {
        ensure!(Instant::now() < deadline, "Live sender timed out");
        for reply in guards_rx.try_iter() {
            connection.display().sync(&q, reply);
        }
        pump(&connection, &mut queue, &mut s)?;
        if pending_observation.is_none() {
            pending_observation = observed_rx.try_recv().ok();
        }
        if let Some((events, text, reply)) = &pending_observation {
            ensure!(
                s.events.len() <= *events,
                "Extra receiver events: {:?}",
                s.events
            );
            if s.events.len() == *events {
                ensure!(
                    text.starts_with(&s.text),
                    "Receiver text mismatch: expected {text:?}, got {:?}; events={:?}",
                    s.text,
                    s.events
                );
            }
            if s.events.len() == *events
                && s.text == *text
                && s.pending_commit.is_empty()
                && s.held.is_empty()
                && s.masks.0 == 0
                && s.masks.1 == 0
                && s.masks.2 == 0
                && super::validate_baseline(&s.xkb.as_ref().unwrap().get_keymap()).is_ok()
            {
                let _ = reply.send(Ok(()));
                pending_observation = None;
            }
        }
        match done_rx.try_recv() {
            Ok(result) => {
                result.map_err(anyhow::Error::msg).with_context(|| format!(
                    "Receiver: text={:?}, events={:?}, held={:?}, masks={:?}, baseline={:?}",
                    s.text, s.events, s.held, s.masks,
                    super::validate_baseline(&s.xkb.as_ref().unwrap().get_keymap())))?;
                break;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                bail!("Sender exited without result")
            }
        }
    }
    sender
        .join()
        .map_err(|_| anyhow::anyhow!("Sender panicked"))?;
    ensure!(
        s.events.len() == 4,
        "Unexpected receiver events: {:?}",
        s.events
    );
    for (pair, text) in s.events.chunks_exact(2).zip([URL, UNICODE]) {
        ensure!(
            pair[0].0 == "Ctrl+l" && pair[0].1 != 0,
            "Missing Ctrl+l: {pair:?}"
        );
        ensure!(
            pair[1] == (format!("Return:{text}"), 0),
            "Incorrect text/Return modifiers: {pair:?}"
        );
    }
    ensure!(
        s.text.is_empty() && s.pending_commit.is_empty() && s.held.is_empty(),
        "Unsubmitted text or stuck keys"
    );
    ensure!(
        s.masks.0 == 0
            && s.masks.1 == 0
            && s.masks.2 == 0
            && s.xkb
                .as_ref()
                .unwrap()
                .serialize_mods(xkb::STATE_MODS_EFFECTIVE)
                == 0,
        "Modifiers not restored"
    );
    Ok(())
}
