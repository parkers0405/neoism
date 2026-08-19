//! IME preedit state + shared composition decisions for native + web.
//!
//! Originally lifted from the desktop fork's `app/ime.rs`. The
//! [`Ime`] / [`Preedit`] structs hold the current preedit text plus
//! optional cursor offsets; the [`Preedit::new`] constructor derives
//! the end-offset from the byte offset via `unicode-width` so callers
//! get the visual cursor position for free.
//!
//! The free functions below ([`commit_dispatch`],
//! [`preedit_update_action`], [`should_drop_keys_during_compose`],
//! [`assistant_blocks_ime`], [`compose_start_action`],
//! [`compose_end_action`]) are the renderer-neutral decisions every
//! host must make when wiring IME. The web frontend forwards the
//! browser's `composition*` events through the same vocabulary the
//! desktop fork hands winit's [`Ime`] event off to, so both produce
//! identical chrome behavior:
//!
//! 1. IME enable/disable matches focus state of an editable surface.
//! 2. Preedit text drives the visible cursor glyph but is suppressed
//!    when the assistant overlay or any chrome modal owns the
//!    keyboard.
//! 3. Real key events are dropped while a preedit is in flight —
//!    "mode-locking during compose" — so Escape / Enter / arrow
//!    keystrokes the IME swallowed never leak to nvim or the
//!    terminal block input.
//! 4. The committed string is forwarded to nvim / pty via the same
//!    paste path as a real paste, switching on bracketed-paste when
//!    multiple chars land in one commit.
//! 5. Disabling IME (or compositionend) clears state via
//!    [`Ime::set_enabled(false)`].
//!
//! Stays free of winit / DOM / OS types — hosts hand raw strings +
//! byte offsets in, the state hands width-adjusted offsets back out.

use unicode_width::UnicodeWidthChar;

#[derive(Debug, Default)]
pub struct Ime {
    /// Whether the IME is enabled.
    enabled: bool,

    /// Current IME preedit.
    preedit: Option<Preedit>,
}

impl Ime {
    pub fn new() -> Self {
        Default::default()
    }

    #[inline]
    pub fn set_enabled(&mut self, is_enabled: bool) {
        if is_enabled {
            self.enabled = is_enabled;
        } else {
            // Clear state when disabling IME.
            *self = Default::default();
        }
    }

    #[inline]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    #[inline]
    pub fn set_preedit(&mut self, preedit: Option<Preedit>) {
        self.preedit = preedit;
    }

    #[inline]
    pub fn preedit(&self) -> Option<&Preedit> {
        self.preedit.as_ref()
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Preedit {
    /// The preedit text.
    pub text: String,

    /// Byte offset for cursor start into the preedit text.
    ///
    /// `None` means that the cursor is invisible.
    pub cursor_byte_offset: Option<usize>,

    /// The cursor offset from the end of the preedit in char width.
    pub cursor_end_offset: Option<usize>,
}

impl Preedit {
    pub fn new(text: String, cursor_byte_offset: Option<usize>) -> Self {
        let cursor_end_offset = if let Some(byte_offset) = cursor_byte_offset {
            // Convert byte offset into char offset. Clamp the offset
            // into the text range so a stale browser
            // `compositionupdate` cursor that lands past the new
            // (shorter) preedit doesn't panic the host on the
            // `text[byte_offset..]` slice.
            let clamped = byte_offset.min(text.len());
            let cursor_end_offset = text[clamped..]
                .chars()
                .fold(0, |acc, ch| acc + ch.width().unwrap_or(1));

            Some(cursor_end_offset)
        } else {
            None
        };

        Self {
            text,
            cursor_byte_offset,
            cursor_end_offset,
        }
    }
}

// ---------------------------------------------------------------------------
// Shared IME decisions (host-agnostic).
//
// These are the renderer-neutral rules both the desktop winit host and
// the web DOM host follow when translating native IME events into
// chrome / nvim state. Lifting them here keeps web + desktop in lock
// step — no more "the desktop fork drops keys during compose but the
// web frontend leaks them to nvim".
// ---------------------------------------------------------------------------

/// Threshold (in characters) above which an IME commit is forwarded to
/// the terminal via bracketed-paste rather than as raw keystrokes.
/// Single-character commits (the common case for Japanese / Chinese
/// typewriter input) go through the raw path so terminal modes that
/// care about per-key timing (vim insert mode, readline) see them as
/// individual events.
pub const COMMIT_BRACKETED_PASTE_MIN_CHARS: usize = 2;

/// Decision for routing an IME `Commit` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitDispatch {
    /// The text to forward to the focused input surface (nvim / pty
    /// messenger / terminal block input). Cloned out of the original
    /// commit event so callers can move it into a paste.
    pub text: String,
    /// Whether the host should wrap the text in bracketed-paste
    /// markers (`ESC [ 200 ~` … `ESC [ 201 ~`) when the active mode
    /// supports it.
    pub use_bracketed_paste: bool,
}

/// Pure classifier for an IME `Commit` event.
///
/// Mirrors `frontends/.../desktop/.../app/window_event/ime.rs`'s
/// `Ime::Commit` arm: forward the committed string via the same paste
/// path used for system clipboard pastes, switching on bracketed
/// paste only when multiple chars land in one commit (so single-char
/// commits behave like real keystrokes in vim insert mode).
#[inline]
pub fn commit_dispatch(text: &str) -> CommitDispatch {
    let char_count = text.chars().count();
    CommitDispatch {
        text: text.to_string(),
        use_bracketed_paste: char_count >= COMMIT_BRACKETED_PASTE_MIN_CHARS,
    }
}

/// Decision for an IME `Preedit` / `compositionupdate` event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreeditUpdateAction {
    /// Replace the current preedit with the new value (which may be
    /// `None` when the host fed an empty preedit string — equivalent
    /// to clearing in the desktop fork).
    Replace(Option<Preedit>),
    /// The incoming payload matches what we already display; the host
    /// should skip the state write and the redraw request to avoid
    /// the per-frame flicker the desktop fork already guards against.
    NoChange,
}

/// Pure classifier for a `Preedit(text, cursor_byte_offset)` event.
///
/// Mirrors `Ime::Preedit` in `app/window_event/ime.rs`:
/// 1. Empty text -> clear preedit.
/// 2. Non-empty text + cursor offset -> build a new [`Preedit`].
/// 3. Compare with the existing preedit; suppress the redraw when
///    nothing actually changed.
pub fn preedit_update_action(
    existing: Option<&Preedit>,
    text: String,
    cursor_byte_offset: Option<usize>,
) -> PreeditUpdateAction {
    let incoming = if text.is_empty() {
        None
    } else {
        Some(Preedit::new(text, cursor_byte_offset))
    };
    if existing == incoming.as_ref() {
        PreeditUpdateAction::NoChange
    } else {
        PreeditUpdateAction::Replace(incoming)
    }
}

/// Pure classifier for key events while a preedit is in flight.
///
/// While the IME is showing a preedit popup, every keystroke (Enter
/// to commit, Escape to cancel, arrows to navigate the candidate
/// list) belongs to the IME, not the underlying nvim / terminal
/// surface. The desktop fork drops these keys in
/// `Screen::process_key_event`; the web frontend must do the same
/// (browser `keydown` events fire alongside `compositionupdate` with
/// `KeyboardEvent.isComposing === true`).
///
/// Returns `true` when the host should swallow the key event.
#[inline]
pub fn should_drop_keys_during_compose(has_preedit: bool) -> bool {
    has_preedit
}

/// Pure classifier for whether the assistant overlay swallows IME.
///
/// When the assistant overlay is the active modal it owns the
/// keyboard — IME events should fall on the overlay's input, not on
/// the focused editor / terminal surface below. Mirrors the early
/// return in `Application::handle_ime` when
/// `route.window.screen.renderer.assistant.is_active()`.
///
/// Returns `true` when the host should ignore the IME event entirely
/// (no preedit display, no commit forwarding).
#[inline]
pub fn assistant_blocks_ime(assistant_active: bool) -> bool {
    assistant_active
}

/// Pure classifier for whether a host `keydown` was fired by the IME
/// mid-composition and must be swallowed. Combines the standard
/// `KeyboardEvent.isComposing` flag (Chromium / Firefox / Safari /
/// WKWebView) with the legacy `keyCode === 229` fallback some
/// IBus/fcitx + Chromium combos still emit for the final commit
/// cycle. Hosts with no legacy key-code concept (winit) pass `0`.
///
/// Returns `true` when the host should swallow the key event.
#[inline]
pub fn key_event_is_ime_composing(is_composing: bool, key_code: u32) -> bool {
    is_composing || key_code == 229
}

/// Decision for a `compositionstart` / `Ime::Enabled` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComposeStartAction {
    /// Whether the host should toggle [`Ime::set_enabled(true)`].
    /// Always `true` for now (the desktop fork unconditionally
    /// enables on `Ime::Enabled`); kept as a struct so future
    /// extensions (e.g. suppress when the assistant overlay is open)
    /// don't churn callsites.
    pub mark_enabled: bool,
}

/// Pure classifier for `compositionstart` / `Ime::Enabled`.
#[inline]
pub fn compose_start_action() -> ComposeStartAction {
    ComposeStartAction { mark_enabled: true }
}

/// Decision for a `compositionend` / `Ime::Disabled` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComposeEndAction {
    /// Whether the host should toggle [`Ime::set_enabled(false)`] —
    /// which also clears the preedit via the `Default` reset in
    /// [`Ime::set_enabled`].
    pub mark_disabled: bool,
}

/// Pure classifier for `compositionend` / `Ime::Disabled`.
#[inline]
pub fn compose_end_action() -> ComposeEndAction {
    ComposeEndAction {
        mark_disabled: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabling_ime_clears_preedit() {
        let mut ime = Ime::new();
        ime.set_enabled(true);
        ime.set_preedit(Some(Preedit::new("abc".to_string(), Some(0))));
        ime.set_enabled(false);
        assert!(!ime.is_enabled());
        assert!(ime.preedit().is_none());
    }

    #[test]
    fn preedit_derives_end_offset_from_byte_offset() {
        let p = Preedit::new("ab".to_string(), Some(1));
        assert_eq!(p.cursor_end_offset, Some(1));
    }

    #[test]
    fn preedit_skips_offset_when_byte_offset_none() {
        let p = Preedit::new("ab".to_string(), None);
        assert_eq!(p.cursor_end_offset, None);
    }

    #[test]
    fn preedit_clamps_stale_byte_offset_into_range() {
        // A stale `compositionupdate` cursor at byte offset 10 on a
        // 3-byte preedit must not panic. The clamp pulls it back to
        // `text.len()`, giving an end-offset of 0.
        let p = Preedit::new("abc".to_string(), Some(10));
        assert_eq!(p.cursor_end_offset, Some(0));
    }

    #[test]
    fn preedit_width_handles_wide_chars() {
        // CJK fullwidth chars report width 2 each via unicode-width.
        let p = Preedit::new("あい".to_string(), Some(0));
        assert_eq!(p.cursor_end_offset, Some(4));
    }

    #[test]
    fn commit_dispatch_uses_raw_for_single_char() {
        let d = commit_dispatch("a");
        assert_eq!(d.text, "a");
        assert!(!d.use_bracketed_paste);
    }

    #[test]
    fn commit_dispatch_uses_raw_for_single_wide_char() {
        // One CJK char is still one char even though it's 3 bytes
        // UTF-8 — the threshold gates on `chars().count()`, not byte
        // length, so vim insert mode sees it as a single keystroke.
        let d = commit_dispatch("あ");
        assert_eq!(d.text, "あ");
        assert!(!d.use_bracketed_paste);
    }

    #[test]
    fn commit_dispatch_uses_bracketed_for_multi_char() {
        let d = commit_dispatch("ab");
        assert!(d.use_bracketed_paste);
    }

    #[test]
    fn commit_dispatch_uses_bracketed_for_multi_cjk() {
        let d = commit_dispatch("こんにちは");
        assert!(d.use_bracketed_paste);
    }

    #[test]
    fn commit_dispatch_empty_string_stays_raw() {
        // Edge case: empty commit (some IMEs fire a 0-length commit
        // on cancel). 0 < threshold, so no bracketed paste.
        let d = commit_dispatch("");
        assert!(!d.use_bracketed_paste);
    }

    #[test]
    fn preedit_update_replace_when_empty_clears() {
        let prev = Preedit::new("abc".to_string(), Some(0));
        let action = preedit_update_action(Some(&prev), String::new(), None);
        assert_eq!(action, PreeditUpdateAction::Replace(None));
    }

    #[test]
    fn preedit_update_replace_when_text_changes() {
        let prev = Preedit::new("a".to_string(), Some(0));
        let action = preedit_update_action(Some(&prev), "ab".to_string(), Some(2));
        match action {
            PreeditUpdateAction::Replace(Some(p)) => {
                assert_eq!(p.text, "ab");
                assert_eq!(p.cursor_byte_offset, Some(2));
            }
            other => panic!("expected Replace(Some), got {other:?}"),
        }
    }

    #[test]
    fn preedit_update_no_change_when_identical() {
        let prev = Preedit::new("abc".to_string(), Some(1));
        let action = preedit_update_action(Some(&prev), "abc".to_string(), Some(1));
        assert_eq!(action, PreeditUpdateAction::NoChange);
    }

    #[test]
    fn preedit_update_no_change_when_both_empty() {
        let action = preedit_update_action(None, String::new(), None);
        assert_eq!(action, PreeditUpdateAction::NoChange);
    }

    #[test]
    fn preedit_update_replace_when_cursor_offset_only_changes() {
        // Same text, different cursor — the visible caret moves so
        // the host must redraw.
        let prev = Preedit::new("abc".to_string(), Some(1));
        let action = preedit_update_action(Some(&prev), "abc".to_string(), Some(2));
        assert!(matches!(action, PreeditUpdateAction::Replace(Some(_))));
    }

    #[test]
    fn keys_are_dropped_during_compose() {
        assert!(should_drop_keys_during_compose(true));
        assert!(!should_drop_keys_during_compose(false));
    }

    #[test]
    fn assistant_blocks_ime_when_active() {
        assert!(assistant_blocks_ime(true));
        assert!(!assistant_blocks_ime(false));
    }

    #[test]
    fn key_event_is_composing_covers_flag_and_legacy_code() {
        assert!(key_event_is_ime_composing(true, 0));
        assert!(key_event_is_ime_composing(false, 229));
        assert!(key_event_is_ime_composing(true, 229));
        assert!(!key_event_is_ime_composing(false, 0));
        assert!(!key_event_is_ime_composing(false, 65));
    }

    #[test]
    fn compose_start_marks_enabled() {
        let action = compose_start_action();
        assert!(action.mark_enabled);
    }

    #[test]
    fn compose_end_marks_disabled() {
        let action = compose_end_action();
        assert!(action.mark_disabled);
    }
}
