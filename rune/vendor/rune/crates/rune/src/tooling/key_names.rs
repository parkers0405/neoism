//! Key names, modifiers and unmodified characters in headless scripts and `--keys`.
use crate::edit::Key;

pub(crate) fn key_named(name: &str) -> Option<Key> {
    Some(match name {
        "Enter" => Key::Enter,
        "Backspace" => Key::Backspace,
        "Delete" => Key::Delete,
        "Left" => Key::Left,
        "Right" => Key::Right,
        "Up" => Key::Up,
        "Down" => Key::Down,
        "Home" => Key::Home,
        "End" => Key::End,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,
        "Escape" => Key::Escape,
        "Tab" => Key::Tab,
        "ContextMenu" => Key::ContextMenu,
        "Insert" => Key::Insert,
        "PrintScreen" => Key::PrintScreen,
        "ScrollLock" => Key::ScrollLock,
        "Pause" => Key::Pause,
        f if f.len() > 1 && f.starts_with('F') && f[1..].parse::<u8>().is_ok_and(|n| (1..=24).contains(&n)) => {
            Key::Function(f[1..].parse().unwrap())
        }
        c if c.chars().count() == 1 => Key::Text(c.to_owned()),
        _ => return None,
    })
}

/// A script step's modifier flags. `"cmd"` is the primary shortcut modifier, held as the shell
/// reports it: with Super on macOS and with Ctrl elsewhere, so a script's "mod+k" works on every
/// platform.
pub(crate) fn script_mods(e: &serde_json::Value) -> crate::Mods {
    let flag = |k: &str| e[k].as_bool().unwrap_or(false);
    let cmd = flag("cmd");
    let mac = cfg!(target_os = "macos");
    crate::Mods { shift: flag("shift"), cmd, alt: flag("alt"), ctrl: flag("ctrl") || (cmd && !mac), logo: flag("logo") || (cmd && mac) }
}

/// A script key's `Tx::key_base`: its `"base"`, else a one-character name typed without Shift
/// or Option (which change the character the key types), lowercased.
pub(crate) fn script_key_base(e: &serde_json::Value, name: &str, mods: crate::Mods) -> Option<char> {
    let one = |s: &str| { let mut c = s.chars(); c.next().filter(|_| c.next().is_none()) };
    match e["base"].as_str() {
        Some(base) => one(base),
        None => one(name).filter(|_| !mods.shift && !mods.alt).map(|c| c.to_ascii_lowercase()),
    }
}
