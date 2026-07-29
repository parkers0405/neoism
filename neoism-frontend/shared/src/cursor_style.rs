//! User cursor styling: a hex color that overrides the theme's cursor
//! accent, plus animated presets (rainbow first; more later).
//!
//! One source of truth for every screen — the desktop renderer, the
//! wasm chrome, and the remote-caret painters all resolve the live
//! cursor color through here so a picked color or a rainbow cursor
//! looks identical locally and on collaborators' screens.
//!
//! Rainbow is CPU-side hue cycling (the same trick as the agent
//! pane's thinking scramble in `panels/agent_pane/view/user_input.rs`):
//! every frame derives the color from a shared process clock, so all
//! rainbow cursors in one window — local AND remote peers' — sweep in
//! phase instead of strobing independently.

use std::sync::OnceLock;

use web_time::Instant;

/// Which preset paints the cursor. `Solid` uses the theme accent or
/// the user's `[neoism] cursor-color` override; animated presets
/// ignore the static color entirely.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorStyle {
    #[default]
    Solid,
    Rainbow,
}

impl CursorStyle {
    /// Config-string mapping (`[neoism] cursor-style`). Unknown names
    /// fall back to `Solid` so a typo never bricks the cursor.
    pub fn from_str(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "rainbow" => CursorStyle::Rainbow,
            _ => CursorStyle::Solid,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            CursorStyle::Solid => "solid",
            CursorStyle::Rainbow => "rainbow",
        }
    }

    pub fn is_animated(self) -> bool {
        matches!(self, CursorStyle::Rainbow)
    }
}

/// Parse `#RRGGBB` / `RRGGBB` / `#RGB` into `0xRRGGBB`. Returns `None`
/// for anything else so callers can fall back to the theme accent.
pub fn parse_hex_color(hex: &str) -> Option<u32> {
    let digits = hex.trim().trim_start_matches('#');
    match digits.len() {
        6 => u32::from_str_radix(digits, 16).ok(),
        3 => {
            let short = u32::from_str_radix(digits, 16).ok()?;
            let r = (short >> 8) & 0xf;
            let g = (short >> 4) & 0xf;
            let b = short & 0xf;
            Some((r * 0x11) << 16 | (g * 0x11) << 8 | (b * 0x11))
        }
        _ => None,
    }
}

/// Full hue sweep every 3 seconds — fast enough to read as "rainbow",
/// slow enough not to strobe.
const RAINBOW_HUE_DEG_PER_SEC: f32 = 120.0;
const RAINBOW_SATURATION: f32 = 0.85;
const RAINBOW_LIGHTNESS: f32 = 0.6;

/// Shared process clock for rainbow phase. Seconds since first call —
/// small values, so f32 keeps millisecond precision for days (a unix
/// epoch f32 quantizes to ~128s and would freeze the animation).
/// `web_time` keeps this wasm-safe (std `Instant` panics on wasm32).
pub fn rainbow_now_seconds() -> f32 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_secs_f32()
}

/// Viewer-local caret blink on the shared process clock. Pass
/// `now_seconds` from [`rainbow_now_seconds`] (a.k.a.
/// `presence_orb_now_seconds()` — the SAME clock the presence redraw
/// owner already ticks), so a remote caret pulses in phase without any
/// synced state and animates for free while a peer is on screen.
///
/// Cadence mirrors the local caret's `DEFAULT_BLINK_PERIOD_MS` square
/// wave in [`crate::editor::optimistic`]: solid for the first half of
/// each period, hidden for the second — a remote caret blinks
/// identically to the local one, just off the viewer's own clock.
pub fn cursor_blink_visible(now_seconds: f32) -> bool {
    let half_period_secs =
        crate::editor::optimistic::DEFAULT_BLINK_PERIOD_MS.max(2) as f32 / 1000.0 / 2.0;
    if half_period_secs <= 0.0 {
        return true;
    }
    ((now_seconds / half_period_secs).floor() as i64).rem_euclid(2) == 0
}

/// The rainbow color for `now_seconds` on the shared clock.
pub fn rainbow_color_f32(now_seconds: f32) -> [f32; 4] {
    let hue = (now_seconds * RAINBOW_HUE_DEG_PER_SEC).rem_euclid(360.0);
    hsl_to_f32(hue, RAINBOW_SATURATION, RAINBOW_LIGHTNESS)
}

pub fn rainbow_color_u8(now_seconds: f32) -> [u8; 4] {
    let c = rainbow_color_f32(now_seconds);
    [
        (c[0] * 255.0) as u8,
        (c[1] * 255.0) as u8,
        (c[2] * 255.0) as u8,
        255,
    ]
}

/// Phase-offset variant so multiple rainbow elements (e.g. several
/// remote peers) can fan out instead of being identical, while still
/// riding the same clock.
pub fn rainbow_color_f32_offset(now_seconds: f32, phase_deg: f32) -> [f32; 4] {
    let hue = (now_seconds * RAINBOW_HUE_DEG_PER_SEC + phase_deg).rem_euclid(360.0);
    hsl_to_f32(hue, RAINBOW_SATURATION, RAINBOW_LIGHTNESS)
}

pub fn hex_to_f32(color: u32) -> [f32; 4] {
    [
        ((color >> 16) & 0xff) as f32 / 255.0,
        ((color >> 8) & 0xff) as f32 / 255.0,
        (color & 0xff) as f32 / 255.0,
        1.0,
    ]
}

pub fn hex_to_u8(color: u32) -> [u8; 3] {
    [
        ((color >> 16) & 0xff) as u8,
        ((color >> 8) & 0xff) as u8,
        (color & 0xff) as u8,
    ]
}

fn hsl_to_f32(h: f32, s: f32, l: f32) -> [f32; 4] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h_prime = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - ((h_prime % 2.0) - 1.0).abs());
    let (r1, g1, b1) = match h_prime as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [
        (r1 + m).clamp(0.0, 1.0),
        (g1 + m).clamp(0.0, 1.0),
        (b1 + m).clamp(0.0, 1.0),
        1.0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_colors_with_and_without_hash() {
        assert_eq!(parse_hex_color("#ff00ff"), Some(0xff00ff));
        assert_eq!(parse_hex_color("1A2b3C"), Some(0x1a2b3c));
        assert_eq!(parse_hex_color("#f0f"), Some(0xff00ff));
        assert_eq!(parse_hex_color(" #aabbcc "), Some(0xaabbcc));
        assert_eq!(parse_hex_color("not-a-color"), None);
        assert_eq!(parse_hex_color("#12345"), None);
        assert_eq!(parse_hex_color(""), None);
    }

    #[test]
    fn cursor_style_round_trips_config_names() {
        assert_eq!(CursorStyle::from_str("rainbow"), CursorStyle::Rainbow);
        assert_eq!(CursorStyle::from_str("RAINBOW"), CursorStyle::Rainbow);
        assert_eq!(CursorStyle::from_str("solid"), CursorStyle::Solid);
        assert_eq!(CursorStyle::from_str("typo"), CursorStyle::Solid);
        assert!(CursorStyle::Rainbow.is_animated());
        assert!(!CursorStyle::Solid.is_animated());
    }

    #[test]
    fn rainbow_cycles_through_distinct_hues() {
        let a = rainbow_color_f32(0.0);
        let b = rainbow_color_f32(1.0);
        let c = rainbow_color_f32(2.0);
        assert_ne!(a, b);
        assert_ne!(b, c);
        // 3s period: same phase one full sweep later.
        let wrapped = rainbow_color_f32(3.0);
        for (x, y) in a.iter().zip(wrapped.iter()) {
            assert!((x - y).abs() < 1e-4);
        }
    }
}
