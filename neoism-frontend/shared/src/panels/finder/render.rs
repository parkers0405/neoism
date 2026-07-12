// Sugarloaf draw routines: chrome (input + caret + badge + separator),
// the result list (per-row file/grep/git layouts), the preview pane
// (file load + line-by-line render with the lightweight syntax
// highlighter), plus the small animation tick helpers.

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;
use web_time::Instant;

use super::modes::FinderMode;
use super::state::{
    Finder, CARET_BLINK_MS, CARET_WIDTH, COLUMN_DIVIDER_WIDTH, CURSOR_ANIMATION_LENGTH,
    DEPTH_BG, DEPTH_ELEMENT, FINDER_HEIGHT, FINDER_MARGIN_TOP, FINDER_PADDING,
    FINDER_RADIUS, INPUT_FONT_SIZE, INPUT_HEIGHT, INPUT_PADDING_X, LEFT_COL_RATIO,
    LIST_SCROLL_ANIMATION_LENGTH, OPEN_POP_MS, ORDER, PREVIEW_FONT_SIZE,
    PREVIEW_LINE_HEIGHT, PREVIEW_MAX_LINES, PREVIEW_PADDING,
    PREVIEW_SCROLL_ANIMATION_LENGTH, RESULT_FONT_SIZE, RESULT_ITEM_HEIGHT,
    SEPARATOR_HEIGHT,
};
use super::types::Result_;
use crate::animation::{ease_out_back, ease_out_cubic};
use crate::panels::file_tree::{self, icons::icon_for_file};
use crate::primitives::geom::snap_to_device_px;
use crate::primitives::text::truncate_to_fit;
use crate::primitives::IdeTheme;
use crate::services::{FilesService, SearchService};
use crate::syntax::{highlight_line, syn_color, Lang};

#[allow(clippy::too_many_arguments)]
fn draw_modal_frame_top(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    stroke: f32,
    radius: f32,
    stroke_color: [f32; 4],
    fill_color: [f32; 4],
    depth: f32,
    order: u8,
) {
    let outer_radii = [radius, radius, 0.0, 0.0];
    sugarloaf.overlay_quad(x, y, width, height, stroke_color, outer_radii, depth, order);
    let inner_radius = (radius - stroke).max(0.0);
    let inner_radii = [inner_radius, inner_radius, 0.0, 0.0];
    sugarloaf.overlay_quad(
        x + stroke,
        y + stroke,
        (width - stroke * 2.0).max(0.0),
        (height - stroke * 2.0).max(0.0),
        fill_color,
        inner_radii,
        depth + 0.001,
        order,
    );
}

impl Finder {
    pub(super) fn tick_list_scroll(&mut self) -> f32 {
        if self.list_scroll_spring.position == 0.0 {
            self.last_list_scroll_frame = Instant::now();
            return 0.0;
        }
        let now = Instant::now();
        let dt = now
            .saturating_duration_since(self.last_list_scroll_frame)
            .as_secs_f32()
            .min(0.05);
        self.last_list_scroll_frame = now;
        self.list_scroll_spring
            .update(dt, LIST_SCROLL_ANIMATION_LENGTH);
        self.list_scroll_spring.position
    }

    pub(super) fn tick_cursor(&mut self) -> f32 {
        if self.cursor_spring.position == 0.0 {
            self.last_cursor_frame = Instant::now();
            return 0.0;
        }
        let now = Instant::now();
        let dt = now
            .saturating_duration_since(self.last_cursor_frame)
            .as_secs_f32()
            .min(0.05);
        self.last_cursor_frame = now;
        self.cursor_spring.update(dt, CURSOR_ANIMATION_LENGTH);
        self.cursor_spring.position
    }

    pub(super) fn push_preview_target(
        &mut self,
        path: &str,
        start_line: u32,
        line_height: f32,
    ) {
        if self.preview_path.as_deref() != Some(path) {
            self.preview_path = Some(path.to_string());
            self.preview_start_line = start_line;
            self.preview_spring.reset();
            return;
        }
        if self.preview_start_line == start_line {
            return;
        }

        let was_idle = self.preview_spring.position == 0.0;
        let rows = start_line as i32 - self.preview_start_line as i32;
        self.preview_spring.position += rows as f32 * line_height;
        let cap = PREVIEW_MAX_LINES as f32 * line_height;
        self.preview_spring.position = self.preview_spring.position.clamp(-cap, cap);
        if was_idle {
            self.last_preview_frame = Instant::now();
        }
        self.preview_start_line = start_line;
    }

    pub(super) fn tick_preview(&mut self) -> f32 {
        if self.preview_spring.position == 0.0 {
            self.last_preview_frame = Instant::now();
            return 0.0;
        }
        let now = Instant::now();
        let dt = now
            .saturating_duration_since(self.last_preview_frame)
            .as_secs_f32()
            .min(0.05);
        self.last_preview_frame = now;
        self.preview_spring
            .update(dt, PREVIEW_SCROLL_ANIMATION_LENGTH);
        self.preview_spring.position
    }

    pub(super) fn open_pop_transform(&self, scale: f32) -> (f32, f32) {
        if !self.pop_on_open {
            return (1.0, 0.0);
        }

        let t = (Instant::now()
            .saturating_duration_since(self.open_pop_started)
            .as_secs_f32()
            * 1000.0
            / OPEN_POP_MS)
            .clamp(0.0, 1.0);
        let eased = ease_out_back(t).min(1.04);
        let pop_scale = 0.94 + (eased * 0.06);
        let pop_offset_y = (1.0 - ease_out_cubic(t)) * 14.0 * scale;
        (pop_scale, pop_offset_y)
    }

    pub fn render(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        dimensions: (f32, f32, f32),
        theme: &IdeTheme,
        search: &dyn SearchService,
        files: &dyn FilesService,
    ) {
        if !self.enabled {
            return;
        }
        self.tick(search);

        let (window_width, window_height, scale_factor) = dimensions;
        let logical_w = window_width / scale_factor;
        let logical_h = window_height / scale_factor;

        let scale = self.scale;
        let pad = FINDER_PADDING * scale;
        let input_h = INPUT_HEIGHT * scale;
        let row_h = RESULT_ITEM_HEIGHT * scale;
        let max_visible_rows = self.max_visible_rows(scale);
        // Collapse vertically when we don't have enough rows to fill the
        // body — empty skeleton lines were the bug the user flagged in
        // #8. Reserve a single empty row for the "Start typing…" / "No
        // matches" hint so the box never collapses to just an input.
        let visible_rows_actual = if self.results.is_empty() {
            1usize
        } else {
            self.results.len().min(max_visible_rows)
        };
        let body_h = (row_h * visible_rows_actual as f32) + 4.0 * scale;
        // Reserve preview space when there's a selection to show
        // anything meaningful inside it; otherwise drop it entirely so
        // the box is just the input + result strip.
        let preview_active = self.preview_enabled() && !self.results.is_empty();
        let body_with_preview_min = if preview_active {
            (FINDER_HEIGHT * 0.55 * scale).max(body_h)
        } else {
            body_h
        };
        let height = pad * 2.0 + input_h + SEPARATOR_HEIGHT + body_with_preview_min;
        let mut height = height.min(logical_h - 96.0);
        let base_width = (self.overlay_width() * scale).min(logical_w - 32.0);
        let mut width = base_width;
        let final_x = (logical_w - width) / 2.0;
        let final_y = FINDER_MARGIN_TOP * scale;
        let (pop_scale, pop_offset_y) = self.open_pop_transform(scale);
        width *= pop_scale;
        height *= pop_scale;
        let x = final_x + (base_width - width) / 2.0;
        let y = final_y + pop_offset_y;

        // Keep the body fully opaque so document/editor text behind it
        // never bleeds through the input, list, or preview panes.
        let _ = (logical_w, logical_h);

        let frame_stroke = (file_tree::FRAME_STROKE * scale).max(2.0);
        draw_modal_frame_top(
            sugarloaf,
            x,
            y,
            width,
            height,
            frame_stroke,
            FINDER_RADIUS * scale,
            theme.f32(theme.surface),
            theme.f32(theme.panel_bg()),
            DEPTH_BG,
            ORDER,
        );
        let pad = FINDER_PADDING * scale;
        let inner_x = x + frame_stroke + pad;
        let inner_y = y + frame_stroke + pad;
        let inner_w = (width - frame_stroke * 2.0 - pad * 2.0).max(0.0);
        let inner_h = (height - frame_stroke * 2.0 - pad * 2.0).max(0.0);
        let left_w = if preview_active {
            (inner_w * LEFT_COL_RATIO).floor()
        } else {
            inner_w
        };

        // Input field — left column, top.
        let input_h = INPUT_HEIGHT * scale;
        let input_font = INPUT_FONT_SIZE * scale;
        let input_pad_x = INPUT_PADDING_X * scale;
        let mode_glyph = match self.mode {
            FinderMode::Files => "› ",
            FinderMode::Grep => "/ ",
            FinderMode::GitChanges => "git ",
        };
        let placeholder = match self.mode {
            FinderMode::Files => "Find file in project...",
            FinderMode::Grep => "Grep across project...",
            FinderMode::GitChanges => "Filter git changes...",
        };
        let display_owned;
        let search_mode_label = self.search_mode_label();
        let badge_text = format!("[{}]", search_mode_label);
        let badge_font = (input_font * 0.92).max(1.0);
        let badge_opts = DrawOpts {
            font_size: badge_font,
            color: theme.u8(theme.muted),
            clip_rect: Some([inner_x, inner_y, left_w, input_h]),
            ..DrawOpts::default()
        };
        let badge_w = sugarloaf
            .overlay_text_mut()
            .measure(&badge_text, &badge_opts);
        let badge_gap = 10.0 * scale;
        let input_text_w = (left_w - badge_w - badge_gap).max(0.0);
        let display_text = if self.query.is_empty() {
            placeholder
        } else {
            display_owned = format!("{}{}", mode_glyph, self.query);
            display_owned.as_str()
        };
        let text_color = if self.query.is_empty() {
            theme.u8(theme.muted)
        } else {
            theme.u8(theme.fg)
        };
        let input_opts = DrawOpts {
            font_size: input_font,
            color: text_color,
            clip_rect: Some([inner_x, inner_y, input_text_w, input_h]),
            ..DrawOpts::default()
        };
        let text_x = inner_x + input_pad_x;
        let text_y = inner_y + (input_h - input_font) / 2.0;
        let input_rendered_width =
            sugarloaf
                .overlay_text_mut()
                .draw(text_x, text_y, display_text, &input_opts);

        // Caret.
        let elapsed_ms = Instant::now()
            .saturating_duration_since(self.caret_blink_start)
            .as_millis();
        let caret_visible = (elapsed_ms / CARET_BLINK_MS).is_multiple_of(2);
        if caret_visible {
            let prefix_w = if self.query.is_empty() {
                0.0
            } else {
                input_rendered_width
            };
            let max_caret_x = inner_x + input_text_w - input_pad_x - CARET_WIDTH * scale;
            let caret_x = (text_x + prefix_w).min(max_caret_x.max(text_x));
            let caret_height = input_font + 4.0;
            let caret_y = inner_y + (input_h - caret_height) / 2.0 + 2.0;
            sugarloaf.overlay_rect(
                caret_x,
                caret_y,
                CARET_WIDTH * scale,
                caret_height,
                theme.f32(theme.fg),
                DEPTH_ELEMENT,
                ORDER,
            );
        }

        let badge_x = inner_x + left_w - badge_w;
        let badge_y = inner_y + (input_h - badge_font) / 2.0;
        sugarloaf
            .overlay_text_mut()
            .draw(badge_x, badge_y, &badge_text, &badge_opts);

        // Separator under input.
        let sep_y = inner_y + input_h;
        sugarloaf.overlay_rect(
            inner_x,
            sep_y,
            left_w,
            SEPARATOR_HEIGHT,
            theme.f32(theme.border),
            DEPTH_ELEMENT,
            ORDER,
        );

        // Vertical divider between columns. Only draw it when there's
        // actually a preview pane to delineate; skipping it on empty
        // states avoids the orphan vertical line floating to the right
        // of the input box.
        let divider_x = inner_x + left_w;
        if preview_active {
            sugarloaf.overlay_rect(
                divider_x,
                inner_y,
                COLUMN_DIVIDER_WIDTH,
                inner_h,
                theme.f32(theme.border),
                DEPTH_ELEMENT,
                ORDER,
            );
        }

        // Result list.
        let results_y = sep_y + SEPARATOR_HEIGHT + 4.0 * scale;
        let row_h = RESULT_ITEM_HEIGHT * scale;
        let row_font = RESULT_FONT_SIZE * scale;
        let icon_font = row_font;
        let icon_gap = 8.0 * scale;
        let visible_rows =
            (((inner_y + inner_h) - results_y) / row_h).floor().max(1.0) as usize;
        let list_clip = [inner_x, results_y, left_w, (inner_y + inner_h) - results_y];
        self.clamp_scroll(visible_rows);
        let list_scroll_offset = snap_to_device_px(self.tick_list_scroll(), scale_factor);
        // Snap cursor offset too — same reason as list_scroll. The
        // selected-row highlight rides on this spring and was the only
        // chrome panel surface still emitting sub-pixel y values.
        let cursor_offset = snap_to_device_px(self.tick_cursor(), scale_factor);
        let list_bottom = inner_y + inner_h;
        self.selected_cursor_rect = None;

        if !self.results.is_empty() && self.selected_index < self.results.len() {
            let row_ix = self.selected_index as isize - self.scroll_offset as isize;
            let item_y =
                results_y + row_h * row_ix as f32 + list_scroll_offset + cursor_offset;
            let item_bottom = item_y + row_h;
            let visible_y = item_y.max(results_y);
            let visible_h = item_bottom.min(list_bottom) - visible_y;
            if visible_h > 0.0 {
                sugarloaf.overlay_rounded_rect(
                    inner_x,
                    visible_y,
                    left_w,
                    visible_h,
                    theme.f32(theme.hover),
                    DEPTH_ELEMENT,
                    3.0,
                    ORDER,
                );
                let cursor_w = (row_font * 0.6).max(2.0);
                let cursor_x =
                    (inner_x + input_pad_x - cursor_w - 2.0 * scale).max(inner_x);
                let cursor_h = (row_h - 5.0 * scale).max(row_font).min(row_h);
                let cursor_y = (item_y + (row_h - cursor_h) / 2.0)
                    .clamp(results_y, (list_bottom - cursor_h).max(results_y));
                self.selected_cursor_rect =
                    Some([cursor_x, cursor_y, cursor_w, cursor_h]);
            }
        }

        // Snapshot rows we want to render so we don't borrow self while
        // calling sugarloaf APIs that need &mut.
        let total = self.results.len();
        let overscan =
            ((list_scroll_offset.abs() / row_h).ceil() as usize).saturating_add(1);
        let start = self.scroll_offset.saturating_sub(overscan);
        let end = (self.scroll_offset + visible_rows + overscan).min(total);
        let visible: Vec<(usize, bool, Result_)> = self.results[start..end]
            .iter()
            .enumerate()
            .map(|(i, (_, r))| {
                let actual = start + i;
                (actual, actual == self.selected_index, r.clone())
            })
            .collect();

        for (actual_i, _is_selected, row) in visible.iter() {
            let display_i = *actual_i as isize - self.scroll_offset as isize;
            let item_y = results_y + row_h * display_i as f32 + list_scroll_offset;
            if item_y + row_h <= results_y || item_y >= list_bottom {
                continue;
            }
            let baseline = item_y + (row_h - row_font) / 2.0;
            let icon_y = item_y + (row_h - icon_font) / 2.0;
            // Width budget — left column minus the row's left padding.
            // Anything wider than this needs to be truncated with `…`
            // so it doesn't spill into the divider / preview pane.
            let available_w = left_w - input_pad_x * 2.0;
            match row {
                Result_::File(f) => {
                    let file_name = leaf_name(&f.path);
                    let (icon_glyph, icon_color) = icon_for_file(file_name);
                    let icon_opts = DrawOpts {
                        font_size: icon_font,
                        color: icon_color,
                        clip_rect: Some(list_clip),
                        ..DrawOpts::default()
                    };
                    let dir_opts = DrawOpts {
                        font_size: row_font,
                        color: theme.u8(theme.muted),
                        clip_rect: Some(list_clip),
                        ..DrawOpts::default()
                    };
                    let name_opts = DrawOpts {
                        font_size: row_font,
                        color: theme.u8(theme.fg),
                        clip_rect: Some(list_clip),
                        ..DrawOpts::default()
                    };
                    let icon_x = inner_x + input_pad_x;
                    let icon_w = sugarloaf
                        .overlay_text_mut()
                        .draw(icon_x, icon_y, icon_glyph, &icon_opts);
                    let cursor_x = icon_x + icon_w + icon_gap;
                    let text_w = (available_w - icon_w - icon_gap).max(0.0);
                    let name_w =
                        sugarloaf.overlay_text_mut().measure(file_name, &name_opts);
                    let dir = parent_path(&f.path);
                    let dir_text = if dir.is_empty() {
                        String::new()
                    } else {
                        format!("  {dir}")
                    };
                    if name_w >= text_w {
                        let name =
                            truncate_to_fit(file_name, text_w, sugarloaf, &name_opts);
                        sugarloaf
                            .overlay_text_mut()
                            .draw(cursor_x, baseline, &name, &name_opts);
                    } else {
                        sugarloaf
                            .overlay_text_mut()
                            .draw(cursor_x, baseline, file_name, &name_opts);
                        if !dir_text.is_empty() {
                            let dir_budget = (text_w - name_w).max(0.0);
                            let dir = truncate_to_fit(
                                &dir_text, dir_budget, sugarloaf, &dir_opts,
                            );
                            sugarloaf.overlay_text_mut().draw(
                                cursor_x + name_w,
                                baseline,
                                &dir,
                                &dir_opts,
                            );
                        }
                    }
                }
                Result_::Grep(g) => {
                    let file_name = leaf_name(&g.path);
                    let (icon_glyph, icon_color) = icon_for_file(file_name);
                    let icon_opts = DrawOpts {
                        font_size: icon_font,
                        color: icon_color,
                        clip_rect: Some(list_clip),
                        ..DrawOpts::default()
                    };
                    let icon_x = inner_x + input_pad_x;
                    let icon_w = sugarloaf
                        .overlay_text_mut()
                        .draw(icon_x, icon_y, icon_glyph, &icon_opts);
                    // file:line  text — header fixed, body fills the rest.
                    let header = format!("{}:{}", short_path(&g.path), g.line);
                    let header_opts = DrawOpts {
                        font_size: row_font,
                        color: theme.u8(theme.muted),
                        clip_rect: Some(list_clip),
                        ..DrawOpts::default()
                    };
                    let text_opts = DrawOpts {
                        font_size: row_font,
                        color: theme.u8(theme.fg),
                        clip_rect: Some(list_clip),
                        ..DrawOpts::default()
                    };
                    let cursor_x = icon_x + icon_w + icon_gap;
                    let text_w = (available_w - icon_w - icon_gap).max(0.0);
                    let header_trimmed =
                        truncate_to_fit(&header, text_w * 0.45, sugarloaf, &header_opts);
                    let header_w = sugarloaf.overlay_text_mut().draw(
                        cursor_x,
                        baseline,
                        &header_trimmed,
                        &header_opts,
                    );
                    let body_x = cursor_x + header_w + (8.0 * scale);
                    let body_w = (cursor_x + text_w) - body_x;
                    let body = truncate_to_fit(
                        g.text.trim_start(),
                        body_w,
                        sugarloaf,
                        &text_opts,
                    );
                    sugarloaf
                        .overlay_text_mut()
                        .draw(body_x, baseline, &body, &text_opts);
                }
                Result_::Git(g) => {
                    let file_name = leaf_name(&g.path);
                    let (icon_glyph, icon_color) = icon_for_file(file_name);
                    let icon_opts = DrawOpts {
                        font_size: icon_font,
                        color: icon_color,
                        clip_rect: Some(list_clip),
                        ..DrawOpts::default()
                    };
                    let icon_x = inner_x + input_pad_x;
                    let icon_w = sugarloaf
                        .overlay_text_mut()
                        .draw(icon_x, icon_y, icon_glyph, &icon_opts);
                    let marker_opts = DrawOpts {
                        font_size: row_font,
                        color: g.status.color(theme),
                        bold: true,
                        clip_rect: Some(list_clip),
                        ..DrawOpts::default()
                    };
                    let path_opts = DrawOpts {
                        font_size: row_font,
                        color: theme.u8(theme.fg),
                        clip_rect: Some(list_clip),
                        ..DrawOpts::default()
                    };
                    let text_opts = DrawOpts {
                        font_size: row_font,
                        color: theme.u8(theme.muted),
                        clip_rect: Some(list_clip),
                        ..DrawOpts::default()
                    };
                    let cursor_x = icon_x + icon_w + icon_gap;
                    let text_w = (available_w - icon_w - icon_gap).max(0.0);
                    let marker = g.status.marker();
                    let marker_w = sugarloaf.overlay_text_mut().draw(
                        cursor_x,
                        baseline,
                        marker,
                        &marker_opts,
                    );
                    let path_x = cursor_x + marker_w + 8.0 * scale;
                    let header = format!("{}:{}", short_path(&g.path), g.line);
                    let header_budget = text_w * 0.58;
                    let header_trimmed =
                        truncate_to_fit(&header, header_budget, sugarloaf, &path_opts);
                    let header_w = sugarloaf.overlay_text_mut().draw(
                        path_x,
                        baseline,
                        &header_trimmed,
                        &path_opts,
                    );
                    let body_x = path_x + header_w + 8.0 * scale;
                    let body_w = (cursor_x + text_w) - body_x;
                    let body = truncate_to_fit(
                        g.text.trim_start(),
                        body_w,
                        sugarloaf,
                        &text_opts,
                    );
                    sugarloaf
                        .overlay_text_mut()
                        .draw(body_x, baseline, &body, &text_opts);
                }
            }
        }

        // Empty state hint when no results yet.
        if total == 0 {
            let hint = if self.query.is_empty() {
                match self.mode {
                    FinderMode::Files => "Start typing to search files…",
                    FinderMode::Grep => "Start typing to grep…",
                    FinderMode::GitChanges => "No git changes",
                }
            } else if matches!(self.mode, FinderMode::Grep)
                && self.grep_query_too_short(&self.effective_search_key())
            {
                "Type at least 2 chars to grep…"
            } else {
                "No matches"
            };
            let opts = DrawOpts {
                font_size: row_font,
                color: theme.u8(theme.muted),
                clip_rect: Some(list_clip),
                ..DrawOpts::default()
            };
            sugarloaf.overlay_text_mut().draw(
                inner_x + input_pad_x,
                results_y + (row_h - row_font) / 2.0,
                hint,
                &opts,
            );
        }

        // Preview pane — only when there's a selection to preview, so
        // the empty-state box is just the input + hint row (no
        // floating empty preview column).
        if preview_active {
            let preview_x = divider_x + COLUMN_DIVIDER_WIDTH + (PREVIEW_PADDING * scale);
            let preview_w = (inner_x + inner_w) - preview_x - (PREVIEW_PADDING * scale);
            let preview_y = inner_y + (PREVIEW_PADDING * scale);
            let preview_h = inner_h - (PREVIEW_PADDING * scale * 2.0);
            self.render_preview(
                sugarloaf, preview_x, preview_y, preview_w, preview_h, scale, theme,
                files,
            );
        }
    }

    fn render_preview(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        scale: f32,
        theme: &IdeTheme,
        files: &dyn FilesService,
    ) {
        let Some((_score, row)) = self.results.get(self.selected_index) else {
            return;
        };
        let rel_path = row.path().to_string();
        let target_line = row.line().unwrap_or(1);
        let highlight_lineno = row.line();
        let git_status = match row {
            Result_::Git(g) => Some(g.status),
            _ => None,
        };
        let mut full_path = self.cwd.clone();
        full_path.push(&rel_path);

        // Header shows the relative path; path is read-only chrome.
        let header_font = PREVIEW_FONT_SIZE * scale;
        let header_opts = DrawOpts {
            font_size: header_font,
            color: theme.u8(theme.muted),
            clip_rect: Some([x, y, w, PREVIEW_LINE_HEIGHT * scale]),
            ..DrawOpts::default()
        };
        let header_text = truncate_to_fit(&rel_path, w, sugarloaf, &header_opts);
        sugarloaf
            .overlay_text_mut()
            .draw(x, y, &header_text, &header_opts);

        let body_y_start = y + (PREVIEW_LINE_HEIGHT * scale);
        let line_height = PREVIEW_LINE_HEIGHT * scale;
        let body_clip = [x, body_y_start, w, (y + h - body_y_start).max(0.0)];
        let max_lines = ((h - line_height) / line_height).floor().max(1.0) as usize;
        let max_lines = max_lines.min(PREVIEW_MAX_LINES);

        if self.preview_content_path.as_deref() != Some(rel_path.as_str()) {
            self.preview_content_path = Some(rel_path.clone());
            // Pull preview bytes through `FilesService` so native + web
            // share the same code path. Lossy UTF-8 decode mirrors the
            // desktop's `read_to_string` behaviour closely enough for a
            // read-only preview (invalid bytes become U+FFFD).
            match files.read_file(&full_path) {
                Ok(bytes) => {
                    let content = String::from_utf8_lossy(&bytes);
                    self.preview_content_lines =
                        content.lines().map(ToString::to_string).collect();
                    self.preview_content_unreadable = false;
                }
                Err(_) => {
                    self.preview_content_lines.clear();
                    self.preview_content_unreadable = true;
                }
            }
        }

        if self.preview_content_unreadable {
            let opts = DrawOpts {
                font_size: PREVIEW_FONT_SIZE * scale,
                color: theme.u8(theme.muted),
                clip_rect: Some(body_clip),
                ..DrawOpts::default()
            };
            sugarloaf
                .overlay_text_mut()
                .draw(x, body_y_start, "<unreadable>", &opts);
            return;
        }
        if self.preview_content_lines.is_empty() {
            return;
        }

        let total = self.preview_content_lines.len();
        let target = (target_line as usize).max(1).min(total);
        let half = max_lines / 2;
        let start = target.saturating_sub(half);
        let end = (start + max_lines).min(total);
        let start = end.saturating_sub(max_lines).max(1);

        self.push_preview_target(&rel_path, start as u32, line_height);
        let preview_offset =
            snap_to_device_px(self.tick_preview(), sugarloaf.scale_factor());
        let overscan =
            ((preview_offset.abs() / line_height).ceil() as usize).saturating_add(1);
        let visual_start = start.saturating_sub(overscan).max(1);
        let visual_end = (end + overscan).min(total);

        let lang = Lang::from_path(&rel_path);
        let char_w = PREVIEW_FONT_SIZE * scale * 0.6;
        let max_chars = if char_w <= 0.0 {
            usize::MAX
        } else {
            ((w / char_w).floor() as usize).saturating_sub(8)
        };

        let num_opts = DrawOpts {
            font_size: PREVIEW_FONT_SIZE * scale,
            color: theme.u8(theme.muted),
            clip_rect: Some(body_clip),
            ..DrawOpts::default()
        };

        for lineno in visual_start..=visual_end {
            let row_y = body_y_start
                + (lineno as i32 - start as i32) as f32 * line_height
                + preview_offset;
            if row_y + line_height <= body_y_start || row_y >= y + h {
                continue;
            }
            let raw = self
                .preview_content_lines
                .get(lineno - 1)
                .map(String::as_str)
                .unwrap_or_default();
            let text: String = raw.chars().take(max_chars).collect();
            let num_str = format!("{:>4}  ", lineno);
            // Per-token color rendering. Lines that aren't the current
            // grep match are dimmed slightly so the matched line pops.
            let is_match = Some(lineno as u32) == highlight_lineno;
            if is_match {
                let row_bg = git_status
                    .map(|status| status.f32_alpha(theme, 0.24))
                    .unwrap_or_else(|| theme.f32_alpha(theme.hover, 0.55));
                sugarloaf.overlay_rect(
                    x,
                    row_y,
                    w,
                    line_height,
                    row_bg,
                    DEPTH_ELEMENT,
                    ORDER,
                );
                if let Some(status) = git_status {
                    let rail_w = (3.0 * scale).max(2.0);
                    sugarloaf.overlay_rect(
                        x,
                        row_y,
                        rail_w,
                        line_height,
                        status.f32_alpha(theme, 0.95),
                        DEPTH_ELEMENT + 0.001,
                        ORDER,
                    );
                    sugarloaf.overlay_rect(
                        x,
                        row_y + line_height - (1.0 * scale).max(1.0),
                        w,
                        (1.0 * scale).max(1.0),
                        status.f32_alpha(theme, 0.70),
                        DEPTH_ELEMENT + 0.001,
                        ORDER,
                    );
                }
            }
            let line_num_opts;
            let num_opts_ref = if is_match {
                if let Some(status) = git_status {
                    line_num_opts = DrawOpts {
                        color: status.color(theme),
                        bold: true,
                        ..num_opts
                    };
                    &line_num_opts
                } else {
                    &num_opts
                }
            } else {
                &num_opts
            };
            let num_w =
                sugarloaf
                    .overlay_text_mut()
                    .draw(x, row_y, &num_str, num_opts_ref);
            let mut tx = x + num_w;
            for (tok, slice) in highlight_line(&text, lang) {
                let color = syn_color(tok, theme, !is_match);
                let opts = DrawOpts {
                    font_size: PREVIEW_FONT_SIZE * scale,
                    color,
                    clip_rect: Some(body_clip),
                    ..DrawOpts::default()
                };
                let advance = sugarloaf.overlay_text_mut().draw(tx, row_y, slice, &opts);
                tx += advance;
            }
        }

        sugarloaf
            .overlay_text_mut()
            .draw(x, y, &header_text, &header_opts);
    }
}

fn leaf_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn parent_path(path: &str) -> &str {
    if let Some(idx) = path.rfind('/') {
        &path[..idx]
    } else {
        ""
    }
}

/// Squeeze a long path to a `…/last_two_segments` form so it fits the
/// narrow grep result column.
fn short_path(path: &str) -> String {
    let segs: Vec<&str> = path.split('/').collect();
    if segs.len() <= 2 {
        return path.to_string();
    }
    format!(".../{}/{}", segs[segs.len() - 2], segs[segs.len() - 1])
}
