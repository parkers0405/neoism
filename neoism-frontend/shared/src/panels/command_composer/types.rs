//! Public types and shared constants used across the composer modules.
//!
//! - `ComposerFrame` is the per-frame snapshot returned to the screen.
//! - `InputTextStyle` / `InputClassification` describe the zsh-syntax-
//!   highlighting style classification handed in by the renderer.
//! - `StyledSpan`, `WrappedLine`, `InputWrapLayout` are internal helpers
//!   shared between the classifier, wrap, render, and update modules.

/// Pixel fallback for non-grid callers. Normal terminal rendering uses
/// row-based reservation below: command input lines + one hint row.
pub const COMPOSER_BASE_HEIGHT: f32 = 44.0;
pub(super) const FONT_SIZE: f32 = 13.0;
pub(super) const HINT_FONT_SIZE: f32 = 10.5;
pub(super) const SHELL_BADGE_FONT_SIZE: f32 = 12.0;
/// Matches the status line's faux-bold pass so the shell badge has the
/// same visual stroke weight as the bottom status chrome.
pub(super) const FAUX_BOLD_OFFSET: f32 = 0.6;
/// Edge-to-edge — chassis spans the full pane width. The user wants
/// the composer band to read as part of the pane chrome, not a
/// floating chip inset from the sides.
pub(super) const OUTER_PAD_X: f32 = 0.0;
/// No external gap above the status strip. The status line has a
/// higher draw order and covers the join; adding a bottom offset here
/// would push the rounded top into the last reserved terminal row.
pub const COMPOSER_BOTTOM_PAD: f32 = 0.0;
/// Distance the chassis BG / rounded plate paints above the cell-aligned
/// `chassis_y`. Must equal the `top_pad` shrink applied in
/// `renderer/mod.rs::render_command_composer` so the rounded lip stays
/// inside the reserved composer band instead of covering output.
pub const COMPOSER_TOP_OVERHANG: f32 = 14.0;
pub(super) const CHASSIS_RADIUS: f32 = 14.0;
pub(super) const CHIP_RADIUS: f32 = 6.0;
pub(super) const CHIP_PAD_X: f32 = 8.0;
pub(super) const CHIP_GAP: f32 = 8.0;
/// Temporary visual pass: hide the lower shell badge / shortcut hint
/// row while keeping its implementation below easy to re-enable.
pub(super) const SHOW_FOOTER_HINT_ROW: bool = false;
/// Fallback caret blink half-period when the active terminal has no
/// blink interval (config disabled). Matches the trail_cursor system's
/// idle cadence so the composer's caret never feels out of sync.
pub(super) const CARET_BLINK_FALLBACK_MS: f32 = 530.0;
pub(super) const PROMPT_BURST_MS: f32 = 320.0;
pub(super) const SHELL_TRANSITION_MS: f32 = 320.0;
/// Grow-with-content ceiling for the input area. Beyond this the text
/// window scrolls internally (cursor-anchored) and the render pass
/// paints a "N more lines" pill for anything hidden above/below.
/// Reservation call sites additionally clamp against the pane height
/// so short panes never lose more than roughly half their rows.
pub(super) const COMPOSER_MAX_INPUT_LINES: usize = 10;
pub(super) const COMPOSER_WRAP_HARD_LIMIT: usize = 512;
pub(super) const COMPLETION_POP_MS: f32 = 180.0;
pub(super) const COMPLETION_MAX_VISIBLE_RESULTS: usize = 8;
pub(super) const COMPLETION_ROW_HEIGHT: f32 = 32.0;
pub(super) const COMPLETION_FONT_SIZE: f32 = 13.0;
pub(super) const COMPLETION_SCROLL_ANIMATION_LENGTH: f32 = 0.30;
pub(super) const COMPLETION_CURSOR_ANIMATION_LENGTH: f32 = 0.12;
pub(super) const COMPLETION_SCROLL_OFF_ROWS: usize = 2;
pub(super) const PROMPT_CHEVRONS: usize = 3;
pub(super) const PROMPT_SCRAMBLE: &[u8] = b">/\\|*+#?";
pub(super) const SHELL_SCRAMBLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
pub(super) const GLYPH_SHELL: &str = "\u{f120}";
pub(super) const SCALE_MIN: f32 = 0.5;
pub(super) const SCALE_MAX: f32 = 3.0;

pub(super) const DEPTH: f32 = 0.0;
pub(super) const ORDER_CHASSIS_BG: u8 = 12;
pub(super) const ORDER_CHASSIS_BORDER: u8 = 13;
pub(super) const ORDER_CHIP_BG: u8 = 14;
pub(super) const ORDER_CARET: u8 = 15;
pub(super) const ORDER_STATUS_JOIN: u8 = 20;

/// Per-frame snapshot the screen needs to keep the rest of the system
/// honest — primarily the caret rect for the cursor-visibility damage
/// path and the chassis bounds for hit-testing future button work.
#[derive(Clone, Copy, Debug, Default)]
pub struct ComposerFrame {
    /// Chassis outer rect — reserved for upcoming hit-test work
    /// (clicking the composer to focus the active terminal pane,
    /// dragging to resize, etc.).
    #[allow(dead_code)]
    pub chassis_rect: [f32; 4],
    pub caret_rect: Option<[f32; 4]>,
    /// Painted text band, in host logical pixels; owns wheel input at edges too.
    pub text_viewport_rect: [f32; 4],
    /// Send chip rect — reserved for upcoming hit-test work
    /// (clicking the chip to submit the buffered command).
    #[allow(dead_code)]
    pub send_chip_rect: [f32; 4],
}

/// zsh-syntax-highlighting style classification of the buffered input.
/// Built by the renderer (which has the executable cache) and handed to
/// the composer so paint stays cheap. The palette mirrors the user's
/// zsh-syntax-highlighting defaults: thick/bold colored command words,
/// yellow strings/package tools, blue underlined paths, purple
/// redirections/reserved words, and red dangerous/unknown tokens.
#[derive(Clone, Copy, Debug)]
pub struct InputTextStyle {
    pub color: [u8; 4],
    pub bold: bool,
    pub underline: bool,
}

impl InputTextStyle {
    pub fn plain(color: [u8; 4]) -> Self {
        Self {
            color,
            bold: false,
            underline: false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct InputClassification {
    pub command: InputTextStyle,
    pub arg: InputTextStyle,
    pub string: InputTextStyle,
    pub path: InputTextStyle,
    pub glob: InputTextStyle,
    pub redirection: InputTextStyle,
}

impl InputClassification {
    pub fn neutral(fg: [u8; 4]) -> Self {
        let plain = InputTextStyle::plain(fg);
        Self {
            command: plain,
            arg: plain,
            string: plain,
            path: plain,
            glob: plain,
            redirection: plain,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct StyledSpan {
    pub start: usize,
    pub end: usize,
    pub style: InputTextStyle,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct WrappedLine {
    pub start: usize,
    pub end: usize,
    pub width_limit: f32,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct InputWrapLayout {
    pub first_width: f32,
    pub wrapped_width: f32,
    pub cell_width: f32,
}
