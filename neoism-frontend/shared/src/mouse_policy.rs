use neoism_terminal_types::position::{Column, Line, Pos, Side};

/// Determine which side of a terminal cell a mouse x-position falls on.
///
/// `margin_x` is already pre-scaled in physical pixels, `cell_width` is the
/// float cell width, and `grid_width` is the total grid area width in physical
/// pixels.
///
/// Uses a 60% threshold. Clicks land on a cell until the cursor is past 60%
/// across it, and a drag must cross 60% of the next cell before it is included.
#[inline]
pub fn cell_side_by_pos(
    x: usize,
    margin_x: f32,
    cell_width: f32,
    grid_width: f32,
) -> Side {
    let x_in_grid = (x as f32 - margin_x).max(0.0);
    let cell_x = x_in_grid % cell_width;
    let threshold = cell_width * 0.6;

    let additional_padding = (grid_width - margin_x) % cell_width;
    let end_of_grid = grid_width - margin_x - additional_padding;

    if cell_x >= threshold || x as f32 >= end_of_grid {
        Side::Right
    } else {
        Side::Left
    }
}

/// Modifier bitset used by terminal mouse reports.
///
/// Mirrors the VT/xterm legacy encoding: shift adds 4, alt adds 8, control
/// adds 16. The decision lives here so the desktop and web frontends emit the
/// same bytes for the same modifier state without copy-pasting magic numbers.
#[inline]
pub fn mouse_report_modifier_bits(shift: bool, alt: bool, control: bool) -> u8 {
    let mut mods = 0u8;
    if shift {
        mods += 4;
    }
    if alt {
        mods += 8;
    }
    if control {
        mods += 16;
    }
    mods
}

/// Pick the encoded button byte for a non-SGR mouse report.
///
/// SGR reports always carry the original button code plus a separate
/// pressed/released indicator. Legacy (non-SGR) reports collapse all releases
/// onto button code 3, so the desktop frontend used to branch on
/// `ElementState`. This pure helper takes a boolean (`pressed`) and the raw
/// button, applies the mods, and returns the final byte. Callers in SGR mode
/// should call this with `pressed = true` (i.e. just `button + mods`).
#[inline]
pub fn mouse_report_legacy_button_byte(button: u8, mods: u8, pressed: bool) -> u8 {
    if pressed {
        button + mods
    } else {
        3 + mods
    }
}

/// Maximum column or row value the legacy mouse reporting protocol can encode.
///
/// xterm's non-UTF8 mouse protocol caps positions at 223 (offset 32 + 1 from
/// 255). The UTF8 extension widens this to 2015. Callers above the cap must
/// suppress the report entirely — there is no graceful truncation.
#[inline]
pub fn legacy_mouse_report_max_point(utf8: bool) -> u16 {
    if utf8 {
        2015
    } else {
        223
    }
}

/// Encode a legacy (non-SGR) mouse report.
///
/// Returns `None` if `row` or `col` exceed the protocol's encodable range —
/// see [`legacy_mouse_report_max_point`]. When UTF8 mode is on, positions at
/// or above column 95 (and row 95 for the row dimension) use the two-byte
/// extended encoding; otherwise a single byte is emitted.
///
/// The result is the full byte sequence the PTY should receive, including the
/// `\x1b[M` lead-in.
pub fn encode_normal_mouse_report(
    position: Pos,
    button: u8,
    utf8: bool,
) -> Option<Vec<u8>> {
    let Pos { row, col } = position;
    let max_point = legacy_mouse_report_max_point(utf8);
    if row.0 as u16 >= max_point || col.0 as u16 >= max_point {
        return None;
    }

    let mut msg = vec![b'\x1b', b'[', b'M', 32 + button];

    let encode_extended = |pos: usize| -> [u8; 2] {
        let pos = 32 + 1 + pos;
        let first = 0xC0 + pos / 64;
        let second = 0x80 + (pos & 63);
        [first as u8, second as u8]
    };

    if utf8 && col >= Column(95) {
        msg.extend_from_slice(&encode_extended(col.0));
    } else {
        msg.push(32 + 1 + col.0 as u8);
    }

    if utf8 && row >= Line(95) {
        msg.extend_from_slice(&encode_extended(row.0 as usize));
    } else {
        msg.push(32 + 1 + row.0 as u8);
    }

    Some(msg)
}

/// Encode an SGR mouse report.
///
/// SGR reports carry full position/button info plus a final `M` (press) or
/// `m` (release) byte. Coordinates are 1-based.
pub fn encode_sgr_mouse_report(position: Pos, button: u8, pressed: bool) -> Vec<u8> {
    let c = if pressed { 'M' } else { 'm' };
    format!(
        "\x1b[<{};{};{}{}",
        button,
        position.col + 1,
        position.row + 1,
        c
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_side_uses_sixty_percent_threshold_with_float_precision() {
        let cell_width = 16.41_f32;
        let margin_x = 8.0;
        let grid_width = 8.0 + 96.0 * cell_width;

        assert_eq!(
            cell_side_by_pos(16, margin_x, cell_width, grid_width),
            Side::Left,
        );
        assert_eq!(
            cell_side_by_pos(17, margin_x, cell_width, grid_width),
            Side::Left,
        );
        assert_eq!(
            cell_side_by_pos(18, margin_x, cell_width, grid_width),
            Side::Right,
        );
    }

    #[test]
    fn cell_side_does_not_drift_at_high_columns() {
        let cell_width = 16.41_f32;
        let margin_x = 8.0;
        let grid_width = 8.0 + 96.0 * cell_width;

        assert_eq!(
            cell_side_by_pos(1256, margin_x, cell_width, grid_width),
            Side::Left,
        );
        assert_eq!(
            cell_side_by_pos(1266, margin_x, cell_width, grid_width),
            Side::Right,
        );
    }

    #[test]
    fn cell_side_uses_prescaled_margin() {
        let cell_width = 16.0;
        let margin_x = 40.0;
        let grid_width = 40.0 + 80.0 * 16.0;

        assert_eq!(
            cell_side_by_pos(41, margin_x, cell_width, grid_width),
            Side::Left,
        );
        assert_eq!(
            cell_side_by_pos(50, margin_x, cell_width, grid_width),
            Side::Right,
        );
        assert_eq!(
            cell_side_by_pos(49, margin_x, cell_width, grid_width),
            Side::Left,
        );
        assert_eq!(
            cell_side_by_pos(30, margin_x, cell_width, grid_width),
            Side::Left,
        );
    }

    #[test]
    fn cell_side_treats_grid_padding_as_right_side() {
        let cell_width = 16.0;
        let margin_x = 0.0;
        let grid_width = 3.0 * cell_width + 6.0;

        assert_eq!(
            cell_side_by_pos(33, margin_x, cell_width, grid_width),
            Side::Left,
        );
        assert_eq!(
            cell_side_by_pos(48, margin_x, cell_width, grid_width),
            Side::Right,
        );
    }

    #[test]
    fn mouse_report_modifier_bits_combine_additively() {
        assert_eq!(mouse_report_modifier_bits(false, false, false), 0);
        assert_eq!(mouse_report_modifier_bits(true, false, false), 4);
        assert_eq!(mouse_report_modifier_bits(false, true, false), 8);
        assert_eq!(mouse_report_modifier_bits(false, false, true), 16);
        assert_eq!(mouse_report_modifier_bits(true, true, true), 28);
    }

    #[test]
    fn legacy_button_byte_collapses_release_to_three() {
        // Press: button code preserved.
        assert_eq!(mouse_report_legacy_button_byte(0, 4, true), 4);
        assert_eq!(mouse_report_legacy_button_byte(2, 16, true), 18);
        // Release: button code dropped, mods preserved.
        assert_eq!(mouse_report_legacy_button_byte(0, 4, false), 7);
        assert_eq!(mouse_report_legacy_button_byte(2, 16, false), 19);
    }

    #[test]
    fn legacy_max_point_widens_for_utf8() {
        assert_eq!(legacy_mouse_report_max_point(false), 223);
        assert_eq!(legacy_mouse_report_max_point(true), 2015);
    }

    #[test]
    fn normal_mouse_report_rejects_out_of_range_positions() {
        // Non-utf8 cap is 223 (>= rejects).
        assert!(
            encode_normal_mouse_report(Pos::new(Line(0), Column(223)), 0, false,)
                .is_none()
        );
        assert!(
            encode_normal_mouse_report(Pos::new(Line(223), Column(0)), 0, false,)
                .is_none()
        );
        // UTF8 extends cap to 2015.
        assert!(
            encode_normal_mouse_report(Pos::new(Line(0), Column(223)), 0, true,)
                .is_some()
        );
    }

    #[test]
    fn normal_mouse_report_encodes_lead_in_and_offsets() {
        let bytes = encode_normal_mouse_report(Pos::new(Line(0), Column(0)), 32, false)
            .expect("in range");
        assert_eq!(bytes[..3], [b'\x1b', b'[', b'M']);
        assert_eq!(bytes[3], 32 + 32);
        // Both col & row encode as 32 + 1 + 0 == 33.
        assert_eq!(bytes[4], 33);
        assert_eq!(bytes[5], 33);
        assert_eq!(bytes.len(), 6);
    }

    #[test]
    fn normal_mouse_report_uses_two_byte_encoding_in_utf8_above_95() {
        // col 95 should switch to the two-byte form when utf8 is on.
        let bytes = encode_normal_mouse_report(Pos::new(Line(0), Column(100)), 0, true)
            .expect("in range");
        // 4 lead-in bytes + 2 col bytes + 1 row byte.
        assert_eq!(bytes.len(), 4 + 2 + 1);
        // Non-utf8 falls back to single byte even past 95 (lossy, but matches legacy).
        let bytes = encode_normal_mouse_report(Pos::new(Line(0), Column(100)), 0, false)
            .expect("in range");
        assert_eq!(bytes.len(), 4 + 1 + 1);
    }

    #[test]
    fn sgr_mouse_report_uses_uppercase_m_for_press_and_lowercase_for_release() {
        let press = encode_sgr_mouse_report(Pos::new(Line(4), Column(9)), 0, true);
        assert_eq!(press, b"\x1b[<0;10;5M");
        let release = encode_sgr_mouse_report(Pos::new(Line(4), Column(9)), 0, false);
        assert_eq!(release, b"\x1b[<0;10;5m");
    }
}
