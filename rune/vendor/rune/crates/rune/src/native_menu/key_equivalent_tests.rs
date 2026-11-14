use super::key_equivalent::*;
use crate::Shortcut;

fn equivalent(accelerator: &str) -> (String, usize) {
    let e = KeyEquivalent::new(&accelerator.parse().unwrap());
    (e.key, e.mask)
}
fn eq(key: &str, mask: usize) -> (String, usize) { (key.into(), mask) }

#[test]
fn equivalents_are_the_characters_the_accelerators_name() {
    assert_eq!(equivalent("cmd+b"), eq("b", COMMAND));
    assert_eq!(equivalent("cmd+="), eq("=", COMMAND));
    assert_eq!(equivalent("cmd+-"), eq("-", COMMAND));
    assert_eq!(equivalent("cmd+r"), eq("r", COMMAND));
    assert_eq!(equivalent("cmd+1"), eq("1", COMMAND));
    assert_eq!(equivalent("ctrl+cmd+s"), eq("s", CONTROL | COMMAND));
    // ⌃⌘F, never the function-key flag (1 << 23) that AppKit draws as 🌐.
    assert_eq!(equivalent("ctrl+cmd+f"), eq("f", CONTROL | COMMAND));
    // Shift stays in the mask beside the unshifted character.
    assert_eq!(equivalent("cmd+shift+z"), eq("z", SHIFT | COMMAND));
    assert_eq!(equivalent("cmd+shift+="), eq("=", SHIFT | COMMAND));
    assert_eq!(equivalent("ctrl+alt+shift+cmd+k"), eq("k", CONTROL | OPTION | SHIFT | COMMAND));
}

#[test]
fn named_keys_use_appkit_characters() {
    assert_eq!(equivalent("alt+cmd+left"), eq("\u{f702}", OPTION | COMMAND));
    assert_eq!(equivalent("cmd+up"), eq("\u{f700}", COMMAND));
    assert_eq!(equivalent("f5"), eq("\u{f708}", 0));
    assert_eq!(equivalent("shift+f24"), eq("\u{f71b}", SHIFT));
    assert_eq!(equivalent("cmd+enter"), eq("\r", COMMAND));
    assert_eq!(equivalent("cmd+backspace"), eq("\u{8}", COMMAND));
    assert_eq!(equivalent("cmd+delete"), eq("\u{f728}", COMMAND));
    assert_eq!(equivalent("cmd+escape"), eq("\u{1b}", COMMAND));
    assert_eq!(equivalent("ctrl+tab"), eq("\t", CONTROL));
    assert_eq!(equivalent("cmd+space"), eq(" ", COMMAND));
}

#[test]
fn every_key_has_one_lowercase_character() {
    let names = ["a", "z", "0", "9", "minus", "equal", "comma", "period", "slash", "semicolon", "quote",
        "bracketleft", "bracketright", "backslash", "backquote", "f1", "f24", "escape", "enter", "space", "tab",
        "backspace", "delete", "insert", "home", "end", "pageup", "pagedown", "left", "right", "up", "down"];
    for name in names {
        let (key, _) = equivalent(&format!("cmd+{name}"));
        assert_eq!(key.chars().count(), 1, "{name}: {key:?}");
        assert_eq!(key.to_lowercase(), key, "{name}: a capital would add Shift");
    }
}

#[test]
fn menu_keycaps_show_the_accelerator() {
    let caps = |s: &str| s.parse::<Shortcut>().unwrap().menu_keycaps();
    let mac = cfg!(target_os = "macos");
    assert_eq!(caps("cmd+b"), [if mac { "⌘" } else { "Super" }, "B"]);
    assert_eq!(caps("ctrl+cmd+f").last().unwrap(), "F");
    assert_eq!(caps("cmd+=").last().unwrap(), "=");
    assert_eq!(caps("cmd+shift+z").last().unwrap(), "Z");
    assert_eq!(caps("ctrl+f5").last().unwrap(), "F5");
    assert_eq!(caps("ctrl+left").last().unwrap(), "←");
}

/// Dvorak's characters at QWERTY positions, by Carbon key code, under any modifiers.
#[cfg(target_os = "macos")]
fn dvorak(key: u32, _modifiers: u32) -> Option<String> {
    Some(match key { 11 => "x", 45 => "b", 3 => "u", 16 => "f", 15 => "p", 1 => "o", 6 => ";", 24 => "]", 27 => "[", _ => return None }.into())
}

#[cfg(target_os = "macos")]
#[test]
fn dvorak_moves_keycaps_but_not_menu_equivalents() {
    crate::shortcuts::force_layout(Some(dvorak));
    let shortcut = |s: &str| s.parse::<Shortcut>().unwrap();
    // Layout labels follow the key's position: QWERTY B types "x" on Dvorak.
    assert_eq!(shortcut("cmd+b").keycaps(), ["⌘", "X"]);
    assert_eq!(shortcut("cmd+=").keycaps(), ["⌘", "]"]);
    for (accelerator, key, mask) in [("cmd+b", "b", COMMAND), ("cmd+=", "=", COMMAND), ("cmd+-", "-", COMMAND),
        ("cmd+r", "r", COMMAND), ("ctrl+cmd+s", "s", CONTROL | COMMAND), ("ctrl+cmd+f", "f", CONTROL | COMMAND),
        ("cmd+shift+z", "z", SHIFT | COMMAND), ("cmd+left", "\u{f702}", COMMAND), ("f5", "\u{f708}", 0)] {
        assert_eq!(equivalent(accelerator), eq(key, mask), "{accelerator}");
        let caps = shortcut(accelerator).menu_keycaps();
        let label = caps.last().unwrap();
        assert!(key.chars().all(|c| c as u32 >= 0xf700) || *label == key.to_uppercase(), "{accelerator}: {caps:?}");
    }
    crate::shortcuts::force_layout(None);
}
