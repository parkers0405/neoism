// Copyright (c) 2023-present, Raphael Amorim.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Foreground-row builder. Performs run-level shaping (so ligatures
//! like `=>`, `!=`, `fi` form correctly) and emits one `CellText` per
//! shaped glyph (not per input cell). Decoration sprites (underlines /
//! strikethrough) are emitted in z-ordered phases around the glyph
//! pass: underlines first (draw under), glyphs, then strikethroughs
//! (draw on top).

use neoism_backend::sugarloaf::font::FontLibrary;
use neoism_backend::sugarloaf::grid::{CellText, GridRenderer};
use neoism_terminal_core::colors::term::TermColors;
use neoism_terminal_core::crosswords::grid::row::Row;
use neoism_terminal_core::crosswords::pos::Column;
use neoism_terminal_core::crosswords::square::Square;
use neoism_terminal_core::crosswords::style::{StyleFlags, StyleSet};
#[cfg(target_os = "macos")]
use neoism_ui::terminal_grid_emit::glyph_cell_offsets_utf16;
use neoism_ui::terminal_grid_emit::{
    cell_in_row_sel, glyph_cell_offsets_utf8, rounded_terminal_cell_size,
    shaping_style_flags, terminal_font_size_u16, terminal_size_bucket, RowSelection,
};
use smallvec::SmallVec;

use crate::host::Renderer;
use crate::terminal::grid_emit::cell_color::{cell_fg, cell_fg_selected};
use crate::terminal::grid_emit::decoration::{
    decoration_color, decoration_thickness, ensure_decoration_slot,
    underline_style_from_flags, DecorationStyle,
};
use crate::terminal::grid_emit::glyph_raster::ensure_glyph_by_id;
use crate::terminal::grid_emit::hints::{
    cell_fg_hinted, cell_in_hover_underline, cell_in_row_hints, HintTag, RowHint,
};
use crate::terminal::grid_emit::run_shaping::{
    is_run_breaker, run_cache_get, run_cache_put, run_hash, GridGlyphRasterizer,
    RunCacheEntry,
};

#[cfg(target_os = "macos")]
use crate::terminal::grid_emit::run_shaping::shape_run_ct;
#[cfg(not(target_os = "macos"))]
use crate::terminal::grid_emit::run_shaping::shape_run_swash;

/// Run-level fg emission. Shapes once per run, emits one CellText per
/// shaped glyph. Works on both macOS (CoreText) and non-macOS (swash).
///
/// Emits in three ordered phases so decoration z-order matches
/// 's: underlines first (drawn under glyphs), glyphs, then
/// strikethroughs (drawn on top).
#[allow(clippy::too_many_arguments)]
pub fn build_row_fg(
    row: &Row<Square>,
    cols: usize,
    y: u16,
    style_set: &StyleSet,
    renderer: &Renderer,
    term_colors: &TermColors,
    rasterizer: &mut GridGlyphRasterizer,
    grid: &mut GridRenderer,
    size_px: f32,
    cell_w: f32,
    cell_h: f32,
    row_sel: Option<RowSelection>,
    row_hints: &[RowHint],
    pixel_offset_y: i32,
    font_library: &FontLibrary,
    fg_scratch: &mut Vec<CellText>,
) {
    fg_scratch.clear();

    // Same resize-race guard as `build_row_bg` — see that fn's
    // comment. The bg/fg pair must clamp to the same width so the
    // emitted CellBg and CellText vectors line up cell-for-cell.
    let cols = cols.min(row.len());

    let size_bucket = terminal_size_bucket(size_px);
    let size_u16 = terminal_font_size_u16(size_px);

    let cell_w_u32 = rounded_terminal_cell_size(cell_w);
    let cell_h_u32 = rounded_terminal_cell_size(cell_h);
    let thickness = decoration_thickness(size_px);

    // Row-level state hoisted out of the per-glyph emit loop. Same
    // optimisation as `build_row_bg`'s fast path — avoids the
    // `cell_in_row_sel` + `cell_in_row_hints` calls per glyph when
    // the row has no selection / no color-changing hints.
    let has_sel = row_sel.is_some();
    let has_color_hints = row_hints.iter().any(|rh| rh.tag != HintTag::HyperlinkHover);
    let needs_per_cell_check = has_sel || has_color_hints;

    // Phase 1: underline pass. Emit before glyphs so grayscale quads
    // draw under the characters.
    emit_underlines(
        row,
        cols,
        y,
        style_set,
        renderer,
        term_colors,
        grid,
        cell_w_u32,
        cell_h_u32,
        thickness,
        row_sel,
        row_hints,
        pixel_offset_y,
        fg_scratch,
    );

    let mut x: usize = 0;
    while x < cols {
        let sq = row[Column(x)];
        if is_run_breaker(sq) {
            x += 1;
            continue;
        }

        // Open a run at x.
        let ch = sq.c();
        let run_style_flags = shaping_style_flags(style_set.get(sq.style_id()).flags);
        let (font_id, is_emoji) =
            rasterizer.resolve_font(ch, run_style_flags, font_library);
        let run_start = x;

        #[cfg(target_os = "macos")]
        {
            rasterizer.run_utf16_scratch.clear();
            rasterizer.run_cell_starts.clear();
            rasterizer
                .run_cell_starts
                .push(rasterizer.run_utf16_scratch.len() as u32);
            let mut buf = [0u16; 2];
            rasterizer
                .run_utf16_scratch
                .extend_from_slice(ch.encode_utf16(&mut buf));
        }
        #[cfg(not(target_os = "macos"))]
        {
            rasterizer.run_str_scratch.clear();
            rasterizer.run_str_scratch.push(ch);
        }

        // Extend the run while (font_id, style_flags) match.
        let mut end = x + 1;
        while end < cols {
            let sq2 = row[Column(end)];
            if is_run_breaker(sq2) {
                break;
            }
            let ch2 = sq2.c();
            let style2_flags = shaping_style_flags(style_set.get(sq2.style_id()).flags);
            if style2_flags != run_style_flags {
                break;
            }
            let (font_id2, _) = rasterizer.resolve_font(ch2, style2_flags, font_library);
            if font_id2 != font_id {
                break;
            }
            #[cfg(target_os = "macos")]
            {
                rasterizer
                    .run_cell_starts
                    .push(rasterizer.run_utf16_scratch.len() as u32);
                let mut buf = [0u16; 2];
                rasterizer
                    .run_utf16_scratch
                    .extend_from_slice(ch2.encode_utf16(&mut buf));
            }
            #[cfg(not(target_os = "macos"))]
            {
                rasterizer.run_str_scratch.push(ch2);
            }
            end += 1;
        }

        #[cfg(target_os = "macos")]
        let run_bytes: &[u8] = {
            // Reinterpret the u16 scratch as bytes for the hasher —
            // same alignment rule as `slice::align_to`, but we know
            // u16 → u8 is always well-aligned so this is a trivial
            // cast. Only the byte pattern matters for the hash.
            let s = &rasterizer.run_utf16_scratch;
            // Safety: `u16` has stricter alignment than `u8`; the
            // resulting byte slice aliases `s` read-only for the
            // lifetime of this borrow.
            unsafe {
                core::slice::from_raw_parts(
                    s.as_ptr() as *const u8,
                    s.len() * core::mem::size_of::<u16>(),
                )
            }
        };
        #[cfg(not(target_os = "macos"))]
        let run_bytes: &[u8] = rasterizer.run_str_scratch.as_bytes();
        let hash = run_hash(font_id, size_bucket, run_style_flags, run_bytes);

        // Shape (cached) and capture the primary-font baseline/cell metrics
        // shared by this fallback run.
        let vertical_metrics = if run_cache_get(&mut rasterizer.run_cache, hash).is_some()
        {
            // Cache hit — vertical metrics are already stored.
            rasterizer
                .vertical_metrics_cache
                .get(&(font_id, size_bucket))
                .copied()
                .unwrap_or_else(|| {
                    super::run_shaping::GridVerticalMetrics::fallback(size_u16)
                })
        } else {
            #[cfg(target_os = "macos")]
            let shaped_opt =
                shape_run_ct(rasterizer, font_id, size_u16, size_bucket, font_library);
            #[cfg(not(target_os = "macos"))]
            let shaped_opt =
                shape_run_swash(rasterizer, font_id, size_u16, size_bucket, font_library);
            let Some((glyphs, vertical_metrics)) = shaped_opt else {
                x = end;
                continue;
            };
            run_cache_put(&mut rasterizer.run_cache, RunCacheEntry { hash, glyphs });
            vertical_metrics
        };

        // `cell_h` includes the user's line-height multiplier. Put half of
        // the extra line box above the shared baseline and half below so a
        // taller line-height does not pin every font to the row's top edge.
        let baseline_px = vertical_metrics.baseline_for_cell_height(cell_h);

        let (synthetic_bold, synthetic_italic) =
            rasterizer.get_synthesis(font_id, font_library);

        // Collect (glyph_id, cell_offset) pairs by walking the shape
        // result alongside a monotonic cluster → cell-offset cursor.
        // Done up-front so we can release borrows on `rasterizer`
        // before the emit loop (which takes `&mut rasterizer` for the
        // rasterize + atlas-insert step).
        //
        // Cluster space differs by platform: macOS CoreText reports
        // UTF-16 code-unit offsets, swash reports UTF-8 byte offsets.
        // Each backend walks its own cell-position table.
        //
        // SmallVec inline capacity 64 covers terminal-typical runs
        // (ASCII identifiers, short bursts of non-ligature text)
        // entirely on the stack — no heap touch. Ligature-heavy or
        // shaped emoji runs that outgrow 64 slots spill to heap once.
        let mut glyph_emits: SmallVec<[(u16, u16); 64]> = SmallVec::new();
        {
            let glyphs =
                run_cache_get(&mut rasterizer.run_cache, hash).expect("just inserted");
            #[cfg(target_os = "macos")]
            {
                let offsets = glyph_cell_offsets_utf16(
                    &rasterizer.run_cell_starts,
                    glyphs.iter().map(|g| g.cluster),
                );
                glyph_emits.extend(
                    glyphs
                        .iter()
                        .zip(offsets)
                        .map(|(g, cell_offset)| (g.id, cell_offset)),
                );
            }
            #[cfg(not(target_os = "macos"))]
            {
                let offsets = glyph_cell_offsets_utf8(
                    &rasterizer.run_str_scratch,
                    glyphs.iter().map(|g| g.cluster),
                );
                glyph_emits.extend(
                    glyphs
                        .iter()
                        .zip(offsets)
                        .map(|(g, cell_offset)| (g.id, cell_offset)),
                );
            }
        }

        for &(glyph_id, cell_idx_in_run) in &glyph_emits {
            let grid_col = (run_start as u16).saturating_add(cell_idx_in_run);
            if (grid_col as usize) >= cols {
                continue;
            }

            let Some((_, slot, is_color)) = ensure_glyph_by_id(
                rasterizer,
                grid,
                font_id,
                glyph_id,
                size_bucket,
                size_u16,
                cell_h,
                baseline_px,
                is_emoji,
                synthetic_italic,
                synthetic_bold,
            ) else {
                continue;
            };
            if slot.w == 0 || slot.h == 0 {
                continue;
            }

            // Pull fg from the cluster's first cell. Non-ligature runs
            // end up with one cluster per cell (per-cell colour);
            // ligatures take the first cluster cell's colour.
            let src_col =
                (run_start + cell_idx_in_run as usize).min(cols.saturating_sub(1));
            let src_sq = row[Column(src_col)];
            let (atlas, color) = if is_color {
                // Colour glyphs (emoji) don't take the selection-fg /
                // hint-fg swap — behaviour for
                // bitmap/COLR atlas entries.
                (CellText::ATLAS_COLOR, [255, 255, 255, 255])
            } else if !needs_per_cell_check {
                // Fast path — no selection / color-changing hints on
                // this row.
                (
                    CellText::ATLAS_GRAYSCALE,
                    cell_fg(src_sq, style_set, renderer, term_colors),
                )
            } else {
                let is_sel = cell_in_row_sel(row_sel, src_col as u16);
                let hint_tag = if is_sel {
                    None
                } else {
                    cell_in_row_hints(row_hints, src_col as u16)
                };
                if is_sel {
                    (
                        CellText::ATLAS_GRAYSCALE,
                        cell_fg_selected(src_sq, style_set, renderer, term_colors),
                    )
                } else if let Some(tag) = hint_tag {
                    // Hint-fg wins over the cell's own fg, matching
                    // `.search` / `.search_selected` branches at
                    // `generic.zig:2829-2833` (the fg picker mirrors bg).
                    (CellText::ATLAS_GRAYSCALE, cell_fg_hinted(tag, renderer))
                } else {
                    (
                        CellText::ATLAS_GRAYSCALE,
                        cell_fg(src_sq, style_set, renderer, term_colors),
                    )
                }
            };

            fg_scratch.push(CellText {
                glyph_pos: [slot.x as u32, slot.y as u32],
                glyph_size: [slot.w as u32, slot.h as u32],
                bearings: [slot.bearing_x, slot.bearing_y],
                grid_pos: [grid_col, y],
                color,
                atlas,
                bools: 0,
                _pad: [0, 0],
                pixel_offset_y,
            });
        }

        x = end;
    }

    // Phase 3: strikethrough pass. Emitted last so the strike overlays
    // the glyph.
    emit_strikethroughs(
        row,
        cols,
        y,
        style_set,
        renderer,
        term_colors,
        grid,
        cell_w_u32,
        cell_h_u32,
        thickness,
        row_sel,
        row_hints,
        pixel_offset_y,
        fg_scratch,
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_underlines(
    row: &Row<Square>,
    cols: usize,
    y: u16,
    style_set: &StyleSet,
    renderer: &Renderer,
    term_colors: &TermColors,
    grid: &mut GridRenderer,
    cell_w: u32,
    cell_h: u32,
    thickness: u32,
    row_sel: Option<RowSelection>,
    row_hints: &[RowHint],
    pixel_offset_y: i32,
    fg_scratch: &mut Vec<CellText>,
) {
    for x in 0..cols {
        let sq = row[Column(x)];
        let style = style_set.get(sq.style_id());
        let col = x as u16;
        // SGR underline (UNDER, double, curly, …) wins over the
        // hover-only forced underline. When the cell has no SGR
        // decoration but is inside a hovered hyperlink, emit a plain
        // single-line underline using the cell fg color — same shape
        // as hyperlink-hover affordance.
        let (deco, hover_force) = match underline_style_from_flags(style.flags) {
            Some(d) => (d, false),
            None if cell_in_hover_underline(row_hints, col) => {
                (DecorationStyle::Underline, true)
            }
            None => continue,
        };
        let Some(slot) = ensure_decoration_slot(grid, deco, cell_w, cell_h, thickness)
        else {
            continue;
        };
        if slot.w == 0 || slot.h == 0 {
            continue;
        }
        let color = if cell_in_row_sel(row_sel, col) {
            // Inside selection: underline follows the selection fg so
            // it stays visible against the selection bg. SGR 58 is
            // suppressed here — a theme's selection_foreground
            // overrides per-cell decoration color.
            cell_fg_selected(sq, style_set, renderer, term_colors)
        } else if let Some(tag) = cell_in_row_hints(row_hints, col) {
            // Same reasoning as selection: underline inside a hint
            // should stay legible on the hint bg.
            cell_fg_hinted(tag, renderer)
        } else if hover_force {
            // Hover-only forced underline: use the cell fg so the
            // underline tracks the hyperlink text color (matches
            // hyperlink hover affordance).
            cell_fg(sq, style_set, renderer, term_colors)
        } else {
            decoration_color(sq, &style, style_set, renderer, term_colors)
        };
        fg_scratch.push(CellText {
            glyph_pos: [slot.x as u32, slot.y as u32],
            glyph_size: [slot.w as u32, slot.h as u32],
            bearings: [slot.bearing_x, slot.bearing_y],
            grid_pos: [x as u16, y],
            color,
            atlas: CellText::ATLAS_GRAYSCALE,
            bools: 0,
            _pad: [0, 0],
            pixel_offset_y,
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_strikethroughs(
    row: &Row<Square>,
    cols: usize,
    y: u16,
    style_set: &StyleSet,
    renderer: &Renderer,
    term_colors: &TermColors,
    grid: &mut GridRenderer,
    cell_w: u32,
    cell_h: u32,
    thickness: u32,
    row_sel: Option<RowSelection>,
    row_hints: &[RowHint],
    pixel_offset_y: i32,
    fg_scratch: &mut Vec<CellText>,
) {
    for x in 0..cols {
        let sq = row[Column(x)];
        let style = style_set.get(sq.style_id());
        if !style.flags.contains(StyleFlags::STRIKEOUT) {
            continue;
        }
        let Some(slot) = ensure_decoration_slot(
            grid,
            DecorationStyle::Strikethrough,
            cell_w,
            cell_h,
            thickness,
        ) else {
            continue;
        };
        if slot.w == 0 || slot.h == 0 {
            continue;
        }
        let col = x as u16;
        // Strikethrough always uses the cell fg (there's no SGR for
        // a separate strike color, matching ).
        let color = if cell_in_row_sel(row_sel, col) {
            cell_fg_selected(sq, style_set, renderer, term_colors)
        } else if let Some(tag) = cell_in_row_hints(row_hints, col) {
            cell_fg_hinted(tag, renderer)
        } else {
            cell_fg(sq, style_set, renderer, term_colors)
        };
        fg_scratch.push(CellText {
            glyph_pos: [slot.x as u32, slot.y as u32],
            glyph_size: [slot.w as u32, slot.h as u32],
            bearings: [slot.bearing_x, slot.bearing_y],
            grid_pos: [x as u16, y],
            color,
            atlas: CellText::ATLAS_GRAYSCALE,
            bools: 0,
            _pad: [0, 0],
            pixel_offset_y,
        });
    }
}
