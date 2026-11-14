use super::ShortcutError;
use std::{fmt, str::FromStr};

pub(super) const CTRL: u8 = 1;
pub(super) const ALT: u8 = 2;
pub(super) const SHIFT: u8 = 4;
pub(super) const SUPER: u8 = 8;
const PRIMARY_NAME: u8 = 16;

/// A validated, layout-independent accelerator. cmd/command/logo/meta/win are aliases for
/// super; option/opt is alt; mod (primary) is super on macOS and ctrl elsewhere, as in a
/// [`Chord`](crate::Chord), and is stored as that key; plus is the key that types "+" on a US
/// layout (equal). Stored keys are physical US positions. Windows resolves these to virtual
/// keys at registration; Wayland's portal may rebind the preferred trigger.
/// Display uses ctrl+alt+shift+super order, lowercase keys and normalized aliases.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Shortcut {
    pub(super) mods: u8,
    pub(super) key: super::PhysicalKeyCode,
}

impl FromStr for Shortcut {
    type Err = ShortcutError;
    fn from_str(input: &str) -> Result<Self, Self::Err> { Self::parse_on(input, crate::KeyPlatform::current()) }
}

impl Shortcut {
    /// `str::parse` with `mod` resolved for `platform`: super on macOS, ctrl elsewhere.
    pub fn parse_on(input: &str, platform: crate::KeyPlatform) -> Result<Self, ShortcutError> {
        let invalid = || ShortcutError::InvalidAccelerator(input.into());
        let mut mods = 0;
        // Names given, to refuse a repeat; "mod" is its own name, so "mod+ctrl" is ⌃⌘ on
        // macOS and Ctrl elsewhere rather than an error on one platform.
        let mut named = 0u8;
        let mut key = None;
        for part in input.split('+') {
            let part = part.trim().to_ascii_lowercase();
            let (name, modifier) = match part.as_str() {
                "ctrl" | "control" => (CTRL, CTRL),
                "alt" | "option" | "opt" => (ALT, ALT),
                "shift" => (SHIFT, SHIFT),
                "cmd" | "command" | "super" | "meta" | "logo" | "win" => (SUPER, SUPER),
                "mod" | "primary" => (PRIMARY_NAME, if platform == crate::KeyPlatform::Mac { SUPER } else { CTRL }),
                _ => (0, 0),
            };
            if modifier != 0 {
                if key.is_some() || named & name != 0 { return Err(invalid()); }
                named |= name;
                mods |= modifier;
            } else {
                if key.is_some() { return Err(invalid()); }
                let name = match part.as_str() {
                    "esc" => "escape", "return" | "numpadenter" => "enter", "spacebar" => "space",
                    "arrowleft" => "left", "arrowright" => "right", "arrowup" => "up", "arrowdown" => "down",
                    "del" => "delete", "ins" => "insert", "pgup" => "pageup", "pgdn" => "pagedown",
                    "-" => "minus", "=" | "plus" => "equal", "," => "comma", "." => "period", "/" => "slash",
                    ";" => "semicolon", "'" => "quote", "[" => "bracketleft", "]" => "bracketright",
                    "\\" => "backslash", "`" => "backquote",
                    name => name
                        .strip_prefix("key").filter(|s| s.len() == 1 && s.as_bytes()[0].is_ascii_lowercase())
                        .or_else(|| name.strip_prefix("digit").filter(|s| s.len() == 1 && s.as_bytes()[0].is_ascii_digit()))
                        .unwrap_or(name),
                };
                key = Some(super::keys::named(name).ok_or_else(invalid)?);
            }
        }
        Ok(Self { mods, key: key.ok_or_else(invalid)? })
    }
}

impl fmt::Display for Shortcut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (bit, name) in [(CTRL, "ctrl"), (ALT, "alt"), (SHIFT, "shift"), (SUPER, "super")] {
            if self.mods & bit != 0 { write!(f, "{name}+")?; }
        }
        f.write_str(self.key_name())
    }
}

#[cfg(all(target_os = "linux", feature = "dbus"))]
impl Shortcut {
    pub(super) fn keysym_name(&self) -> String {
        match self.key_name() {
            "escape" => "Escape", "enter" => "Return", "tab" => "Tab", "backspace" => "BackSpace",
            "delete" => "Delete", "insert" => "Insert", "home" => "Home", "end" => "End", "pageup" => "Prior",
            "pagedown" => "Next", "left" => "Left", "right" => "Right", "up" => "Up", "down" => "Down",
            "quote" => "apostrophe", "backquote" => "grave",
            k if k.starts_with('f') && k.len() > 1 => return k.to_uppercase(),
            k => k,
        }.into()
    }

    pub(super) fn portal_trigger(&self) -> String {
        let mut s = String::new();
        for (bit, name) in [(CTRL, "CTRL"), (ALT, "ALT"), (SHIFT, "SHIFT"), (SUPER, "LOGO")] {
            if self.mods & bit != 0 { s.push_str(name); s.push('+'); }
        }
        s.push_str(&self.keysym_name());
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_aliases_round_trip() {
        for (input, expected) in [
            ("CMD+Shift+Space", "shift+super+space"),
            ("OPTION+Control+Return", "ctrl+alt+enter"),
            ("super+SHIFT+ctrl+ALT+F24", "ctrl+alt+shift+super+f24"),
            (" ctrl + A ", "ctrl+a"), ("esc", "escape"), ("cmd+/", "super+slash"),
            ("KeyK", "k"), ("Digit7", "7"), ("BracketLeft", "bracketleft"), ("Semicolon", "semicolon"),
        ] {
            let shortcut: Shortcut = input.parse().unwrap();
            assert_eq!(shortcut.to_string(), expected);
            assert_eq!(shortcut, expected.parse().unwrap());
        }
    }
    #[test]
    fn reject_ambiguous_and_incomplete_accelerators() {
        for input in ["", "cmd", "cmd+", "cmd++a", "cmd+super+a", "a+b", "a+shift", "ctrl+unknown", "F0", "f25", "f01", "é"] {
            assert!(matches!(input.parse::<Shortcut>(), Err(ShortcutError::InvalidAccelerator(_))), "{input}");
        }
    }
}
