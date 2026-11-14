//! How a `Chord` reads: the names a chord string uses, canonical text for settings files, and
//! keycaps per platform. One table per kind of name drives parsing, `Display` and labels.
use std::fmt;

use super::chord::{Chord, ChordKey, ChordMod, KeyPlatform};
use crate::Key;

/// Each modifier's names, the canonical one first, and its keycap on macOS and elsewhere, in
/// the order `Display` writes them. Super's keycap reads "Win" on Windows.
const MODS: &[(ChordMod, &[&str], &str, &str)] = &[
    (ChordMod::Primary, &["mod", "primary"], "⌘", "Ctrl"),
    (ChordMod::Ctrl, &["ctrl", "control"], "⌃", "Ctrl"),
    (ChordMod::Alt, &["alt", "option", "opt"], "⌥", "Alt"),
    (ChordMod::Shift, &["shift"], "⇧", "Shift"),
    (ChordMod::Super, &["super", "cmd", "command", "meta", "win", "logo"], "⌘", "Super"),
];

/// Each named key: the `Key` it arrives as, its names, the canonical one first, and its keycap
/// on macOS and elsewhere.
const KEYS: &[(ChordKey, Key, &[&str], &str, &str)] = &[
    (ChordKey::Enter, Key::Enter, &["enter", "return"], "↩", "Enter"),
    (ChordKey::Tab, Key::Tab, &["tab"], "⇥", "Tab"),
    (ChordKey::Escape, Key::Escape, &["escape", "esc"], "⎋", "Esc"),
    (ChordKey::Backspace, Key::Backspace, &["backspace"], "⌫", "Backspace"),
    (ChordKey::Delete, Key::Delete, &["delete", "del"], "⌦", "Delete"),
    (ChordKey::Insert, Key::Insert, &["insert", "ins"], "Insert", "Insert"),
    (ChordKey::Home, Key::Home, &["home"], "↖", "Home"),
    (ChordKey::End, Key::End, &["end"], "↘", "End"),
    (ChordKey::PageUp, Key::PageUp, &["pageup", "pgup"], "⇞", "Page Up"),
    (ChordKey::PageDown, Key::PageDown, &["pagedown", "pgdn"], "⇟", "Page Down"),
    (ChordKey::Left, Key::Left, &["left", "arrowleft"], "←", "←"),
    (ChordKey::Right, Key::Right, &["right", "arrowright"], "→", "→"),
    (ChordKey::Up, Key::Up, &["up", "arrowup"], "↑", "↑"),
    (ChordKey::Down, Key::Down, &["down", "arrowdown"], "↓", "↓"),
    (ChordKey::ContextMenu, Key::ContextMenu, &["contextmenu", "menu"], "Menu", "Menu"),
    (ChordKey::PrintScreen, Key::PrintScreen, &["printscreen", "prtsc"], "Print Screen", "Print Screen"),
    (ChordKey::ScrollLock, Key::ScrollLock, &["scrolllock"], "Scroll Lock", "Scroll Lock"),
    (ChordKey::Pause, Key::Pause, &["pause", "break"], "Pause", "Pause"),
];

/// Characters a chord string can also name. `Display` writes the name only for the ones the
/// syntax itself uses (space, "+", ","); the rest read as themselves.
const CHARS: &[(char, &str)] = &[
    (' ', "space"), ('+', "plus"), (',', "comma"), ('-', "minus"), ('=', "equal"), ('.', "period"),
    ('/', "slash"), ('\\', "backslash"), ('`', "backquote"), (';', "semicolon"), ('\'', "quote"),
    ('[', "bracketleft"), (']', "bracketright"),
];

pub(super) fn modifier_named(name: &str) -> Option<ChordMod> {
    MODS.iter().find(|m| m.1.contains(&name)).map(|m| m.0)
}

pub(super) fn key_named_word(name: &str) -> Option<ChordKey> {
    KEYS.iter().find(|k| k.2.contains(&name)).map(|k| k.0)
}

pub(super) fn char_named(name: &str) -> Option<char> {
    CHARS.iter().find(|c| c.1 == name).map(|c| c.0)
}

/// The named chord key `key` is.
pub(super) fn named(key: &Key) -> Option<ChordKey> {
    if let Key::Function(n) = key { return Some(ChordKey::Function(*n)); }
    KEYS.iter().find(|k| k.1 == *key).map(|k| k.0)
}

impl Chord {
    /// How the chord reads here: "⇧⌘T" on macOS, "Ctrl+Shift+T" on Windows and Linux.
    pub fn label(&self) -> String { self.label_on(KeyPlatform::current()) }

    /// How the chord reads on `platform`: macOS glyphs run together in Apple's order
    /// (⌃⌥⇧⌘), elsewhere words join with "+" in Ctrl, Alt, Shift, Win (Super) order.
    pub fn label_on(&self, platform: KeyPlatform) -> String {
        self.keycaps_on(platform).join(if platform == KeyPlatform::Mac { "" } else { "+" })
    }

    /// One label per key, modifiers first, for drawing each as a keycap.
    pub fn keycaps_on(&self, platform: KeyPlatform) -> Vec<String> {
        let mac = platform == KeyPlatform::Mac;
        let mut caps: Vec<String> = self.mods_on(platform).into_iter().map(|m| {
            let &(_, _, glyph, word) = MODS.iter().find(|row| row.0 == m).expect("every modifier has a row");
            match (m, platform) {
                (_, KeyPlatform::Mac) => glyph,
                (ChordMod::Super, KeyPlatform::Windows) => "Win",
                _ => word,
            }.to_owned()
        }).collect();
        caps.push(match self.key() {
            ChordKey::Char(' ') => "Space".into(),
            ChordKey::Char(c) => crate::shortcuts::capital(c.to_string()),
            ChordKey::Function(n) => format!("F{n}"),
            key => KEYS.iter().find(|k| k.0 == key).map_or_else(String::new, |k| if mac { k.3 } else { k.4 }.to_owned()),
        });
        caps
    }
}

/// The canonical form `parse` reads back: modifiers in mod, ctrl, alt, shift, super order,
/// then the key ("mod+shift+t", "ctrl+tab", "mod+comma").
impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (m, names, ..) in MODS {
            if self.has(*m) { write!(f, "{}+", names[0])?; }
        }
        match self.key() {
            ChordKey::Char(c @ (' ' | '+' | ',')) => f.write_str(CHARS.iter().find(|n| n.0 == c).map_or("", |n| n.1)),
            ChordKey::Char(c) => write!(f, "{c}"),
            ChordKey::Function(n) => write!(f, "f{n}"),
            key => f.write_str(KEYS.iter().find(|k| k.0 == key).map_or("", |k| k.2[0])),
        }
    }
}
