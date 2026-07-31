use super::*;

// ---------------------------------------------------------------------------
// Block-header chrome layout policy.
//
// The active terminal pane composes a band of "chrome rows" above each
// Warp-style command block. The native renderer historically computed
// every per-frame layout number (panel rects, font size, anchor row,
// icon positions) inline inside the giant `Screen::render` impl. Those
// computations are pure POD math: they take cell width / cell height,
// the panel's logical bounds, and a few scroll-related residuals, and
// return geometry the host paints with. Moving them here keeps the
// fork minimal and lets a web host share the same Warp-style block UI.
// ---------------------------------------------------------------------------

/// Inputs for `block_header_panel_geometry` — everything the active
/// terminal pane already knows from sugarloaf's per-frame layout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockHeaderPanelGeometryInput {
    /// Reuse the shared physical-pixel pane layout. The block-header
    /// overlay paints at logical pixels, so the geometry's `panel_rect`,
    /// `scaled_margin`, `cell_width`, `cell_height` and `columns` are
    /// divided by `scale_factor` below.
    pub grid: GridPanelGeometry,
    /// Terminal smooth-scroll residual in physical pixels. The chrome
    /// overlay shifts with the underlying cells so block headers slide
    /// into view exactly like the PTY rows beneath them.
    pub terminal_scroll_offset_phys: f32,
    /// Visible content rows excluding the off-grid composer band — the
    /// vertical extent of `content_clip_logical`.
    pub terminal_content_rows: u32,
    /// Font size in physical pixels selected for this pane.
    pub font_px_phys: f32,
    /// HiDPI scale factor (sugarloaf logical->physical multiplier).
    pub scale_factor: f32,
}

/// Pure layout output: every value is in logical pixels, ready to feed
/// into either sugarloaf primitives (native) or a web canvas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockHeaderPanelGeometry {
    /// Y of the cell grid's top row, already shifted by the terminal
    /// scroll residual so chrome rides with the rows.
    pub panel_top_logical: f32,
    /// Left/right edges of the panel's chrome strip in logical pixels.
    pub panel_left_logical: f32,
    pub panel_right_logical: f32,
    /// Logical cell width/height (after the scale divide + min clamp).
    pub cell_w_logical: f32,
    pub cell_h_logical: f32,
    /// Logical-pixel font size selected for chrome glyphs.
    pub font_size_logical: f32,
    /// Static viewport clip [x, y, w, h]; matches the GPU's terminal
    /// viewport so chrome painted into the clip can reveal partial rows
    /// at the edge instead of bleeding into the chrome bar.
    pub content_clip_logical: [f32; 4],
}

/// Compute the logical-pixel geometry the active pane uses to paint
/// per-block chrome (the rows reserved above every command block).
///
/// Mirrors the inline math previously in `Screen::render` around the
/// `ActiveBlockHeaders` capture: panel top is shifted by the terminal
/// smooth-scroll residual so chrome rides with the PTY cells, while
/// `content_clip_logical` stays anchored to the static viewport so
/// chrome edges can be partially clipped exactly like a row of text.
pub fn block_header_panel_geometry(
    input: BlockHeaderPanelGeometryInput,
) -> BlockHeaderPanelGeometry {
    let scale = if input.scale_factor > 0.0 && input.scale_factor.is_finite() {
        input.scale_factor
    } else {
        1.0
    };
    let cell_w_logical = (input.grid.cell_width / scale).max(1.0);
    let cell_h_logical = (input.grid.cell_height / scale).max(1.0);
    let panel_top_logical_raw =
        (input.grid.panel_rect[1] + input.grid.scaled_margin.top) / scale;
    let terminal_scroll_offset_logical = input.terminal_scroll_offset_phys / scale;
    let panel_top_logical = panel_top_logical_raw + terminal_scroll_offset_logical;
    let panel_left_logical =
        (input.grid.panel_rect[0] + input.grid.scaled_margin.left) / scale;
    let columns_for_width = input.grid.columns.max(1) as f32;
    let panel_right_logical = panel_left_logical + columns_for_width * cell_w_logical;
    let content_clip_logical = [
        panel_left_logical,
        panel_top_logical_raw,
        (panel_right_logical - panel_left_logical).max(0.0),
        input.terminal_content_rows as f32 * cell_h_logical,
    ];
    let font_size_logical = (input.font_px_phys / scale).max(1.0);
    BlockHeaderPanelGeometry {
        panel_top_logical,
        panel_left_logical,
        panel_right_logical,
        cell_w_logical,
        cell_h_logical,
        font_size_logical,
        content_clip_logical,
    }
}

/// Per-row metrics used by `render_block_chrome_overlay` for one chrome
/// row inside a block-header span. Pure function of the panel geometry
/// plus the row index inside the span.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockHeaderRowMetrics {
    /// Logical-pixel top of the chrome row.
    pub row_top: f32,
    /// Logical-pixel baseline-ish y where text should be drawn so the
    /// glyph sits vertically centered inside the cell row. Matches the
    /// `(cell_h - font_size) * 0.5 - 1.0` half-leading the native side
    /// used inline.
    pub text_y: f32,
    /// Logical-pixel clamped font size: never larger than the cell
    /// height and never below 8px so glyphs stay legible at small zoom.
    pub clamped_font_size: f32,
    /// Logical-pixel horizontal budget reserved on the right side for
    /// the hover-action icons (copy + favorite + filter).
    pub action_reserve: f32,
}

/// Compute the per-row metrics for one chrome row inside a block
/// header. `display_row` is the absolute display-row index used by the
/// chrome row stream (same coordinate space as
/// `BlockHeaderSpan::start_display_row`).
pub fn block_header_row_metrics(
    geom: BlockHeaderPanelGeometry,
    display_row: isize,
) -> BlockHeaderRowMetrics {
    let width = (geom.panel_right_logical - geom.panel_left_logical).max(0.0);
    let clamped_font_size = geom
        .font_size_logical
        .clamp(8.0, geom.cell_h_logical.max(8.0));
    let row_top = geom.panel_top_logical + display_row as f32 * geom.cell_h_logical;
    let text_y = row_top + (geom.cell_h_logical - clamped_font_size) * 0.5 - 1.0;
    let action_reserve = (geom.cell_h_logical * 3.4 + 24.0).min(width * 0.35);
    BlockHeaderRowMetrics {
        row_top,
        text_y,
        clamped_font_size,
        action_reserve,
    }
}

/// Inputs for the per-block hover-icon layout. The block-header overlay
/// renders three icons (copy + favorite + filter) on the right edge of
/// one anchor row inside a block-header span.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockHoverIconLayoutInput {
    pub panel_top_logical: f32,
    pub panel_right_logical: f32,
    pub cell_h_logical: f32,
    /// Display row inside the block span the icons should anchor to.
    pub anchor_display_row: isize,
}

/// Logical-pixel rects for the copy + favorite + filter hover icons
/// plus the union rect used to detect occlusion against modals/popovers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockHoverIconLayout {
    pub copy_rect: [f32; 4],
    pub favorite_rect: [f32; 4],
    pub filter_rect: [f32; 4],
    pub icon_union: [f32; 4],
}

/// Pure layout for the per-block hover icon strip. Lifted verbatim
/// from `render_block_hover_icons`: 0.85 of cell_h tall, 8px edge pad,
/// 6px gap, filter right-most, favorite in the middle, copy left-most.
pub fn block_hover_icon_layout(input: BlockHoverIconLayoutInput) -> BlockHoverIconLayout {
    let icon_size = input.cell_h_logical * 0.85;
    let gap = 6.0_f32;
    let edge_pad = 8.0_f32;
    let icon_right = input.panel_right_logical - edge_pad;
    let icon_y = input.panel_top_logical
        + input.anchor_display_row as f32 * input.cell_h_logical
        + (input.cell_h_logical - icon_size) * 0.5;
    let filter_rect = [icon_right - icon_size, icon_y, icon_size, icon_size];
    let favorite_rect = [
        icon_right - icon_size * 2.0 - gap,
        icon_y,
        icon_size,
        icon_size,
    ];
    let copy_rect = [
        icon_right - icon_size * 3.0 - gap * 2.0,
        icon_y,
        icon_size,
        icon_size,
    ];
    let icon_union = [
        copy_rect[0],
        copy_rect[1].min(filter_rect[1]),
        (filter_rect[0] + filter_rect[2] - copy_rect[0]).max(0.0),
        copy_rect[3].max(filter_rect[3]),
    ];
    BlockHoverIconLayout {
        copy_rect,
        favorite_rect,
        filter_rect,
        icon_union,
    }
}

/// Pick the anchor display row inside one block-header span for hover
/// icons. The native code clamps the COMMAND row offset by the span's
/// chrome rebase so partial spans still resolve a row.
pub fn block_hover_icon_anchor_row(
    start_display_row: isize,
    end_display_row: isize,
    first_chrome_row: usize,
    command_chrome_row: usize,
) -> isize {
    let command_offset = command_chrome_row.saturating_sub(first_chrome_row) as isize;
    (start_display_row + command_offset).min(end_display_row - 1)
}

// ---------------------------------------------------------------------------
// Block-status theme/glyph resolution.
//
// `BlockStatusKind` lives in `neoism_ui::terminal_blocks::command`. The
// helpers below take the status enum and return the pure decision the
// chrome overlay needs (glyph string, color token). Native hosts pass
// the returned token through `IdeTheme::u8` / `IdeTheme::f32` to get
// the RGBA the GPU expects.
// ---------------------------------------------------------------------------

/// Which `IdeTheme` color token the status indicator should use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockStatusColorToken {
    Yellow,
    Green,
    Red,
}

/// Map a block status to the chrome color token. `Running` reuses
/// yellow as the loader spinner accent; finished states pick green/red.
pub fn block_status_color_token(
    status: crate::terminal_blocks::BlockStatusKind,
) -> BlockStatusColorToken {
    use crate::terminal_blocks::BlockStatusKind;
    match status {
        BlockStatusKind::Running => BlockStatusColorToken::Yellow,
        BlockStatusKind::Ok => BlockStatusColorToken::Green,
        BlockStatusKind::Error(_) => BlockStatusColorToken::Red,
    }
}

/// Pick the static glyph the status indicator should render. `Running`
/// returns `None` so the host paints the animated loader instead.
pub fn block_status_glyph(
    status: crate::terminal_blocks::BlockStatusKind,
) -> Option<&'static str> {
    use crate::terminal_blocks::BlockStatusKind;
    match status {
        BlockStatusKind::Running => None,
        BlockStatusKind::Ok => Some("\u{2022}"),
        BlockStatusKind::Error(_) => Some("\u{2022}"),
    }
}

// ---------------------------------------------------------------------------
// Animation timing policy.
//
// The native renderer threads a single per-frame `animation_phase` into
// every animated chrome surface (block-header loader, splash ripple).
// The phase is just wall-clock seconds mod a rollover so an f32 keeps
// sub-millisecond resolution. Lifting the math out lets web hosts
// produce the same phase from `performance.now()` without porting the
// loader code itself.
// ---------------------------------------------------------------------------

/// Compute the seconds-since-epoch animation phase the renderer threads
/// into per-frame chrome (block-header loaders, splash ripple). Wraps
/// at 10,000 so an f32 mantissa keeps sub-millisecond resolution.
pub fn animation_phase_from_unix_secs(unix_seconds: u64, subsec_nanos: u32) -> f32 {
    let nanos = subsec_nanos.min(999_999_999);
    (unix_seconds % 10_000) as f32 + nanos as f32 / 1_000_000_000.0
}

/// 2D position along a square orbit of radius `half`. `t` is a phase in
/// orbits — every integer increment is a full lap. Each side takes a
/// quarter of the orbit.
pub fn loader_orbit_position(t: f32, half: f32) -> (f32, f32) {
    let p = t.rem_euclid(1.0) * 4.0;
    if p < 1.0 {
        (-half + p * 2.0 * half, -half)
    } else if p < 2.0 {
        (half, -half + (p - 1.0) * 2.0 * half)
    } else if p < 3.0 {
        (half - (p - 2.0) * 2.0 * half, half)
    } else {
        (-half, half - (p - 3.0) * 2.0 * half)
    }
}

/// Trail-color picker for the running-block loader. The 7-entry pastel
/// palette rotates by a hash of `tick` and `trail` so consecutive frames
/// don't repeat the same hue at the same trail offset.
pub fn loader_pastel_color(tick: usize, trail: usize, alpha: f32) -> [f32; 4] {
    const PASTELS: &[[f32; 3]] = &[
        [0.98, 0.38, 0.62],
        [0.98, 0.54, 0.26],
        [0.94, 0.74, 0.22],
        [0.34, 0.84, 0.48],
        [0.24, 0.74, 0.94],
        [0.46, 0.52, 0.98],
        [0.76, 0.42, 0.98],
    ];
    let mixed = tick
        .wrapping_mul(5)
        .wrapping_add(trail.wrapping_mul(3))
        .wrapping_add(tick.rotate_left(3));
    let rgb = PASTELS[mixed % PASTELS.len()];
    [rgb[0], rgb[1], rgb[2], alpha]
}

/// Phase + tick used by the running-block loader. `phase` drives the
/// orbit progression; `tick` indexes into the pastel palette so the
/// loader's hue cycles at ~12 Hz no matter the host frame rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoaderAnimationFrame {
    pub phase: f32,
    pub tick: usize,
}

/// Derive the running-block loader's frame from the global animation
/// phase. The 1.35x multiplier matches the native loader's perceived
/// speed; the 12 Hz tick is what made the palette feel "alive" without
/// jittering per-frame.
pub fn loader_animation_frame(animation_phase: f32) -> LoaderAnimationFrame {
    let phase = animation_phase * 1.35;
    let tick_raw = animation_phase * 12.0;
    let tick = if tick_raw.is_finite() && tick_raw >= 0.0 {
        tick_raw.floor() as usize
    } else {
        0
    };
    LoaderAnimationFrame { phase, tick }
}

// ---------------------------------------------------------------------------
// OpenCode TUI activity scanner.
//
// This intentionally mirrors `packages/tui/src/ui/spinner.ts` in the
// repository's OpenCode reference tree. Keep the discrete 40ms cadence,
// asymmetric end holds, six-cell trail, and inactive-dot fading together:
// changing any one of them produces a noticeably different animation.
// ---------------------------------------------------------------------------

pub const OPENCODE_SCANNER_WIDTH: usize = 8;
const OPENCODE_SCANNER_TRAIL: usize = 6;
const OPENCODE_SCANNER_HOLD_END: usize = 9;
const OPENCODE_SCANNER_HOLD_START: usize = 30;
const OPENCODE_SCANNER_FORWARD: usize = OPENCODE_SCANNER_WIDTH;
const OPENCODE_SCANNER_BACKWARD: usize = OPENCODE_SCANNER_WIDTH - 1;
const OPENCODE_SCANNER_FRAMES: usize = OPENCODE_SCANNER_FORWARD
    + OPENCODE_SCANNER_HOLD_END
    + OPENCODE_SCANNER_BACKWARD
    + OPENCODE_SCANNER_HOLD_START;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OpenCodeScannerCell {
    /// Active cells use the solid block; inactive cells use the half-height dot.
    pub active: bool,
    /// Alpha after applying either the six-step trail or inactive fade.
    pub alpha: f32,
    /// OpenCode blooms the first trailing block to 1.15x brightness.
    pub brightness: f32,
}

/// Resolve one exact OpenCode TUI scanner frame from elapsed seconds.
///
/// The source spinner advances at 40ms and renders eight cells using `■`
/// for the six-step active trail and `⬝` for inactive positions.
pub fn opencode_scanner_frame(
    elapsed_seconds: f32,
) -> [OpenCodeScannerCell; OPENCODE_SCANNER_WIDTH] {
    let frame = if elapsed_seconds.is_finite() {
        (elapsed_seconds * 25.0 + 0.0001)
            .floor()
            .rem_euclid(OPENCODE_SCANNER_FRAMES as f32) as usize
    } else {
        0
    };

    let (
        head,
        holding,
        hold_progress,
        hold_total,
        movement_progress,
        movement_total,
        forward,
    ) = if frame < OPENCODE_SCANNER_FORWARD {
        (frame, false, 0, 0, frame, OPENCODE_SCANNER_FORWARD, true)
    } else if frame < OPENCODE_SCANNER_FORWARD + OPENCODE_SCANNER_HOLD_END {
        (
            OPENCODE_SCANNER_WIDTH - 1,
            true,
            frame - OPENCODE_SCANNER_FORWARD,
            OPENCODE_SCANNER_HOLD_END,
            0,
            0,
            true,
        )
    } else if frame
        < OPENCODE_SCANNER_FORWARD + OPENCODE_SCANNER_HOLD_END + OPENCODE_SCANNER_BACKWARD
    {
        let backward_index = frame - OPENCODE_SCANNER_FORWARD - OPENCODE_SCANNER_HOLD_END;
        (
            OPENCODE_SCANNER_WIDTH - 2 - backward_index,
            false,
            0,
            0,
            backward_index,
            OPENCODE_SCANNER_BACKWARD,
            false,
        )
    } else {
        (
            0,
            true,
            frame
                - OPENCODE_SCANNER_FORWARD
                - OPENCODE_SCANNER_HOLD_END
                - OPENCODE_SCANNER_BACKWARD,
            OPENCODE_SCANNER_HOLD_START,
            0,
            0,
            false,
        )
    };

    let fade_factor = if holding && hold_total > 0 {
        let progress = (hold_progress as f32 / hold_total as f32).min(1.0);
        (1.0 - progress * 0.7).max(0.3)
    } else if !holding && movement_total > 0 {
        let progress = (movement_progress as f32
            / movement_total.saturating_sub(1).max(1) as f32)
            .min(1.0);
        0.3 + progress * 0.7
    } else {
        1.0
    };
    let inactive_alpha = 0.6 * fade_factor;

    std::array::from_fn(|cell| {
        let directional_distance = if forward {
            head as isize - cell as isize
        } else {
            cell as isize - head as isize
        };
        let trail_index = if holding {
            directional_distance + hold_progress as isize
        } else if directional_distance > 0
            && directional_distance < OPENCODE_SCANNER_TRAIL as isize
        {
            directional_distance
        } else if directional_distance == 0 {
            0
        } else {
            -1
        };

        if (0..OPENCODE_SCANNER_TRAIL as isize).contains(&trail_index) {
            let trail_index = trail_index as usize;
            let (alpha, brightness) = match trail_index {
                0 => (1.0, 1.0),
                1 => (0.9, 1.15),
                index => (0.65_f32.powi(index as i32 - 1), 1.0),
            };
            OpenCodeScannerCell {
                active: true,
                alpha,
                brightness,
            }
        } else {
            OpenCodeScannerCell {
                active: false,
                alpha: inactive_alpha,
                brightness: 1.0,
            }
        }
    })
}

/// The compact spinner OpenCode uses beside thinking, running tools, and
/// background tasks. It is deliberately separate from the eight-cell footer
/// scanner above.
pub const OPENCODE_TASK_SPINNER_FRAMES: [&str; 10] =
    ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Resolve the OpenCode task spinner's current braille glyph (80ms per frame).
pub fn opencode_task_spinner_frame(elapsed_seconds: f32) -> &'static str {
    let frame = if elapsed_seconds.is_finite() {
        (elapsed_seconds * 12.5 + 0.0001).floor() as isize
    } else {
        0
    };
    OPENCODE_TASK_SPINNER_FRAMES
        [frame.rem_euclid(OPENCODE_TASK_SPINNER_FRAMES.len() as isize) as usize]
}
