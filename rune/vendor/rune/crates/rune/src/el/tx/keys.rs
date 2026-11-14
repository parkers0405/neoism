//! `Tx`'s reads about the key a key handler is seeing: repeat, physical key, unmodified
//! character, modifiers and the focused element.

use crate::Tx;

impl Tx {
    /// Whether the key a key handler is seeing is the OS auto-repeat of a held key: true for
    /// each repeat, false for the first press and outside key handlers. One-shot actions
    /// (start recording on Enter, say) can return early on a repeat; arrows and Tab usually
    /// want every repeat.
    pub fn key_repeat(&self) -> bool { self.key_repeat }

    /// In a key handler, the physical key behind the logical key it's seeing: where the key
    /// sits, whatever the keyboard layout and Shift make it type. `KeyEvent` carries the
    /// character, which moves between layouts once Shift is held (`⌘⇧7` arrives as
    /// `Key::Command('&')` on a US layout and `Key::Command('/')` on a German one), so a shifted
    /// shortcut matches this instead: `tx.physical_key() == Some(PhysicalKeyCode::Digit7)`.
    /// `None` for a key with no physical origin (`Runtime::key`, a script's `"key"` without a
    /// `"code"`) and outside key handlers.
    pub fn physical_key(&self) -> Option<crate::PhysicalKeyCode> { self.physical_key }

    /// In a key handler, the character the key types under the current keyboard layout with
    /// no modifiers held: `'a'` for ⌥A (which arrives as `Key::Text("å")` on a US Mac layout)
    /// and ⇧A, and `'a'` for AZERTY's A key, which sits where QWERTY's Q does. A binding by
    /// letter, as macOS's `DefaultKeyBinding.dict` binds ⌥-letters, matches this rather than
    /// [`Tx::physical_key`], which names where the key sits (`KeyQ` there). `None` off
    /// macOS, Windows and Linux, for a key that types no single character (arrows, Return),
    /// for a key with no physical origin (`Runtime::key`, a script's `"key"` without a
    /// `"code"`) and outside key handlers.
    pub fn key_base(&self) -> Option<char> { self.key_base }

    /// The modifier keys held as this handler runs, so a click can tell Shift-click (extend a
    /// list's selection) and Cmd-click (toggle a row) from a plain one. A key handler reads
    /// the key's own [`KeyEvent::mods`](crate::KeyEvent) instead.
    pub fn mods(&self) -> crate::Mods { self.mods }

    /// When the input this handler runs for arrived, to measure input-to-photon latency
    /// against [`FrameEncoded`](crate::FrameEncoded): on macOS the timestamp of the `NSEvent`
    /// AppKit dispatched it in, elsewhere when the window took it from winit. Input that piles
    /// up between frames keeps each event's own time. `None` in handlers no input caused
    /// (timers, polls, updates, layout callbacks) and for input fed to a `Runtime` directly
    /// (`Runtime::key` and kin, headless scripts).
    pub fn input_time(&self) -> Option<crate::time::Instant> { self.input_time }

    /// In a key handler, the key of the element that had focus when the key arrived, `None`
    /// when nothing had (and in every other handler). An unkeyed element reads as `#n`, its
    /// index among its siblings, and a focused [`Link`](crate::Link) as its id. A window's `on_key` hears
    /// a key either before anything else (nothing focused) or after the focused control passed
    /// it on, so `tx.focused().is_none()` picks out the first case: plain arrows can drive the
    /// window (a settings pane) only while no control has focus, as in System Settings. The
    /// whole order is under "Key routing" in the rune README.
    pub fn focused(&self) -> Option<&str> {
        self.focused.as_deref()
    }
}
