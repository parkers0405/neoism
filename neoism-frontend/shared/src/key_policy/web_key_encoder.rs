//! Web (DOM) → PTY key-byte encoder mirroring the desktop pipeline.
//!
//! Desktop (GOLDEN STANDARD) turns a pressed key into PTY bytes in
//! `Screen::process_key_event` (desktop `screen/selection/key_event.rs`)
//! through three stages:
//!
//! 1. **Pre-byte dispatch + bindings table** — chrome/tab/workspace
//!    shortcuts and the `bindings/defaults.rs` table run first. A
//!    matching `Action::Esc(..)` binding writes its literal bytes; any
//!    other matching action consumes the key with NO bytes.
//!    This is where DECCKM app-cursor SS3 sequences (`ESC O A` …),
//!    plain-arrow CSI, `ESC [2~`-style tildes, the Backspace family,
//!    F1–F4 SS3 and Shift+Tab live.
//! 2. **Alt masking** — `Screen::alt_send_esc` + the shared
//!    [`crate::key_policy::mask_alt_for_output`] decide whether ALT
//!    stays in the modifier set (alt-as-meta ESC prefix).
//! 3. **Output shape** — the shared
//!    [`crate::selection_input::should_build_key_sequence`] predicate
//!    forks between the kitty/CSI builder
//!    ([`crate::key_policy::kitty_sequence::build`], the exact port of
//!    desktop `input/kitty_keyboard.rs::build_key_sequence`) and the
//!    raw-UTF-8 path
//!    ([`crate::selection_input::build_non_kitty_terminal_bytes`]).
//!
//! This module reproduces that decision path from a DOM
//! `KeyboardEvent` POD (`key` / `code` / modifier booleans / `repeat`)
//! plus the live terminal mode bits, so the web frontend's outbound
//! terminal stream is byte-for-byte identical to desktop. The wasm
//! bridge exposes it as `ChromeBridge::encode_terminal_key`.
//!
//! Platform notes (the web host behaves like the desktop Linux build):
//!
//! * `alt_send_esc` uses the non-macOS branch: ALT always means meta.
//! * `text_with_all_modifiers` does not exist in the DOM; it is
//!   synthesized with the standard xterm control-character mapping
//!   (Ctrl+A → 0x01, Ctrl+Space/@ → 0x00, Ctrl+[ → 0x1b, …), which is
//!   what winit reports on X11/Wayland.
//! * Left/right modifier location: the shared kitty builder collapses
//!   winit's `KeyLocation::Left` check into its `numpad_location`
//!   flag (`false` selects the Left kitty code). We pass
//!   `!code.ends_with("Left")` for pure modifier keys so
//!   `ShiftRight` → 57447 exactly like desktop.

use crate::key_policy::kitty_sequence::{
    build as build_kitty_sequence, KittyKeyEvent, KittyKeyName, KittyKeyState,
    KittyLogicalKey, SequenceModifiers,
};
use crate::key_policy::{physical_key_binding_char, PhysicalKeyCode};
use crate::selection_input::{
    build_non_kitty_terminal_bytes, should_build_key_sequence, KeySequenceShapeInput,
    OutputLogicalKey,
};

/// POD snapshot of the DOM `KeyboardEvent` fields the encoder needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WebTerminalKeyInput<'a> {
    /// `KeyboardEvent.key` — logical key ("a", "!", "ArrowUp", "F5", …).
    pub key: &'a str,
    /// `KeyboardEvent.code` — physical key ("KeyA", "Numpad5", …).
    pub code: &'a str,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// `KeyboardEvent.metaKey` (Super / Cmd).
    pub super_key: bool,
    /// `KeyboardEvent.repeat` — feeds kitty event-type `:2` reports.
    pub repeat: bool,
}

/// Live terminal mode bits the encoding decisions read.
///
/// Hosts derive these from `crosswords::Mode` (desktop `Screen::get_mode`,
/// wasm `terminal.inner.mode()`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WebTerminalKeyModes {
    /// DECCKM (`Mode::APP_CURSOR`): arrows/Home/End send SS3.
    pub app_cursor: bool,
    /// `Mode::ALT_SCREEN`: Shift+Home/End/PgUp/PgDn stop scrolling the
    /// host viewport and fall through to the shift-modified escapes.
    pub alt_screen: bool,
    /// `Mode::VI`: desktop's mid-dispatch consumes every remaining key
    /// (`MidKeyDispatchAction::ConsumeViMode`) — nothing reaches the PTY.
    pub vi: bool,
    /// Kitty `Mode::DISAMBIGUATE_ESC_CODES`.
    pub disambiguate_esc_codes: bool,
    /// Kitty `Mode::REPORT_EVENT_TYPES`.
    pub report_event_types: bool,
    /// Kitty `Mode::REPORT_ALTERNATE_KEYS`.
    pub report_alternate_keys: bool,
    /// Kitty `Mode::REPORT_ALL_KEYS_AS_ESC`.
    pub report_all_keys_as_esc: bool,
    /// Kitty `Mode::REPORT_ASSOCIATED_TEXT`.
    pub report_associated_text: bool,
}

/// Encode one pressed (or repeated) key into the PTY byte sequence.
///
/// Returns an empty `Vec` when the key produces no PTY bytes in the
/// current mode — either because it has no terminal representation or
/// because the desktop pipeline consumes it host-side (scrollback
/// paging, tab management, chrome focus, copy/paste, font size, …).
/// Key RELEASES are not encoded (the web host only forwards keydown;
/// desktop's release path is gated on `REPORT_EVENT_TYPES` which the
/// host must wire separately before releases matter).
pub fn encode_web_terminal_key(
    input: &WebTerminalKeyInput<'_>,
    modes: &WebTerminalKeyModes,
) -> Vec<u8> {
    // Desktop: `mid_key_event_dispatch` → `ConsumeViMode` swallows every
    // key that reaches the byte stage while vi mode is active.
    if modes.vi {
        return Vec::new();
    }

    let logical = classify_dom_key(input.key);

    if let Some(bytes) = binding_stage(input, modes, &logical) {
        return bytes;
    }

    // ---- Stage 2/3: alt masking + output shape (desktop key_event.rs
    // lines 621-656). ----
    let text = synthesized_text(input, &logical);

    // `Screen::alt_send_esc`, non-macOS branch: for named keys ALT is
    // always meaningful; for character keys only when the event text is
    // a single character (multi-char/dead-key events scrub ALT).
    let alt_send_esc = match &logical {
        WebLogicalKey::Named(_) => input.alt,
        _ => input.alt && text.chars().count() == 1,
    };
    let alt = input.alt && alt_send_esc;

    let mods_empty = !input.shift && !input.ctrl && !alt && !input.super_key;
    let mods_shift_only = input.shift && !input.ctrl && !alt && !input.super_key;
    let key_on_numpad = input.code.starts_with("Numpad");

    let key_tag = output_logical_key_tag(&logical);
    let build_sequence = should_build_key_sequence(KeySequenceShapeInput {
        key: key_tag,
        key_on_numpad,
        text_empty: text.is_empty(),
        mods_empty,
        mods_shift_only,
        report_all_keys_as_esc: modes.report_all_keys_as_esc,
        disambiguate_esc_codes: modes.disambiguate_esc_codes,
    });

    if !build_sequence {
        return build_non_kitty_terminal_bytes(&text, alt);
    }

    let mut sequence_mods = SequenceModifiers::empty();
    sequence_mods.set(SequenceModifiers::SHIFT, input.shift);
    sequence_mods.set(SequenceModifiers::ALT, alt);
    sequence_mods.set(SequenceModifiers::CONTROL, input.ctrl);
    sequence_mods.set(SequenceModifiers::SUPER, input.super_key);

    let kitty_logical = match &logical {
        WebLogicalKey::Named(named) => KittyLogicalKey::Named(*named),
        WebLogicalKey::Character(ch) => KittyLogicalKey::Character {
            text: ch.to_string(),
            base: dom_code_base_char(input.code),
        },
        WebLogicalKey::Unidentified => KittyLogicalKey::Unidentified,
    };
    // The shared builder folds desktop's `KeyLocation::Left` distinction
    // for pure modifier keys into `numpad_location == false`; feed it
    // `true` for right/standard-side modifiers so ShiftRight & co. get
    // the desktop non-left kitty codes (57447…).
    let numpad_location = key_on_numpad
        || (is_modifier_named(&logical) && !input.code.ends_with("Left"));

    let event = KittyKeyEvent {
        logical_key: kitty_logical,
        state: KittyKeyState::Pressed,
        repeat: input.repeat,
        numpad_location,
        text_with_all_modifiers: if text.is_empty() {
            None
        } else {
            Some(text)
        },
    };

    build_kitty_sequence(
        &event,
        sequence_mods,
        modes.report_all_keys_as_esc,
        modes.disambiguate_esc_codes,
        modes.report_event_types,
        modes.report_alternate_keys,
        modes.report_associated_text,
    )
}

// ---------------------------------------------------------------------------
// DOM key classification
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
enum WebLogicalKey {
    Named(KittyKeyName),
    Character(char),
    /// Dead keys, IME process keys, unmapped named keys.
    Unidentified,
}

fn classify_dom_key(key: &str) -> WebLogicalKey {
    if let Some(named) = dom_named_key(key) {
        return WebLogicalKey::Named(named);
    }
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) => WebLogicalKey::Character(ch),
        _ => WebLogicalKey::Unidentified,
    }
}

/// Map a DOM `KeyboardEvent.key` name to the kitty named-key enum.
/// Mirrors winit's `NamedKey` coverage for keys the encoder can emit.
fn dom_named_key(key: &str) -> Option<KittyKeyName> {
    use KittyKeyName::*;
    Some(match key {
        // DOM reports the space bar as the literal " " character, but
        // desktop sees winit `Named(Space)` with text " ".
        " " => Space,
        "Enter" => Enter,
        "Escape" => Escape,
        "Tab" => Tab,
        "Backspace" => Backspace,
        "Delete" => Delete,
        "Insert" => Insert,
        "Home" => Home,
        "End" => End,
        "PageUp" => PageUp,
        "PageDown" => PageDown,
        "ArrowUp" => ArrowUp,
        "ArrowDown" => ArrowDown,
        "ArrowLeft" => ArrowLeft,
        "ArrowRight" => ArrowRight,
        "F1" => F1,
        "F2" => F2,
        "F3" => F3,
        "F4" => F4,
        "F5" => F5,
        "F6" => F6,
        "F7" => F7,
        "F8" => F8,
        "F9" => F9,
        "F10" => F10,
        "F11" => F11,
        "F12" => F12,
        "F13" => F13,
        "F14" => F14,
        "F15" => F15,
        "F16" => F16,
        "F17" => F17,
        "F18" => F18,
        "F19" => F19,
        "F20" => F20,
        "F21" => F21,
        "F22" => F22,
        "F23" => F23,
        "F24" => F24,
        "F25" => F25,
        "F26" => F26,
        "F27" => F27,
        "F28" => F28,
        "F29" => F29,
        "F30" => F30,
        "F31" => F31,
        "F32" => F32,
        "F33" => F33,
        "F34" => F34,
        "F35" => F35,
        "CapsLock" => CapsLock,
        "NumLock" => NumLock,
        "ScrollLock" => ScrollLock,
        "PrintScreen" => PrintScreen,
        "Pause" => Pause,
        "ContextMenu" => ContextMenu,
        "Shift" => Shift,
        "Control" => Control,
        "Alt" => Alt,
        "Meta" | "Super" | "OS" => Super,
        "Hyper" => Hyper,
        "MediaPlay" => MediaPlay,
        "MediaPause" => MediaPause,
        "MediaPlayPause" => MediaPlayPause,
        "MediaStop" => MediaStop,
        "MediaFastForward" => MediaFastForward,
        "MediaRewind" => MediaRewind,
        "MediaTrackNext" => MediaTrackNext,
        "MediaTrackPrevious" => MediaTrackPrevious,
        "MediaRecord" => MediaRecord,
        "AudioVolumeDown" => AudioVolumeDown,
        "AudioVolumeUp" => AudioVolumeUp,
        "AudioVolumeMute" => AudioVolumeMute,
        _ => return None,
    })
}

fn is_modifier_named(logical: &WebLogicalKey) -> bool {
    matches!(
        logical,
        WebLogicalKey::Named(
            KittyKeyName::Shift
                | KittyKeyName::Control
                | KittyKeyName::Alt
                | KittyKeyName::Super
                | KittyKeyName::Hyper
                | KittyKeyName::Meta
        )
    )
}

/// The `OutputLogicalKey` tag `Screen::should_build_sequence` derives
/// from a winit key. Space is the only named key left whose
/// `to_text()` is `Some(_)` once Enter/Tab/Backspace/Escape have their
/// dedicated tags.
fn output_logical_key_tag(logical: &WebLogicalKey) -> OutputLogicalKey {
    match logical {
        WebLogicalKey::Named(KittyKeyName::Escape) => OutputLogicalKey::Escape,
        WebLogicalKey::Named(KittyKeyName::Tab) => OutputLogicalKey::Tab,
        WebLogicalKey::Named(KittyKeyName::Enter) => OutputLogicalKey::Enter,
        WebLogicalKey::Named(KittyKeyName::Backspace) => OutputLogicalKey::Backspace,
        WebLogicalKey::Named(KittyKeyName::Space) => OutputLogicalKey::NamedWithText,
        WebLogicalKey::Named(_) => OutputLogicalKey::NamedWithoutText,
        _ => OutputLogicalKey::NonNamed,
    }
}

// ---------------------------------------------------------------------------
// text_with_all_modifiers synthesis
// ---------------------------------------------------------------------------

/// Synthesize the winit `text_with_all_modifiers()` payload desktop
/// reads (`Screen::text_for_key_event`). On X11/Wayland winit reports
/// the control-mapped character when Ctrl is held; named keys report
/// their `NamedKey::to_text()` (Enter `\r`, Tab `\t`, Space ` `,
/// Backspace `\x08`, Escape `\x1b`), Ctrl+Space maps to NUL.
fn synthesized_text(input: &WebTerminalKeyInput<'_>, logical: &WebLogicalKey) -> String {
    match logical {
        WebLogicalKey::Named(KittyKeyName::Enter) => "\r".to_string(),
        WebLogicalKey::Named(KittyKeyName::Tab) => "\t".to_string(),
        WebLogicalKey::Named(KittyKeyName::Escape) => "\x1b".to_string(),
        WebLogicalKey::Named(KittyKeyName::Backspace) => "\x08".to_string(),
        WebLogicalKey::Named(KittyKeyName::Space) => {
            if input.ctrl {
                "\0".to_string()
            } else {
                " ".to_string()
            }
        }
        WebLogicalKey::Named(_) => String::new(),
        WebLogicalKey::Character(ch) => {
            if input.ctrl {
                control_mapped_char(*ch).unwrap_or(*ch).to_string()
            } else {
                ch.to_string()
            }
        }
        WebLogicalKey::Unidentified => String::new(),
    }
}

/// Standard xterm Ctrl+key → control character mapping.
fn control_mapped_char(ch: char) -> Option<char> {
    Some(match ch {
        'a'..='z' => ((ch as u8 - b'a') + 1) as char,
        'A'..='Z' => ((ch as u8 - b'A') + 1) as char,
        ' ' | '@' | '2' => '\0',
        '[' | '3' => '\x1b',
        '\\' | '4' => '\x1c',
        ']' | '5' => '\x1d',
        '^' | '6' => '\x1e',
        '_' | '7' | '/' => '\x1f',
        '8' | '?' => '\x7f',
        _ => return None,
    })
}

/// Base (unshifted) character for a DOM physical `code`, feeding the
/// kitty alternate-key report (`1` for the `!` key). Reuses the shared
/// physical-key table desktop's binding matcher uses.
fn dom_code_base_char(code: &str) -> Option<String> {
    physical_key_binding_char(dom_code_to_physical(code)).map(str::to_string)
}

fn dom_code_to_physical(code: &str) -> PhysicalKeyCode {
    use PhysicalKeyCode::*;
    match code {
        "KeyA" => KeyA,
        "KeyB" => KeyB,
        "KeyC" => KeyC,
        "KeyD" => KeyD,
        "KeyE" => KeyE,
        "KeyF" => KeyF,
        "KeyG" => KeyG,
        "KeyH" => KeyH,
        "KeyI" => KeyI,
        "KeyJ" => KeyJ,
        "KeyK" => KeyK,
        "KeyL" => KeyL,
        "KeyM" => KeyM,
        "KeyN" => KeyN,
        "KeyO" => KeyO,
        "KeyP" => KeyP,
        "KeyQ" => KeyQ,
        "KeyR" => KeyR,
        "KeyS" => KeyS,
        "KeyT" => KeyT,
        "KeyU" => KeyU,
        "KeyV" => KeyV,
        "KeyW" => KeyW,
        "KeyX" => KeyX,
        "KeyY" => KeyY,
        "KeyZ" => KeyZ,
        "Digit0" => Digit0,
        "Digit1" => Digit1,
        "Digit2" => Digit2,
        "Digit3" => Digit3,
        "Digit4" => Digit4,
        "Digit5" => Digit5,
        "Digit6" => Digit6,
        "Digit7" => Digit7,
        "Digit8" => Digit8,
        "Digit9" => Digit9,
        "Numpad0" => Numpad0,
        "Numpad1" => Numpad1,
        "Numpad2" => Numpad2,
        "Numpad3" => Numpad3,
        "Numpad4" => Numpad4,
        "Numpad5" => Numpad5,
        "Numpad6" => Numpad6,
        "Numpad7" => Numpad7,
        "Numpad8" => Numpad8,
        "Numpad9" => Numpad9,
        "Backquote" => Backquote,
        "Backslash" => Backslash,
        "IntlBackslash" => IntlBackslash,
        "IntlRo" => IntlRo,
        "BracketLeft" => BracketLeft,
        "BracketRight" => BracketRight,
        "Comma" => Comma,
        "Equal" => Equal,
        "NumpadAdd" => NumpadAdd,
        "Minus" => Minus,
        "NumpadSubtract" => NumpadSubtract,
        "Period" => Period,
        "Quote" => Quote,
        "Semicolon" => Semicolon,
        "Slash" => Slash,
        "NumpadDivide" => NumpadDivide,
        "Space" => Space,
        _ => Unknown,
    }
}

// ---------------------------------------------------------------------------
// Stage 1: pre-byte dispatch + default-bindings table
// ---------------------------------------------------------------------------

/// Desktop's pre-byte pipeline: chrome/tab/workspace dispatch, then the
/// `bindings/defaults.rs` + `bindings/platform/linux.rs` tables.
///
/// Returns:
/// * `Some(bytes)`  — an `Action::Esc(..)` binding matched: literal bytes.
/// * `Some(empty)`  — a host-side action consumed the key: NO bytes.
/// * `None`         — fall through to the alt-mask + output-shape stage.
///
/// Binding modifier matching is EXACT (`Binding::is_triggered_by`
/// compares `self.mods == mods`), so each arm checks the full modifier
/// tuple. `search`/`hint` states don't exist on the web host, so
/// `~SEARCH` guards are always satisfied.
fn binding_stage(
    input: &WebTerminalKeyInput<'_>,
    modes: &WebTerminalKeyModes,
    logical: &WebLogicalKey,
) -> Option<Vec<u8>> {
    let WebTerminalKeyInput {
        ctrl,
        alt,
        shift,
        super_key,
        ..
    } = *input;

    let none = !shift && !ctrl && !alt && !super_key;
    let shift_only = shift && !ctrl && !alt && !super_key;
    let ctrl_only = ctrl && !shift && !alt && !super_key;
    let alt_only = alt && !shift && !ctrl && !super_key;
    let super_only = super_key && !shift && !ctrl && !alt;
    let ctrl_shift = ctrl && shift && !alt && !super_key;
    let alt_shift = alt && shift && !ctrl && !super_key;
    let ctrl_alt = ctrl && alt && !shift && !super_key;
    let ctrl_shift_alt = ctrl && shift && alt && !super_key;

    let consumed = || Some(Vec::new());
    let esc = |s: &str| Some(s.as_bytes().to_vec());
    let lower_char = match logical {
        WebLogicalKey::Character(ch) => Some(ch.to_ascii_lowercase()),
        _ => None,
    };

    use KittyKeyName as N;
    let named = match logical {
        WebLogicalKey::Named(named) => Some(*named),
        _ => None,
    };
    let is_arrow = matches!(
        named,
        Some(N::ArrowUp | N::ArrowDown | N::ArrowLeft | N::ArrowRight)
    );

    // --- Font size (`Screen::font_size_action_for_key` →
    // `font_size_action_decide`): Ctrl-or-Super (never Alt) + =/+/-/0. ---
    let zoom_modifier =
        (ctrl && !alt && !super_key) || (super_key && !ctrl && !alt);
    if zoom_modifier {
        let key_is = |needle: char| lower_char == Some(needle);
        let code_is = |needle: &str| input.code == needle;
        if key_is('=')
            || key_is('+')
            || code_is("Equal")
            || code_is("NumpadAdd")
        {
            return consumed();
        }
        if key_is('-')
            || code_is("NumpadSubtract")
            || (!shift && code_is("Minus"))
        {
            return consumed();
        }
        if key_is('0') || (!shift && (code_is("Digit0") || code_is("Numpad0"))) {
            return consumed();
        }
    }

    // --- Buffer-tab strip focus (`handle_buffer_tab_focus_key`):
    // the Alt+Up and Alt+Down branches both end in an unconditional
    // `return true` (close_focus.rs:416-546), so desktop ALWAYS
    // consumes them. ---
    if alt_only && matches!(named, Some(N::ArrowUp | N::ArrowDown)) {
        return consumed();
    }

    // --- Chrome focus / resize (`is_chrome_focus_key` /
    // `is_chrome_resize_key` via `early_key_event_dispatch`). ---
    if alt_only && matches!(named, Some(N::ArrowLeft | N::ArrowRight)) {
        return consumed();
    }
    if ctrl_alt && is_arrow {
        return consumed();
    }

    // --- Ctrl+Shift tab management (consumed on press AND release
    // before the terminal byte path, key_event.rs:185-221). ---
    if ctrl_shift
        && (lower_char == Some('t')
            || lower_char == Some('w')
            || matches!(named, Some(N::ArrowLeft | N::ArrowRight)))
    {
        return consumed();
    }

    // --- Workspace/early dispatch (key_event.rs:231-314). ---
    if named == Some(N::Tab) && ctrl && !alt && !super_key {
        return consumed(); // top-level workspace tab switch (any shift)
    }
    if named == Some(N::Tab) && alt && !ctrl && !super_key {
        return consumed(); // workspace buffer tab switch (any shift)
    }
    if named == Some(N::Insert) && ctrl_only {
        return consumed(); // Ctrl+Insert → copy (Hyprland Super+C leak guard)
    }
    if named == Some(N::Insert) && shift_only {
        return consumed(); // Shift+Insert → paste
    }
    if ((alt && !super_key) || (super_key && !alt))
        && shift
        && !ctrl
        && lower_char == Some('t')
    {
        return consumed(); // move active tab into the split stack
    }

    // --- `process_key_bindings` prelude (key_bindings.rs:78-213). ---
    if super_only && matches!(lower_char, Some('c') | Some('v')) {
        return consumed(); // Super copy / paste
    }
    if alt_only && lower_char.is_some_and(|ch| ch.is_ascii_digit()) {
        return consumed(); // Alt+digit workspace switch
    }

    // --- defaults.rs bindings, in table order. ---
    // Ctrl+L: ClearLogNotice + `Action::Esc("\x0c")` (~VI only — matches
    // even in kitty modes, so desktop sends the literal form feed).
    if ctrl_only && lower_char == Some('l') {
        return esc("\x0c");
    }
    // Shift+Home/End/PgUp/PgDn scroll the viewport OUTSIDE the alt
    // screen (~ALT_SCREEN); on the alt screen they fall through to the
    // shift-modified escapes.
    if shift_only && !modes.alt_screen {
        if matches!(named, Some(N::Home | N::End | N::PageUp | N::PageDown)) {
            return consumed();
        }
    }
    // DECCKM app-cursor SS3 (+APP_CURSOR, ~VI, unmodified).
    if none && modes.app_cursor {
        match named {
            Some(N::Home) => return esc("\x1bOH"),
            Some(N::End) => return esc("\x1bOF"),
            Some(N::ArrowUp) => return esc("\x1bOA"),
            Some(N::ArrowDown) => return esc("\x1bOB"),
            Some(N::ArrowRight) => return esc("\x1bOC"),
            Some(N::ArrowLeft) => return esc("\x1bOD"),
            _ => {}
        }
    }
    // Alt+Shift+Space → ToggleViMode.
    if alt_shift && named == Some(N::Space) {
        return consumed();
    }
    // Super+arrows → Action::None (explicitly swallowed).
    if super_only && is_arrow {
        return consumed();
    }
    // Plain arrows (~APP_CURSOR, ~VI, ~SEARCH, ~ALL_KEYS_AS_ESC).
    if none && !modes.app_cursor && !modes.report_all_keys_as_esc {
        match named {
            Some(N::ArrowUp) => return esc("\x1b[A"),
            Some(N::ArrowDown) => return esc("\x1b[B"),
            Some(N::ArrowRight) => return esc("\x1b[C"),
            Some(N::ArrowLeft) => return esc("\x1b[D"),
            _ => {}
        }
    }
    // Insert/Delete/PgUp/PgDn tildes (~ALL_KEYS_AS_ESC, ~DISAMBIGUATE_KEYS).
    if none && !modes.report_all_keys_as_esc && !modes.disambiguate_esc_codes {
        match named {
            Some(N::Insert) => return esc("\x1b[2~"),
            Some(N::Delete) => return esc("\x1b[3~"),
            Some(N::PageUp) => return esc("\x1b[5~"),
            Some(N::PageDown) => return esc("\x1b[6~"),
            _ => {}
        }
    }
    // Backspace family. Plain Backspace is guarded only by
    // ~ALL_KEYS_AS_ESC (defaults.rs:136); the Alt/Shift variants also
    // carry ~DISAMBIGUATE_KEYS.
    if named == Some(N::Backspace) && !modes.report_all_keys_as_esc {
        if none {
            return esc("\x7f");
        }
        if !modes.disambiguate_esc_codes {
            if alt_only {
                return esc("\x1b\x7f");
            }
            if shift_only {
                return esc("\x7f");
            }
        }
    }
    // F1-F4 SS3 (~ALL_KEYS_AS_ESC, ~DISAMBIGUATE_KEYS, unmodified).
    if none && !modes.report_all_keys_as_esc && !modes.disambiguate_esc_codes {
        match named {
            Some(N::F1) => return esc("\x1bOP"),
            Some(N::F2) => return esc("\x1bOQ"),
            Some(N::F3) => return esc("\x1bOR"),
            Some(N::F4) => return esc("\x1bOS"),
            _ => {}
        }
    }
    // Shift+Tab / Shift+Alt+Tab (~ALL_KEYS_AS_ESC, ~DISAMBIGUATE_KEYS).
    if named == Some(N::Tab)
        && !modes.report_all_keys_as_esc
        && !modes.disambiguate_esc_codes
    {
        if shift_only {
            return esc("\x1b[Z");
        }
        if alt_shift {
            return esc("\x1b\x1b[Z");
        }
    }
    // IDE chrome toggles: Alt+E/G/N (defaults.rs) + Alt+P palette
    // (platform/linux.rs), ~VI.
    if alt_only && matches!(lower_char, Some('e') | Some('g') | Some('n') | Some('p')) {
        return consumed();
    }

    // --- platform/linux.rs consuming bindings (exact Ctrl+Shift set +
    // navigation/splits). All host-side actions: NO bytes. ---
    if ctrl_shift {
        if matches!(
            lower_char,
            Some('v')
                | Some('c')
                | Some('f')
                | Some('b')
                | Some('n')
                | Some('p')
                | Some('t')
                | Some('w')
                | Some('r')
                | Some('d')
                | Some(',')
                | Some('[')
                | Some(']')
        ) {
            return consumed();
        }
    }
    // Alt+Shift+Left/Right → MoveActiveBufferTabToPrev/Next.
    if alt_shift && matches!(named, Some(N::ArrowLeft | N::ArrowRight)) {
        return consumed();
    }
    // Ctrl+Shift+Alt+arrows → MoveDivider*.
    if ctrl_shift_alt && is_arrow {
        return consumed();
    }

    None
}

// ---------------------------------------------------------------------------
// Desktop byte-parity tests
//
// Every expectation below is hand-traced through the DESKTOP pipeline:
// bindings/defaults.rs (Action::Esc literals), then
// screen/selection/key_event.rs alt-masking + should_build_sequence,
// then input/kitty_keyboard.rs::build_key_sequence (of which
// key_policy::kitty_sequence::build is the verbatim shared port).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const LEGACY: WebTerminalKeyModes = WebTerminalKeyModes {
        app_cursor: false,
        alt_screen: false,
        vi: false,
        disambiguate_esc_codes: false,
        report_event_types: false,
        report_alternate_keys: false,
        report_all_keys_as_esc: false,
        report_associated_text: false,
    };

    const KITTY_DISAMBIGUATE: WebTerminalKeyModes = WebTerminalKeyModes {
        disambiguate_esc_codes: true,
        ..LEGACY
    };

    fn event<'a>(key: &'a str, code: &'a str) -> WebTerminalKeyInput<'a> {
        WebTerminalKeyInput {
            key,
            code,
            ctrl: false,
            alt: false,
            shift: false,
            super_key: false,
            repeat: false,
        }
    }

    fn encode(input: WebTerminalKeyInput<'_>, modes: WebTerminalKeyModes) -> Vec<u8> {
        encode_web_terminal_key(&input, &modes)
    }

    // Desktop: no binding matches F5; should_build_sequence →
    // NamedWithoutText → true; build_key_sequence named_normal →
    // base "15", no mods ⇒ ESC [ 1 5 ~.
    #[test]
    fn f5_plain_matches_desktop() {
        assert_eq!(encode(event("F5", "F5"), LEGACY), b"\x1b[15~");
    }

    // Desktop: no ctrl+arrow binding; named_normal ArrowRight with
    // CONTROL ⇒ one_based "1", mods 4+1=5 ⇒ ESC [ 1 ; 5 C.
    #[test]
    fn ctrl_right_matches_desktop() {
        let mut e = event("ArrowRight", "ArrowRight");
        e.ctrl = true;
        assert_eq!(encode(e, LEGACY), b"\x1b[1;5C");
    }

    // Desktop: alt_send_esc (Linux) keeps ALT for single-char text;
    // should_build_sequence → NonNamed + non-empty text → raw path ⇒
    // ESC prefix + "b".
    #[test]
    fn alt_b_matches_desktop() {
        let mut e = event("b", "KeyB");
        e.alt = true;
        assert_eq!(encode(e, LEGACY), b"\x1bb");
    }

    // Desktop primary screen: `Home, SHIFT, ~ALT_SCREEN → ScrollToTop`
    // binding consumes the key — NO bytes.
    #[test]
    fn shift_home_primary_screen_consumed() {
        let mut e = event("Home", "Home");
        e.shift = true;
        assert_eq!(encode(e, LEGACY), b"");
    }

    // Desktop alt screen: the ~ALT_SCREEN binding no longer matches ⇒
    // build_key_sequence named_normal Home with SHIFT ⇒ ESC [ 1 ; 2 H.
    #[test]
    fn shift_home_alt_screen_matches_desktop() {
        let mut e = event("Home", "Home");
        e.shift = true;
        let modes = WebTerminalKeyModes {
            alt_screen: true,
            ..LEGACY
        };
        assert_eq!(encode(e, modes), b"\x1b[1;2H");
    }

    // Desktop: F3 named_normal (legacy path, NOT the kitty "13~"
    // divergence) with SHIFT|CONTROL ⇒ mods 1+4+1=6 ⇒ ESC [ 1 ; 6 R.
    #[test]
    fn ctrl_shift_f3_matches_desktop() {
        let mut e = event("F3", "F3");
        e.ctrl = true;
        e.shift = true;
        assert_eq!(encode(e, LEGACY), b"\x1b[1;6R");
    }

    // Desktop DECCKM: `ArrowUp, +APP_CURSOR → Esc("\x1bOA")` binding.
    #[test]
    fn arrow_up_app_cursor_matches_desktop() {
        let modes = WebTerminalKeyModes {
            app_cursor: true,
            ..LEGACY
        };
        assert_eq!(encode(event("ArrowUp", "ArrowUp"), modes), b"\x1bOA");
        // And the ~APP_CURSOR binding otherwise.
        assert_eq!(encode(event("ArrowUp", "ArrowUp"), LEGACY), b"\x1b[A");
    }

    // Desktop kitty disambiguate: should_build_sequence(escape) → true;
    // control-char branch base "27", no mods ⇒ ESC [ 2 7 u.
    #[test]
    fn escape_kitty_mode_matches_desktop() {
        assert_eq!(
            encode(event("Escape", "Escape"), KITTY_DISAMBIGUATE),
            b"\x1b[27u"
        );
        // Legacy: raw text path emits the bare 0x1b byte.
        assert_eq!(encode(event("Escape", "Escape"), LEGACY), b"\x1b");
    }

    // Desktop REPORT_ALL_KEYS_AS_ESC: textual branch, codepoint 97 ⇒
    // ESC [ 9 7 u.
    #[test]
    fn char_a_kitty_encode_all_matches_desktop() {
        let modes = WebTerminalKeyModes {
            report_all_keys_as_esc: true,
            ..LEGACY
        };
        assert_eq!(encode(event("a", "KeyA"), modes), b"\x1b[97u");
    }

    // Desktop legacy: raw text path with winit's control-mapped text ⇒
    // single 0x03. Kitty disambiguate: textual branch 99 with mods 5.
    #[test]
    fn ctrl_c_both_modes_match_desktop() {
        let mut e = event("c", "KeyC");
        e.ctrl = true;
        assert_eq!(encode(e, LEGACY), [0x03]);
        assert_eq!(encode(e, KITTY_DISAMBIGUATE), b"\x1b[99;5u");
    }

    // Desktop: `Tab, SHIFT, ~DISAMBIGUATE_KEYS → Esc("\x1b[Z")`; in
    // disambiguate mode the binding is excluded and the control-char
    // branch emits base 9 with SHIFT ⇒ ESC [ 9 ; 2 u.
    #[test]
    fn shift_tab_both_modes_match_desktop() {
        let mut e = event("Tab", "Tab");
        e.shift = true;
        assert_eq!(encode(e, LEGACY), b"\x1b[Z");
        assert_eq!(encode(e, KITTY_DISAMBIGUATE), b"\x1b[9;2u");
    }

    // Desktop consumes EVERY alt-only arrow host-side: Alt+Up/Down via
    // `handle_buffer_tab_focus_key` (unconditional `return true`),
    // Alt+Left/Right via the chrome-focus dispatch. NO bytes.
    #[test]
    fn alt_only_arrows_consumed_like_desktop() {
        for key in ["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"] {
            let mut e = event(key, key);
            e.alt = true;
            assert_eq!(encode(e, LEGACY), b"", "alt-only {key}");
        }
    }

    // Desktop: a named key without a host-side binding keeps ALT as a
    // modifier (alt_send_esc named branch) ⇒ ESC [ 1 5 ; 3 ~ for
    // Alt+F5, ESC [ 1 ; 3 H for Alt+Home.
    #[test]
    fn alt_modifier_on_unbound_named_matches_desktop() {
        let mut f5 = event("F5", "F5");
        f5.alt = true;
        assert_eq!(encode(f5, LEGACY), b"\x1b[15;3~");
        let mut home = event("Home", "Home");
        home.alt = true;
        assert_eq!(encode(home, LEGACY), b"\x1b[1;3H");
    }

    // Desktop kitty numpad: KeyLocation::Numpad + disambiguate ⇒
    // try_build_numpad "57404" ⇒ ESC [ 5 7 4 0 4 u.
    #[test]
    fn numpad5_kitty_matches_desktop() {
        assert_eq!(
            encode(event("5", "Numpad5"), KITTY_DISAMBIGUATE),
            b"\x1b[57404u"
        );
        // Legacy numpad digit: plain text byte.
        assert_eq!(encode(event("5", "Numpad5"), LEGACY), b"5");
    }

    // Desktop quirk kept for parity: the Ctrl+L `Esc("\x0c")` binding
    // carries only ~VI, so even kitty-mode terminals receive the
    // literal form feed instead of ESC[108;5u.
    #[test]
    fn ctrl_l_binding_wins_even_in_kitty_mode() {
        let mut e = event("l", "KeyL");
        e.ctrl = true;
        assert_eq!(encode(e, LEGACY), [0x0c]);
        assert_eq!(encode(e, KITTY_DISAMBIGUATE), [0x0c]);
    }

    // Desktop Backspace family: plain/shift → 0x7f bindings,
    // Alt → ESC 0x7f binding, Ctrl (no binding) → raw winit text 0x08.
    #[test]
    fn backspace_family_matches_desktop() {
        assert_eq!(encode(event("Backspace", "Backspace"), LEGACY), [0x7f]);
        let mut shift = event("Backspace", "Backspace");
        shift.shift = true;
        assert_eq!(encode(shift, LEGACY), [0x7f]);
        let mut alt = event("Backspace", "Backspace");
        alt.alt = true;
        assert_eq!(encode(alt, LEGACY), b"\x1b\x7f");
        let mut ctrl = event("Backspace", "Backspace");
        ctrl.ctrl = true;
        assert_eq!(encode(ctrl, LEGACY), [0x08]);
    }

    // Desktop kitty disambiguate: plain Enter stays legacy 0x0d
    // (should_build_sequence Enter arm → raw text); Shift+Enter is
    // disambiguated to CSI-u 13;2.
    #[test]
    fn enter_kitty_mode_matches_desktop() {
        assert_eq!(encode(event("Enter", "Enter"), KITTY_DISAMBIGUATE), [0x0d]);
        let mut shifted = event("Enter", "Enter");
        shifted.shift = true;
        assert_eq!(encode(shifted, KITTY_DISAMBIGUATE), b"\x1b[13;2u");
        assert_eq!(encode(shifted, LEGACY), [0x0d]);
    }

    // Desktop: Super+arrows are Action::None bindings; Alt+Left is the
    // chrome focus key. Both consume with NO bytes.
    #[test]
    fn host_consumed_arrows_emit_nothing() {
        let mut sup = event("ArrowLeft", "ArrowLeft");
        sup.super_key = true;
        assert_eq!(encode(sup, LEGACY), b"");
        let mut alt = event("ArrowLeft", "ArrowLeft");
        alt.alt = true;
        assert_eq!(encode(alt, LEGACY), b"");
    }

    // Desktop REPORT_EVENT_TYPES quirk: the plain-arrow binding has no
    // ~REPORT_EVENT_TYPES guard, so an unmodified repeat still emits
    // the literal CSI; a modified arrow reaches build_key_sequence and
    // gains the kitty :2 repeat event type.
    #[test]
    fn report_event_types_repeat_matches_desktop() {
        let modes = WebTerminalKeyModes {
            report_event_types: true,
            ..LEGACY
        };
        let mut plain = event("ArrowLeft", "ArrowLeft");
        plain.repeat = true;
        assert_eq!(encode(plain, modes), b"\x1b[D");
        let mut ctrl = event("ArrowLeft", "ArrowLeft");
        ctrl.repeat = true;
        ctrl.ctrl = true;
        assert_eq!(encode(ctrl, modes), b"\x1b[1;5:2D");
    }

    // Desktop: Ctrl+Shift+C is the Copy binding (consumed even with no
    // selection so 0x03 can't leak).
    #[test]
    fn ctrl_shift_c_consumed() {
        let mut e = event("C", "KeyC");
        e.ctrl = true;
        e.shift = true;
        assert_eq!(encode(e, LEGACY), b"");
    }

    // Desktop tildes: plain Delete binding; Ctrl+Delete has no binding
    // and flows through build_key_sequence.
    #[test]
    fn delete_and_paging_match_desktop() {
        assert_eq!(encode(event("Delete", "Delete"), LEGACY), b"\x1b[3~");
        let mut ctrl = event("Delete", "Delete");
        ctrl.ctrl = true;
        assert_eq!(encode(ctrl, LEGACY), b"\x1b[3;5~");
        assert_eq!(encode(event("PageUp", "PageUp"), LEGACY), b"\x1b[5~");
        assert_eq!(encode(event("PageDown", "PageDown"), LEGACY), b"\x1b[6~");
        assert_eq!(encode(event("Home", "Home"), LEGACY), b"\x1b[H");
        assert_eq!(encode(event("End", "End"), LEGACY), b"\x1b[F");
    }

    // Desktop kitty: Ctrl+Space hits the control-char branch (base 32);
    // legacy sends the NUL winit reports as text.
    #[test]
    fn ctrl_space_matches_desktop() {
        let mut e = event(" ", "Space");
        e.ctrl = true;
        assert_eq!(encode(e, LEGACY), [0x00]);
        assert_eq!(encode(e, KITTY_DISAMBIGUATE), b"\x1b[32;5u");
    }

    // Desktop vi mode: mid_key_event_dispatch consumes the byte path
    // entirely.
    #[test]
    fn vi_mode_consumes_everything() {
        let modes = WebTerminalKeyModes {
            vi: true,
            ..LEGACY
        };
        assert_eq!(encode(event("a", "KeyA"), modes), b"");
        assert_eq!(encode(event("ArrowUp", "ArrowUp"), modes), b"");
    }

    // Kitty alternate-key reporting: with REPORT_ALL_KEYS_AS_ESC the
    // textual branch reports base '1' for Shift+1 ('!') as
    // unicode-key-code:alternate, exactly like desktop's
    // key_without_modifiers branch. In plain disambiguate mode desktop's
    // should_build_sequence keeps shift-only printables on the raw text
    // path (mods_shift_only excludes them), so '!' stays a single byte.
    #[test]
    fn alternate_key_report_matches_desktop() {
        let mut e = event("!", "Digit1");
        e.shift = true;
        let encode_all = WebTerminalKeyModes {
            report_all_keys_as_esc: true,
            report_alternate_keys: true,
            ..LEGACY
        };
        assert_eq!(encode(e, encode_all), b"\x1b[49:33;2u");
        let disambiguate_only = WebTerminalKeyModes {
            report_alternate_keys: true,
            ..KITTY_DISAMBIGUATE
        };
        assert_eq!(encode(e, disambiguate_only), b"!");
    }
}
