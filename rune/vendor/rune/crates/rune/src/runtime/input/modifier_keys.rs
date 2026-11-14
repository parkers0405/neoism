//! Modifier keys pressed and released on their own (`Key::Modifier`), for runtimes that opt in.
//!
//! Hosts already pass every physical key, modifiers included, to `physical_with_fallback`, so
//! the logical event is made here once for windows, overlays, layer surfaces and headless runs.
//! It is made in queue order, once the focused `on_physical_input` handler has declined the
//! physical event, so a shortcut recorder sees modifiers first, as it sees every key.
//! Presses come only from physical keys. A release also comes from a modifier that a
//! `PhysicalInput::Modifiers` turns off while its key is down (a key-up the platform never
//! sent), and every held key is released on `FocusLost`.

use super::*;
use crate::{Key, Modifier, Mods, PhysicalInput, PhysicalKeyCode as Code};

const ALL: [Modifier; 9] = [Modifier::ShiftLeft, Modifier::ShiftRight, Modifier::ControlLeft, Modifier::ControlRight,
    Modifier::AltLeft, Modifier::AltRight, Modifier::SuperLeft, Modifier::SuperRight, Modifier::Fn];

#[derive(Default)]
pub(super) struct ModifierKeys {
    pub(super) enabled: bool,
    /// Bit `i` set while `ALL[i]` is down.
    held: u16,
    /// The last `PhysicalInput::Modifiers`, to see a flag turn off. A flag that was never on
    /// says nothing: AltGr (Right Alt on many layouts) is down without `alt` on X11 and Wayland.
    last: Mods,
}

fn modifier(code: Code) -> Option<Modifier> {
    Some(match code {
        Code::ShiftLeft => Modifier::ShiftLeft,
        Code::ShiftRight => Modifier::ShiftRight,
        Code::ControlLeft => Modifier::ControlLeft,
        Code::ControlRight => Modifier::ControlRight,
        Code::AltLeft => Modifier::AltLeft,
        Code::AltRight => Modifier::AltRight,
        Code::SuperLeft => Modifier::SuperLeft,
        Code::SuperRight => Modifier::SuperRight,
        Code::Fn => Modifier::Fn,
        _ => return None,
    })
}

fn bit(m: Modifier) -> u16 { 1 << ALL.iter().position(|&a| a == m).unwrap_or(0) }

/// Both sides of `m`'s modifier.
fn pair(m: Modifier) -> u16 {
    use Modifier::*;
    match m {
        ShiftLeft | ShiftRight => bit(ShiftLeft) | bit(ShiftRight),
        ControlLeft | ControlRight => bit(ControlLeft) | bit(ControlRight),
        AltLeft | AltRight => bit(AltLeft) | bit(AltRight),
        SuperLeft | SuperRight => bit(SuperLeft) | bit(SuperRight),
        _ => bit(m),
    }
}

/// Whether `mods` has `m`'s modifier on. Fn has no flag.
fn active(mods: Mods, m: Modifier) -> bool {
    use Modifier::*;
    match m {
        ShiftLeft | ShiftRight => mods.shift,
        ControlLeft | ControlRight => mods.ctrl,
        AltLeft | AltRight => mods.alt,
        SuperLeft | SuperRight => mods.logo,
        _ => false,
    }
}

/// `mods` with `m`'s modifier set to `on`, including `cmd` (Command on macOS, Ctrl elsewhere).
fn with(mut mods: Mods, m: Modifier, on: bool) -> Mods {
    use Modifier::*;
    let mac = cfg!(target_os = "macos");
    match m {
        ShiftLeft | ShiftRight => mods.shift = on,
        ControlLeft | ControlRight => { mods.ctrl = on; if !mac { mods.cmd = on; } }
        AltLeft | AltRight => mods.alt = on,
        SuperLeft | SuperRight => { mods.logo = on; if mac { mods.cmd = on; } }
        _ => {}
    }
    mods
}

/// Windows clears Control and Alt in the flags while AltGr (Right Alt) is down, even with
/// Left Control or Left Alt held.
const ALTGR_CLEARS_FLAGS: bool = cfg!(target_os = "windows");

impl ModifierKeys {
    fn event(&self, m: Modifier, pressed: bool, mods: Mods) -> KeyEvent {
        // Platforms differ on whether a modifier's own key event carries the flags from
        // before or after it; report them after, from the keys held.
        KeyEvent { key: Key::Modifier { modifier: m, pressed }, mods: with(mods, m, self.held & pair(m) != 0) }
    }

    /// The keys `event` presses and releases, without changing anything. A press while held,
    /// a release not held and a repeat change nothing. `Modifiers` releases held keys whose
    /// flag turns off (Fn has none, so only its key-up or focus loss ends it); `FocusLost`
    /// releases them all.
    fn changes(&self, event: &PhysicalInput, altgr_clears: bool) -> (u16, u16) {
        if !self.enabled { return (0, 0); }
        match *event {
            PhysicalInput::Key { key, pressed, repeat, .. } => match modifier(key) {
                Some(m) if !repeat && (self.held & bit(m) != 0) != pressed => if pressed { (bit(m), 0) } else { (0, bit(m)) },
                _ => (0, 0),
            },
            PhysicalInput::Modifiers(now) => {
                let altgr = altgr_clears && self.held & bit(Modifier::AltRight) != 0;
                let lifted = ALL.into_iter().filter(|&m| {
                    use Modifier::*;
                    let cleared = altgr && matches!(m, ControlLeft | ControlRight | AltLeft | AltRight);
                    self.held & bit(m) != 0 && active(self.last, m) && !active(now, m) && !cleared
                });
                (0, lifted.fold(0, |all, m| all | bit(m)))
            }
            PhysicalInput::FocusLost => (0, self.held),
        }
    }

    /// Whether `event` presses or releases a modifier key.
    fn pending(&self, event: &PhysicalInput) -> bool { self.changes(event, ALTGR_CLEARS_FLAGS) != (0, 0) }

    /// Takes `event` in, with the `Key::Modifier` events it makes. None when a physical handler
    /// `consumed` it: a press it took is never held, so its release is never sent either, and a
    /// release or focus loss it took ends held keys silently. That is the one way `on_key` sees
    /// a press without its release: a handler, such as a shortcut recorder that starts while a
    /// modifier is down, takes the key-up.
    fn apply(&mut self, event: &PhysicalInput, consumed: bool, altgr_clears: bool) -> Vec<KeyEvent> {
        let (pressed, released) = self.changes(event, altgr_clears);
        let mods = match *event {
            PhysicalInput::Key { mods, .. } => mods,
            PhysicalInput::Modifiers(now) => { self.last = now; now }
            PhysicalInput::FocusLost => { self.last = Mods::default(); Mods::default() }
        };
        let mut out = Vec::new();
        for m in ALL {
            if pressed & bit(m) != 0 && !consumed {
                self.held |= bit(m);
                out.push(self.event(m, true, mods));
            } else if released & bit(m) != 0 {
                self.held &= !bit(m);
                if !consumed { out.push(self.event(m, false, mods)); }
            }
        }
        out
    }
}

impl<S: 'static> Runtime<S> {
    /// Send `Key::Modifier` presses and releases to `on_key` and the focused input's key
    /// handler. Off by default: handlers written before it may act on any key.
    pub fn set_modifier_keys(&mut self, enabled: bool) {
        if self.modifier_keys.enabled != enabled { self.modifier_keys.held = 0; }
        self.modifier_keys.enabled = enabled;
    }

    pub fn modifier_keys(&self) -> bool { self.modifier_keys.enabled }

    /// Whether `event` presses or releases a modifier key, which needs a frame to deliver.
    pub(super) fn modifier_pending(&self, event: &PhysicalInput) -> bool { self.modifier_keys.pending(event) }

    /// After the focused physical handler saw `event` (and `consumed` it or not): route the
    /// modifier keys it presses and releases. True if a handler took one.
    pub(super) fn modifier_dispatch(&mut self, event: &PhysicalInput, consumed: bool, tx: &mut Tx, live: &mut Option<bool>) -> bool {
        let mut ran = false;
        let code = match *event { PhysicalInput::Key { key, .. } => Some(key), _ => None };
        for ev in self.modifier_keys.apply(event, consumed, ALTGR_CLEARS_FLAGS) {
            let mut one = self.transaction();
            one.physical_key = code;
            let took = self.route_modifier(&ev, &mut one);
            fold_live(live, took, &one);
            ran |= took;
            merge(tx, one);
        }
        ran
    }

    /// A modifier key goes to the app and the focused input's key handler, in `route_key`'s
    /// order, and to nothing built in: no editing, selection, focus traversal or focus ring.
    pub(super) fn route_modifier(&mut self, ev: &KeyEvent, one: &mut Tx) -> bool {
        if !self.modifier_keys.enabled { return false; }
        one.key_repeat = false;
        let app_first = self.focus.path().is_none();
        if app_first && let Some(f) = self.on_key && Tx::took(self.app_key(f, ev, one), one) { return true; }
        if let Some(f) = self.focused_input().and_then(|input| input.key.clone())
            && Tx::took(f(states!(self), ev, one), one)
        {
            return true;
        }
        if !app_first && let Some(f) = self.on_key && Tx::took(self.app_key(f, ev, one), one) {
            log_event!(self, "key {:?}", ev.key);
            return true;
        }
        matches!(ev.key, Key::Modifier { pressed: false, .. }) && self.route_key_up(ev, one)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cx, El, Env, KeySide, TextEdit, TextStyle, col, el, text_input};

    #[derive(Default)]
    struct Keys { edit: TextEdit, app: Vec<KeyEvent>, field: Vec<KeyEvent>, field_takes: bool, chords: usize,
        recording: bool, recorded: Vec<PhysicalInput>, focus_lost: usize }
    fn view(s: &Keys, _: &mut Cx<Keys>) -> El {
        col().size(300.0, 200.0)
            .child(text_input(&s.edit, TextStyle::new(14.0, 400.0)).single_line().key("field").size(200.0, 32.0)
                .on_edit(|s: &mut Keys| &mut s.edit)
                .on_input_key(|s: &mut Keys, ev, _| {
                    if !matches!(ev.key, Key::Modifier { .. }) { return false; }
                    s.field.push(ev.clone());
                    s.field_takes
                }))
            .child(el().key("blank").size(100.0, 40.0))
            // A shortcut recorder: it takes every key while recording.
            .child(el().key("recorder").size(100.0, 40.0).focusable(true)
                .on_physical_input(|s: &mut Keys, ev, _| {
                    if matches!(ev, PhysicalInput::FocusLost) { s.focus_lost += 1; }
                    if s.recording { s.recorded.push(*ev); }
                    s.recording
                }))
    }
    fn runtime(enabled: bool) -> Runtime<Keys> {
        let mut rt = Runtime::new(Keys::default(), view, (300.0, 200.0), Env::new(true, 1.0));
        rt.set_modifier_keys(enabled);
        rt.on_key = Some(|s, ev, _| {
            // A typical chord binding: Cmd with any key.
            if ev.mods.cmd && matches!(ev.key, Key::Text(_) | Key::Command(_)) { s.chords += 1; }
            s.app.push(ev.clone());
            true
        });
        rt.frame(0.0);
        rt
    }
    fn click(rt: &mut Runtime<Keys>, key: &str) {
        let (x, y) = rt.find(key).unwrap(); rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(0.0);
    }
    /// A physical modifier key, as hosts send it: `mods` from before the change, like macOS.
    fn modifier(rt: &mut Runtime<Keys>, code: Code, pressed: bool, mods: Mods) -> bool {
        let redraw = rt.physical_with_fallback(PhysicalInput::Key { key: code, mods, pressed, repeat: false }, None);
        rt.frame(0.0);
        redraw
    }
    fn flags(rt: &mut Runtime<Keys>, mods: Mods) -> bool {
        let redraw = rt.physical_with_fallback(PhysicalInput::Modifiers(mods), None);
        rt.frame(0.0);
        redraw
    }
    fn modifiers(log: &[KeyEvent]) -> Vec<(Modifier, bool)> {
        log.iter().filter_map(|ev| match ev.key { Key::Modifier { modifier, pressed } => Some((modifier, pressed)), _ => None }).collect()
    }
    const SHIFT: Mods = Mods { shift: true, ctrl: false, alt: false, logo: false, cmd: false };
    const ALT: Mods = Mods { shift: false, ctrl: false, alt: true, logo: false, cmd: false };

    #[test]
    fn shift_alone_sends_press_and_release_with_side_and_mods_after_the_change() {
        let mut rt = runtime(true);
        assert!(modifier(&mut rt, Code::ShiftLeft, true, Mods::default()), "a press needs a frame");
        assert!(!flags(&mut rt, SHIFT), "the flags of a press already sent add nothing");
        assert!(modifier(&mut rt, Code::ShiftLeft, false, SHIFT));
        assert!(!flags(&mut rt, Mods::default()));
        assert_eq!(modifiers(&rt.state.app), [(Modifier::ShiftLeft, true), (Modifier::ShiftLeft, false)]);
        assert_eq!((rt.state.app[0].mods, rt.state.app[1].mods), (SHIFT, Mods::default()));
        assert_eq!(Modifier::ShiftLeft.side(), Some(KeySide::Left));
        assert!(!rt.focus.keyboard(), "a modifier tap shows no focus ring");
    }

    #[test]
    fn right_option_is_told_apart_from_left_option() {
        let mut rt = runtime(true);
        modifier(&mut rt, Code::AltRight, true, Mods::default());
        modifier(&mut rt, Code::AltLeft, true, ALT);
        modifier(&mut rt, Code::AltRight, false, ALT);
        modifier(&mut rt, Code::AltLeft, false, ALT);
        assert_eq!(modifiers(&rt.state.app), [(Modifier::AltRight, true), (Modifier::AltLeft, true),
            (Modifier::AltRight, false), (Modifier::AltLeft, false)]);
        let alt: Vec<bool> = rt.state.app.iter().map(|ev| ev.mods.alt).collect();
        assert_eq!(alt, [true, true, true, false], "Option stays on while the other side is down");
        assert_eq!((Modifier::AltRight.side(), Modifier::Fn.side()), (Some(KeySide::Right), None));
    }

    #[test]
    fn a_held_modifier_then_a_letter_is_the_usual_chord() {
        let mut rt = runtime(true);
        let cmd = Mods { cmd: true, ..Mods::default() };
        let code = if cfg!(target_os = "macos") { Code::SuperLeft } else { Code::ControlLeft };
        modifier(&mut rt, code, true, Mods::default());
        assert_eq!(rt.state.app[0].mods.cmd, true);
        assert_eq!(rt.state.chords, 0, "the modifier alone is no chord");
        rt.physical_with_fallback(PhysicalInput::Key { key: Code::KeyK, mods: cmd, pressed: true, repeat: false },
            Some(KeyEvent { key: Key::Command('k'), mods: cmd }));
        rt.frame(0.0);
        assert_eq!(rt.state.app[1], KeyEvent { key: Key::Command('k'), mods: cmd });
        assert_eq!(rt.state.chords, 1);
        // Shift held types capitals in a field as before.
        click(&mut rt, "field");
        modifier(&mut rt, Code::ShiftRight, true, Mods::default());
        rt.physical_with_fallback(PhysicalInput::Key { key: Code::KeyA, mods: SHIFT, pressed: true, repeat: false },
            Some(KeyEvent { key: Key::Text("A".into()), mods: SHIFT }));
        rt.frame(0.0);
        assert_eq!(rt.state.edit.text, "A");
    }

    #[test]
    fn modifier_keys_never_fire_chords_insert_text_or_repeat() {
        let mut rt = runtime(true);
        click(&mut rt, "field");
        rt.state.app.clear();
        for code in [Code::SuperLeft, Code::ControlLeft, Code::AltLeft, Code::ShiftLeft, Code::Fn] {
            modifier(&mut rt, code, true, Mods::default());
            rt.physical_with_fallback(PhysicalInput::Key { key: code, mods: Mods::default(), pressed: true, repeat: true }, None);
            rt.frame(0.0);
            modifier(&mut rt, code, false, Mods::default());
        }
        assert_eq!(rt.state.chords, 0);
        assert_eq!(rt.state.edit.text, "");
        assert_eq!(rt.state.app.len(), 10, "one press and one release each, no repeats");
        assert!(rt.state.app.iter().all(|ev| matches!(ev.key, Key::Modifier { .. })));
        // A KeyEvent sent straight to a field's editor changes nothing either.
        let ev = KeyEvent { key: Key::Modifier { modifier: Modifier::ShiftLeft, pressed: true }, mods: SHIFT };
        assert_eq!(rt.state.edit.key(&ev, None), crate::edit::EditOutcome::default());
    }

    #[test]
    fn a_focused_field_hears_modifiers_before_the_window_unless_nothing_is_focused() {
        let mut rt = runtime(true);
        modifier(&mut rt, Code::ShiftLeft, true, Mods::default());
        assert_eq!((rt.state.app.len(), rt.state.field.len()), (1, 0), "nothing focused: the window's on_key");
        modifier(&mut rt, Code::ShiftLeft, false, SHIFT);
        click(&mut rt, "field");
        rt.state.app.clear();
        rt.state.field_takes = true;
        modifier(&mut rt, Code::ControlRight, true, Mods::default());
        assert_eq!((rt.state.app.len(), rt.state.field.len()), (0, 1), "the field took it");
        rt.state.field_takes = false;
        modifier(&mut rt, Code::ControlRight, false, Mods { ctrl: true, ..Mods::default() });
        assert_eq!((rt.state.app.len(), rt.state.field.len()), (1, 2), "the field passed it on");
        assert_eq!(rt.state.app[0].key, Key::Modifier { modifier: Modifier::ControlRight, pressed: false });
    }

    #[test]
    fn off_by_default_existing_handlers_see_no_modifier_keys() {
        let mut rt = runtime(false);
        assert!(!rt.modifier_keys());
        modifier(&mut rt, Code::ShiftLeft, true, Mods::default());
        assert!(!modifier(&mut rt, Code::ShiftLeft, false, SHIFT), "an unused release needs no frame");
        assert!(!flags(&mut rt, Mods::default()));
        rt.physical_input(PhysicalInput::FocusLost);
        rt.frame(0.0);
        assert!(rt.state.app.is_empty());
    }

    #[test]
    fn a_key_up_the_platform_never_sent_comes_from_the_flags_or_focus_loss() {
        let mut rt = runtime(true);
        let both = Mods { shift: true, alt: true, ..Mods::default() };
        modifier(&mut rt, Code::ShiftLeft, true, Mods::default());
        flags(&mut rt, SHIFT);
        modifier(&mut rt, Code::AltRight, true, SHIFT);
        flags(&mut rt, both);
        modifier(&mut rt, Code::Fn, true, both);
        assert!(flags(&mut rt, ALT), "Shift went off without its key-up");
        assert_eq!(modifiers(&rt.state.app)[3..], [(Modifier::ShiftLeft, false)]);
        rt.physical_input(PhysicalInput::FocusLost);
        rt.frame(0.0);
        assert_eq!(modifiers(&rt.state.app)[4..], [(Modifier::AltRight, false), (Modifier::Fn, false)]);
        assert!(rt.state.app[4..].iter().all(|ev| ev.mods == Mods::default()));
        assert!(!modifier(&mut rt, Code::AltRight, false, Mods::default()), "a late key-up is not sent twice");
        assert_eq!(rt.state.app.len(), 6);
    }

    #[test]
    fn altgr_held_without_the_alt_flag_stays_down_until_its_key_up() {
        // X11 and Wayland report AltGr (Right Alt on many layouts) as Level 3, not `alt`.
        let mut rt = runtime(true);
        modifier(&mut rt, Code::AltRight, true, Mods::default());
        flags(&mut rt, Mods::default());
        flags(&mut rt, SHIFT);
        assert_eq!(modifiers(&rt.state.app), [(Modifier::AltRight, true)]);
        modifier(&mut rt, Code::AltRight, false, SHIFT);
        assert_eq!(modifiers(&rt.state.app), [(Modifier::AltRight, true), (Modifier::AltRight, false)]);
    }

    #[test]
    fn a_recorder_takes_modifiers_first_and_on_key_never_hears_half_of_what_it_took() {
        let mut rt = runtime(true);
        click(&mut rt, "recorder");
        rt.state.recording = true;
        // Recording ⌥K: the recorder takes every key, and on_key hears none of it.
        modifier(&mut rt, Code::AltLeft, true, Mods::default());
        flags(&mut rt, ALT);
        rt.physical_with_fallback(PhysicalInput::Key { key: Code::KeyK, mods: ALT, pressed: true, repeat: false },
            Some(KeyEvent { key: Key::Text("k".into()), mods: ALT }));
        rt.frame(0.0);
        rt.state.recording = false;
        assert!(!flags(&mut rt, Mods::default()), "Option's flag off is no release: its press was taken");
        assert!(!modifier(&mut rt, Code::AltLeft, false, ALT), "nor is its key-up");
        assert_eq!(rt.state.recorded.len(), 3);
        assert!(rt.state.app.is_empty());
        // A release the recorder takes ends the key silently, so the next press is heard.
        modifier(&mut rt, Code::ShiftLeft, true, Mods::default());
        rt.state.recording = true;
        modifier(&mut rt, Code::ShiftLeft, false, SHIFT);
        modifier(&mut rt, Code::AltRight, true, Mods::default());
        rt.state.recording = false;
        modifier(&mut rt, Code::ShiftLeft, true, Mods::default());
        assert_eq!(modifiers(&rt.state.app), [(Modifier::ShiftLeft, true), (Modifier::ShiftLeft, true)]);
        // A focus loss the recorder takes ends held keys silently, releases included.
        rt.state.recording = true;
        rt.physical_input(PhysicalInput::FocusLost);
        rt.frame(0.0);
        rt.state.recording = false;
        assert!(matches!(rt.state.recorded.last(), Some(PhysicalInput::FocusLost)));
        assert_eq!(rt.state.app.len(), 2);
        assert!(!modifier(&mut rt, Code::ShiftLeft, false, SHIFT), "Shift was already ended");
        modifier(&mut rt, Code::AltRight, true, Mods::default());
        assert_eq!(modifiers(&rt.state.app)[2..], [(Modifier::AltRight, true)], "a key the recorder took is heard next time");
    }

    #[test]
    fn a_blur_or_a_hide_releases_held_keys_once() {
        let mut rt = runtime(true);
        click(&mut rt, "recorder");
        rt.window_focus(true);
        modifier(&mut rt, Code::ShiftLeft, true, Mods::default());
        // How `--desktop` and headless scripts blur a window.
        rt.window_focus(false);
        rt.frame(0.0);
        assert_eq!(modifiers(&rt.state.app), [(Modifier::ShiftLeft, true), (Modifier::ShiftLeft, false)]);
        assert_eq!(rt.state.focus_lost, 1, "the recorder hears the blur too");
        // A hide, then the blur it may or may not bring.
        rt.window_focus(true);
        modifier(&mut rt, Code::AltRight, true, Mods::default());
        rt.keys_lost();
        assert!(rt.keys_queued(), "a hidden host steps a frame to deliver it");
        rt.window_focus(false);
        rt.frame(0.0);
        assert_eq!(modifiers(&rt.state.app)[2..], [(Modifier::AltRight, true), (Modifier::AltRight, false)]);
        assert_eq!(rt.state.focus_lost, 2);
        rt.window_focus(true);
        rt.keys_lost();
        rt.frame(0.0);
        assert_eq!(rt.state.focus_lost, 3, "a hide without a blur, after focus came back");
    }

    #[test]
    fn a_focus_move_inside_the_window_keeps_modifiers_held() {
        let mut rt = runtime(true);
        modifier(&mut rt, Code::ShiftLeft, true, Mods::default());
        click(&mut rt, "recorder");
        click(&mut rt, "field");
        modifier(&mut rt, Code::ShiftLeft, false, SHIFT);
        assert_eq!(modifiers(&rt.state.app), [(Modifier::ShiftLeft, true), (Modifier::ShiftLeft, false)]);
    }

    #[test]
    fn windows_altgr_clearing_control_and_alt_is_no_release() {
        let key = |key, pressed, mods| PhysicalInput::Key { key, mods, pressed, repeat: false };
        let ctrl = Mods { ctrl: true, ..Mods::default() };
        let sent = |out: Vec<KeyEvent>| modifiers(&out);
        for altgr_clears in [true, false] {
            let mut keys = ModifierKeys { enabled: true, ..ModifierKeys::default() };
            keys.apply(&key(Code::ControlLeft, true, Mods::default()), false, altgr_clears);
            keys.apply(&PhysicalInput::Modifiers(ctrl), false, altgr_clears);
            keys.apply(&key(Code::AltRight, true, ctrl), false, altgr_clears);
            // Windows clears both flags while AltGr is down; X11 and Wayland leave Control on.
            let cleared = sent(keys.apply(&PhysicalInput::Modifiers(Mods::default()), false, altgr_clears));
            if !altgr_clears {
                assert_eq!(cleared, [(Modifier::ControlLeft, false)], "elsewhere a flag off is a key-up");
                continue;
            }
            assert!(cleared.is_empty(), "Left Control is still down");
            assert_eq!(sent(keys.apply(&key(Code::AltRight, false, Mods::default()), false, true)), [(Modifier::AltRight, false)]);
            assert!(sent(keys.apply(&PhysicalInput::Modifiers(ctrl), false, true)).is_empty());
            assert_eq!(sent(keys.apply(&key(Code::ControlLeft, false, ctrl), false, true)), [(Modifier::ControlLeft, false)]);
        }
    }
}
