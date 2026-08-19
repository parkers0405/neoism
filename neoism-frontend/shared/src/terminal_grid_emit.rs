//! Shared terminal-grid emission policy.
//!
//! Host renderers still own glyph atlases and draw submission. This
//! module owns the deterministic row/hint/decoration decisions that
//! native and web terminal grid renderers must keep identical.

use neoism_terminal_core::ansi::CursorShape;
use neoism_terminal_core::crosswords::pos::{Line, Pos};
use neoism_terminal_core::crosswords::search::Match;
use neoism_terminal_core::crosswords::style::StyleFlags;
use neoism_terminal_core::selection::SelectionRange;

/// Bits of `StyleFlags` that affect terminal run shaping or font
/// selection. Decoration/color/dim flags do not change glyph shaping
/// and should not split runs.
pub const SHAPING_FLAG_MASK: u16 = StyleFlags::BOLD.bits() | StyleFlags::ITALIC.bits();

#[inline]
pub fn shaping_style_flags(flags: StyleFlags) -> u8 {
    (flags.bits() & SHAPING_FLAG_MASK) as u8
}

#[inline]
pub fn terminal_size_bucket(size_px: f32) -> u16 {
    (size_px * 4.0).round().clamp(0.0, u16::MAX as f32) as u16
}

#[inline]
pub fn terminal_font_size_u16(size_px: f32) -> u16 {
    size_px.round().clamp(1.0, u16::MAX as f32) as u16
}

#[inline]
pub fn rounded_terminal_cell_size(size_px: f32) -> u32 {
    size_px.round().clamp(1.0, u32::MAX as f32) as u32
}

#[inline]
pub fn is_terminal_run_breaker(is_bg_only: bool, ch: char) -> bool {
    is_bg_only || ch == '\0' || ch == ' '
}

/// Map shaped glyph clusters in a UTF-8 run back to terminal cell
/// offsets. Swash reports cluster offsets in UTF-8 bytes.
pub fn glyph_cell_offsets_utf8<I>(run: &str, clusters: I) -> Vec<u16>
where
    I: IntoIterator<Item = u32>,
{
    let mut out = Vec::new();
    let mut char_cursor = run.char_indices().peekable();
    let mut cell_idx_in_run: u16 = 0;
    for cluster in clusters {
        while let Some(&(byte_offset, _)) = char_cursor.peek() {
            if (byte_offset as u32) >= cluster {
                break;
            }
            char_cursor.next();
            cell_idx_in_run = cell_idx_in_run.saturating_add(1);
        }
        out.push(cell_idx_in_run);
    }
    out
}

/// Map shaped glyph clusters in a UTF-16 run back to terminal cell
/// offsets. CoreText reports cluster offsets in UTF-16 code units;
/// `cell_starts[i]` is the UTF-16 offset where cell `i` begins.
pub fn glyph_cell_offsets_utf16<I>(cell_starts: &[u32], clusters: I) -> Vec<u16>
where
    I: IntoIterator<Item = u32>,
{
    let mut out = Vec::new();
    let mut cell_idx_in_run: u16 = 0;
    for cluster in clusters {
        while (cell_idx_in_run as usize + 1) < cell_starts.len()
            && cell_starts[cell_idx_in_run as usize + 1] <= cluster
        {
            cell_idx_in_run = cell_idx_in_run.saturating_add(1);
        }
        out.push(cell_idx_in_run);
    }
    out
}

/// Atlas placement derived from a raw font-rasterized glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalGlyphPlacement {
    pub width: u16,
    pub height: u16,
    pub bearing_x: i16,
    pub bearing_y: i16,
}

/// Convert raw font-rasterizer geometry into the terminal grid atlas
/// coordinate convention.
///
/// Platform rasterizers report `left` / `top` in their native
/// baseline-relative convention. The host supplies the primary-font baseline
/// already centered inside the configured cell. The grid atlas stores
/// `bearing_y` relative to the cell bottom: `cell_h - baseline + top`.
#[inline]
pub fn terminal_glyph_placement(
    width: u32,
    height: u32,
    left: i32,
    top: i32,
    cell_h: f32,
    baseline_px: i16,
) -> TerminalGlyphPlacement {
    let top_i16 = top.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    let cell_h_i16 = cell_h.round().clamp(0.0, i16::MAX as f32) as i16;
    TerminalGlyphPlacement {
        width: width.min(u16::MAX as u32) as u16,
        height: height.min(u16::MAX as u32) as u16,
        bearing_x: left.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
        bearing_y: cell_h_i16
            .saturating_sub(baseline_px)
            .saturating_add(top_i16),
    }
}

/// Search-hint category at a cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HintTag {
    Match,
    Focused,
    HyperlinkHover,
}

/// Per-row hint interval, closed on both ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowHint {
    pub lo: u16,
    pub hi: u16,
    pub tag: HintTag,
}

/// Compute visible hint intervals for one rendered row.
///
/// Matches can span multiple rows; the first and last rows clip to the
/// match columns, and interior rows cover the full row width.
pub fn row_hints_for(
    hint_matches: Option<&[Match]>,
    focused_match: Option<&Match>,
    hover_hyperlink: Option<(Pos, Pos)>,
    y: usize,
    cols: usize,
    display_offset: i32,
    out: &mut Vec<RowHint>,
) {
    out.clear();
    if cols == 0 {
        return;
    }

    let line = Line((y as i32) - display_offset);
    let cols_max = cols.saturating_sub(1) as u16;

    let pos_pair_to_row_hint = |start: Pos, end: Pos, tag: HintTag| -> Option<RowHint> {
        if line < start.row || line > end.row {
            return None;
        }
        let lo = if line == start.row {
            start.col.0 as u16
        } else {
            0
        };
        let hi = if line == end.row {
            end.col.0 as u16
        } else {
            cols_max
        };
        Some(RowHint {
            lo: lo.min(cols_max),
            hi: hi.min(cols_max),
            tag,
        })
    };

    let to_row_hint =
        |m: &Match, tag: HintTag| pos_pair_to_row_hint(*m.start(), *m.end(), tag);

    let is_same_match = |a: &Match, b: &Match| -> bool {
        let (a_start, a_end) = (*a.start(), *a.end());
        let (b_start, b_end) = (*b.start(), *b.end());
        a_start.row == b_start.row
            && a_start.col == b_start.col
            && a_end.row == b_end.row
            && a_end.col == b_end.col
    };

    // Hover is first so underline queries win even when the same cells
    // also have search-match color.
    if let Some((start, end)) = hover_hyperlink {
        if let Some(rh) = pos_pair_to_row_hint(start, end, HintTag::HyperlinkHover) {
            out.push(rh);
        }
    }

    let Some(matches) = hint_matches else {
        return;
    };

    if let Some(fm) = focused_match {
        if let Some(rh) = to_row_hint(fm, HintTag::Focused) {
            out.push(rh);
        }
    }
    for m in matches {
        if let Some(fm) = focused_match {
            if is_same_match(m, fm) {
                continue;
            }
        }
        if let Some(rh) = to_row_hint(m, HintTag::Match) {
            out.push(rh);
        }
    }
}

#[inline]
pub fn cell_in_row_hints(row_hints: &[RowHint], col: u16) -> Option<HintTag> {
    for rh in row_hints {
        if rh.tag == HintTag::HyperlinkHover {
            continue;
        }
        if col >= rh.lo && col <= rh.hi {
            return Some(rh.tag);
        }
    }
    None
}

#[inline]
pub fn cell_in_hover_underline(row_hints: &[RowHint], col: u16) -> bool {
    row_hints
        .iter()
        .any(|rh| rh.tag == HintTag::HyperlinkHover && col >= rh.lo && col <= rh.hi)
}

#[inline]
pub fn hint_foreground(
    tag: HintTag,
    match_fg: [f32; 4],
    focused_fg: [f32; 4],
) -> [f32; 4] {
    match tag {
        HintTag::Focused => focused_fg,
        HintTag::Match => match_fg,
        HintTag::HyperlinkHover => [0.0, 0.0, 0.0, 0.0],
    }
}

#[inline]
pub fn rgba_f32_to_u8(c: [f32; 4]) -> [u8; 4] {
    [
        (c[0].clamp(0.0, 1.0) * 255.0) as u8,
        (c[1].clamp(0.0, 1.0) * 255.0) as u8,
        (c[2].clamp(0.0, 1.0) * 255.0) as u8,
        (c[3].clamp(0.0, 1.0) * 255.0) as u8,
    ]
}

/// Per-row selection interval, in column indices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowSelection {
    pub lo: u16,
    pub hi: u16,
}

/// Compute the terminal selection interval for visible row `y`.
///
/// `display_offset` translates visible-row index to absolute terminal
/// row. Linear selections expand middle rows to full width; block
/// selections use the same clamped span on every row in the band.
pub fn row_selection_for(
    sel: Option<SelectionRange>,
    y: usize,
    cols: usize,
    display_offset: i32,
) -> Option<RowSelection> {
    let sel = sel?;
    if cols == 0 {
        return None;
    }

    let line = Line((y as i32) - display_offset);
    if line < sel.start.row || line > sel.end.row {
        return None;
    }

    let cols_max = cols.saturating_sub(1);
    if sel.is_block {
        let lo = sel.start.col.0.min(cols_max);
        let hi = sel.end.col.0.min(cols_max);
        return Some(RowSelection {
            lo: lo as u16,
            hi: hi as u16,
        });
    }

    let lo = if line == sel.start.row {
        sel.start.col.0
    } else {
        0
    };
    let hi = if line == sel.end.row {
        sel.end.col.0
    } else {
        cols_max
    };
    Some(RowSelection {
        lo: lo.min(cols_max) as u16,
        hi: hi.min(cols_max) as u16,
    })
}

#[inline]
pub fn cell_in_row_sel(row_sel: Option<RowSelection>, col: u16) -> bool {
    match row_sel {
        Some(s) => col >= s.lo && col <= s.hi,
        None => false,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum DecorationStyle {
    Underline = 0,
    DoubleUnderline = 1,
    DottedUnderline = 2,
    DashedUnderline = 3,
    CurlyUnderline = 4,
    Strikethrough = 5,
}

/// Sentinel font_id base for decoration sprites.
pub const DECORATION_FONT_ID_BASE: u32 = 0xFFFF_FF00;

#[inline]
pub fn decoration_thickness(size_px: f32) -> u32 {
    (size_px * 0.075).round().max(1.0) as u32
}

#[inline]
pub fn underline_gap_below(cell_h: u32) -> u32 {
    (cell_h / 20).max(1)
}

pub fn rasterize_decoration(
    style: DecorationStyle,
    cell_w: u32,
    cell_h: u32,
    thickness: u32,
) -> (Vec<u8>, u32, u32, i16) {
    match style {
        DecorationStyle::Underline => {
            let bytes = vec![0xFFu8; (cell_w * thickness) as usize];
            let bearing_y = (thickness + underline_gap_below(cell_h)) as i16;
            (bytes, cell_w, thickness, bearing_y)
        }
        DecorationStyle::DoubleUnderline => {
            let gap = thickness;
            let h = thickness * 2 + gap;
            let mut bytes = vec![0u8; (cell_w * h) as usize];
            let row_w = cell_w as usize;
            for row in 0..thickness as usize {
                let start = row * row_w;
                bytes[start..start + row_w].fill(0xFF);
            }
            for row in (thickness + gap) as usize..h as usize {
                let start = row * row_w;
                bytes[start..start + row_w].fill(0xFF);
            }
            let bearing_y = (h + underline_gap_below(cell_h)) as i16;
            (bytes, cell_w, h, bearing_y)
        }
        DecorationStyle::DottedUnderline => {
            let h = thickness;
            let diameter = thickness.max(1);
            let period = diameter * 2;
            let mut bytes = vec![0u8; (cell_w * h) as usize];
            let row_w = cell_w as usize;
            let mut x = 0u32;
            while x < cell_w {
                let end = (x + diameter).min(cell_w);
                for row in 0..h as usize {
                    let start = row * row_w + x as usize;
                    bytes[start..start + (end - x) as usize].fill(0xFF);
                }
                x += period;
            }
            let bearing_y = (h + underline_gap_below(cell_h)) as i16;
            (bytes, cell_w, h, bearing_y)
        }
        DecorationStyle::DashedUnderline => {
            let h = thickness;
            let b1 = cell_w / 4;
            let b2 = cell_w / 2;
            let b3 = (cell_w * 3) / 4;
            let mut bytes = vec![0u8; (cell_w * h) as usize];
            let row_w = cell_w as usize;
            for (x_lo, x_hi) in [(0u32, b1), (b2, b3)] {
                if x_hi <= x_lo {
                    continue;
                }
                for row in 0..h as usize {
                    let start = row * row_w + x_lo as usize;
                    let end = row * row_w + x_hi as usize;
                    bytes[start..end].fill(0xFF);
                }
            }
            let bearing_y = (h + underline_gap_below(cell_h)) as i16;
            (bytes, cell_w, h, bearing_y)
        }
        DecorationStyle::CurlyUnderline => {
            use core::f32::consts::PI;
            let amp = (cell_w as f32 / PI).max(thickness as f32);
            let amp_i = amp.ceil() as u32;
            let h = amp_i + thickness + 1;
            let mut bytes = vec![0u8; (cell_w * h) as usize];
            let row_w = cell_w as usize;
            let half_t = thickness as f32 * 0.5;
            let baseline = h as f32 - half_t - 0.5;
            for col in 0..cell_w {
                let x_norm = (col as f32 + 0.5) / cell_w as f32;
                let s = 0.5 * (1.0 - (x_norm * 2.0 * PI).cos());
                let y_center = baseline - s * amp;
                let y_lo = (y_center - half_t).floor().max(0.0) as u32;
                let y_hi = ((y_center + half_t).ceil() as u32).min(h);
                for row in y_lo..y_hi {
                    bytes[row as usize * row_w + col as usize] = 0xFF;
                }
            }
            let bearing_y = (h + underline_gap_below(cell_h)) as i16;
            (bytes, cell_w, h, bearing_y)
        }
        DecorationStyle::Strikethrough => {
            let bytes = vec![0xFFu8; (cell_w * thickness) as usize];
            let center_from_bottom = cell_h / 2;
            let bearing_y = center_from_bottom as i16 + (thickness as i16 + 1) / 2;
            (bytes, cell_w, thickness, bearing_y)
        }
    }
}

/// One solid sub-rectangle of a rasterized sprite, offset from the
/// CELL's top-left corner in sprite/physical px. Produced by
/// [`sprite_solid_quads`] for hosts that draw decoration / cursor
/// sprites as plain quads instead of atlas glyphs (the web renderer's
/// sugarloaf surface has no terminal grid atlas). Geometry stays
/// byte-identical to the atlas path because it derives from the same
/// `rasterize_*` bitmaps and the same bearing convention.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpriteQuad {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// Decompose a grayscale sprite bitmap (row-major, `w * h` bytes,
/// `0` = transparent) into solid rectangles: horizontal RLE runs,
/// merged vertically when consecutive rows carry an identical run.
/// `x_off` / `y_off` translate sprite-local coordinates into
/// cell-local coordinates (the atlas path's `bearing_x` /
/// `cell_h - bearing_y`).
pub fn sprite_solid_quads(bytes: &[u8], w: u32, x_off: i32, y_off: i32) -> Vec<SpriteQuad> {
    let mut out: Vec<SpriteQuad> = Vec::new();
    if w == 0 {
        return out;
    }
    let h = bytes.len() as u32 / w;
    // Indices into `out` of the quads whose bottom edge touches the
    // previous row, keyed implicitly by scan order.
    let mut prev_row: Vec<usize> = Vec::new();
    let mut cur_row: Vec<usize> = Vec::new();
    for row in 0..h {
        cur_row.clear();
        let base = (row * w) as usize;
        let mut col = 0u32;
        while col < w {
            if bytes[base + col as usize] < 0x80 {
                col += 1;
                continue;
            }
            let start = col;
            while col < w && bytes[base + col as usize] >= 0x80 {
                col += 1;
            }
            let run_w = col - start;
            // Vertical merge: a quad from the previous row with the
            // same x/w extends down by one row.
            let merged = prev_row.iter().copied().find(|&idx| {
                let q = &out[idx];
                q.x == start as i32 + x_off && q.w == run_w
            });
            match merged {
                Some(idx) => {
                    out[idx].h += 1;
                    cur_row.push(idx);
                }
                None => {
                    out.push(SpriteQuad {
                        x: start as i32 + x_off,
                        y: row as i32 + y_off,
                        w: run_w,
                        h: 1,
                    });
                    cur_row.push(out.len() - 1);
                }
            }
        }
        std::mem::swap(&mut prev_row, &mut cur_row);
    }
    out
}

/// [`rasterize_decoration`] decomposed into cell-local solid quads for
/// quad-drawing hosts. Placement follows the atlas convention: the
/// sprite's top edge sits at `cell_h - bearing_y` below the cell top.
pub fn decoration_quads(
    style: DecorationStyle,
    cell_w: u32,
    cell_h: u32,
    thickness: u32,
) -> Vec<SpriteQuad> {
    let (bytes, w, _h, bearing_y) = rasterize_decoration(style, cell_w, cell_h, thickness);
    sprite_solid_quads(&bytes, w, 0, cell_h as i32 - bearing_y as i32)
}

/// [`rasterize_cursor`] decomposed into cell-local solid quads for
/// quad-drawing hosts. Same bearing convention as [`decoration_quads`];
/// `bearing_x` shifts the sprite horizontally (the bar cursor centers
/// on the cell's left edge via a negative bearing).
pub fn cursor_quads(
    style: CursorSpriteStyle,
    cell_w: u32,
    cell_h: u32,
    thickness: u32,
) -> Vec<SpriteQuad> {
    let (bytes, w, _h, bearing_x, bearing_y) =
        rasterize_cursor(style, cell_w, cell_h, thickness);
    sprite_solid_quads(
        &bytes,
        w as u32,
        bearing_x as i32,
        cell_h as i32 - bearing_y as i32,
    )
}

#[inline]
pub fn underline_style_from_flags(flags: StyleFlags) -> Option<DecorationStyle> {
    if flags.contains(StyleFlags::UNDERLINE) {
        Some(DecorationStyle::Underline)
    } else if flags.contains(StyleFlags::DOUBLE_UNDERLINE) {
        Some(DecorationStyle::DoubleUnderline)
    } else if flags.contains(StyleFlags::UNDERCURL) {
        Some(DecorationStyle::CurlyUnderline)
    } else if flags.contains(StyleFlags::DOTTED_UNDERLINE) {
        Some(DecorationStyle::DottedUnderline)
    } else if flags.contains(StyleFlags::DASHED_UNDERLINE) {
        Some(DecorationStyle::DashedUnderline)
    } else {
        None
    }
}

/// Sentinel font_id base for cursor sprites.
pub const CURSOR_FONT_ID_BASE: u32 = 0xFFFF_FE00;

/// Cursor sprite styles. Each variant maps to a distinct rasterized
/// bitmap stored in the host renderer's grayscale atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum CursorSpriteStyle {
    /// Full-cell filled rectangle. Drawn under text so inverted text
    /// composites on top.
    Block = 0,
    /// Outlined rectangle for inactive panes.
    Hollow = 1,
    /// Vertical bar, `thickness` px wide, centered on the left edge
    /// of the cursor cell.
    Bar = 2,
    /// Horizontal bar at the underline baseline.
    Underline = 3,
}

impl CursorSpriteStyle {
    /// Block cursors land in the under-text cursor slot; all other
    /// cursor sprites overlay text.
    #[inline]
    pub fn is_block_slot(self) -> bool {
        matches!(self, CursorSpriteStyle::Block)
    }
}

/// Top-level cursor render decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorRenderStyle {
    /// Active focused block, painted with text inversion.
    Block,
    /// Outlined rectangle for inactive split panels.
    BlockHollow,
    /// Vertical bar.
    Bar,
    /// Underscore at the cell baseline.
    Underline,
}

impl CursorRenderStyle {
    #[inline]
    pub fn sprite(self) -> CursorSpriteStyle {
        match self {
            CursorRenderStyle::Block => CursorSpriteStyle::Block,
            CursorRenderStyle::BlockHollow => CursorSpriteStyle::Hollow,
            CursorRenderStyle::Bar => CursorSpriteStyle::Bar,
            CursorRenderStyle::Underline => CursorSpriteStyle::Underline,
        }
    }
}

/// Inputs to the cursor-style decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorRenderInputs {
    /// `false` when DECTCEM hides the cursor.
    pub visible: bool,
    /// `true` when this panel currently has focus.
    pub focused: bool,
    /// `true` for the visible half of a blink cycle. Pass `true`
    /// when blink is disabled.
    pub blink_visible: bool,
    /// `true` when the cursor is blinking.
    pub blinking: bool,
    /// `true` while an IME pre-edit string is active.
    pub preedit: bool,
    /// The terminal-side configured cursor shape.
    pub shape: CursorShape,
}

/// Decide which cursor variant to render this frame, or `None` to
/// skip emission entirely. Priority order is:
/// preedit > visibility > focus > blink > terminal shape.
pub fn cursor_render_style(opts: CursorRenderInputs) -> Option<CursorRenderStyle> {
    if opts.preedit {
        return Some(CursorRenderStyle::Block);
    }
    if !opts.visible || opts.shape == CursorShape::Hidden {
        return None;
    }
    if !opts.focused {
        return Some(CursorRenderStyle::BlockHollow);
    }
    if opts.blinking && !opts.blink_visible {
        return None;
    }
    Some(match opts.shape {
        CursorShape::Block => CursorRenderStyle::Block,
        CursorShape::Underline => CursorRenderStyle::Underline,
        CursorShape::Beam => CursorRenderStyle::Bar,
        CursorShape::Hidden => unreachable!("hidden shape is filtered above"),
    })
}

/// Cursor stroke thickness in physical px. Host renderers can pass
/// this into `rasterize_cursor`; capped to avoid oversized frames at
/// high zoom.
#[inline]
pub fn cursor_thickness(cell_h: u32) -> u32 {
    (cell_h / 16).clamp(1, 2)
}

/// Per-style sprite bitmap + bearings.
///
/// Returns `(bytes, width, height, bearing_x, bearing_y)` for a
/// grayscale atlas sprite.
pub fn rasterize_cursor(
    style: CursorSpriteStyle,
    cell_w: u32,
    cell_h: u32,
    thickness: u32,
) -> (Vec<u8>, u16, u16, i16, i16) {
    let t = thickness.max(1);
    match style {
        CursorSpriteStyle::Block => {
            let bytes = vec![0xFFu8; (cell_w * cell_h) as usize];
            (
                bytes,
                cell_w.min(u16::MAX as u32) as u16,
                cell_h.min(u16::MAX as u32) as u16,
                0,
                cell_h.min(i16::MAX as u32) as i16,
            )
        }
        CursorSpriteStyle::Hollow => {
            let row_w = cell_w as usize;
            let h = cell_h as usize;
            let mut bytes = vec![0u8; row_w * h];
            let ti = (t as usize).max(1);
            for row in 0..ti.min(h) {
                let s = row * row_w;
                bytes[s..s + row_w].fill(0xFF);
            }
            for row in h.saturating_sub(ti)..h {
                let s = row * row_w;
                bytes[s..s + row_w].fill(0xFF);
            }
            for row in ti..h.saturating_sub(ti) {
                let s = row * row_w;
                for col in 0..ti.min(row_w) {
                    bytes[s + col] = 0xFF;
                }
                for col in row_w.saturating_sub(ti)..row_w {
                    bytes[s + col] = 0xFF;
                }
            }
            (
                bytes,
                cell_w.min(u16::MAX as u32) as u16,
                cell_h.min(u16::MAX as u32) as u16,
                0,
                cell_h.min(i16::MAX as u32) as i16,
            )
        }
        CursorSpriteStyle::Bar => {
            let bytes = vec![0xFFu8; (t * cell_h) as usize];
            let bearing_x = -((t as i16 + 1) / 2);
            (
                bytes,
                t.min(u16::MAX as u32) as u16,
                cell_h.min(u16::MAX as u32) as u16,
                bearing_x,
                cell_h.min(i16::MAX as u32) as i16,
            )
        }
        CursorSpriteStyle::Underline => {
            let bytes = vec![0xFFu8; (cell_w * t) as usize];
            let bearing_y = (t + underline_gap_below(cell_h)) as i16;
            (
                bytes,
                cell_w.min(u16::MAX as u32) as u16,
                t.min(u16::MAX as u32) as u16,
                0,
                bearing_y,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_terminal_core::crosswords::pos::{Column, Line, Pos};

    fn pos(row: i32, col: usize) -> Pos {
        Pos::new(Line(row), Column(col))
    }

    #[test]
    fn shaping_flags_only_include_font_selecting_bits() {
        let flags = StyleFlags::BOLD
            | StyleFlags::ITALIC
            | StyleFlags::UNDERLINE
            | StyleFlags::DIM;

        assert_eq!(
            shaping_style_flags(flags),
            (StyleFlags::BOLD.bits() | StyleFlags::ITALIC.bits()) as u8
        );
        assert_eq!(terminal_size_bucket(13.24), 53);
        assert_eq!(terminal_font_size_u16(0.2), 1);
        assert_eq!(rounded_terminal_cell_size(8.6), 9);
        assert!(is_terminal_run_breaker(true, 'x'));
        assert!(is_terminal_run_breaker(false, '\0'));
        assert!(is_terminal_run_breaker(false, ' '));
        assert!(!is_terminal_run_breaker(false, 'x'));
    }

    #[test]
    fn glyph_cell_offsets_map_utf8_clusters_to_terminal_cells() {
        // "aé中" has byte starts 0, 1, 3. Repeated clusters model
        // multiple glyphs produced from the same terminal cell.
        let offsets = glyph_cell_offsets_utf8("aé中", [0, 1, 1, 3, 99]);
        assert_eq!(offsets, vec![0, 1, 1, 2, 3]);
    }

    #[test]
    fn glyph_cell_offsets_map_utf16_clusters_to_terminal_cells() {
        // Cell starts correspond to "a", "😀", "b" in UTF-16 code
        // units: 0, 1, 3.
        let offsets = glyph_cell_offsets_utf16(&[0, 1, 3], [0, 1, 2, 3, 99]);
        assert_eq!(offsets, vec![0, 1, 1, 2, 2]);
    }

    #[test]
    fn terminal_glyph_placement_uses_cell_bottom_bearing_convention() {
        assert_eq!(
            terminal_glyph_placement(20, 30, -2, 7, 40.4, 25),
            TerminalGlyphPlacement {
                width: 20,
                height: 30,
                bearing_x: -2,
                bearing_y: 22,
            }
        );
    }

    #[test]
    fn terminal_glyph_placement_clamps_host_metrics() {
        assert_eq!(
            terminal_glyph_placement(
                u32::MAX,
                u32::MAX,
                i32::MIN,
                i32::MAX,
                f32::MAX,
                i16::MIN,
            ),
            TerminalGlyphPlacement {
                width: u16::MAX,
                height: u16::MAX,
                bearing_x: i16::MIN,
                bearing_y: i16::MAX,
            }
        );
    }

    #[test]
    fn row_hints_clip_multiline_matches_to_visible_row() {
        let matches = vec![pos(1, 3)..=pos(3, 5)];
        let mut out = Vec::new();

        row_hints_for(Some(&matches), None, None, 2, 10, 0, &mut out);
        assert_eq!(
            out,
            vec![RowHint {
                lo: 0,
                hi: 9,
                tag: HintTag::Match
            }]
        );

        row_hints_for(Some(&matches), None, None, 3, 10, 0, &mut out);
        assert_eq!(
            out,
            vec![RowHint {
                lo: 0,
                hi: 5,
                tag: HintTag::Match
            }]
        );
    }

    #[test]
    fn focused_match_precedes_regular_match_and_hover_only_underlines() {
        let focused = pos(0, 2)..=pos(0, 4);
        let matches = vec![focused.clone(), pos(0, 6)..=pos(0, 8)];
        let mut out = Vec::new();

        row_hints_for(
            Some(&matches),
            Some(&focused),
            Some((pos(0, 3), pos(0, 7))),
            0,
            12,
            0,
            &mut out,
        );

        assert_eq!(out[0].tag, HintTag::HyperlinkHover);
        assert_eq!(cell_in_row_hints(&out, 3), Some(HintTag::Focused));
        assert!(cell_in_hover_underline(&out, 3));
        assert_eq!(cell_in_row_hints(&out, 7), Some(HintTag::Match));
    }

    #[test]
    fn decoration_geometry_is_stable() {
        assert_eq!(decoration_thickness(13.0), 1);
        assert_eq!(underline_gap_below(40), 2);

        let (bytes, w, h, bearing_y) =
            rasterize_decoration(DecorationStyle::DoubleUnderline, 8, 40, 2);
        assert_eq!((w, h, bearing_y), (8, 6, 8));
        assert!(bytes[0..8].iter().all(|b| *b == 0xFF));
        assert!(bytes[16..32].iter().all(|b| *b == 0));
        assert!(bytes[32..48].iter().all(|b| *b == 0xFF));
    }

    #[test]
    fn underline_style_flag_precedence_matches_desktop() {
        let flags = StyleFlags::UNDERLINE | StyleFlags::DOUBLE_UNDERLINE;
        assert_eq!(
            underline_style_from_flags(flags),
            Some(DecorationStyle::Underline)
        );
    }

    #[test]
    fn row_selection_for_linear_selection_expands_middle_rows() {
        let sel = SelectionRange::new(pos(1, 3), pos(3, 5), false);

        assert_eq!(
            row_selection_for(Some(sel), 1, 10, 0),
            Some(RowSelection { lo: 3, hi: 9 })
        );
        assert_eq!(
            row_selection_for(Some(sel), 2, 10, 0),
            Some(RowSelection { lo: 0, hi: 9 })
        );
        assert_eq!(
            row_selection_for(Some(sel), 3, 10, 0),
            Some(RowSelection { lo: 0, hi: 5 })
        );
        assert_eq!(row_selection_for(Some(sel), 4, 10, 0), None);
    }

    #[test]
    fn row_selection_for_block_selection_reuses_clamped_span() {
        let sel = SelectionRange::new(pos(1, 2), pos(3, 20), true);

        assert_eq!(
            row_selection_for(Some(sel), 2, 8, 0),
            Some(RowSelection { lo: 2, hi: 7 })
        );
        assert!(cell_in_row_sel(row_selection_for(Some(sel), 2, 8, 0), 6));
        assert!(!cell_in_row_sel(row_selection_for(Some(sel), 2, 8, 0), 1));
        assert_eq!(row_selection_for(Some(sel), 2, 0, 0), None);
    }

    #[test]
    fn cursor_render_style_obeys_visibility_focus_and_blink_priority() {
        let base = CursorRenderInputs {
            visible: true,
            focused: true,
            blink_visible: true,
            blinking: false,
            preedit: false,
            shape: CursorShape::Beam,
        };

        assert_eq!(cursor_render_style(base), Some(CursorRenderStyle::Bar));
        assert_eq!(
            cursor_render_style(CursorRenderInputs {
                focused: false,
                ..base
            }),
            Some(CursorRenderStyle::BlockHollow)
        );
        assert_eq!(
            cursor_render_style(CursorRenderInputs {
                blinking: true,
                blink_visible: false,
                ..base
            }),
            None
        );
        assert_eq!(
            cursor_render_style(CursorRenderInputs {
                visible: false,
                preedit: true,
                ..base
            }),
            Some(CursorRenderStyle::Block)
        );
        assert_eq!(
            cursor_render_style(CursorRenderInputs {
                shape: CursorShape::Hidden,
                ..base
            }),
            None
        );
    }

    #[test]
    fn cursor_sprite_geometry_is_stable() {
        assert_eq!(cursor_thickness(15), 1);
        assert_eq!(cursor_thickness(32), 2);

        let (bytes, w, h, bearing_x, bearing_y) =
            rasterize_cursor(CursorSpriteStyle::Bar, 10, 24, cursor_thickness(24));
        assert_eq!((w, h, bearing_x, bearing_y), (1, 24, -1, 24));
        assert!(bytes.iter().all(|b| *b == 0xFF));

        let (bytes, w, h, bearing_x, bearing_y) =
            rasterize_cursor(CursorSpriteStyle::Underline, 10, 40, 2);
        assert_eq!((w, h, bearing_x, bearing_y), (10, 2, 0, 4));
        assert_eq!(bytes.len(), 20);
        assert!(bytes.iter().all(|b| *b == 0xFF));
    }

    #[test]
    fn sprite_solid_quads_rle_and_vertical_merge() {
        // 4x3 bitmap: full top row, then two rows with left/right
        // single-px columns (a hollow-box slice).
        #[rustfmt::skip]
        let bytes = [
            0xFFu8, 0xFF, 0xFF, 0xFF,
            0xFF,   0x00, 0x00, 0xFF,
            0xFF,   0x00, 0x00, 0xFF,
        ];
        let quads = sprite_solid_quads(&bytes, 4, 0, 0);
        assert_eq!(
            quads,
            vec![
                SpriteQuad { x: 0, y: 0, w: 4, h: 1 },
                SpriteQuad { x: 0, y: 1, w: 1, h: 2 },
                SpriteQuad { x: 3, y: 1, w: 1, h: 2 },
            ]
        );
    }

    #[test]
    fn decoration_quads_match_atlas_placement() {
        // Plain underline: one quad, `gap` px above the cell bottom.
        let quads = decoration_quads(DecorationStyle::Underline, 8, 40, 2);
        assert_eq!(quads, vec![SpriteQuad { x: 0, y: 36, w: 8, h: 2 }]);

        // Double underline: two bars separated by a `thickness` gap.
        let quads = decoration_quads(DecorationStyle::DoubleUnderline, 8, 40, 2);
        assert_eq!(
            quads,
            vec![
                SpriteQuad { x: 0, y: 32, w: 8, h: 2 },
                SpriteQuad { x: 0, y: 36, w: 8, h: 2 },
            ]
        );

        // Strikethrough: centered on the cell's vertical midpoint.
        let quads = decoration_quads(DecorationStyle::Strikethrough, 8, 40, 2);
        assert_eq!(quads, vec![SpriteQuad { x: 0, y: 19, w: 8, h: 2 }]);

        // Curly underline decomposes into per-column quads that stay
        // inside the sprite band above the bottom gap.
        let quads = decoration_quads(DecorationStyle::CurlyUnderline, 8, 40, 2);
        assert!(!quads.is_empty());
        let gap = underline_gap_below(40) as i32;
        assert!(quads.iter().all(|q| q.y + q.h as i32 <= 40 - gap));
    }

    #[test]
    fn cursor_quads_match_atlas_placement() {
        // Block: one full-cell quad.
        let quads = cursor_quads(CursorSpriteStyle::Block, 8, 16, 1);
        assert_eq!(quads, vec![SpriteQuad { x: 0, y: 0, w: 8, h: 16 }]);

        // Hollow: 4 border quads (top, left, right, bottom).
        let quads = cursor_quads(CursorSpriteStyle::Hollow, 5, 5, 1);
        assert_eq!(
            quads,
            vec![
                SpriteQuad { x: 0, y: 0, w: 5, h: 1 },
                SpriteQuad { x: 0, y: 1, w: 1, h: 3 },
                SpriteQuad { x: 4, y: 1, w: 1, h: 3 },
                SpriteQuad { x: 0, y: 4, w: 5, h: 1 },
            ]
        );

        // Bar: thickness-wide column centered on the cell's left edge
        // (negative bearing_x).
        let quads = cursor_quads(CursorSpriteStyle::Bar, 10, 24, 1);
        assert_eq!(quads, vec![SpriteQuad { x: -1, y: 0, w: 1, h: 24 }]);

        // Underline cursor: bar `gap` px above the cell bottom.
        let quads = cursor_quads(CursorSpriteStyle::Underline, 10, 40, 2);
        assert_eq!(quads, vec![SpriteQuad { x: 0, y: 36, w: 10, h: 2 }]);
    }

    #[test]
    fn hollow_cursor_rasterizes_border_only() {
        let (bytes, w, h, bearing_x, bearing_y) =
            rasterize_cursor(CursorSpriteStyle::Hollow, 5, 5, 1);
        assert_eq!((w, h, bearing_x, bearing_y), (5, 5, 0, 5));
        assert_eq!(bytes[0..5], [0xFF; 5]);
        assert_eq!(bytes[20..25], [0xFF; 5]);
        assert_eq!(bytes[6], 0);
        assert_eq!(bytes[12], 0);
        assert_eq!(bytes[18], 0);
        assert_eq!(bytes[5], 0xFF);
        assert_eq!(bytes[9], 0xFF);
    }
}
