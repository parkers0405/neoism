//! Checked XGrabKey requests on a private connection, with lock-modifier variants. A held
//! chord fires once (`x11_repeat`).
use super::{Backend, Shortcut, ShortcutError, ShortcutId, Sink, parse::*};
use std::{collections::HashMap, io::{Read, Write}, os::{fd::AsRawFd, unix::net::UnixStream}, sync::{Arc, Mutex}, thread::JoinHandle};
use x11rb::{connection::Connection, errors::ReplyError, protocol::{ErrorKind, Event, xproto::*}, rust_connection::RustConnection};
use winit::platform::scancode::PhysicalKeyExtScancode;

#[derive(Clone, Copy)]
struct Grab { root: u32, code: u8, mods: u16 }
type Registrations = Arc<Mutex<HashMap<ShortcutId, Vec<Grab>>>>;
pub(super) struct Native {
    connection: Arc<RustConnection>,
    registrations: Registrations,
    stop: UnixStream,
    listener: Option<JoinHandle<()>>,
}
fn error(e: impl std::fmt::Display) -> ShortcutError { ShortcutError::Backend(e.to_string()) }
fn reply_error(e: ReplyError) -> ShortcutError {
    match e {
        ReplyError::X11Error(e) if e.error_kind == ErrorKind::Access => ShortcutError::Conflict,
        e => error(e),
    }
}

impl Native {
    pub(super) fn new(sink: Sink) -> Result<Self, ShortcutError> {
        let (connection, _) = x11rb::connect(None).map_err(|_| ShortcutError::Unsupported)?;
        let connection = Arc::new(connection);
        let registrations: Registrations = Arc::default();
        let (stop, mut wake) = UnixStream::pair().map_err(error)?;
        let conn = connection.clone();
        let keys = registrations.clone();
        let listener = std::thread::Builder::new().name("rune-shortcuts-x11".into()).spawn(move || {
            let mut fds = [libc::pollfd { fd: conn.stream().as_raw_fd(), events: libc::POLLIN, revents: 0 },
                libc::pollfd { fd: wake.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
            let mut repeats = super::x11_repeat::Repeats::default();
            loop {
                loop {
                    match conn.poll_for_event() {
                        Ok(Some(Event::KeyRelease(event))) => repeats.release(event.detail, event.time),
                        Ok(Some(Event::KeyPress(event))) if !repeats.press(event.detail, event.time) => {}
                        Ok(Some(Event::KeyPress(event))) => {
                            let id = keys.lock().unwrap().iter().find(|(_, grabs)| grabs.iter().any(|g|
                                g.root == event.root && g.code == event.detail && g.mods == u16::from(event.state) & 0xff)).map(|(id, _)| *id);
                            if let Some(id) = id { sink(id, None); }
                        }
                        Ok(Some(_)) => {}, Ok(None) => break, Err(_) => return,
                    }
                }
                // SAFETY: Both descriptors stay open for this thread's lifetime; poll only writes revents.
                if unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) } < 0 {
                    if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted { continue; }
                    return;
                }
                if fds[1].revents != 0 {
                    let mut byte = [0];
                    if wake.read_exact(&mut byte).is_err() || byte[0] == 0 { return; }
                }
                if fds[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 { return; }
            }
        }).map_err(error)?;
        Ok(Self { connection, registrations, stop, listener: Some(listener) })
    }

    fn release(&self, grabs: &[Grab]) -> Result<(), ShortcutError> {
        let mut result = Ok(());
        for g in grabs {
            if let Err(error) = self.connection.ungrab_key(g.code, g.root, ModMask::from(g.mods))
                .map_err(error).and_then(|cookie| cookie.check().map_err(reply_error)) {
                result = Err(error);
            }
        }
        result
    }
}

impl Backend for Native {
    fn register(&mut self, id: ShortcutId, shortcut: &Shortcut) -> Result<(), ShortcutError> {
        let conn = &self.connection;
        let setup = conn.setup();
        let mapping = conn.get_keyboard_mapping(setup.min_keycode, setup.max_keycode - setup.min_keycode + 1)
            .map_err(error)?.reply().map_err(reply_error)?;
        let code = shortcut.physical_key().to_scancode().and_then(|c| u8::try_from(c + 8).ok())
            .filter(|c| (setup.min_keycode..=setup.max_keycode).contains(c))
            .ok_or_else(|| ShortcutError::InvalidAccelerator(shortcut.to_string()))?;
        let mut numcodes = Vec::new();
        for (index, syms) in mapping.keysyms.chunks(mapping.keysyms_per_keycode as usize).enumerate() {
            let code = setup.min_keycode + index as u8;
            if syms.contains(&0xff7f) { numcodes.push(code); } // XK_Num_Lock
        }
        let modifiers = conn.get_modifier_mapping().map_err(error)?.reply().map_err(reply_error)?;
        let mut numlock = 0;
        if modifiers.keycodes_per_modifier() != 0 {
            for (index, keys) in modifiers.keycodes.chunks(modifiers.keycodes_per_modifier() as usize).enumerate() {
                if keys.iter().any(|k| numcodes.contains(k)) { numlock |= 1 << index; }
            }
        }
        let mut base = 0;
        for (bit, x) in [(CTRL, 4), (ALT, 8), (SHIFT, 1), (SUPER, 64)] { if shortcut.mods & bit != 0 { base |= x; } }
        let mut variants = vec![base, base | 2, base | numlock, base | numlock | 2];
        variants.sort_unstable(); variants.dedup();
        let mut grabbed = Vec::new();
        for screen in &setup.roots {
            for &mods in &variants {
                let result = conn.grab_key(false, screen.root, ModMask::from(mods), code, GrabMode::ASYNC, GrabMode::ASYNC)
                    .map_err(error).and_then(|cookie| cookie.check().map_err(reply_error));
                if let Err(e) = result {
                    if let Err(rollback) = self.release(&grabbed) { eprintln!("rune: shortcut rollback: {rollback}"); }
                    return Err(e);
                }
                grabbed.push(Grab { root: screen.root, code, mods });
            }
        }
        self.registrations.lock().unwrap().insert(id, grabbed);
        if let Err(e) = self.stop.write_all(&[1]) {
            let grabbed = self.registrations.lock().unwrap().remove(&id).unwrap();
            if let Err(rollback) = self.release(&grabbed) { eprintln!("rune: shortcut rollback: {rollback}"); }
            return Err(error(e));
        }
        Ok(())
    }
    fn unregister(&mut self, id: ShortcutId) -> Result<(), ShortcutError> {
        let grabs = self.registrations.lock().unwrap().get(&id).cloned().ok_or(ShortcutError::UnknownId)?;
        self.release(&grabs)?;
        self.registrations.lock().unwrap().remove(&id);
        Ok(())
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        let _ = self.stop.write_all(&[0]);
        if let Some(listener) = self.listener.take() { let _ = listener.join(); }
        // Closing this dedicated connection releases every passive grab, including a partial failure.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bad_access_is_a_conflict() {
        let native = x11rb::x11_utils::X11Error { error_kind: ErrorKind::Access, error_code: 10,
            sequence: 1, bad_value: 0, minor_opcode: 0, major_opcode: 33, extension_name: None, request_name: None };
        assert_eq!(reply_error(ReplyError::X11Error(native)), ShortcutError::Conflict);
    }
}
