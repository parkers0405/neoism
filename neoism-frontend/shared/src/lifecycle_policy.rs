//! Pure lifecycle / app-loop policy shared by native and web.
//!
//! The desktop fork's `frontends/neoism/src/screen/lifecycle.rs`,
//! `frontends/neoism/src/router/*.rs`, `frontends/neoism/src/app/*.rs`,
//! and `frontends/neoism/src/app/messenger.rs` host most of the
//! winit/Sugarloaf/PTY plumbing — but a long tail of pure helpers
//! (byte-preview formatters, nvim-style key formatting, kitty-keyboard
//! sequence gates, vblank math, env-flag parsing, monitor-centered
//! placement, grid-size-to-pixel math, font-size action classification)
//! is renderer-neutral. Those helpers live here so the web frontend can
//! match them byte-for-byte.
//!
//! Renderer-neutral: no Sugarloaf, Taffy, PTY, native-window dependencies.
//! Mode comes from `neoism_terminal_core::crosswords::Mode`, which is
//! already shared.
//!
//! What lives here:
//!
//! * Byte-preview formatters used by the PTY messenger logs
//!   ([`LOG_BYTE_PREVIEW_LIMIT`], [`bytes_hex_for_log`],
//!   [`bytes_text_for_log`]).
//! * `is_ascii_alphabetic_str` predicate.
//! * `named_key_to_nvim_name` mapping for the nvim key notation
//!   (`<S-CR>`, `<C-Esc>`, etc.).
//! * [`format_nvim_key_token`] — the actual nvim-key formatter, taking
//!   POD inputs so callers can adapt their platform key event to it.
//! * [`vblank_interval_from_refresh_rate`] — refresh-rate math.
//! * [`centered_window_position`] — monitor-centered placement math.
//! * [`compute_window_size_from_grid_dims`] — desired-window-size from
//!   columns/rows grid spec.
//! * [`FontSizeAction`] — POD font-size action enum.
//! * [`env_flag_truthy`] — env-var truthy parser used by the freeze
//!   watchdog.
//! * [`frame_over_budget`] — frame-cadence over-budget threshold.

use web_time::Duration;

/// Maximum number of bytes the PTY messenger and lifecycle log
/// formatters preview before truncating with an ellipsis.
///
/// Matches the desktop fork's `LOG_BYTE_PREVIEW_LIMIT` constant
/// duplicated across `app/messenger.rs` and `screen/lifecycle.rs`.
pub const LOG_BYTE_PREVIEW_LIMIT: usize = 96;

/// Hex preview of `bytes`, capped at [`LOG_BYTE_PREVIEW_LIMIT`].
///
/// Returns `"de ad be ef ..."` style spacing, with `" ..."` suffix when
/// truncation occurred. Used by PTY send-write logs.
pub fn bytes_hex_for_log(bytes: &[u8]) -> String {
    let mut out = bytes
        .iter()
        .take(LOG_BYTE_PREVIEW_LIMIT)
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    if bytes.len() > LOG_BYTE_PREVIEW_LIMIT {
        out.push_str(" ...");
    }
    out
}

/// Escaped-text preview of `bytes`, capped at [`LOG_BYTE_PREVIEW_LIMIT`].
///
/// Suffixes `"..."` (no leading space — matches the original) when
/// truncation occurred. Used by PTY send-write logs.
pub fn bytes_text_for_log(bytes: &[u8]) -> String {
    let preview = &bytes[..bytes.len().min(LOG_BYTE_PREVIEW_LIMIT)];
    let mut out = String::from_utf8_lossy(preview).escape_debug().to_string();
    if bytes.len() > LOG_BYTE_PREVIEW_LIMIT {
        out.push_str("...");
    }
    out
}

/// True iff `s` is exactly one ASCII alphabetic character.
///
/// Used by the nvim key formatter to decide when to emit a `S-` prefix
/// and when shifted alpha is sufficient.
pub fn is_ascii_alphabetic_str(s: &str) -> bool {
    let mut it = s.chars();
    match (it.next(), it.next()) {
        (Some(c), None) => c.is_ascii_alphabetic(),
        _ => false,
    }
}

/// Renderer-neutral mirror of winit's `NamedKey` — only the subset the
/// nvim formatter cares about. Callers translate their platform key
/// enum to this kind once at the boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NvimNamedKey {
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    Backspace,
    Delete,
    End,
    Enter,
    Escape,
    Home,
    Insert,
    PageDown,
    PageUp,
    Space,
    Tab,
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
}

/// nvim's "special key" name for a given named key.
///
/// Returns `None` for named keys nvim has no token for, in which case
/// callers fall back to emitting the textual representation. The names
/// match nvim's docs (`:h key-notation`).
pub const fn named_key_to_nvim_name(key: NvimNamedKey) -> &'static str {
    match key {
        NvimNamedKey::ArrowDown => "Down",
        NvimNamedKey::ArrowLeft => "Left",
        NvimNamedKey::ArrowRight => "Right",
        NvimNamedKey::ArrowUp => "Up",
        NvimNamedKey::Backspace => "BS",
        NvimNamedKey::Delete => "Del",
        NvimNamedKey::End => "End",
        NvimNamedKey::Enter => "CR",
        NvimNamedKey::Escape => "Esc",
        NvimNamedKey::Home => "Home",
        NvimNamedKey::Insert => "Insert",
        NvimNamedKey::PageDown => "PageDown",
        NvimNamedKey::PageUp => "PageUp",
        NvimNamedKey::Space => "Space",
        NvimNamedKey::Tab => "Tab",
        NvimNamedKey::F1 => "F1",
        NvimNamedKey::F2 => "F2",
        NvimNamedKey::F3 => "F3",
        NvimNamedKey::F4 => "F4",
        NvimNamedKey::F5 => "F5",
        NvimNamedKey::F6 => "F6",
        NvimNamedKey::F7 => "F7",
        NvimNamedKey::F8 => "F8",
        NvimNamedKey::F9 => "F9",
        NvimNamedKey::F10 => "F10",
        NvimNamedKey::F11 => "F11",
        NvimNamedKey::F12 => "F12",
    }
}

/// POD modifier-state bag for the nvim formatter and the kitty
/// sequence gate. Mirrors the bits winit reports without taking a
/// hard dependency on `neoism_window`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LifecycleMods {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub super_key: bool,
}

impl LifecycleMods {
    pub const fn new(shift: bool, control: bool, alt: bool, super_key: bool) -> Self {
        Self {
            shift,
            control,
            alt,
            super_key,
        }
    }

    pub const fn is_empty(self) -> bool {
        !self.shift && !self.control && !self.alt && !self.super_key
    }
}

/// Format a key event for nvim consumption, returning the token nvim
/// expects (e.g. `<C-CR>`, `<S-Esc>`, `a`, `A`, `<lt>`).
///
/// Inputs:
///
/// * `text` — the resolved character text the key produces. Pass empty
///   when there isn't one.
/// * `named` — `Some(NvimNamedKey)` when the key is one of the named
///   keys nvim has a token for; `None` otherwise.
/// * `mods` — modifier state.
///
/// Returns `None` for modifier-only events (no text and no named key),
/// or when the resolved text is all control characters without being
/// a single ASCII alpha.
pub fn format_nvim_key_token(
    text: &str,
    named: Option<NvimNamedKey>,
    mods: LifecycleMods,
) -> Option<String> {
    let (text, is_special) = if let Some(named) = named {
        (named_key_to_nvim_name(named).to_string(), true)
    } else {
        if text.is_empty() {
            return None;
        }
        if text.chars().all(|c| c.is_control()) && !is_ascii_alphabetic_str(text) {
            return None;
        }
        (text.to_string(), false)
    };

    // nvim expects shifted ascii alphas to be uppercase even when
    // the platform reports lowercase + shift state.
    let text = if mods.shift && is_ascii_alphabetic_str(&text) {
        text.to_uppercase()
    } else {
        text
    };

    // `<` is its own special token to avoid being confused for the
    // start of a key notation.
    let (text, is_special) = if text == "<" {
        ("lt".to_string(), true)
    } else {
        (text, is_special)
    };

    // Modifier prefix. Shift is only emitted for special keys and
    // for Ctrl-letter combos (per nvim's normalization rules).
    let include_shift = is_special || (mods.control && is_ascii_alphabetic_str(&text));
    let mut prefix = String::new();
    if mods.shift && include_shift {
        prefix.push_str("S-");
    }
    if mods.control {
        prefix.push_str("C-");
    }
    if mods.alt {
        prefix.push_str("M-");
    }
    if mods.super_key {
        prefix.push_str("D-");
    }

    if prefix.is_empty() {
        if is_special {
            Some(format!("<{text}>"))
        } else {
            Some(text)
        }
    } else {
        Some(format!("<{prefix}{text}>"))
    }
}

/// Convert a monitor refresh rate in millihertz to a frame interval
/// and the floored refresh-rate in hertz.
///
/// Clamps refresh to at least 1 Hz and the resulting frame time to
/// `[1ns, u64::MAX ns]` to mirror the desktop fork's `f64::clamp`
/// invocation. Used by the route-window animation scheduler.
pub fn vblank_interval_from_refresh_rate(
    refresh_rate_millihertz: u32,
) -> (Duration, f64) {
    let refresh_rate_hz = refresh_rate_millihertz as f64 / 1000.0;
    let refresh_rate_hz = refresh_rate_hz.max(1.0);
    let frame_time_ns = (1_000_000_000.0 / refresh_rate_hz)
        .round()
        .clamp(1.0, u64::MAX as f64) as u64;
    (Duration::from_nanos(frame_time_ns), refresh_rate_hz)
}

/// Compute the top-left placement to center a window of size
/// `(width, height)` on a monitor of size `(monitor_w, monitor_h)`
/// whose top-left is at `(monitor_x, monitor_y)`.
///
/// Returns `(x, y)` in the monitor's native pixel coordinate space.
/// Negative results are allowed (matches the desktop fork — when the
/// window is larger than the monitor, it overhangs symmetrically).
pub const fn centered_window_position(
    monitor_x: i32,
    monitor_y: i32,
    monitor_w: u32,
    monitor_h: u32,
    width: u32,
    height: u32,
) -> (i32, i32) {
    let x = monitor_x + (monitor_w as i32 - width as i32) / 2;
    let y = monitor_y + (monitor_h as i32 - height as i32) / 2;
    (x, y)
}

/// POD bag of the cell + margin dimensions
/// [`compute_window_size_from_grid_dims`] needs. Mirrors the desktop
/// fork's `ContextDimension` + `Panel` margins without depending on
/// either layout type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridSizeDims {
    pub cell_width: f32,
    pub cell_height: f32,
    pub scale: f32,
    pub terminal_margin_left: f32,
    pub terminal_margin_right: f32,
    pub terminal_margin_top: f32,
    pub terminal_margin_bottom: f32,
    pub panel_padding_left: f32,
    pub panel_padding_right: f32,
    pub panel_padding_top: f32,
    pub panel_padding_bottom: f32,
    pub panel_margin_left: f32,
    pub panel_margin_right: f32,
    pub panel_margin_top: f32,
    pub panel_margin_bottom: f32,
}

/// Compute the desired window size in physical pixels from optional
/// `columns` × `rows` grid overrides plus the current cell dimensions.
///
/// Mirrors `router_impl::compute_window_size_from_grid` from the
/// desktop fork. Returns `(physical_width, physical_height)` clamped
/// to `min_physical_width` / `min_physical_height`.
///
/// `columns` / `rows` of `None` or `Some(0)` leaves that axis at
/// `default_window_width` / `default_window_height` (matches the
/// desktop fork's `_ => window_size.width` fallback).
pub fn compute_window_size_from_grid_dims(
    columns: Option<u16>,
    rows: Option<u16>,
    dims: &GridSizeDims,
    default_window_width: u32,
    default_window_height: u32,
    min_physical_width: u32,
    min_physical_height: u32,
) -> (u32, u32) {
    let scale = dims.scale;
    let scale_u32 = scale.round().max(1.0) as u32;

    let physical_width = match columns {
        Some(columns) if columns > 0 => {
            let margin = (dims.terminal_margin_left + dims.terminal_margin_right) * scale;
            let panel_edge = (dims.panel_padding_left
                + dims.panel_padding_right
                + dims.panel_margin_left
                + dims.panel_margin_right)
                * scale;
            let raw = (columns as f32 * dims.cell_width).ceil() as u32
                + margin as u32
                + panel_edge as u32;
            raw.next_multiple_of(scale_u32)
        }
        _ => default_window_width,
    };

    let physical_height = match rows {
        Some(rows) if rows > 0 => {
            let margin = (dims.terminal_margin_top + dims.terminal_margin_bottom) * scale;
            let panel_edge = (dims.panel_padding_top
                + dims.panel_padding_bottom
                + dims.panel_margin_top
                + dims.panel_margin_bottom)
                * scale;
            let raw = (rows as f32 * dims.cell_height).ceil() as u32
                + margin as u32
                + panel_edge as u32;
            raw.next_multiple_of(scale_u32)
        }
        _ => default_window_height,
    };

    (
        physical_width.max(min_physical_width),
        physical_height.max(min_physical_height),
    )
}

/// What the font-size key gate decided this key combo should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontSizeAction {
    Increase,
    Decrease,
    Reset,
}

/// Resolve one workspace-wide zoom step without consulting whichever pane
/// happens to be active.
///
/// Keeping this value canonical is important: a newly-created rich-text
/// surface starts at the configured font size, which must not make the next
/// Ctrl/Cmd-minus jump every existing pane back toward that default.
pub fn font_size_after_action(
    current_font_size: f32,
    configured_font_size: f32,
    action: FontSizeAction,
) -> f32 {
    const MIN_FONT_SIZE: f32 = 6.0;
    const MAX_FONT_SIZE: f32 = 100.0;

    let configured = if configured_font_size.is_finite() {
        configured_font_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
    } else {
        14.0
    };
    let current = if current_font_size.is_finite() {
        current_font_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
    } else {
        configured
    };

    match action {
        FontSizeAction::Increase => (current + 1.0).min(MAX_FONT_SIZE),
        FontSizeAction::Decrease => (current - 1.0).max(MIN_FONT_SIZE),
        FontSizeAction::Reset => configured,
    }
}

/// POD input for the font-size key gate. Callers fill from their
/// platform key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FontSizeKeyInput {
    /// Whether the resolved text/character (after IME, after
    /// shift-collapse) matches `=`. Caller checks every channel
    /// (text_with_all_modifiers, text, logical_key, key_without_modifiers).
    pub text_is_equal: bool,
    /// Whether the resolved text/character matches `+`.
    pub text_is_plus: bool,
    /// Whether the resolved text/character matches `-`.
    pub text_is_minus: bool,
    /// Whether the resolved text/character matches `0`.
    pub text_is_zero: bool,
    /// Whether the physical key is `KeyCode::Equal` or `NumpadAdd`.
    pub physical_is_equal_or_numpad_add: bool,
    /// Whether the physical key is `KeyCode::Minus` or `NumpadSubtract`.
    pub physical_is_minus_or_numpad_subtract: bool,
    /// Whether the physical key is `KeyCode::Minus` specifically.
    pub physical_is_minus: bool,
    /// Whether the physical key is `Digit0` or `Numpad0`.
    pub physical_is_zero: bool,
}

/// Map a Cmd/Ctrl+(=/+/-/0) press to a font-size action, or `None` if
/// the modifier combination doesn't qualify.
///
/// The legal modifier combos are exactly: Ctrl+nothing-else or
/// Super(Cmd)+nothing-else. Shift may be held (Shift+= is `+`).
/// Mirrors `Screen::font_size_action_for_key` from the desktop fork.
pub fn font_size_action_decide(
    input: FontSizeKeyInput,
    mods: LifecycleMods,
) -> Option<FontSizeAction> {
    let zoom_modifier = (mods.control && !mods.alt && !mods.super_key)
        || (mods.super_key && !mods.control && !mods.alt);
    if !zoom_modifier {
        return None;
    }

    if input.text_is_equal || input.text_is_plus || input.physical_is_equal_or_numpad_add
    {
        return Some(FontSizeAction::Increase);
    }
    if input.text_is_minus
        || (input.physical_is_minus_or_numpad_subtract && !input.physical_is_minus)
        || (!mods.shift && input.physical_is_minus)
    {
        return Some(FontSizeAction::Decrease);
    }
    if input.text_is_zero || (!mods.shift && input.physical_is_zero) {
        return Some(FontSizeAction::Reset);
    }
    None
}

/// Parse an environment-variable value as a boolean flag.
///
/// Returns `false` when the value is empty/whitespace/missing or one
/// of `"0"`, `"false"`, `"off"`, `"no"` (case-insensitive). Otherwise
/// returns `true`. Mirrors the desktop fork's `env_flag_enabled`
/// helper.
pub fn env_flag_truthy(value: &str) -> bool {
    let value = value.trim();
    !(value.is_empty()
        || value.eq_ignore_ascii_case("0")
        || value.eq_ignore_ascii_case("false")
        || value.eq_ignore_ascii_case("off")
        || value.eq_ignore_ascii_case("no"))
}

/// Multiplier the frame-cadence stats use to flag a frame interval as
/// "over budget". A frame counts as over-budget when its measured
/// inter-frame interval exceeds `target_interval * OVER_BUDGET_MULTIPLIER`.
pub const OVER_BUDGET_MULTIPLIER: f64 = 1.5;

/// True iff `interval` was over the frame budget for `target_interval`.
///
/// Pulled out of `FrameCadenceStats::record_frame_start` so the web
/// frontend can share the same threshold definition.
pub fn frame_over_budget(interval: Duration, target_interval: Duration) -> bool {
    interval.as_secs_f64() > target_interval.as_secs_f64() * OVER_BUDGET_MULTIPLIER
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_hex_for_log_short() {
        assert_eq!(bytes_hex_for_log(&[]), "");
        assert_eq!(bytes_hex_for_log(&[0xde, 0xad]), "de ad");
        assert_eq!(bytes_hex_for_log(&[0x00, 0xff, 0x10]), "00 ff 10");
    }

    #[test]
    fn bytes_hex_for_log_truncates() {
        let bytes = vec![0xab; LOG_BYTE_PREVIEW_LIMIT + 5];
        let out = bytes_hex_for_log(&bytes);
        assert!(out.ends_with(" ..."));
        // 96 bytes × 3 chars per byte ("ab "), minus the trailing space.
        let head = out.trim_end_matches(" ...");
        let groups: Vec<_> = head.split(' ').collect();
        assert_eq!(groups.len(), LOG_BYTE_PREVIEW_LIMIT);
    }

    #[test]
    fn bytes_text_for_log_short() {
        assert_eq!(bytes_text_for_log(b"hi"), "hi");
        assert_eq!(bytes_text_for_log(b"a\nb"), "a\\nb");
    }

    #[test]
    fn bytes_text_for_log_truncates() {
        let bytes = vec![b'x'; LOG_BYTE_PREVIEW_LIMIT + 3];
        let out = bytes_text_for_log(&bytes);
        assert!(out.ends_with("..."));
        // No leading space on the ellipsis — matches the original.
        assert!(!out.ends_with(" ..."));
    }

    #[test]
    fn ascii_alphabetic_single_char() {
        assert!(is_ascii_alphabetic_str("a"));
        assert!(is_ascii_alphabetic_str("Z"));
        assert!(!is_ascii_alphabetic_str(""));
        assert!(!is_ascii_alphabetic_str("ab"));
        assert!(!is_ascii_alphabetic_str("1"));
        assert!(!is_ascii_alphabetic_str("é"));
    }

    #[test]
    fn nvim_key_named_lookup() {
        assert_eq!(named_key_to_nvim_name(NvimNamedKey::Enter), "CR");
        assert_eq!(named_key_to_nvim_name(NvimNamedKey::Escape), "Esc");
        assert_eq!(named_key_to_nvim_name(NvimNamedKey::Backspace), "BS");
        assert_eq!(named_key_to_nvim_name(NvimNamedKey::Delete), "Del");
        assert_eq!(named_key_to_nvim_name(NvimNamedKey::Space), "Space");
        assert_eq!(named_key_to_nvim_name(NvimNamedKey::F12), "F12");
    }

    #[test]
    fn nvim_format_plain_letter() {
        assert_eq!(
            format_nvim_key_token("a", None, LifecycleMods::default()).as_deref(),
            Some("a")
        );
    }

    #[test]
    fn nvim_format_shifted_letter_uppercases() {
        let mods = LifecycleMods::new(true, false, false, false);
        assert_eq!(format_nvim_key_token("a", None, mods).as_deref(), Some("A"));
    }

    #[test]
    fn nvim_format_ctrl_letter_emits_s() {
        // Ctrl+Shift+a → <S-C-A>.
        let mods = LifecycleMods::new(true, true, false, false);
        assert_eq!(
            format_nvim_key_token("a", None, mods).as_deref(),
            Some("<S-C-A>")
        );
    }

    #[test]
    fn nvim_format_lt_special() {
        assert_eq!(
            format_nvim_key_token("<", None, LifecycleMods::default()).as_deref(),
            Some("<lt>")
        );
    }

    #[test]
    fn nvim_format_named_enter() {
        assert_eq!(
            format_nvim_key_token(
                "",
                Some(NvimNamedKey::Enter),
                LifecycleMods::default()
            )
            .as_deref(),
            Some("<CR>")
        );
    }

    #[test]
    fn nvim_format_shift_enter() {
        let mods = LifecycleMods::new(true, false, false, false);
        assert_eq!(
            format_nvim_key_token("", Some(NvimNamedKey::Enter), mods).as_deref(),
            Some("<S-CR>")
        );
    }

    #[test]
    fn nvim_format_modifier_only_returns_none() {
        assert_eq!(
            format_nvim_key_token("", None, LifecycleMods::default()),
            None
        );
    }

    #[test]
    fn nvim_format_control_chars_dropped() {
        // BEL is a single control char that's not ascii alphabetic →
        // formatter drops it.
        assert_eq!(
            format_nvim_key_token("\u{07}", None, LifecycleMods::default()),
            None
        );
    }

    #[test]
    fn vblank_interval_60hz() {
        let (interval, hz) = vblank_interval_from_refresh_rate(60_000);
        assert_eq!(hz, 60.0);
        // 1e9 / 60 = 16_666_666.67 → rounds to 16_666_667.
        assert_eq!(interval, Duration::from_nanos(16_666_667));
    }

    #[test]
    fn vblank_interval_clamps_to_1hz() {
        // 0 millihertz → clamped to 1 Hz → 1s interval.
        let (interval, hz) = vblank_interval_from_refresh_rate(0);
        assert_eq!(hz, 1.0);
        assert_eq!(interval, Duration::from_secs(1));
    }

    #[test]
    fn centered_position_basic() {
        // Monitor at (0,0), 1000×800; window 200×100 → centered (400, 350).
        assert_eq!(
            centered_window_position(0, 0, 1000, 800, 200, 100),
            (400, 350)
        );
    }

    #[test]
    fn centered_position_offset_monitor() {
        assert_eq!(
            centered_window_position(100, 50, 1000, 800, 200, 100),
            (500, 400)
        );
    }

    #[test]
    fn centered_position_window_larger_than_monitor() {
        // Window wider than monitor → negative x — matches the desktop
        // fork's behavior (overhangs symmetrically).
        let (x, _) = centered_window_position(0, 0, 100, 200, 300, 200);
        assert_eq!(x, -100);
    }

    fn zero_dims(cell_w: f32, cell_h: f32, scale: f32) -> GridSizeDims {
        GridSizeDims {
            cell_width: cell_w,
            cell_height: cell_h,
            scale,
            terminal_margin_left: 0.0,
            terminal_margin_right: 0.0,
            terminal_margin_top: 0.0,
            terminal_margin_bottom: 0.0,
            panel_padding_left: 0.0,
            panel_padding_right: 0.0,
            panel_padding_top: 0.0,
            panel_padding_bottom: 0.0,
            panel_margin_left: 0.0,
            panel_margin_right: 0.0,
            panel_margin_top: 0.0,
            panel_margin_bottom: 0.0,
        }
    }

    #[test]
    fn grid_dims_columns_only() {
        let dims = zero_dims(10.0, 20.0, 2.0);
        // 80 cols × 10 = 800 → next_multiple_of(2) = 800; rows None →
        // keep default height.
        let (w, h) =
            compute_window_size_from_grid_dims(Some(80), None, &dims, 500, 300, 100, 100);
        assert_eq!((w, h), (800, 300));
    }

    #[test]
    fn grid_dims_min_floor() {
        let dims = zero_dims(10.0, 20.0, 1.0);
        let (w, h) = compute_window_size_from_grid_dims(
            Some(10),
            Some(5),
            &dims,
            500,
            300,
            500,
            500,
        );
        // 10*10=100 → min floor 500; 5*20=100 → min floor 500.
        assert_eq!((w, h), (500, 500));
    }

    #[test]
    fn grid_dims_zero_overrides_skipped() {
        let dims = zero_dims(10.0, 20.0, 1.0);
        let (w, h) = compute_window_size_from_grid_dims(
            Some(0),
            Some(0),
            &dims,
            777,
            555,
            100,
            100,
        );
        assert_eq!((w, h), (777, 555));
    }

    #[test]
    fn font_size_no_modifier_returns_none() {
        let input = FontSizeKeyInput {
            text_is_equal: true,
            text_is_plus: false,
            text_is_minus: false,
            text_is_zero: false,
            physical_is_equal_or_numpad_add: false,
            physical_is_minus_or_numpad_subtract: false,
            physical_is_minus: false,
            physical_is_zero: false,
        };
        assert_eq!(
            font_size_action_decide(input, LifecycleMods::default()),
            None
        );
    }

    #[test]
    fn font_size_ctrl_equal_increases() {
        let input = FontSizeKeyInput {
            text_is_equal: true,
            text_is_plus: false,
            text_is_minus: false,
            text_is_zero: false,
            physical_is_equal_or_numpad_add: false,
            physical_is_minus_or_numpad_subtract: false,
            physical_is_minus: false,
            physical_is_zero: false,
        };
        let mods = LifecycleMods::new(false, true, false, false);
        assert_eq!(
            font_size_action_decide(input, mods),
            Some(FontSizeAction::Increase)
        );
    }

    #[test]
    fn font_size_super_minus_decreases() {
        let input = FontSizeKeyInput {
            text_is_equal: false,
            text_is_plus: false,
            text_is_minus: true,
            text_is_zero: false,
            physical_is_equal_or_numpad_add: false,
            physical_is_minus_or_numpad_subtract: false,
            physical_is_minus: false,
            physical_is_zero: false,
        };
        let mods = LifecycleMods::new(false, false, false, true);
        assert_eq!(
            font_size_action_decide(input, mods),
            Some(FontSizeAction::Decrease)
        );
    }

    #[test]
    fn font_size_ctrl_zero_resets() {
        let input = FontSizeKeyInput {
            text_is_equal: false,
            text_is_plus: false,
            text_is_minus: false,
            text_is_zero: true,
            physical_is_equal_or_numpad_add: false,
            physical_is_minus_or_numpad_subtract: false,
            physical_is_minus: false,
            physical_is_zero: false,
        };
        let mods = LifecycleMods::new(false, true, false, false);
        assert_eq!(
            font_size_action_decide(input, mods),
            Some(FontSizeAction::Reset)
        );
    }

    #[test]
    fn font_size_ctrl_alt_rejected() {
        let input = FontSizeKeyInput {
            text_is_equal: true,
            text_is_plus: false,
            text_is_minus: false,
            text_is_zero: false,
            physical_is_equal_or_numpad_add: false,
            physical_is_minus_or_numpad_subtract: false,
            physical_is_minus: false,
            physical_is_zero: false,
        };
        let mods = LifecycleMods::new(false, true, true, false);
        assert_eq!(font_size_action_decide(input, mods), None);
    }

    #[test]
    fn font_size_step_uses_canonical_zoom_not_a_new_pane_default() {
        assert_eq!(
            font_size_after_action(52.0, 14.0, FontSizeAction::Decrease),
            51.0
        );
        assert_eq!(
            font_size_after_action(51.0, 14.0, FontSizeAction::Increase),
            52.0
        );
        assert_eq!(
            font_size_after_action(52.0, 14.0, FontSizeAction::Reset),
            14.0
        );
    }

    #[test]
    fn font_size_step_clamps_and_recovers_from_non_finite_state() {
        assert_eq!(
            font_size_after_action(100.0, 14.0, FontSizeAction::Increase),
            100.0
        );
        assert_eq!(
            font_size_after_action(6.0, 14.0, FontSizeAction::Decrease),
            6.0
        );
        assert_eq!(
            font_size_after_action(f32::NAN, 16.0, FontSizeAction::Decrease),
            15.0
        );
    }

    #[test]
    fn env_flag_truthy_basic() {
        assert!(env_flag_truthy("1"));
        assert!(env_flag_truthy("true"));
        assert!(env_flag_truthy("yes"));
        assert!(env_flag_truthy("on"));
        assert!(env_flag_truthy("anything"));
        assert!(!env_flag_truthy(""));
        assert!(!env_flag_truthy("   "));
        assert!(!env_flag_truthy("0"));
        assert!(!env_flag_truthy("FALSE"));
        assert!(!env_flag_truthy("Off"));
        assert!(!env_flag_truthy("No"));
    }

    #[test]
    fn over_budget_threshold() {
        let target = Duration::from_millis(16);
        // 16ms target, 23ms actual → still under 24ms (16 × 1.5) → not over.
        assert!(!frame_over_budget(Duration::from_millis(23), target));
        // 25ms → over.
        assert!(frame_over_budget(Duration::from_millis(25), target));
    }
}
