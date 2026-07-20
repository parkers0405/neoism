//! GUI painter for the code pane (sugarloaf). This is deliberately the
//! only file in `editor::code` allowed to touch sugarloaf — everything
//! it consumes (styled runs, display columns, gutter digits) comes
//! from the renderer-agnostic core so a tty host can paint the same
//! data as terminal cells.
//!
//! v1 look: nvim-style — line-number gutter (bright current line),
//! full-width cursorline band, `~` markers past EOF, block caret in
//! Normal/Visual and bar caret in Insert, uniform row height. Soft
//! wrap is the default (`CodePane::wrap`): long lines continue on
//! extra VISUAL rows with an empty gutter cell, and all scroll math
//! (spring, center-lock reveal, wheel snap) runs in visual-row space
//! via the cached `WrapIndex`. `wrap = false` restores NoWrap plus a
//! plain horizontal caret-follow (`scroll_x`).

use sugarloaf::{text::DrawOpts, Sugarloaf};

use crate::primitives::ide_theme::IdeTheme;
use crate::syntax::syn_color;

use super::feed::{styled_runs_with_syntax, CodeDiagnosticSeverity, CodeLineDiagnostic};
use super::layout::*;
use super::types::*;
use web_time::Instant;

const DEPTH: f32 = 0.0;
const ORDER_BG: u8 = 3;
const ORDER_TEXT: u8 = 8;
const CODE_FONT_SIZE: f32 = 14.0;
const ROW_HEIGHT_FACTOR: f32 = 1.5;
const GUTTER_PAD_X: f32 = 10.0;
const TEXT_PAD_X: f32 = 8.0;
const SCROLLBAR_W: f32 = 6.0;
const SCROLLBAR_MIN_THUMB_H: f32 = 28.0;

/// Paint the pane. Returns `true` while the scroll glide is still
/// animating (the host must schedule another frame).
pub fn render(
    sugarloaf: &mut Sugarloaf,
    pane: &mut CodePane,
    rect: [f32; 4],
    theme: &IdeTheme,
    text_occlusions: &[[f32; 4]],
    font_scale: f32,
    mouse: Option<[f32; 2]>,
) -> bool {
    let [x, y, w, h] = rect;
    if w <= 0.0 || h <= 0.0 {
        return false;
    }
    let font_scale = font_scale.clamp(0.5, 3.0);

    sugarloaf.rect(None, x, y, w, h, theme.f32(theme.bg), DEPTH, ORDER_BG);

    let font_size = CODE_FONT_SIZE * font_scale;
    let row_h_base = (font_size * ROW_HEIGHT_FACTOR).round();
    // Golden row fit: stretch line spacing a hair so whole rows fill
    // the pane EXACTLY — no leftover band above the status bar or
    // below the breadcrumbs. The stretch is at most 1/rows of a row
    // (~1px per line), invisible. Tiny panes (< 8 rows) keep the base
    // spacing and accept bottom slack instead of visibly stretching.
    let rows_fit = (h / row_h_base).floor().max(1.0);
    let row_h = if rows_fit >= 8.0 {
        h / rows_fit
    } else {
        row_h_base
    };
    let h_content = rows_fit * row_h;
    let grid_y = y;
    let clip = [x, grid_y, w, h_content];
    let base_opts = DrawOpts {
        font_size,
        color: theme.u8(theme.fg),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let cell_w = sugarloaf.text_mut().measure("0", &base_opts).max(1.0);

    if let Some(err) = pane.error.clone() {
        let opts = DrawOpts {
            color: theme.u8(theme.red),
            ..base_opts
        };
        draw_text(sugarloaf, x + 24.0, y + 24.0, &err, &opts, text_occlusions);
        return false;
    }

    let line_count = pane.buffer.line_count();
    let digits = gutter_digits(line_count);
    let gutter_w = GUTTER_PAD_X * 2.0 + digits as f32 * cell_w;
    let text_x = x + gutter_w + TEXT_PAD_X;

    // Whole-buffer syntax pass (multi-line-construct-correct); no-op
    // when the revision hasn't moved.
    let language = pane.language;
    pane.highlight.refresh(&pane.buffer, language);

    // Breadcrumb symbol trail. The parse is debounced: a held arrow
    // key changes the cursor line every frame, and a whole-file
    // tree-sitter parse per step tanks the frame rate — so recompute
    // only once the cursor has been still for a beat.
    const TRAIL_DEBOUNCE_SECS: f32 = 0.15;
    let trail_key = (pane.buffer.revision, pane.buffer.cursor_line);
    if pane.symbol_trail_key != Some(trail_key) {
        pane.symbol_trail_key = Some(trail_key);
        pane.symbol_trail_pending = Some((trail_key, Instant::now()));
    }

    // Wrap layout pass (DisplayMap-lite): text columns available to
    // the buffer between gutter and scrollbar decide how many VISUAL
    // rows each buffer line occupies. The prefix-sum index is cached on
    // (buffer revision, cols) — resize and edits rebuild it (O(lines)),
    // steady frames pay nothing.
    let text_right_pad = TEXT_PAD_X + SCROLLBAR_W + 2.0;
    let text_view_w = (x + w - text_x - text_right_pad).max(cell_w);
    let cols = if pane.wrap {
        ((text_view_w / cell_w).floor() as usize).max(1)
    } else {
        0 // NoWrap sentinel: identity index, horizontal caret-follow.
    };
    let wrap_key = (pane.buffer.revision, cols);
    if pane.wrap_index_key != Some(wrap_key)
        || !pane.wrap_index.is_valid_for(line_count)
    {
        pane.wrap_index = std::sync::Arc::new(WrapIndex::build(
            &pane.buffer.lines,
            cols,
            TAB_DISPLAY_WIDTH,
        ));
        pane.wrap_index_key = Some(wrap_key);
    }
    let wrap = pane.wrap_index.clone();
    let total_rows = wrap.total_rows(line_count);

    // Cursor in visual-row space: segment + column-within-segment come
    // from the same cut math the painter draws with.
    let cursor_line = pane.buffer.cursor_line.min(line_count.saturating_sub(1));
    let (cursor_seg, cursor_local_col) = wrap_visual_position(
        &pane.buffer.lines[cursor_line],
        pane.buffer.cursor_col,
        cols,
        TAB_DISPLAY_WIDTH,
    );
    let cursor_vrow = wrap.first_row_of_line(cursor_line) + cursor_seg;

    // Content extent + scroll clamp + caret reveal (nvim scrolloff).
    let content_h = total_rows as f32 * row_h;
    pane.content_height = content_h;
    pane.scroll_viewport_height = h_content;
    let max_scroll = (content_h - h_content).max(0.0);
    if pane.buffer.follow_cursor {
        pane.buffer.follow_cursor = false;
        pane.last_keyboard_reveal = Some(Instant::now());
        // nvim-style center-lock in PURE INTEGER ROWS (visual rows —
        // the wrap index already mapped the cursor): the cursor row
        // pins mid-viewport (clamps release at the buffer edges).
        // Integer math matters — the float version rounded a stretched
        // row height per keystroke and could flip one row up/down while
        // typing on one line (visible as a bounce). The dead-band means
        // an unchanged desired row NEVER touches the target.
        let rows_fit_i = (h_content / row_h).round().max(1.0) as i64;
        let max_top = ((content_h - h_content) / row_h).round().max(0.0) as i64;
        let half = (rows_fit_i - 1) / 2;
        let desired_center = cursor_vrow as i64 - half;
        let desired_top = desired_center.clamp(0, max_top);
        let current_top = (pane.target_scroll_y / row_h).round() as i64;
        // Centering achievable → hard center-lock (the loved mid-file
        // feel). Clamped at a buffer edge → centering would snap the
        // view (e.g. the first `j` after a top-of-file Ctrl-D dragged
        // everything back to row 0); switch to MINIMAL scroll there:
        // move only to keep the cursor a few rows off the view edge.
        let clamped = desired_top != desired_center;
        let new_top = if !clamped {
            desired_top
        } else {
            const EDGE_SCROLLOFF: i64 = 4;
            let cursor = cursor_vrow as i64;
            let margin = EDGE_SCROLLOFF.min((rows_fit_i - 1) / 2);
            if cursor < current_top + margin {
                (cursor - margin).clamp(0, max_top)
            } else if cursor > current_top + rows_fit_i - 1 - margin {
                (cursor + margin - rows_fit_i + 1).clamp(0, max_top)
            } else {
                current_top
            }
        };
        if std::env::var_os("NEOISM_SCROLL_LOG").is_some() {
            eprintln!(
                "neoism::scroll reveal cursor_vrow={cursor_vrow} desired_center={desired_center} clamped={clamped} current_top={current_top} new_top={new_top} max_top={max_top}"
            );
        }
        if new_top != current_top {
            pane.target_scroll_y = new_top as f32 * row_h;
            pane.target_scroll_raw = pane.target_scroll_y;
        }
    }
    pane.target_scroll_y = pane.target_scroll_y.clamp(0.0, max_scroll);

    // Horizontal caret-follow (NoWrap only): plain minimal adjustment
    // keeping the caret inside the text view with a 2-cell margin.
    if pane.wrap {
        pane.scroll_x = 0.0;
    } else {
        let caret_px = cursor_local_col as f32 * cell_w;
        let margin = (cell_w * 2.0).min(text_view_w * 0.25);
        let low = (caret_px + cell_w + margin - text_view_w).max(0.0);
        let high = (caret_px - margin).max(low);
        pane.scroll_x = pane.scroll_x.clamp(low, high);
    }
    let scroll_x = pane.scroll_x;

    // Scroll animation: critically-damped SPRING (the old nvim-era
    // editor_scroll feel). Unlike exponential decay — which teleports
    // to max velocity on the first frame and reads as a jerk — a
    // spring accelerates from rest, sweeps, and settles briskly with
    // no overshoot and no tail crawl. Velocity carries across target
    // changes, so held arrows and Ctrl-D chains stay one continuous
    // motion.
    let now = Instant::now();
    let dt = pane
        .scroll_last_tick_at
        .map(|at| (now - at).as_secs_f32().clamp(0.0, 0.05))
        .unwrap_or(1.0 / 60.0);
    pane.scroll_last_tick_at = Some(now);
    // Neovide's long-jump rule (rendered_window.rs keeps only 2×height
    // of scrollback): the animated travel is capped to ~a viewport —
    // the far portion of a `:1`-from-line-300 jump TELEPORTS (that
    // content was never on screen) and only the final stretch springs.
    // Uncapped, every frame of a huge sweep redraws the full viewport
    // and the settle phase visibly tanks the frame rate.
    let max_travel = h_content * 1.25;
    let far = pane.target_scroll_y - pane.scroll_y;
    if far.abs() > max_travel {
        pane.scroll_y = pane.target_scroll_y - max_travel * far.signum();
        pane.scroll_velocity_px_s = 0.0;
    }
    let delta = pane.target_scroll_y - pane.scroll_y;
    let scroll_animating =
        delta.abs() > 0.5 || pane.scroll_velocity_px_s.abs() > 1.0;
    if scroll_animating {
        // ω of the critically-damped spring; higher = snappier.
        const OMEGA: f32 = 16.0;
        let accel =
            OMEGA * OMEGA * delta - 2.0 * OMEGA * pane.scroll_velocity_px_s;
        pane.scroll_velocity_px_s += accel * dt;
        pane.scroll_y += pane.scroll_velocity_px_s * dt;
        if (pane.target_scroll_y - pane.scroll_y).abs() < 0.5
            && pane.scroll_velocity_px_s.abs() < 30.0
        {
            pane.scroll_y = pane.target_scroll_y;
            pane.scroll_velocity_px_s = 0.0;
        }
    } else {
        pane.scroll_y = pane.target_scroll_y;
        pane.scroll_velocity_px_s = 0.0;
    }
    pane.scroll_y = pane.scroll_y.clamp(0.0, max_scroll);

    // Symbol-trail parse (whole-file tree-sitter) fires only once the
    // cursor is still AND the glide has settled — a parse mid-sweep is
    // a visible hitch on long jumps.
    if !scroll_animating {
        if let Some((pending_key, since)) = pane.symbol_trail_pending {
            if since.elapsed().as_secs_f32() >= TRAIL_DEBOUNCE_SECS {
                pane.symbol_trail_pending = None;
                let source = pane.buffer.text();
                pane.symbol_trail = if pending_key == trail_key
                    && source.len() <= super::outline::OUTLINE_SOURCE_CUTOFF
                {
                    let cursor_byte = source
                        .split_inclusive('\n')
                        .take(pane.buffer.cursor_line)
                        .map(str::len)
                        .sum::<usize>()
                        + pane.buffer.cursor_col;
                    super::outline::symbol_trail(&source, language, cursor_byte)
                } else {
                    Vec::new()
                };
            }
        }
    }

    let first_row = (pane.scroll_y / row_h).floor().max(0.0) as usize;
    let visible_rows = (h_content / row_h).ceil() as usize + 1;
    let last_row = first_row.saturating_add(visible_rows).min(total_rows);
    let scroll_y = pane.scroll_y;
    let row_screen_y = move |vrow: usize| grid_y + vrow as f32 * row_h - scroll_y;

    // The visible VISUAL rows, resolved to (buffer line, wrap segment,
    // byte range, base display col) once — every paint pass below walks
    // this list. O(visible rows) per frame.
    struct RowView {
        vrow: usize,
        line: usize,
        seg: usize,
        seg_start: usize,
        seg_end: usize,
        base_col: usize,
    }
    let mut visible: Vec<RowView> = Vec::with_capacity(visible_rows.min(256));
    {
        let (mut line_ix, mut seg) = wrap.line_of_row(first_row, line_count);
        let mut vrow = first_row;
        while vrow < last_row && line_ix < line_count {
            let line = &pane.buffer.lines[line_ix];
            let starts = wrap_segment_starts(line, cols, TAB_DISPLAY_WIDTH);
            while seg < starts.len() && vrow < last_row {
                let (seg_start, base_col) = starts[seg];
                let seg_end =
                    starts.get(seg + 1).map(|s| s.0).unwrap_or(line.len());
                visible.push(RowView {
                    vrow,
                    line: line_ix,
                    seg,
                    seg_start,
                    seg_end,
                    base_col,
                });
                vrow += 1;
                seg += 1;
            }
            line_ix += 1;
            seg = 0;
        }
    }
    // Buffer text draws clip at the gutter edge so NoWrap horizontal
    // scroll never slides glyphs over the line numbers.
    let text_clip = [text_x, grid_y, (x + w - text_x).max(0.0), h_content];
    // Highlight bands are rects (no clip support): clamp into the text
    // area manually.
    let clamp_band = move |bx: f32, bw: f32| -> Option<(f32, f32)> {
        let left = bx.max(text_x);
        let right = (bx + bw).min(x + w);
        (right > left).then_some((left, right - left))
    };

    pane.geometry = CodePaneGeometry {
        rect: [x, grid_y, w, h_content],
        text_x,
        gutter_w,
        cell_w,
        row_h,
        first_row,
        scroll_y,
        scroll_x,
        wrap: wrap.clone(),
    };

    // Cursorline band across the full pane width (nvim `cursorline`
    // covers every wrapped row of the cursor's buffer line).
    let cursor_band_y = row_screen_y(wrap.first_row_of_line(cursor_line));
    let cursor_band_h = wrap.rows_of_line(cursor_line) as f32 * row_h;
    if cursor_band_y + cursor_band_h > grid_y && cursor_band_y < grid_y + h_content {
        sugarloaf.rect(
            None,
            x,
            cursor_band_y,
            w,
            cursor_band_h,
            theme.f32_alpha(theme.hover, 0.45),
            DEPTH,
            ORDER_BG,
        );
    }

    // Selection bands (under text, over cursorline).
    // hlsearch: highlight every visible occurrence of the active
    // search pattern (case-sensitive substring, matching `n`/`N`).
    if let Some(pattern) = pane
        .search_highlight
        .as_ref()
        .filter(|pattern| !pattern.is_empty())
        .cloned()
    {
        let hl_color = theme.f32_alpha(theme.yellow, 0.30);
        for rv in &visible {
            let ry = row_screen_y(rv.vrow);
            if ry + row_h <= grid_y || ry >= grid_y + h_content {
                continue;
            }
            let line = &pane.buffer.lines[rv.line];
            let mut from = 0usize;
            while let Some(found) = line[from..].find(pattern.as_str()) {
                let start = from + found;
                let end = start + pattern.len();
                from = end.max(start + 1);
                // Matches spanning a wrap boundary band on both rows.
                let s = start.max(rv.seg_start);
                let e = end.min(rv.seg_end);
                if s >= e {
                    continue;
                }
                let start_col = display_col_for_byte(line, s, TAB_DISPLAY_WIDTH)
                    .saturating_sub(rv.base_col);
                let end_col = display_col_for_byte(line, e, TAB_DISPLAY_WIDTH)
                    .saturating_sub(rv.base_col);
                let Some((bx, bw)) = clamp_band(
                    text_x + start_col as f32 * cell_w - scroll_x,
                    (end_col.saturating_sub(start_col)).max(1) as f32 * cell_w,
                ) else {
                    continue;
                };
                sugarloaf.rect(
                    None, bx, ry, bw, row_h, hl_color, DEPTH, ORDER_BG,
                );
            }
        }
    }

    // Yank flash: quick fading band over the yanked rows.
    const YANK_FLASH_SECS: f32 = 0.28;
    let mut flash_animating = false;
    if let Some((first, last, at)) = pane.buffer.yank_flash {
        let age = at.elapsed().as_secs_f32();
        if age >= YANK_FLASH_SECS {
            pane.buffer.yank_flash = None;
        } else {
            flash_animating = true;
            let alpha = 0.35 * (1.0 - age / YANK_FLASH_SECS);
            // Buffer-line span → visual-row span → one clamped band.
            let first = first.min(line_count.saturating_sub(1));
            let last = last.min(line_count.saturating_sub(1));
            let band_top = row_screen_y(wrap.first_row_of_line(first)).max(grid_y);
            let band_bottom =
                row_screen_y(wrap.first_row_of_line(last + 1)).min(grid_y + h_content);
            if band_bottom > band_top {
                sugarloaf.rect(
                    None,
                    x,
                    band_top,
                    w,
                    band_bottom - band_top,
                    theme.f32_alpha(theme.accent, alpha),
                    DEPTH,
                    ORDER_BG,
                );
            }
        }
    }

    let selection_color = theme.f32_alpha(theme.accent, 0.28);
    for rv in &visible {
        let Some((sel_start, sel_end)) = pane.buffer.selection_on_line(rv.line)
        else {
            continue;
        };
        let band_y = row_screen_y(rv.vrow);
        if band_y + row_h <= grid_y || band_y >= grid_y + h_content {
            continue;
        }
        let line = &pane.buffer.lines[rv.line];
        let s = sel_start.max(rv.seg_start);
        let e = sel_end.min(rv.seg_end);
        let (band_col, band_w) = if s < e {
            let start_col = display_col_for_byte(line, s, TAB_DISPLAY_WIDTH)
                .saturating_sub(rv.base_col);
            let end_col = display_col_for_byte(line, e, TAB_DISPLAY_WIDTH)
                .saturating_sub(rv.base_col);
            (
                start_col,
                (end_col.saturating_sub(start_col)) as f32 * cell_w,
            )
        } else if sel_end >= line.len()
            && sel_start >= sel_end
            && rv.seg_end >= line.len()
        {
            // Empty tail of a multi-line selection still shows a stub
            // (on the LAST wrapped row, where the line end lives).
            (
                display_col_for_byte(line, sel_start, TAB_DISPLAY_WIDTH)
                    .saturating_sub(rv.base_col),
                cell_w * 0.5,
            )
        } else {
            continue;
        };
        if band_w > 0.0 {
            if let Some((bx, bw)) = clamp_band(
                text_x + band_col as f32 * cell_w - scroll_x,
                band_w,
            ) {
                sugarloaf.rect(
                    None, bx, band_y, bw, row_h, selection_color, DEPTH, ORDER_BG,
                );
            }
        }
    }

    // Gutter numbers + text rows. Runs are computed once per buffer
    // line and sliced per wrap segment; continuation rows draw an
    // empty gutter cell (line number only on the first segment — nvim
    // look).
    let text_pad_y = ((row_h - font_size * 1.2) * 0.5).max(0.0);
    let number_dim = theme.u8_alpha(theme.dim, 0.9);
    let number_cursor = theme.u8(theme.fg);
    let mut runs_line = usize::MAX;
    let mut runs: Vec<super::feed::CodeStyledRun> = Vec::new();
    for rv in &visible {
        let ry = row_screen_y(rv.vrow);
        let ty = ry + text_pad_y;
        if rv.seg == 0 {
            let number = format!("{:>width$}", rv.line + 1, width = digits);
            let num_opts = DrawOpts {
                color: if rv.line == pane.buffer.cursor_line {
                    number_cursor
                } else {
                    number_dim
                },
                ..base_opts
            };
            draw_text(
                sugarloaf,
                x + GUTTER_PAD_X,
                ty,
                &number,
                &num_opts,
                text_occlusions,
            );
        }

        let line = &pane.buffer.lines[rv.line];
        if line.is_empty() {
            continue;
        }
        if rv.line != runs_line {
            runs_line = rv.line;
            let selection = pane.buffer.selection_on_line(rv.line);
            let diagnostics: &[CodeLineDiagnostic] = pane
                .diagnostics
                .get(&rv.line)
                .map(|diags| diags.as_slice())
                .unwrap_or(&[]);
            let syntax = pane.highlight.line_runs(rv.line);
            runs = styled_runs_with_syntax(
                line,
                syntax,
                pane.language,
                selection,
                diagnostics,
            );
        }
        for run in &runs {
            let sub_start = run.start.max(rv.seg_start);
            let sub_end = run.end.min(rv.seg_end);
            if sub_start >= sub_end {
                continue;
            }
            let start_col =
                display_col_for_byte(line, sub_start, TAB_DISPLAY_WIDTH);
            let run_x = text_x
                + (start_col.saturating_sub(rv.base_col)) as f32 * cell_w
                - scroll_x;
            let display = expand_tabs_from(
                &line[sub_start..sub_end],
                start_col,
                TAB_DISPLAY_WIDTH,
            );
            let run_opts = DrawOpts {
                color: syn_color(run.token, theme, false),
                clip_rect: Some(text_clip),
                ..base_opts
            };
            draw_text(sugarloaf, run_x, ty, &display, &run_opts, text_occlusions);
            if let Some(severity) = run.severity {
                let end_col =
                    display_col_for_byte(line, sub_end, TAB_DISPLAY_WIDTH);
                let underline_color = match severity {
                    CodeDiagnosticSeverity::Error => theme.f32_alpha(theme.red, 0.9),
                    CodeDiagnosticSeverity::Warn => theme.f32_alpha(theme.yellow, 0.9),
                    _ => theme.f32_alpha(theme.blue, 0.8),
                };
                if let Some((bx, bw)) = clamp_band(
                    run_x,
                    (end_col.saturating_sub(start_col)) as f32 * cell_w,
                ) {
                    sugarloaf.rect(
                        None,
                        bx,
                        ry + row_h - 3.0,
                        bw,
                        2.0,
                        underline_color,
                        DEPTH,
                        ORDER_TEXT,
                    );
                }
            }
        }

        // Inline diagnostic virtual text (nvim style): the strongest
        // message of the line, drawn once after its LAST wrap segment.
        if rv.seg_end >= line.len() {
            if let Some(diag) = pane.diagnostics.get(&rv.line).and_then(|diags| {
                diags
                    .iter()
                    .filter(|d| !d.message.is_empty())
                    .max_by_key(|d| d.severity)
            }) {
                let end_col = display_col_for_byte(line, line.len(), TAB_DISPLAY_WIDTH);
                let vx = text_x
                    + (end_col.saturating_sub(rv.base_col) + 2) as f32 * cell_w
                    - scroll_x;
                let color = match diag.severity {
                    CodeDiagnosticSeverity::Error => theme.u8_alpha(theme.red, 0.8),
                    CodeDiagnosticSeverity::Warn => theme.u8_alpha(theme.yellow, 0.8),
                    _ => theme.u8_alpha(theme.blue, 0.7),
                };
                let mut message = diag.message.replace('\n', "  ");
                // Clamp to the room left on this row (never run off the
                // pane); the full text is one click away (the span's
                // detail card).
                let avail_cells = (((x + w - SCROLLBAR_W - 8.0) - vx) / cell_w)
                    .floor()
                    .max(0.0) as usize;
                let max_chars = avail_cells.saturating_sub(2).min(160);
                if max_chars < 4 {
                    continue;
                }
                if message.chars().count() > max_chars {
                    message =
                        message.chars().take(max_chars).collect::<String>() + "…";
                }
                let virt = format!("■ {message}");
                let virt_opts = DrawOpts {
                    color,
                    clip_rect: Some(text_clip),
                    ..base_opts
                };
                draw_text(sugarloaf, vx, ty, &virt, &virt_opts, text_occlusions);
            }
        }
    }

    // (No `~` end-of-buffer markers — user preference; the area past
    // the last line stays clean background.)

    // Caret: bar in Insert, block in Normal/Visual (nvim look). The
    // block repaints the covered glyph in bg for contrast. The caret
    // box is the GLYPH box, not the full row — rows carry line-spacing
    // (ROW_HEIGHT_FACTOR) and a row-tall caret reads as stretched.
    // Position is visual: the wrap segment's row + the column within
    // that segment (continuation rows restart at the gutter edge).
    let cursor_line_text = &pane.buffer.lines[cursor_line];
    let cursor_row_y = row_screen_y(cursor_vrow);
    let caret_x = text_x + cursor_local_col as f32 * cell_w - scroll_x;
    let caret_h = (font_size * 1.2).min(row_h).round();
    let caret_y = cursor_row_y + ((row_h - caret_h) * 0.5).max(0.0).round();
    if cursor_row_y + row_h > grid_y && cursor_row_y < grid_y + h_content {
        match pane.buffer.mode {
            // When the host's trail cursor owns caret drawing the pane
            // only publishes `cursor_rect`; drawing both doubles the
            // caret.
            _ if pane.caret_drawn_by_host => {}
            CodeMode::Insert => {
                sugarloaf.rect(
                    None,
                    caret_x,
                    caret_y,
                    2.0,
                    caret_h,
                    theme.f32(theme.accent),
                    DEPTH,
                    ORDER_TEXT,
                );
            }
            CodeMode::Normal | CodeMode::Visual => {
                sugarloaf.rect(
                    None,
                    caret_x,
                    caret_y,
                    cell_w,
                    caret_h,
                    theme.f32(theme.accent),
                    DEPTH,
                    ORDER_TEXT,
                );
                let under = cursor_line_text
                    .get(pane.buffer.cursor_col..)
                    .and_then(|tail| tail.chars().next());
                if let Some(c) = under.filter(|c| !c.is_whitespace() && *c != '\t') {
                    let glyph_opts = DrawOpts {
                        color: theme.u8(theme.bg),
                        clip_rect: Some(text_clip),
                        ..base_opts
                    };
                    draw_text(
                        sugarloaf,
                        caret_x,
                        cursor_row_y + text_pad_y,
                        &c.to_string(),
                        &glyph_opts,
                        text_occlusions,
                    );
                }
            }
        }
    }
    let clamp_top = grid_y;
    let clamp_bottom = (grid_y + h_content - caret_h).max(clamp_top);
    let clamp_left = x;
    let clamp_right = (x + w - cell_w).max(clamp_left);
    pane.cursor_rect = Some([
        caret_x.clamp(clamp_left, clamp_right),
        caret_y.clamp(clamp_top, clamp_bottom),
        cell_w,
        caret_h,
    ]);

    // Read-only scrollbar thumb (drag interaction comes with the host
    // wiring pass).
    // Themed scrollbar (shared `ScrollbarStyle` — Mash Up Pack slot),
    // mirroring the markdown pane's bar: rounded track + thumb, hover
    // brightening, and published rects for the host's drag hit tests.
    pane.scrollbar_track = None;
    pane.scrollbar_thumb = None;
    if content_h > h_content + 1.0 {
        use crate::editor::markdown::render::draw::draw_rounded_rect_clipped;
        use crate::primitives::look::scrollbar_style;
        const BAR_MARGIN: f32 = 4.0;
        let style = scrollbar_style();
        let bar_w = style.width_or(SCROLLBAR_W).max(1.0);
        let min_thumb = style.min_thumb_or(SCROLLBAR_MIN_THUMB_H);
        let track_h = (h_content - BAR_MARGIN * 2.0).max(1.0);
        let thumb_h = (track_h * (h_content / content_h))
            .clamp(min_thumb.min(track_h), track_h);
        let progress = (pane.scroll_y / max_scroll.max(1.0)).clamp(0.0, 1.0);
        let thumb_y = grid_y + BAR_MARGIN + (track_h - thumb_h) * progress;
        let track_rect = [x + w - bar_w - BAR_MARGIN, grid_y + BAR_MARGIN, bar_w, track_h];
        let thumb_rect = [track_rect[0], thumb_y, bar_w, thumb_h];
        pane.scrollbar_track = Some(track_rect);
        pane.scrollbar_thumb = Some(thumb_rect);
        let dragging = pane.scrollbar_drag.is_some();
        let hovered = dragging
            || mouse.is_some_and(|[mx, my]| {
                mx >= track_rect[0] - 5.0
                    && mx <= track_rect[0] + track_rect[2] + 5.0
                    && my >= track_rect[1]
                    && my <= track_rect[1] + track_rect[3]
            });
        let radius = style.radius(bar_w, 0.5);
        if let Some(track_color) = style.track_or(Some(theme.f32_alpha(
            theme.border,
            if hovered { 0.22 } else { 0.10 },
        ))) {
            draw_rounded_rect_clipped(
                sugarloaf,
                clip,
                track_rect[0],
                track_rect[1],
                track_rect[2],
                track_rect[3],
                radius,
                track_color,
                DEPTH,
                ORDER_TEXT,
            );
        }
        let thumb_color = if dragging {
            style.thumb_drag_or(theme.f32_alpha(theme.accent, 0.85))
        } else {
            style.thumb_or(theme.f32_alpha(
                theme.border,
                if hovered { 0.95 } else { 0.7 },
            ))
        };
        draw_rounded_rect_clipped(
            sugarloaf,
            clip,
            thumb_rect[0],
            thumb_rect[1],
            thumb_rect[2],
            thumb_rect[3],
            radius,
            thumb_color,
            DEPTH,
            ORDER_TEXT,
        );
    }

    // Keep frames coming while the glide settles or a debounced
    // symbol-trail parse is waiting for the cursor to go still.
    scroll_animating || flash_animating || pane.symbol_trail_pending.is_some()
}

fn rects_intersect(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
}

/// Clip-aware, occlusion-aware single-line text draw (the code pane's
/// counterpart of the markdown renderer's `draw_if_visible`).
fn draw_text(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    text: &str,
    opts: &DrawOpts,
    occlusions: &[[f32; 4]],
) {
    let (clip_top, clip_bottom) = opts
        .clip_rect
        .map(|r| (r[1], r[1] + r[3]))
        .unwrap_or((f32::MIN, f32::MAX));
    let h = opts.font_size * 1.5;
    if y + h < clip_top || y > clip_bottom {
        return;
    }
    if !occlusions.is_empty() {
        let w = sugarloaf.text_mut().measure(text, opts);
        if occlusions
            .iter()
            .any(|rect| rects_intersect([x, y, w, h], *rect))
        {
            return;
        }
    }
    sugarloaf.text_mut().draw(x, y, text, opts);
}
