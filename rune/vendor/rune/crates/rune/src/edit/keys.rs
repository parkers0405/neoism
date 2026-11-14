//! Key events as the shell hands them to handlers and the focused field.
use zeroize::Zeroize;

/// The modifiers held with a key. `shift`, `alt`, `ctrl` and `logo` are keys; `cmd` is the
/// platform's primary shortcut modifier, reported alongside its key: ⌘ on macOS (with `logo`)
/// and Ctrl elsewhere (with `ctrl`), so Ctrl+W on Windows is `{ cmd, ctrl }`. A test of
/// `cmd && !ctrl` therefore never passes off macOS. Match shortcuts with a
/// [`Chord`](crate::Chord) ("mod+w", "ctrl+tab") rather than by comparing flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    /// The primary shortcut modifier: ⌘ on macOS, Ctrl elsewhere (the web reports either).
    pub cmd: bool,
    pub alt: bool,
    /// The Control key, on every platform; off macOS `cmd` is set with it.
    pub ctrl: bool,
    /// The Super/Windows key (Command on macOS, where `cmd` also reports it).
    pub logo: bool,
}

impl Mods {
    /// Whether the platform's primary shortcut modifier is held: `cmd`.
    pub fn primary(self) -> bool { self.cmd }
}

/// A logical key. More keys may come, so a `match` needs a `_` arm.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub enum Key {
    Text(String),
    Enter,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    /// Page Up. Unclaimed, it scrolls the nearest scroll view up by most of a viewport.
    /// Text fields leave it alone, as AppKit's do: the view scrolls and the caret stays.
    PageUp,
    /// Page Down, as `PageUp` the other way.
    PageDown,
    /// Insert (Help on older Mac keyboards). Text fields ignore it; apps can bind it.
    Insert,
    /// Print Screen (SysRq). Text fields ignore it; apps can bind it. Windows sends it only
    /// on release, so its press and release arrive together.
    PrintScreen,
    /// Scroll Lock. Text fields ignore it; apps can bind it.
    ScrollLock,
    /// Pause (Break). Text fields ignore it; apps can bind it.
    Pause,
    Escape,
    Tab,
    /// Any character typed with Cmd (Ctrl off macOS), lowercased: a letter ("a", "c", "v"), a
    /// digit (⌘1) or punctuation (⌘,). Off macOS, a key that types a non-ASCII character
    /// arrives as the US key in its place, so Ctrl+V on a Russian layout is `Command('v')`
    /// (`Tx::key_base` still says "м"). `mods` still says whether Shift, Option or Ctrl were
    /// held too. Space is the exception: Cmd-Space arrives natively as `Text(" ")` with
    /// `mods.cmd` (scripts send `Command(' ')`). The other native `Text` with `mods.cmd` is a
    /// character typed with Left Ctrl + Left Alt on Windows, which is AltGr there ("@" on a
    /// German layout's Q): it inserts, and `Chord::matches_tx` still finds "ctrl+alt+q".
    /// [`KeyEvent::is_command`] matches either form.
    Command(char),
    /// A function key, F1 through F24. Text fields ignore it; apps can bind it.
    Function(u8),
    /// The Menu (Application) key. Unclaimed, it opens the focused element's context menu, as
    /// Shift+F10 does: see [`El::on_secondary_press`](crate::El::on_secondary_press).
    ContextMenu,
    /// A modifier key going down (`pressed`) or up on its own, sent only where the app opts in
    /// with `App::modifier_keys` (or `Runtime::set_modifier_keys`). `mods` holds the modifiers
    /// after the change, as AppKit's flagsChanged does: Shift's press has `shift` set and its
    /// release doesn't, unless the other Shift is still down. A held modifier never repeats.
    /// Text fields, selection, focus traversal and shortcuts ignore it.
    Modifier { modifier: Modifier, pressed: bool },
}

/// A modifier key, by side. Alt is Option on macOS; Super is Command on macOS and the Windows
/// key elsewhere.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Modifier {
    ShiftLeft,
    ShiftRight,
    ControlLeft,
    ControlRight,
    AltLeft,
    AltRight,
    SuperLeft,
    SuperRight,
    /// Fn, or Globe on recent Mac keyboards. Other platforms rarely report it: their keyboards
    /// handle Fn in firmware.
    Fn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeySide {
    Left,
    Right,
}

impl Modifier {
    /// Which side of the keyboard the key is on; None for Fn.
    pub fn side(self) -> Option<KeySide> {
        use Modifier::*;
        match self {
            ShiftLeft | ControlLeft | AltLeft | SuperLeft => Some(KeySide::Left),
            ShiftRight | ControlRight | AltRight | SuperRight => Some(KeySide::Right),
            Fn => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeyEvent {
    pub key: Key,
    pub mods: Mods,
}

impl KeyEvent {
    /// Whether this is Cmd (Ctrl off macOS) plus `c`, whatever else is held: `Key::Command(c)`,
    /// or for `' '` the `Key::Text(" ")` a native Cmd-Space sends. Letters match either case;
    /// check `mods.shift` for ⇧⌘ chords.
    pub fn is_command(&self, c: char) -> bool {
        let c = c.to_ascii_lowercase();
        match &self.key {
            Key::Command(k) => *k == c,
            Key::Text(t) => c == ' ' && t == " " && self.mods.cmd,
            _ => false,
        }
    }

    /// Text typed with Cmd (macOS) or Ctrl held, such as an unclaimed Shift-Cmd-Space. It never
    /// inserts, matching browsers and AppKit, but still reaches key handlers. Ctrl+Alt is AltGr
    /// on Windows and Linux, so text typed with both still inserts there; Option alone types.
    pub(crate) fn is_command_chord(&self) -> bool {
        let m = self.mods;
        matches!(self.key, Key::Text(_)) && (m.cmd || m.ctrl) && (cfg!(target_os = "macos") || !m.alt)
    }

    /// The standard edit command this is (paste, copy, cut, select all, undo, redo, format):
    /// `Key::Command(c)` with the primary modifier alone, ⌘ on macOS and Ctrl elsewhere, or
    /// Shift too for ⇧⌘Z. Another modifier (Ctrl+Alt+V, ⌃⌘C, Ctrl+Shift+V) makes it an
    /// app's chord, which reaches `on_key` instead.
    pub(crate) fn edit_command(&self) -> Option<char> {
        let Key::Command(c) = self.key else { return None };
        (crate::input::chord::primary_alone(self.mods) && (c == 'z' || !self.mods.shift)).then_some(c)
    }
    /// Alt+Space off macOS: the window menu's key. It never types a space, and on Windows opens
    /// the window menu when no handler takes it. (Option+Space types on macOS.)
    pub(crate) fn is_window_menu_chord(&self) -> bool {
        let m = self.mods;
        !cfg!(target_os = "macos") && matches!(&self.key, Key::Text(t) if t == " ") && m.alt && !m.ctrl && !m.cmd && !m.logo
    }
    /// A macOS Control-letter text binding (Ctrl-A, Ctrl-E, Ctrl-K, …), as in every AppKit field.
    pub(crate) fn control_binding(&self) -> Option<char> {
        let m = self.mods;
        let Key::Text(s) = &self.key else { return None };
        let c = s.chars().next().filter(|_| s.chars().count() == 1)?.to_ascii_lowercase();
        (cfg!(target_os = "macos") && m.ctrl && !m.cmd && !m.alt && "abefpndhk".contains(c)).then_some(c)
    }
}

/// The key pressed with no modifiers.
impl From<Key> for KeyEvent {
    fn from(key: Key) -> Self {
        KeyEvent { key, mods: Mods::default() }
    }
}

impl Drop for KeyEvent {
    fn drop(&mut self) {
        if let Key::Text(text) = &mut self.key { text.zeroize(); }
    }
}
