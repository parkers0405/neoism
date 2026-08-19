//! Shared keyboard/IME policy.
//!
//! Pure decision functions extracted from the desktop fork's
//! `screen/selection.rs` so the web frontend can apply the same gating
//! rules. None of these touch the OS — callers (desktop or web) pass
//! POD inputs (modifier bits, single chars, numeric metrics, mode
//! flags) and apply the returned decision to their own platform APIs
//! (PTY write, IME area, command dispatch).
//!
//! What lives here:
//!
//! * IME cursor pixel geometry (`ImeCursorInput`, `ImeCursorOutput`,
//!   [`ime_cursor_pixel_position`], [`ime_cursor_position_significantly_changed`]).
//! * Key-release suppression gate ([`should_suppress_key_release`]).
//! * Alt+Digit → workspace index ([`workspace_index_for_alt_digit`]).
//! * Alt-modifier mask for output ([`mask_alt_for_output`]).
//! * Whether a named-key release is reportable in the current terminal
//!   mode ([`named_key_release_reportable`]).
//! * Config key-name normalization ([`normalize_config_key_name`]).
//! * Terminal keyboard protocol mode classification
//!   ([`classify_terminal_keyboard_input_mode`]).
//!
//! What stays in the desktop fork:
//!
//! * The actual `KeyEvent` / `Modifiers` / `Window` types from
//!   `neoism_window`.
//! * The PTY write, IME area call, clipboard system call, terminal
//!   lock acquisition, and damage tracking.

pub mod kitty_sequence;
pub mod web_key_encoder;

/// Minimal modifier-state POD used by [`mask_alt_for_output`].
///
/// Mirrors the `KeyModifierMask` shape from the historical
/// `editor::selection_model` extraction without taking a hard
/// dependency on that module — keeps the key_policy module
/// self-contained even when the shared-crate module set is being
/// reshuffled around it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct KeyModifierMask {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
}

impl KeyModifierMask {
    pub const fn new(shift: bool, control: bool, alt: bool) -> Self {
        Self {
            shift,
            control,
            alt,
        }
    }
}

/// Platform-neutral modifier state for shortcut routing decisions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ShortcutModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub super_key: bool,
}

impl ShortcutModifiers {
    pub const fn new(shift: bool, control: bool, alt: bool, super_key: bool) -> Self {
        Self {
            shift,
            control,
            alt,
            super_key,
        }
    }
}

/// Platform-neutral key snapshot for desktop/web shortcut routing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ShortcutKeyInput {
    pub mods: ShortcutModifiers,
    pub is_tab: bool,
    pub is_t: bool,
    pub is_w: bool,
    pub is_arrow_left: bool,
    pub is_arrow_right: bool,
    pub digit: Option<char>,
}

/// Pre-terminal Ctrl+Shift shortcut consumed on both press and release.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CtrlShiftWorkspaceShortcut {
    CreateWorkspaceTerminalTab,
    CreateTab,
    SelectActiveBufferTabPrevious,
    SelectActiveBufferTabNext,
}

/// Key-binding prelude shortcut consumed on pressed events.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyBindingPressShortcut {
    SelectTopLevelWorkspace(usize),
    SelectWorkspaceBufferTab { previous: bool },
    SelectActiveBufferTab { previous: bool },
    CreateWorkspaceTerminalTab,
    CreateTab,
}

/// Shared branch for Rust-owned Ctrl+Shift tab management shortcuts.
///
/// Desktop consumes these before terminal byte emission on press and release so
/// release reports cannot leak into the newly focused PTY. Web uses the same
/// branch to keep tab-management shortcuts out of the terminal stream.
pub const fn ctrl_shift_workspace_shortcut(
    input: ShortcutKeyInput,
) -> Option<CtrlShiftWorkspaceShortcut> {
    let mods = input.mods;
    if !(mods.control && mods.shift && !mods.alt && !mods.super_key) {
        return None;
    }

    if input.is_t {
        Some(CtrlShiftWorkspaceShortcut::CreateWorkspaceTerminalTab)
    } else if input.is_w {
        Some(CtrlShiftWorkspaceShortcut::CreateTab)
    } else if input.is_arrow_left {
        Some(CtrlShiftWorkspaceShortcut::SelectActiveBufferTabPrevious)
    } else if input.is_arrow_right {
        Some(CtrlShiftWorkspaceShortcut::SelectActiveBufferTabNext)
    } else {
        None
    }
}

/// Shared pressed-event shortcut prelude used before configured bindings.
pub fn key_binding_press_shortcut(
    input: ShortcutKeyInput,
) -> Option<KeyBindingPressShortcut> {
    if !input.is_tab {
        if let Some(digit) = input.digit {
            if let Some(index) = workspace_index_for_alt_digit(
                digit,
                input.mods.shift,
                input.mods.control,
                input.mods.alt,
                input.mods.super_key,
            ) {
                return Some(KeyBindingPressShortcut::SelectTopLevelWorkspace(index));
            }
        }
    }

    if input.mods.alt && !input.mods.control && !input.mods.super_key && input.is_tab {
        return Some(KeyBindingPressShortcut::SelectWorkspaceBufferTab {
            previous: input.mods.shift,
        });
    }

    if input.mods.control && !input.mods.alt && !input.mods.super_key && input.is_tab {
        return Some(KeyBindingPressShortcut::SelectActiveBufferTab {
            previous: input.mods.shift,
        });
    }

    if input.mods.control && input.mods.shift && input.is_t {
        return Some(KeyBindingPressShortcut::CreateWorkspaceTerminalTab);
    }

    if input.mods.control && input.mods.shift && input.is_w {
        return Some(KeyBindingPressShortcut::CreateTab);
    }

    None
}

/// Platform-neutral keyboard location used by config binding policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum KeyPolicyLocation {
    Standard,
    Numpad,
}

/// Platform-neutral key identifier used by config binding policy.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum KeyPolicyKey {
    Character(String),
    Named(KeyPolicyNamedKey),
}

/// Named keys Neoism accepts in user-configured bindings.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum KeyPolicyNamedKey {
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    Backspace,
    Delete,
    End,
    Enter,
    Escape,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    Home,
    Insert,
    PageDown,
    PageUp,
    Space,
    Tab,
}

/// Normalized config key-name decision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigKeyName {
    pub key: KeyPolicyKey,
    pub location: KeyPolicyLocation,
}

/// Platform-neutral terminal keyboard protocol switches.
///
/// Hosts derive these booleans from their terminal mode bits, then pass the
/// result to the keyboard encoder. Keeping the classification here lets desktop
/// and web agree on when kitty keyboard protocol, event-type reports, alternate
/// key reports, and associated text reports are active.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TerminalKeyboardInputMode {
    pub kitty_sequence: bool,
    pub kitty_encode_all: bool,
    pub kitty_event_type: bool,
    pub report_alternate_keys: bool,
    pub report_associated_text: bool,
}

/// Classify terminal keyboard input mode bits into encoder switches.
///
/// `key_repeat` and `key_released` are per-event attributes because kitty event
/// type reporting is only emitted for repeats and releases.
pub const fn classify_terminal_keyboard_input_mode(
    report_all_keys_as_esc: bool,
    disambiguate_esc_codes: bool,
    report_event_types: bool,
    report_alternate_keys: bool,
    report_associated_text: bool,
    key_repeat: bool,
    key_released: bool,
) -> TerminalKeyboardInputMode {
    TerminalKeyboardInputMode {
        kitty_sequence: report_all_keys_as_esc
            || disambiguate_esc_codes
            || report_event_types,
        kitty_encode_all: report_all_keys_as_esc,
        kitty_event_type: report_event_types && (key_repeat || key_released),
        report_alternate_keys,
        report_associated_text,
    }
}

/// Whether a text payload should be appended as kitty associated text.
pub fn should_report_terminal_associated_text(
    mode: TerminalKeyboardInputMode,
    key_released: bool,
    text: &str,
) -> bool {
    mode.report_associated_text
        && !key_released
        && !text.is_empty()
        && !is_terminal_control_character(text)
}

/// Check whether the `text` is `0x7f`, `C0` or `C1` control code.
pub fn is_terminal_control_character(text: &str) -> bool {
    let Some(codepoint) = text.bytes().next() else {
        return false;
    };
    text.len() == 1 && (codepoint < 0x20 || (0x7f..=0x9f).contains(&codepoint))
}

/// Normalize a user-configured key name into portable key policy.
///
/// This is intentionally independent of winit/DOM key objects. Desktop
/// translates the returned POD into `neoism_window` keys, while web can
/// translate the same decision into browser keyboard handling.
pub fn normalize_config_key_name(key_name: &str) -> Option<ConfigKeyName> {
    let key_name = key_name.trim();
    if key_name.chars().count() == 1 {
        return Some(ConfigKeyName {
            key: KeyPolicyKey::Character(key_name.to_lowercase()),
            location: KeyPolicyLocation::Standard,
        });
    }

    let normalized = key_name.to_lowercase();
    use KeyPolicyLocation::{Numpad, Standard};
    use KeyPolicyNamedKey::{
        ArrowDown, ArrowLeft, ArrowRight, ArrowUp, Backspace, Delete, End, Enter, Escape,
        Home, Insert, PageDown, PageUp, Space, Tab, F1, F10, F11, F12, F2, F3, F4, F5,
        F6, F7, F8, F9,
    };

    let (key, location) = match normalized.as_str() {
        "home" => (KeyPolicyKey::Named(Home), Standard),
        "space" => (KeyPolicyKey::Named(Space), Standard),
        "delete" => (KeyPolicyKey::Named(Delete), Standard),
        "esc" | "escape" => (KeyPolicyKey::Named(Escape), Standard),
        "insert" => (KeyPolicyKey::Named(Insert), Standard),
        "pageup" => (KeyPolicyKey::Named(PageUp), Standard),
        "pagedown" => (KeyPolicyKey::Named(PageDown), Standard),
        "end" => (KeyPolicyKey::Named(End), Standard),
        "up" => (KeyPolicyKey::Named(ArrowUp), Standard),
        "back" | "backspace" => (KeyPolicyKey::Named(Backspace), Standard),
        "down" => (KeyPolicyKey::Named(ArrowDown), Standard),
        "left" => (KeyPolicyKey::Named(ArrowLeft), Standard),
        "right" => (KeyPolicyKey::Named(ArrowRight), Standard),
        "return" | "enter" => (KeyPolicyKey::Named(Enter), Standard),
        "tab" => (KeyPolicyKey::Named(Tab), Standard),
        "colon" => (KeyPolicyKey::Character(":".into()), Standard),
        "f1" => (KeyPolicyKey::Named(F1), Standard),
        "f2" => (KeyPolicyKey::Named(F2), Standard),
        "f3" => (KeyPolicyKey::Named(F3), Standard),
        "f4" => (KeyPolicyKey::Named(F4), Standard),
        "f5" => (KeyPolicyKey::Named(F5), Standard),
        "f6" => (KeyPolicyKey::Named(F6), Standard),
        "f7" => (KeyPolicyKey::Named(F7), Standard),
        "f8" => (KeyPolicyKey::Named(F8), Standard),
        "f9" => (KeyPolicyKey::Named(F9), Standard),
        "f10" => (KeyPolicyKey::Named(F10), Standard),
        "f11" => (KeyPolicyKey::Named(F11), Standard),
        "f12" => (KeyPolicyKey::Named(F12), Standard),
        "numpadenter" => (KeyPolicyKey::Named(Enter), Numpad),
        "numpadadd" => (KeyPolicyKey::Character("+".into()), Numpad),
        "numpadcomma" => (KeyPolicyKey::Character(",".into()), Numpad),
        "numpaddecimal" => (KeyPolicyKey::Character(".".into()), Numpad),
        "numpaddivide" => (KeyPolicyKey::Character("/".into()), Numpad),
        "numpadequals" => (KeyPolicyKey::Character("=".into()), Numpad),
        "numpadsubtract" => (KeyPolicyKey::Character("-".into()), Numpad),
        "numpadmultiply" => (KeyPolicyKey::Character("*".into()), Numpad),
        "numpad0" => (KeyPolicyKey::Character("0".into()), Numpad),
        "numpad1" => (KeyPolicyKey::Character("1".into()), Numpad),
        "numpad2" => (KeyPolicyKey::Character("2".into()), Numpad),
        "numpad3" => (KeyPolicyKey::Character("3".into()), Numpad),
        "numpad4" => (KeyPolicyKey::Character("4".into()), Numpad),
        "numpad5" => (KeyPolicyKey::Character("5".into()), Numpad),
        "numpad6" => (KeyPolicyKey::Character("6".into()), Numpad),
        "numpad7" => (KeyPolicyKey::Character("7".into()), Numpad),
        "numpad8" => (KeyPolicyKey::Character("8".into()), Numpad),
        "numpad9" => (KeyPolicyKey::Character("9".into()), Numpad),
        _ => return None,
    };

    Some(ConfigKeyName { key, location })
}

/// POD input to the IME cursor pixel-position math.
///
/// All distances are physical pixels (caller does scale → pixel
/// conversion). `cell_width` and `cell_height` must be `> 0.0`; the
/// helper returns `None` otherwise so the desktop fork can keep its
/// existing warning log.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImeCursorInput {
    pub panel_left_px: f32,
    pub panel_top_px: f32,
    pub scaled_margin_left_px: f32,
    pub scaled_margin_top_px: f32,
    pub cell_width_px: f32,
    pub cell_height_px: f32,
    pub cursor_col: usize,
    /// Row in the visible grid. Signed because the terminal core's
    /// `Line` is `i32`; the helper casts to `f32` like the original
    /// desktop code so a stray negative row produces a negative pixel
    /// Y and falls through the safety guard below.
    pub cursor_row: i32,
}

/// POD output of the IME cursor pixel-position math.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImeCursorOutput {
    pub pixel_x: f32,
    pub pixel_y: f32,
    pub cell_width: f32,
    pub cell_height: f32,
}

/// Compute the IME cursor area in physical pixels.
///
/// Returns `None` if any dimension is non-positive or if the resulting
/// pixel coordinates are NaN / negative — exactly the guards the
/// desktop fork already enforced before calling
/// `window.set_ime_cursor_area`.
pub fn ime_cursor_pixel_position(input: ImeCursorInput) -> Option<ImeCursorOutput> {
    if input.cell_width_px <= 0.0 || input.cell_height_px <= 0.0 {
        return None;
    }

    let origin_x = input.panel_left_px + input.scaled_margin_left_px;
    let origin_y = input.panel_top_px + input.scaled_margin_top_px;

    let pixel_x = origin_x
        + (input.cursor_col as f32 * input.cell_width_px)
        + (input.cell_width_px * 0.5);
    let pixel_y = origin_y + (input.cursor_row as f32 * input.cell_height_px);

    if pixel_x.is_nan() || pixel_y.is_nan() || pixel_x < 0.0 || pixel_y < 0.0 {
        return None;
    }

    Some(ImeCursorOutput {
        pixel_x,
        pixel_y,
        cell_width: input.cell_width_px,
        cell_height: input.cell_height_px,
    })
}

/// Returns true if the new IME cursor pixel position differs from
/// `last` by at least 1 px in either axis. Desktop uses this to skip
/// redundant `set_ime_cursor_area` calls; web can use the same gate
/// when caching its remote IME hint.
pub fn ime_cursor_position_significantly_changed(
    last: Option<(f32, f32)>,
    pixel_x: f32,
    pixel_y: f32,
) -> bool {
    match last {
        Some((last_x, last_y)) => {
            (pixel_x - last_x).abs() >= 1.0 || (pixel_y - last_y).abs() >= 1.0
        }
        None => true,
    }
}

/// Decision: should the terminal swallow a key-release event without
/// reporting it?
///
/// Returns true when ANY of the following holds:
///
/// * The terminal hasn't opted into REPORT_EVENT_TYPES (the kitty
///   keyboard protocol extension that surfaces releases), OR
/// * Vi mode is active (vi key bindings don't care about releases), OR
/// * The terminal search overlay is open, OR
/// * Hint mode is active.
///
/// Each of those gates already lived inline in
/// `process_key_event`'s release branch; this helper just lifts the
/// boolean policy out so it's testable in isolation and shareable
/// with the web frontend's release handling.
pub const fn should_suppress_key_release(
    report_event_types: bool,
    vi_mode: bool,
    search_active: bool,
    hint_active: bool,
) -> bool {
    !report_event_types || vi_mode || search_active || hint_active
}

/// Whether a named key (`Enter` / `Tab` / `Backspace`) should produce
/// a release-event report.
///
/// Without `REPORT_ALL_KEYS_AS_ESC`, kitty-keyboard treats those
/// three named keys as unreportable on release — every other key
/// still reports. Returns `true` if the release should be encoded
/// and written to the PTY.
pub const fn named_key_release_reportable(
    is_enter_tab_or_backspace: bool,
    report_all_keys_as_esc: bool,
) -> bool {
    !is_enter_tab_or_backspace || report_all_keys_as_esc
}

/// Map an Alt+Digit shortcut to a 0-based workspace index.
///
/// Returns `Some(0..10)` when:
///
/// * Alt is held, Ctrl / Super / Shift are all NOT held, and
/// * `digit` is a printable `'0'..='9'` (`'0'` maps to index 9 — the
///   conventional "tenth workspace" slot).
///
/// Returns `None` otherwise — callers fall through to the rest of
/// their key-binding chain.
pub fn workspace_index_for_alt_digit(
    digit: char,
    shift: bool,
    control: bool,
    alt: bool,
    super_key: bool,
) -> Option<usize> {
    if !alt || control || super_key || shift {
        return None;
    }
    match digit {
        '1' => Some(0),
        '2' => Some(1),
        '3' => Some(2),
        '4' => Some(3),
        '5' => Some(4),
        '6' => Some(5),
        '7' => Some(6),
        '8' => Some(7),
        '9' => Some(8),
        '0' => Some(9),
        _ => None,
    }
}

/// Modifier-mask policy: if the alt modifier is meant to be
/// `alt-as-meta` (sends an ESC prefix byte), keep it in the mask the
/// terminal output stage sees. Otherwise scrub ALT so the output
/// stage doesn't treat the alt-composed text as having an alt
/// modifier on top of itself.
///
/// Returns the modifier set the output stage should use. The desktop
/// fork already had this `if alt_send_esc { mods } else { mods & !ALT }`
/// pattern inline twice in `process_key_event`; lifted here so the
/// web frontend can match it.
pub const fn mask_alt_for_output(
    mods: KeyModifierMask,
    alt_send_esc: bool,
) -> KeyModifierMask {
    if alt_send_esc {
        mods
    } else {
        KeyModifierMask::new(mods.shift, mods.control, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(col: usize, row: i32) -> ImeCursorInput {
        ImeCursorInput {
            panel_left_px: 100.0,
            panel_top_px: 50.0,
            scaled_margin_left_px: 10.0,
            scaled_margin_top_px: 5.0,
            cell_width_px: 8.0,
            cell_height_px: 16.0,
            cursor_col: col,
            cursor_row: row,
        }
    }

    #[test]
    fn ime_cursor_geometry_origin() {
        let out = ime_cursor_pixel_position(input(0, 0)).unwrap();
        // origin_x = 100 + 10 = 110; pixel_x = 110 + 0*8 + 4 = 114.
        assert_eq!(out.pixel_x, 114.0);
        // origin_y = 50 + 5 = 55; pixel_y = 55 + 0*16 = 55.
        assert_eq!(out.pixel_y, 55.0);
        assert_eq!(out.cell_width, 8.0);
        assert_eq!(out.cell_height, 16.0);
    }

    #[test]
    fn ime_cursor_geometry_offset_cell() {
        let out = ime_cursor_pixel_position(input(3, 2)).unwrap();
        // 110 + 3*8 + 4 = 138, 55 + 2*16 = 87.
        assert_eq!(out.pixel_x, 138.0);
        assert_eq!(out.pixel_y, 87.0);
    }

    #[test]
    fn ime_cursor_rejects_zero_cell() {
        let mut inp = input(1, 1);
        inp.cell_width_px = 0.0;
        assert!(ime_cursor_pixel_position(inp).is_none());
        let mut inp = input(1, 1);
        inp.cell_height_px = -1.0;
        assert!(ime_cursor_pixel_position(inp).is_none());
    }

    #[test]
    fn ime_cursor_rejects_negative_origin() {
        let mut inp = input(0, 0);
        inp.panel_left_px = -1000.0;
        // pixel_x = -1000 + 10 + 4 = -986 → negative → None.
        assert!(ime_cursor_pixel_position(inp).is_none());
    }

    #[test]
    fn ime_position_change_detection() {
        assert!(ime_cursor_position_significantly_changed(None, 1.0, 1.0));
        assert!(!ime_cursor_position_significantly_changed(
            Some((10.0, 20.0)),
            10.0,
            20.0
        ));
        assert!(!ime_cursor_position_significantly_changed(
            Some((10.0, 20.0)),
            10.5,
            20.0
        ));
        assert!(ime_cursor_position_significantly_changed(
            Some((10.0, 20.0)),
            11.0,
            20.0
        ));
        assert!(ime_cursor_position_significantly_changed(
            Some((10.0, 20.0)),
            10.0,
            21.5
        ));
    }

    #[test]
    fn suppress_key_release_gates() {
        // No gate active and event types reported → don't suppress.
        assert!(!should_suppress_key_release(true, false, false, false));
        // Without REPORT_EVENT_TYPES, always suppress.
        assert!(should_suppress_key_release(false, false, false, false));
        // Vi mode suppresses releases even with REPORT_EVENT_TYPES.
        assert!(should_suppress_key_release(true, true, false, false));
        // Search active suppresses.
        assert!(should_suppress_key_release(true, false, true, false));
        // Hint active suppresses.
        assert!(should_suppress_key_release(true, false, false, true));
    }

    #[test]
    fn named_key_release_reportable_rules() {
        // Plain key (not enter/tab/bksp) always reportable.
        assert!(named_key_release_reportable(false, false));
        assert!(named_key_release_reportable(false, true));
        // Enter/Tab/Backspace only reportable when REPORT_ALL_KEYS_AS_ESC.
        assert!(!named_key_release_reportable(true, false));
        assert!(named_key_release_reportable(true, true));
    }

    #[test]
    fn terminal_keyboard_mode_classifies_kitty_flags() {
        assert_eq!(
            classify_terminal_keyboard_input_mode(
                false, false, false, false, false, false, false
            ),
            TerminalKeyboardInputMode::default()
        );

        let mode = classify_terminal_keyboard_input_mode(
            false, true, false, true, true, false, false,
        );
        assert!(mode.kitty_sequence);
        assert!(!mode.kitty_encode_all);
        assert!(!mode.kitty_event_type);
        assert!(mode.report_alternate_keys);
        assert!(mode.report_associated_text);

        let mode = classify_terminal_keyboard_input_mode(
            true, false, true, false, false, true, false,
        );
        assert!(mode.kitty_sequence);
        assert!(mode.kitty_encode_all);
        assert!(mode.kitty_event_type);

        let mode = classify_terminal_keyboard_input_mode(
            false, false, true, false, false, false, true,
        );
        assert!(mode.kitty_sequence);
        assert!(mode.kitty_event_type);
    }

    #[test]
    fn associated_text_policy_rejects_release_empty_and_control_text() {
        let mode = classify_terminal_keyboard_input_mode(
            false, true, false, false, true, false, false,
        );

        assert!(should_report_terminal_associated_text(mode, false, "é"));
        assert!(!should_report_terminal_associated_text(mode, true, "é"));
        assert!(!should_report_terminal_associated_text(mode, false, ""));
        assert!(!should_report_terminal_associated_text(mode, false, "\n"));
        assert!(!should_report_terminal_associated_text(
            mode, false, "\u{7f}"
        ));

        let disabled = TerminalKeyboardInputMode {
            report_associated_text: false,
            ..mode
        };
        assert!(!should_report_terminal_associated_text(
            disabled, false, "é"
        ));
    }

    #[test]
    fn workspace_index_alt_digit_mapping() {
        assert_eq!(
            workspace_index_for_alt_digit('1', false, false, true, false),
            Some(0)
        );
        assert_eq!(
            workspace_index_for_alt_digit('5', false, false, true, false),
            Some(4)
        );
        // '0' maps to the tenth slot.
        assert_eq!(
            workspace_index_for_alt_digit('0', false, false, true, false),
            Some(9)
        );
    }

    #[test]
    fn workspace_index_requires_pure_alt() {
        // No alt: nope.
        assert_eq!(
            workspace_index_for_alt_digit('1', false, false, false, false),
            None
        );
        // Alt + control: nope.
        assert_eq!(
            workspace_index_for_alt_digit('1', false, true, true, false),
            None
        );
        // Alt + super: nope.
        assert_eq!(
            workspace_index_for_alt_digit('1', false, false, true, true),
            None
        );
        // Alt + shift: nope (Alt+Shift+digits is reserved for splits).
        assert_eq!(
            workspace_index_for_alt_digit('1', true, false, true, false),
            None
        );
        // Non-digit char: nope.
        assert_eq!(
            workspace_index_for_alt_digit('a', false, false, true, false),
            None
        );
    }

    fn shortcut_input(mods: ShortcutModifiers) -> ShortcutKeyInput {
        ShortcutKeyInput {
            mods,
            ..ShortcutKeyInput::default()
        }
    }

    #[test]
    fn ctrl_shift_workspace_shortcuts_require_no_extra_modifiers() {
        let mut input = shortcut_input(ShortcutModifiers::new(true, true, false, false));
        input.is_t = true;
        assert_eq!(
            ctrl_shift_workspace_shortcut(input),
            Some(CtrlShiftWorkspaceShortcut::CreateWorkspaceTerminalTab)
        );

        input.is_t = false;
        input.is_arrow_left = true;
        assert_eq!(
            ctrl_shift_workspace_shortcut(input),
            Some(CtrlShiftWorkspaceShortcut::SelectActiveBufferTabPrevious)
        );

        input.mods.alt = true;
        assert_eq!(ctrl_shift_workspace_shortcut(input), None);
    }

    #[test]
    fn key_binding_press_shortcut_maps_digit_and_tab_switches() {
        let mut input = shortcut_input(ShortcutModifiers::new(false, false, true, false));
        input.digit = Some('4');
        assert_eq!(
            key_binding_press_shortcut(input),
            Some(KeyBindingPressShortcut::SelectTopLevelWorkspace(3))
        );

        input.digit = None;
        input.is_tab = true;
        assert_eq!(
            key_binding_press_shortcut(input),
            Some(KeyBindingPressShortcut::SelectWorkspaceBufferTab { previous: false })
        );

        input.mods = ShortcutModifiers::new(true, true, false, false);
        assert_eq!(
            key_binding_press_shortcut(input),
            Some(KeyBindingPressShortcut::SelectActiveBufferTab { previous: true })
        );
    }

    #[test]
    fn key_binding_press_shortcut_preserves_ctrl_shift_tab_creation_rules() {
        let mut input = shortcut_input(ShortcutModifiers::new(true, true, false, false));
        input.is_t = true;
        assert_eq!(
            key_binding_press_shortcut(input),
            Some(KeyBindingPressShortcut::CreateWorkspaceTerminalTab)
        );

        input.is_t = false;
        input.is_w = true;
        assert_eq!(
            key_binding_press_shortcut(input),
            Some(KeyBindingPressShortcut::CreateTab)
        );
    }

    #[test]
    fn mask_alt_for_output_preserves_when_alt_meta() {
        let mods = KeyModifierMask::new(true, true, true);
        let masked = mask_alt_for_output(mods, true);
        assert_eq!(masked, mods);
    }

    #[test]
    fn mask_alt_for_output_scrubs_alt_when_not_meta() {
        let mods = KeyModifierMask::new(true, false, true);
        let masked = mask_alt_for_output(mods, false);
        assert_eq!(masked.shift, true);
        assert_eq!(masked.control, false);
        assert_eq!(masked.alt, false);
    }

    #[test]
    fn normalize_config_key_name_lowercases_single_characters() {
        assert_eq!(
            normalize_config_key_name("Q"),
            Some(ConfigKeyName {
                key: KeyPolicyKey::Character("q".into()),
                location: KeyPolicyLocation::Standard,
            })
        );
    }

    #[test]
    fn normalize_config_key_name_maps_named_aliases() {
        assert_eq!(
            normalize_config_key_name("return"),
            Some(ConfigKeyName {
                key: KeyPolicyKey::Named(KeyPolicyNamedKey::Enter),
                location: KeyPolicyLocation::Standard,
            })
        );
        assert_eq!(
            normalize_config_key_name("escape"),
            Some(ConfigKeyName {
                key: KeyPolicyKey::Named(KeyPolicyNamedKey::Escape),
                location: KeyPolicyLocation::Standard,
            })
        );
        assert_eq!(
            normalize_config_key_name("back"),
            normalize_config_key_name("backspace")
        );
    }

    #[test]
    fn normalize_config_key_name_maps_numpad_location() {
        assert_eq!(
            normalize_config_key_name("numpad7"),
            Some(ConfigKeyName {
                key: KeyPolicyKey::Character("7".into()),
                location: KeyPolicyLocation::Numpad,
            })
        );
        assert_eq!(
            normalize_config_key_name("numpadenter"),
            Some(ConfigKeyName {
                key: KeyPolicyKey::Named(KeyPolicyNamedKey::Enter),
                location: KeyPolicyLocation::Numpad,
            })
        );
    }

    #[test]
    fn normalize_config_key_name_rejects_unknown_names() {
        assert_eq!(normalize_config_key_name("not-a-key"), None);
    }

    #[test]
    fn physical_key_binding_char_maps_letters_and_digits() {
        assert_eq!(physical_key_binding_char(PhysicalKeyCode::KeyA), Some("a"));
        assert_eq!(physical_key_binding_char(PhysicalKeyCode::KeyZ), Some("z"));
        assert_eq!(
            physical_key_binding_char(PhysicalKeyCode::Digit0),
            Some("0")
        );
        assert_eq!(
            physical_key_binding_char(PhysicalKeyCode::Numpad9),
            Some("9")
        );
    }

    #[test]
    fn physical_key_binding_char_maps_punctuation() {
        assert_eq!(
            physical_key_binding_char(PhysicalKeyCode::Backquote),
            Some("`")
        );
        assert_eq!(
            physical_key_binding_char(PhysicalKeyCode::Backslash),
            Some("\\")
        );
        assert_eq!(physical_key_binding_char(PhysicalKeyCode::Slash), Some("/"));
        assert_eq!(
            physical_key_binding_char(PhysicalKeyCode::NumpadDivide),
            Some("/")
        );
        assert_eq!(physical_key_binding_char(PhysicalKeyCode::Space), Some(" "));
    }

    #[test]
    fn physical_key_binding_char_returns_none_for_unmapped() {
        assert_eq!(physical_key_binding_char(PhysicalKeyCode::Unknown), None);
    }
}

/// Renderer-neutral keyboard physical codes that participate in
/// configured key bindings.
///
/// Desktop translates `neoism_window::keyboard::KeyCode::*` into these
/// at the boundary; web translates from its DOM `KeyboardEvent.code`.
/// Anything not in this list maps to [`PhysicalKeyCode::Unknown`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhysicalKeyCode {
    KeyA,
    KeyB,
    KeyC,
    KeyD,
    KeyE,
    KeyF,
    KeyG,
    KeyH,
    KeyI,
    KeyJ,
    KeyK,
    KeyL,
    KeyM,
    KeyN,
    KeyO,
    KeyP,
    KeyQ,
    KeyR,
    KeyS,
    KeyT,
    KeyU,
    KeyV,
    KeyW,
    KeyX,
    KeyY,
    KeyZ,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    Numpad0,
    Numpad1,
    Numpad2,
    Numpad3,
    Numpad4,
    Numpad5,
    Numpad6,
    Numpad7,
    Numpad8,
    Numpad9,
    Backquote,
    Backslash,
    IntlBackslash,
    IntlRo,
    BracketLeft,
    BracketRight,
    Comma,
    Equal,
    NumpadAdd,
    Minus,
    NumpadSubtract,
    Period,
    Quote,
    Semicolon,
    Slash,
    NumpadDivide,
    Space,
    Unknown,
}

/// Map a renderer-neutral physical key code to the binding character it
/// represents (always lowercase for letters).
///
/// Mirrors the desktop `KeyCode::* → &'static str` table that powered
/// macOS Alt-shortcut matching. Sharing it lets the web frontend (where
/// the physical layout is also available) match user-configured key
/// bindings exactly the same way.
pub const fn physical_key_binding_char(code: PhysicalKeyCode) -> Option<&'static str> {
    use PhysicalKeyCode::*;
    match code {
        KeyA => Some("a"),
        KeyB => Some("b"),
        KeyC => Some("c"),
        KeyD => Some("d"),
        KeyE => Some("e"),
        KeyF => Some("f"),
        KeyG => Some("g"),
        KeyH => Some("h"),
        KeyI => Some("i"),
        KeyJ => Some("j"),
        KeyK => Some("k"),
        KeyL => Some("l"),
        KeyM => Some("m"),
        KeyN => Some("n"),
        KeyO => Some("o"),
        KeyP => Some("p"),
        KeyQ => Some("q"),
        KeyR => Some("r"),
        KeyS => Some("s"),
        KeyT => Some("t"),
        KeyU => Some("u"),
        KeyV => Some("v"),
        KeyW => Some("w"),
        KeyX => Some("x"),
        KeyY => Some("y"),
        KeyZ => Some("z"),
        Digit0 | Numpad0 => Some("0"),
        Digit1 | Numpad1 => Some("1"),
        Digit2 | Numpad2 => Some("2"),
        Digit3 | Numpad3 => Some("3"),
        Digit4 | Numpad4 => Some("4"),
        Digit5 | Numpad5 => Some("5"),
        Digit6 | Numpad6 => Some("6"),
        Digit7 | Numpad7 => Some("7"),
        Digit8 | Numpad8 => Some("8"),
        Digit9 | Numpad9 => Some("9"),
        Backquote => Some("`"),
        Backslash | IntlBackslash | IntlRo => Some("\\"),
        BracketLeft => Some("["),
        BracketRight => Some("]"),
        Comma => Some(","),
        Equal | NumpadAdd => Some("="),
        Minus | NumpadSubtract => Some("-"),
        Period => Some("."),
        Quote => Some("'"),
        Semicolon => Some(";"),
        Slash | NumpadDivide => Some("/"),
        Space => Some(" "),
        Unknown => None,
    }
}

/// POD mirror of the subset of a `winit::event::KeyEvent` the island
/// rename field reads. Native callers translate their event before
/// invoking [`island_key_from_winit`] so the policy stays free of
/// winit types.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IslandKeyInput<'a> {
    /// `true` for ElementState::Pressed (the rename field only reacts
    /// on key-down, including repeat events).
    pub pressed: bool,
    pub named: Option<IslandNamedKey>,
    /// Modifier-aware text (winit's `text_with_all_modifiers`) when the
    /// caller has it; falls back to plain `text` when not.
    pub text: Option<&'a str>,
}

/// Subset of `winit::keyboard::NamedKey` the island rename field
/// recognizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IslandNamedKey {
    Escape,
    Enter,
    Backspace,
}

/// Pure island-rename key translation. Returns the
/// `widgets::island::IslandRenameKey` variant the caller should feed
/// into `Island::handle_rename_input`, or `None` if the event should
/// fall through to the next handler.
pub fn island_key_from_winit(
    input: IslandKeyInput<'_>,
) -> Option<crate::widgets::island::IslandRenameKey> {
    use crate::widgets::island::IslandRenameKey;

    if !input.pressed {
        return None;
    }
    match input.named {
        Some(IslandNamedKey::Escape) => Some(IslandRenameKey::Escape),
        Some(IslandNamedKey::Enter) => Some(IslandRenameKey::Enter),
        Some(IslandNamedKey::Backspace) => Some(IslandRenameKey::Backspace),
        None => {
            let text = input.text.unwrap_or("");
            let ch = text.chars().next()?;
            if ch.is_control() {
                None
            } else {
                Some(IslandRenameKey::Character(ch))
            }
        }
    }
}

#[cfg(test)]
mod island_key_tests {
    use super::*;
    use crate::widgets::island::IslandRenameKey;

    #[test]
    fn key_release_returns_none() {
        let input = IslandKeyInput {
            pressed: false,
            named: Some(IslandNamedKey::Enter),
            text: None,
        };
        assert!(island_key_from_winit(input).is_none());
    }

    #[test]
    fn enter_maps_to_enter() {
        let input = IslandKeyInput {
            pressed: true,
            named: Some(IslandNamedKey::Enter),
            text: None,
        };
        assert_eq!(island_key_from_winit(input), Some(IslandRenameKey::Enter));
    }

    #[test]
    fn character_passes_through() {
        let input = IslandKeyInput {
            pressed: true,
            named: None,
            text: Some("a"),
        };
        assert_eq!(
            island_key_from_winit(input),
            Some(IslandRenameKey::Character('a'))
        );
    }

    #[test]
    fn control_character_suppressed() {
        let input = IslandKeyInput {
            pressed: true,
            named: None,
            text: Some("\t"),
        };
        assert!(island_key_from_winit(input).is_none());
    }
}
