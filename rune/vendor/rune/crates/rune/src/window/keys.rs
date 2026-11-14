//! winit keys and modifiers as Rune's.
use winit::keyboard::{Key as WKey, ModifiersState, NamedKey};

use crate::edit::{Key, KeyEvent, Mods};

pub(crate) fn map_mods(m: ModifiersState) -> Mods {
    Mods { shift: m.shift_key(), cmd: if cfg!(target_os = "macos") { m.super_key() } else { m.control_key() },
        alt: m.alt_key(), ctrl: m.control_key(), logo: m.super_key() }
}

pub(crate) fn map_key(key: &WKey, m: ModifiersState) -> Option<KeyEvent> {
    let mods = map_mods(m);
    let key = match key {
        WKey::Named(n) => match n {
            NamedKey::Enter => Key::Enter,
            NamedKey::Backspace => Key::Backspace,
            NamedKey::Delete => Key::Delete,
            NamedKey::ArrowLeft => Key::Left,
            NamedKey::ArrowRight => Key::Right,
            NamedKey::ArrowUp => Key::Up,
            NamedKey::ArrowDown => Key::Down,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            NamedKey::PageUp => Key::PageUp,
            NamedKey::PageDown => Key::PageDown,
            NamedKey::Insert => Key::Insert,
            NamedKey::PrintScreen => Key::PrintScreen,
            NamedKey::ScrollLock => Key::ScrollLock,
            NamedKey::Pause => Key::Pause,
            NamedKey::Escape => Key::Escape,
            NamedKey::Tab => Key::Tab,
            NamedKey::Space => Key::Text(" ".into()),
            NamedKey::F1 => Key::Function(1),
            NamedKey::F2 => Key::Function(2),
            NamedKey::F3 => Key::Function(3),
            NamedKey::F4 => Key::Function(4),
            NamedKey::F5 => Key::Function(5),
            NamedKey::F6 => Key::Function(6),
            NamedKey::F7 => Key::Function(7),
            NamedKey::F8 => Key::Function(8),
            NamedKey::F9 => Key::Function(9),
            NamedKey::F10 => Key::Function(10),
            NamedKey::F11 => Key::Function(11),
            NamedKey::F12 => Key::Function(12),
            NamedKey::F13 => Key::Function(13),
            NamedKey::F14 => Key::Function(14),
            NamedKey::F15 => Key::Function(15),
            NamedKey::F16 => Key::Function(16),
            NamedKey::F17 => Key::Function(17),
            NamedKey::F18 => Key::Function(18),
            NamedKey::F19 => Key::Function(19),
            NamedKey::F20 => Key::Function(20),
            NamedKey::F21 => Key::Function(21),
            NamedKey::F22 => Key::Function(22),
            NamedKey::F23 => Key::Function(23),
            NamedKey::F24 => Key::Function(24),
            NamedKey::ContextMenu => Key::ContextMenu,
            _ => return None,
        },
        // Windows treats Left Ctrl + Left Alt as AltGr: a character typed with both is text
        // ("@" on a German layout's Q), not a chord. A chord that types nothing comes through
        // `map_key_event`'s fallback instead.
        WKey::Character(c) if mods.cmd && !(cfg!(target_os = "windows") && mods.ctrl && mods.alt) => {
            Key::Command(c.chars().next()?.to_ascii_lowercase())
        }
        WKey::Character(c) => Key::Text(c.to_string()),
        _ => return None,
    };
    Some(KeyEvent { key, mods })
}

/// `map_key` for a window's key event. Windows names no logical key for a chord whose
/// modifiers type nothing on the layout, such as Ctrl+Alt+B on US English (Ctrl+Alt is AltGr
/// there); it still arrives as a chord on the character the key types unmodified. Off macOS, a
/// chord on a key that types a non-ASCII character (Ctrl+М on a Russian layout, Ctrl+Ü on a
/// German one) arrives as the US key in its place, `Command('v')`, as Windows' own shortcuts
/// follow the key's Latin virtual key and macOS's switch to a Latin layout under ⌘: Ctrl+V
/// pastes on every layout.
pub(crate) fn map_key_event(event: &winit::event::KeyEvent, m: ModifiersState) -> Option<KeyEvent> {
    let mut ev = map_key(&event.logical_key, m).or_else(|| unidentified(event, m))?;
    if !cfg!(target_os = "macos")
        && let Key::Command(c) = ev.key
        && !c.is_ascii()
        && let winit::keyboard::PhysicalKey::Code(code) = event.physical_key
        && let Some(us) = crate::shortcuts::us_char(code)
    {
        ev.key = Key::Command(us);
    }
    Some(ev)
}

fn unidentified(event: &winit::event::KeyEvent, m: ModifiersState) -> Option<KeyEvent> {
    let WKey::Unidentified(_) = event.logical_key else { return None };
    let mods = map_mods(m);
    if !(mods.cmd || mods.ctrl) { return None; }
    let c = key_base(event).filter(|c| !c.is_control() && *c != ' ')?;
    let key = if mods.cmd { Key::Command(c.to_ascii_lowercase()) } else { Key::Text(c.to_string()) };
    Some(KeyEvent { key, mods })
}

/// The character `event`'s key types under the current layout with no modifiers, for
/// `Tx::key_base`.
pub(crate) fn key_base(event: &winit::event::KeyEvent) -> Option<char> {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
        match event.key_without_modifiers() {
            WKey::Character(s) => { let mut c = s.chars(); c.next().filter(|_| c.next().is_none()) }
            WKey::Named(NamedKey::Space) => Some(' '),
            _ => None,
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    { let _ = event; None }
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod layout_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_that_type_nothing_reach_handlers() {
        for (named, key) in [
            (NamedKey::ScrollLock, Key::ScrollLock), (NamedKey::Pause, Key::Pause), (NamedKey::ContextMenu, Key::ContextMenu),
            (NamedKey::PrintScreen, Key::PrintScreen), (NamedKey::Insert, Key::Insert),
        ] {
            assert_eq!(map_key(&WKey::Named(named), ModifiersState::empty()).map(|ev| ev.key.clone()), Some(key));
        }
    }
}
