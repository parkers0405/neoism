//! In-app key chords: "mod+shift+t" as data, matched against a `KeyEvent` and drawn per
//! platform. `mod` is the platform's primary modifier (⌘ on macOS, Ctrl elsewhere); `ctrl`,
//! `alt`, `shift` and `super` name keys. Both sides of a match resolve to the keys held on
//! the platform, so "mod+b" and "ctrl+b" are the same chord on Windows and Linux, and
//! different ones (⌘B and ⌃B) on macOS. `Keybind` adds alternatives and per-platform chords.
use std::{fmt, hash::{Hash, Hasher}, str::FromStr};

use super::chord_label::{char_named, key_named_word, modifier_named, named};
use crate::{Key, KeyEvent, Mods, PhysicalKeyCode};

pub(super) const PRIMARY: u8 = 1;
const CTRL: u8 = 2;
const ALT: u8 = 4;
pub(super) const SHIFT: u8 = 8;
const SUPER: u8 = 16;

/// The platform whose keyboard conventions a chord resolves and draws with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyPlatform {
    /// ⌘ is the primary modifier; Control is a separate key. Glyph labels: "⇧⌘T".
    Mac,
    /// Ctrl is the primary modifier. Word labels: "Ctrl+Shift+T", the logo key is "Win".
    Windows,
    /// Ctrl is the primary modifier. Word labels, the logo key is "Super". The web uses this
    /// in a browser that isn't on an Apple device.
    Linux,
}

impl KeyPlatform {
    /// The platform this build runs on.
    #[cfg(not(target_arch = "wasm32"))]
    pub const fn current() -> Self {
        if cfg!(any(target_os = "macos", target_os = "ios")) { return Self::Mac; }
        if cfg!(target_os = "windows") { return Self::Windows; }
        Self::Linux
    }

    /// The platform the browser runs on: `Mac` when `navigator.platform` names an Apple
    /// device (as the browser's own shortcuts do), else `Linux`.
    #[cfg(target_arch = "wasm32")]
    pub fn current() -> Self {
        static PLATFORM: std::sync::OnceLock<KeyPlatform> = std::sync::OnceLock::new();
        *PLATFORM.get_or_init(|| {
            let platform = js_sys::Reflect::get(&js_sys::global(), &"navigator".into())
                .and_then(|navigator| js_sys::Reflect::get(&navigator, &"platform".into()))
                .ok().and_then(|p| p.as_string()).unwrap_or_default();
            if ["Mac", "iPod", "iPhone", "iPad"].iter().any(|apple| platform.contains(apple)) { Self::Mac } else { Self::Linux }
        })
    }

    pub(super) fn mac(self) -> bool { matches!(self, Self::Mac) }
}

/// One modifier as a chord names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChordMod {
    /// "mod": ⌘ on macOS, Ctrl elsewhere.
    Primary,
    /// "ctrl": the Control key. Off macOS it is the primary modifier, so it equals `Primary`.
    Ctrl,
    /// "alt": Option on macOS.
    Alt,
    Shift,
    /// "super": the logo key (Windows key, Super); ⌘ on macOS, where it equals `Primary`.
    Super,
}

impl ChordMod {
    pub(super) const fn bit(self) -> u8 {
        match self { Self::Primary => PRIMARY, Self::Ctrl => CTRL, Self::Alt => ALT, Self::Shift => SHIFT, Self::Super => SUPER }
    }
}

/// The key a chord ends in. A character is the one the key types without Shift ('t', '[',
/// '1', ','), lowercase; name the unshifted key and add `shift` ("mod+shift+[", not "mod+{").
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChordKey {
    Char(char),
    Enter,
    Tab,
    Escape,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Left,
    Right,
    Up,
    Down,
    /// F1 through F24.
    Function(u8),
    ContextMenu,
    PrintScreen,
    ScrollLock,
    Pause,
}

/// A key with modifiers, as an app binds it: `Chord::parse("mod+shift+t")`. Parsing is
/// case-insensitive and ignores spaces around each "+"; modifiers are `mod` (alias `primary`),
/// `ctrl` (`control`), `alt` (`option`, `opt`), `shift` and `super` (`cmd`, `command`, `meta`,
/// `win`, `logo`: the literal ⌘ or logo key, which is rarely what a cross-platform binding
/// wants). Keys are a character or a name: `enter` (`return`), `tab`, `escape` (`esc`),
/// `backspace`, `delete` (`del`), `insert`, `home`, `end`, `pageup`, `pagedown`,
/// `left`/`right`/`up`/`down` (`arrowleft`, …), `space`, `plus` (or a trailing "+":
/// "mod++"), `comma`, the punctuation names (`minus`, `bracketleft`, …), `f1`–`f24`,
/// `contextmenu` (`menu`), `printscreen`, `scrolllock` and `pause`. "{" and "}" read as Shift
/// with "[" and "]", the keys that type them on a US layout. `Display` writes the canonical
/// form back ("mod+shift+t"), for settings files.
#[derive(Clone, Copy, Debug)]
pub struct Chord {
    mods: u8,
    key: ChordKey,
}

/// A chord or keybind string that doesn't parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChordError(String);

impl fmt::Display for ChordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "invalid key chord: {}", self.0) }
}

impl std::error::Error for ChordError {}

impl Chord {
    /// The key alone; add modifiers with `with`. The key is stored as parsing would write it:
    /// a character lowercased, '{' and '}' as Shift with '[' and ']', a tab, newline, escape,
    /// backspace or delete character as that key, and a function key within F1–F24.
    pub const fn new(key: ChordKey) -> Self {
        let (mods, key) = match key {
            ChordKey::Char('{') => (SHIFT, ChordKey::Char('[')),
            ChordKey::Char('}') => (SHIFT, ChordKey::Char(']')),
            ChordKey::Char('\t') => (0, ChordKey::Tab),
            ChordKey::Char('\n' | '\r') => (0, ChordKey::Enter),
            ChordKey::Char('\u{1b}') => (0, ChordKey::Escape),
            ChordKey::Char('\u{8}') => (0, ChordKey::Backspace),
            ChordKey::Char('\u{7f}') => (0, ChordKey::Delete),
            ChordKey::Char(c) => (0, ChordKey::Char(c.to_ascii_lowercase())),
            ChordKey::Function(n) => (0, ChordKey::Function(if n < 1 { 1 } else if n > 24 { 24 } else { n })),
            key => (0, key),
        };
        Self { mods, key }
    }

    /// This chord with `m` held too.
    pub const fn with(self, m: ChordMod) -> Self { Self { mods: self.mods | m.bit(), key: self.key } }

    pub fn parse(s: &str) -> Result<Self, ChordError> { s.parse() }

    /// The key, a character lowercased.
    pub fn key(&self) -> ChordKey {
        match self.key { ChordKey::Char(c) => ChordKey::Char(lower(c)), key => key }
    }

    /// Whether the chord names `m`, as written (before platform resolution).
    pub fn has(&self, m: ChordMod) -> bool { self.mods & m.bit() != 0 }

    /// Whether `ev` is this chord on this platform: the same keys held, no more and no fewer,
    /// and the same key. Letters match either case. A shifted character also matches the key
    /// that types it unshifted on a US layout ('{' for "shift+["); use `matches_tx` in a key
    /// handler to follow the user's layout instead.
    pub fn matches(&self, ev: &KeyEvent) -> bool { self.matches_on(ev, KeyPlatform::current()) }

    /// `matches`, also comparing a character chord with `tx.key_base()`, the character the
    /// key types with no modifiers under the current layout (⌥B types "∫" on a Mac, ⇧[ types
    /// "{"), so the binding follows the layout as the menu bar's does. Where neither that
    /// character nor the typed one is ASCII (a Cyrillic or Greek layout, "ü" on German) an
    /// ASCII chord falls back to `tx.physical_key()`, the key in that place on a US layout.
    pub fn matches_tx(&self, ev: &KeyEvent, tx: &crate::Tx) -> bool { self.matches_tx_on(ev, tx, KeyPlatform::current()) }

    /// `matches`, resolving the chord with `platform`'s conventions: for tests, or an app that
    /// lets the user pick a keymap. `ev.mods` reads as this build reports them.
    pub fn matches_on(&self, ev: &KeyEvent, platform: KeyPlatform) -> bool { self.matches_with(ev, None, None, platform) }

    /// `matches_tx` with `platform`'s conventions; see `matches_on`.
    pub fn matches_tx_on(&self, ev: &KeyEvent, tx: &crate::Tx, platform: KeyPlatform) -> bool {
        self.matches_with(ev, tx.key_base(), tx.physical_key(), platform)
    }

    /// Whether `self` and `other` are the same keys on `platform`: "mod+b" and "ctrl+b" are
    /// on Windows and Linux, not on macOS. For conflict checks in a shortcut table or editor.
    pub fn same_keys(&self, other: &Chord, platform: KeyPlatform) -> bool {
        self.key() == other.key() && fold(self.mods, platform) == fold(other.mods, platform)
    }

    /// The modifiers this chord needs on `platform`, in label order: ⌃⌥⇧⌘ on macOS, where
    /// `Super` folds into `Primary`; Ctrl, Alt, Shift, Super elsewhere, where `Ctrl` does.
    pub fn mods_on(&self, platform: KeyPlatform) -> Vec<ChordMod> {
        let bits = fold(self.mods, platform);
        let order = if platform.mac() {
            [ChordMod::Ctrl, ChordMod::Alt, ChordMod::Shift, ChordMod::Primary]
        } else {
            [ChordMod::Primary, ChordMod::Alt, ChordMod::Shift, ChordMod::Super]
        };
        order.into_iter().filter(|m| bits & m.bit() != 0).collect()
    }

    fn matches_with(&self, ev: &KeyEvent, base: Option<char>, physical: Option<PhysicalKeyCode>, platform: KeyPlatform) -> bool {
        let (want, held) = (fold(self.mods, platform), held(ev.mods, platform));
        let ChordKey::Char(c) = self.key() else { return want == held && named(&ev.key) == Some(self.key) };
        let Some(typed) = typed(&ev.key) else { return false };
        let base = base.map(lower);
        // Past the typed character, the key's own: its unmodified character when the layout
        // gives one, else the US key that types it shifted, and past a non-Latin character
        // the US key in its place, as browsers' shortcut handling falls back to `code`.
        let us = || c.is_ascii() && !typed.is_ascii() && physical.and_then(crate::shortcuts::us_char) == Some(c);
        if want == held {
            return typed == c || match base {
                Some(b) => b == c || (!b.is_ascii() && us()),
                None => (ev.mods.shift && us_unshifted(typed) == Some(c)) || us(),
            };
        }
        // A chord on a shifted character ("mod+plus") is that character with or without the
        // Shift that types it: Ctrl+Shift+= on a US layout, Ctrl++ on a German one.
        held == want | SHIFT && !c.is_alphanumeric() && typed == c && base != Some(c)
    }
}

impl PartialEq for Chord {
    fn eq(&self, other: &Self) -> bool { self.mods == other.mods && self.key() == other.key() }
}

impl Eq for Chord {}

impl Hash for Chord {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.mods.hash(state);
        self.key().hash(state);
    }
}

impl FromStr for Chord {
    type Err = ChordError;
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let invalid = || ChordError(input.into());
        let mut parts: Vec<&str> = input.split('+').map(str::trim).collect();
        // Two empty parts at the end are the plus key: "mod++", "mod + +", "+".
        let plus = parts.len() >= 2 && parts[parts.len() - 2..].iter().all(|p| p.is_empty());
        if plus {
            parts.truncate(parts.len() - 2);
            if parts == [""] { parts.clear(); }
        }
        let mut mods = 0;
        let mut key = plus.then_some(ChordKey::Char('+'));
        for part in parts {
            let part = part.to_lowercase();
            if let Some(m) = modifier_named(&part) {
                if key.is_some() || mods & m.bit() != 0 { return Err(invalid()); }
                mods |= m.bit();
                continue;
            }
            if key.is_some() { return Err(invalid()); }
            key = Some(key_named(&part).ok_or_else(invalid)?);
        }
        let chord = Self::new(key.ok_or_else(invalid)?);
        Ok(Self { mods: mods | chord.mods, key: chord.key })
    }
}

/// A command's chords: any of several alternatives ("mod+t, mod+n"), with other chords on
/// some platforms. `Keybind::new("mod+b").on(KeyPlatform::Windows, "alt+home")` is ⌘B on
/// macOS, Ctrl+B on Linux and Alt+Home on Windows. "" and "none" bind nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Keybind {
    chords: Vec<Chord>,
    platforms: Vec<(KeyPlatform, Vec<Chord>)>,
}

impl Keybind {
    /// A keybind from a literal in the app's code. Panics if it doesn't parse; parse user
    /// input (a settings file) with `str::parse` instead.
    #[track_caller]
    pub fn new(spec: &str) -> Self {
        spec.parse().unwrap_or_else(|e| panic!("{e}"))
    }

    /// These chords instead on `platform`. Panics if `spec` doesn't parse, as `new` does.
    #[track_caller]
    pub fn on(self, platform: KeyPlatform, spec: &str) -> Self {
        self.on_chords(platform, &parse_list(spec).unwrap_or_else(|e| panic!("{e}")))
    }

    /// `chords` instead on `platform`: `on` for chords built as data.
    pub fn on_chords(mut self, platform: KeyPlatform, chords: &[Chord]) -> Self {
        self.platforms.retain(|(p, _)| *p != platform);
        self.platforms.push((platform, chords.to_vec()));
        self
    }

    /// The chords that apply on `platform`.
    pub fn chords_on(&self, platform: KeyPlatform) -> &[Chord] {
        self.platforms.iter().find(|(p, _)| *p == platform).map_or(&self.chords, |(_, c)| c)
    }

    /// The chords that apply here.
    pub fn chords(&self) -> &[Chord] { self.chords_on(KeyPlatform::current()) }

    /// Whether `ev` is any of this platform's chords; see `Chord::matches`.
    pub fn matches(&self, ev: &KeyEvent) -> bool { self.matches_on(ev, KeyPlatform::current()) }

    /// Whether `ev` is any of this platform's chords; see `Chord::matches_tx`.
    pub fn matches_tx(&self, ev: &KeyEvent, tx: &crate::Tx) -> bool { self.matches_tx_on(ev, tx, KeyPlatform::current()) }

    /// Whether `ev` is any of `platform`'s chords; see `Chord::matches_on`.
    pub fn matches_on(&self, ev: &KeyEvent, platform: KeyPlatform) -> bool {
        self.chords_on(platform).iter().any(|c| c.matches_on(ev, platform))
    }

    /// Whether `ev` is any of `platform`'s chords; see `Chord::matches_tx_on`.
    pub fn matches_tx_on(&self, ev: &KeyEvent, tx: &crate::Tx, platform: KeyPlatform) -> bool {
        self.chords_on(platform).iter().any(|c| c.matches_tx_on(ev, tx, platform))
    }

    /// The first chord's label here ("⇧⌘T", "Ctrl+Shift+T"), or "" when nothing is bound.
    pub fn label(&self) -> String { self.label_on(KeyPlatform::current()) }

    /// The first chord's label on `platform`, or "" when nothing is bound there.
    pub fn label_on(&self, platform: KeyPlatform) -> String {
        self.chords_on(platform).first().map(|c| c.label_on(platform)).unwrap_or_default()
    }

    /// The string `parse` reads back as `platform`'s chords: "mod+t, mod+n", or "none".
    pub fn spec_on(&self, platform: KeyPlatform) -> String {
        let chords = self.chords_on(platform);
        if chords.is_empty() { return "none".into(); }
        chords.iter().map(Chord::to_string).collect::<Vec<_>>().join(", ")
    }
}

impl From<Chord> for Keybind {
    fn from(chord: Chord) -> Self { Self { chords: vec![chord], platforms: Vec::new() } }
}

impl From<&[Chord]> for Keybind {
    fn from(chords: &[Chord]) -> Self { Self { chords: chords.to_vec(), platforms: Vec::new() } }
}

impl FromStr for Keybind {
    type Err = ChordError;
    fn from_str(spec: &str) -> Result<Self, Self::Err> {
        Ok(Self { chords: parse_list(spec)?, platforms: Vec::new() })
    }
}

/// The chords that apply here, comma-separated (`spec_on` the current platform), so a
/// settings file written here reads back as what this platform binds.
impl fmt::Display for Keybind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.spec_on(KeyPlatform::current())) }
}

fn parse_list(spec: &str) -> Result<Vec<Chord>, ChordError> {
    let spec = spec.trim();
    if spec.is_empty() || spec.eq_ignore_ascii_case("none") { return Ok(Vec::new()); }
    spec.split(',').map(str::parse).collect()
}

/// Whether `m` holds the primary modifier and no other but Shift, or none at all (a script's
/// `Key::Command`): ⌘ alone on macOS, Ctrl alone elsewhere. For the edit commands a text
/// field takes (copy, paste, undo), which another modifier turns into an app's chord.
pub(crate) fn primary_alone(m: Mods) -> bool {
    matches!(held(m, KeyPlatform::current()) & !SHIFT, 0 | PRIMARY)
}

/// `bits` with the modifiers that are one key on `platform` merged: Ctrl into the primary
/// off macOS, the logo key into it on macOS.
fn fold(bits: u8, platform: KeyPlatform) -> u8 {
    let same = if platform.mac() { SUPER } else { CTRL };
    if bits & same != 0 { bits & !same | PRIMARY } else { bits }
}

/// The keys `m` says are held, as chord bits for `platform`. `cmd` repeats the key this build
/// reports as primary: ⌘ (`logo`) on macOS, Ctrl elsewhere (iOS too), Ctrl or ⌘ on the web. A `Mods`
/// with `cmd` and neither key (a script, a test) holds `platform`'s primary.
fn held(m: Mods, platform: KeyPlatform) -> u8 {
    let bare = m.cmd && !m.ctrl && !m.logo;
    let (ctrl, logo) = if bare || cfg!(target_arch = "wasm32") {
        (m.ctrl, m.logo)
    } else if cfg!(target_os = "macos") {
        (m.ctrl, m.logo || m.cmd)
    } else {
        (m.ctrl || m.cmd, m.logo)
    };
    let (ctrl_bit, logo_bit) = if platform.mac() { (CTRL, PRIMARY) } else { (PRIMARY, SUPER) };
    [(bare, PRIMARY), (ctrl, ctrl_bit), (logo, logo_bit), (m.alt, ALT), (m.shift, SHIFT)]
        .into_iter().filter(|(on, _)| *on).fold(0, |bits, (_, bit)| bits | bit)
}

fn key_named(name: &str) -> Option<ChordKey> {
    if let Some(key) = key_named_word(name) { return Some(key); }
    if let Some(c) = char_named(name) { return Some(ChordKey::Char(c)); }
    if let Some(n) = name.strip_prefix('f').filter(|n| !n.starts_with(['0', '+'])).and_then(|n| n.parse::<u8>().ok()) {
        return (1..=24).contains(&n).then_some(ChordKey::Function(n));
    }
    let mut chars = name.chars();
    match (chars.next(), chars.next()) { (Some(c), None) => Some(ChordKey::Char(lower(c))), _ => None }
}

/// The one character `key` types, lowercased.
fn typed(key: &Key) -> Option<char> {
    match key {
        Key::Command(c) => Some(lower(*c)),
        Key::Text(t) => {
            let mut chars = t.chars();
            match (chars.next(), chars.next()) { (Some(c), None) => Some(lower(c)), _ => None }
        }
        _ => None,
    }
}

/// `c` lowercased when that is one character.
pub(super) fn lower(c: char) -> char {
    let mut l = c.to_lowercase();
    match (l.next(), l.next()) { (Some(l), None) => l, _ => c }
}

/// The key that types `c` with Shift on a US layout.
fn us_unshifted(c: char) -> Option<char> {
    let (shifted, plain) = ("~!@#$%^&*()_+{}|:\"<>?", "`1234567890-=[]\\;',./");
    shifted.chars().position(|s| s == c).and_then(|i| plain.chars().nth(i))
}
