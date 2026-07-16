use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::primitives::ide_theme::IdeTheme;
pub(super) use crate::primitives::{snap_to_device_px, truncate_to_fit};
use crate::widgets::diff_card::CardSpec;
use crate::widgets::{diff_card, scrollbar};

use super::state::GitDiffPanel;
use super::types::{FocusSection, Rect, VisualRowKind};
use super::{
    branch_glyph, check_glyph, chevron_down_glyph, chevron_right_glyph, close_glyph,
    folder_glyph, folder_open_glyph, BRANCH_MENU_MAX_ROWS, CARD_GAP_TOP, CARD_PAD_X,
    CARD_VGAP, CHECKBOX_SIZE, CLOSE_HIT, COMMIT_BUTTON_HEIGHT, COMMIT_FONT_SIZE,
    COMMIT_INPUT_HEIGHT, COMMIT_INPUT_MAX_LINES, DEPTH, FILES_CARD_MAX_VISIBLE_ROWS,
    FILES_CARD_MIN_VISIBLE_ROWS, FILE_FONT_SIZE, FILE_ROW_HEIGHT, FRAME_RADIUS,
    FRAME_STROKE, HEADER_FONT_SIZE, HEADER_HEIGHT, ORDER_ACCENT, ORDER_FRAME,
    ORDER_INNER, ORDER_LINE_BG, ORDER_MENU_BG, ORDER_MENU_ROW, ORDER_MENU_TEXT,
    ORDER_ROW_BG, ORDER_SCROLL, PADDING_X, SCROLLBAR_HIT_PAD, STATS_FONT_SIZE,
    STATS_HEIGHT, TREE_INDENT,
};

impl GitDiffPanel {
    pub fn render(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        window_w: f32,
        chrome_top: f32,
        bottom_y: f32,
        theme: &IdeTheme,
    ) {
        if !self.visible {
            self.panel_rect = Rect::ZERO;
            self.close_rect = Rect::ZERO;
            self.files_card_rect = Rect::ZERO;
            self.files_body_rect = Rect::ZERO;
            self.diff_card_rect = Rect::ZERO;
            self.file_row_rects.clear();
            self.file_checkbox_rects.clear();
            self.folder_row_rects.clear();
            self.selected_cursor_rect = None;
            self.commit_box_rect = Rect::ZERO;
            self.commit_button_rect = Rect::ZERO;
            self.stage_all_rect = Rect::ZERO;
            self.branch_button_rect = Rect::ZERO;
            self.branch_menu_rect = Rect::ZERO;
            self.branch_filter_rect = Rect::ZERO;
            self.branch_menu_row_rects.clear();
            return;
        }

        let s = self.scale;
        // Use the same width the chrome layout already reserved on
        // the right edge — `effective_width` honours the user-resized
        // `self.width` plus the window-relative cap.
        let target_w = self.effective_width(window_w);
        let height = (bottom_y - chrome_top).max(80.0);
        let open_progress = self.open_progress();
        let panel_x = window_w - target_w * open_progress;
        let panel_y = chrome_top;

        self.panel_rect = Rect {
            x: panel_x,
            y: panel_y,
            w: target_w,
            h: height,
        };

        let frame_stroke = (FRAME_STROKE * s).max(2.0);
        let frame_radius = FRAME_RADIUS * s;
        let inner_radius = (frame_radius - frame_stroke).max(0.0);

        // Frame: surface outer + bg inner — mirrors `file_tree::render`.
        sugarloaf.quad(
            None,
            panel_x,
            panel_y,
            target_w,
            height,
            theme.f32(theme.surface),
            [frame_radius, frame_radius, 0.0, 0.0],
            DEPTH,
            ORDER_FRAME,
        );
        sugarloaf.quad(
            None,
            panel_x + frame_stroke,
            panel_y + frame_stroke,
            (target_w - frame_stroke * 2.0).max(0.0),
            (height - frame_stroke).max(0.0),
            theme.f32(theme.bg),
            [inner_radius, inner_radius, 0.0, 0.0],
            DEPTH,
            ORDER_INNER,
        );

        let content_x = panel_x + frame_stroke;
        let content_y = panel_y + frame_stroke;
        let content_w = (target_w - frame_stroke * 2.0).max(0.0);
        let content_bottom = panel_y + height;
        let inner_x = content_x + PADDING_X * s;

        let mut cursor_y = content_y;

        // ── Top chrome: branch selector + close ──────────────────────
        let header_h = HEADER_HEIGHT * s;
        let header_clip = [content_x, cursor_y, content_w, header_h];
        let icon_opts = DrawOpts {
            font_size: HEADER_FONT_SIZE * s,
            color: theme.u8(theme.dim),
            clip_rect: Some(header_clip),
            ..DrawOpts::default()
        };
        let branch_name_opts = DrawOpts {
            font_size: HEADER_FONT_SIZE * s,
            color: theme.u8(theme.fg),
            bold: true,
            clip_rect: Some(header_clip),
            ..DrawOpts::default()
        };
        let branch_label = self
            .data
            .lock()
            .ok()
            .and_then(|d| d.branch.clone())
            .unwrap_or_default();
        let branch_menu_open = self.branch_menu_open;
        let branch_focused = self.focused && self.section == FocusSection::Branch;

        // Close button (top-right) — drawn first so the branch button's
        // width budget can stop short of it.
        let close_size = CLOSE_HIT * s;
        let close_x = content_x + content_w - close_size - 6.0 * s;
        let close_y = cursor_y + (header_h - close_size) / 2.0;
        self.close_rect = Rect {
            x: close_x,
            y: close_y,
            w: close_size,
            h: close_size,
        };
        sugarloaf.rounded_rect(
            None,
            close_x,
            close_y,
            close_size,
            close_size,
            theme.f32(theme.hover),
            DEPTH,
            5.0 * s,
            ORDER_ROW_BG,
        );
        let close_opts = DrawOpts {
            font_size: 12.0 * s,
            color: theme.u8(theme.muted),
            bold: true,
            clip_rect: Some([close_x, close_y, close_size, close_size]),
            ..DrawOpts::default()
        };
        let cw = sugarloaf.text_mut().measure(close_glyph(), &close_opts);
        sugarloaf.text_mut().draw(
            close_x + (close_size - cw) / 2.0,
            close_y + (close_size - 12.0 * s) / 2.0 - 1.0 * s,
            close_glyph(),
            &close_opts,
        );

        // ── Panel title "Git" (top-left) ─────────────────────────────
        let title_opts = DrawOpts {
            font_size: HEADER_FONT_SIZE * s,
            color: theme.u8(theme.fg),
            bold: true,
            clip_rect: Some(header_clip),
            ..DrawOpts::default()
        };
        let title_y = cursor_y + (header_h - HEADER_FONT_SIZE * s) / 2.0 - 1.0 * s;
        let title_w = sugarloaf
            .text_mut()
            .draw(inner_x, title_y, "Git", &title_opts);
        let title_right = inner_x + title_w;

        // Branch selector button — branch glyph + name + ▾ chevron.
        // Anchored on the RIGHT, immediately left of the close button;
        // clicking it opens the branch dropdown.
        let display_branch = if branch_label.is_empty() {
            "detached".to_string()
        } else {
            branch_label.clone()
        };
        let btn_pad_x = 8.0 * s;
        let icon_w = sugarloaf.text_mut().measure(branch_glyph(), &icon_opts);
        let chevron_w = sugarloaf
            .text_mut()
            .measure(chevron_down_glyph(), &icon_opts);
        // Available span: between the title (+ gap) on the left and the
        // close button (− gap) on the right.
        let btn_right = close_x - 8.0 * s;
        let btn_left_bound = title_right + 12.0 * s;
        let max_btn_w = (btn_right - btn_left_bound).max(0.0);
        let fixed_w = btn_pad_x * 2.0 + icon_w + 6.0 * s + 8.0 * s + chevron_w;
        let name_budget = (max_btn_w - fixed_w).max(0.0);
        let name_fit =
            truncate_to_fit(&display_branch, name_budget, sugarloaf, &branch_name_opts);
        let name_w = sugarloaf
            .text_mut()
            .measure(name_fit.as_str(), &branch_name_opts);
        let btn_w = (btn_pad_x * 2.0 + icon_w + 6.0 * s + name_w + 8.0 * s + chevron_w)
            .min(max_btn_w);
        let btn_h = (header_h - 8.0 * s).max(HEADER_FONT_SIZE * s + 6.0 * s);
        // Right-align so the button's trailing edge hugs the close "x".
        let btn_x = btn_right - btn_w;
        let btn_y = cursor_y + (header_h - btn_h) / 2.0;
        let btn_radius = 6.0 * s;
        if branch_focused {
            let ring = (1.5 * s).max(1.0);
            sugarloaf.rounded_rect(
                None,
                btn_x - ring,
                btn_y - ring,
                btn_w + ring * 2.0,
                btn_h + ring * 2.0,
                theme.f32(theme.accent),
                DEPTH,
                btn_radius + ring,
                ORDER_ROW_BG,
            );
        }
        let btn_bg = if branch_menu_open || branch_focused {
            theme.f32(theme.hover)
        } else {
            theme.f32(theme.surface)
        };
        sugarloaf.rounded_rect(
            None,
            btn_x,
            btn_y,
            btn_w,
            btn_h,
            btn_bg,
            DEPTH,
            btn_radius,
            ORDER_ROW_BG + 1,
        );
        self.branch_button_rect = Rect {
            x: btn_x,
            y: btn_y,
            w: btn_w,
            h: btn_h,
        };
        let btn_text_y = btn_y + (btn_h - HEADER_FONT_SIZE * s) / 2.0 - 1.0 * s;
        let mut bx = btn_x + btn_pad_x;
        bx += sugarloaf
            .text_mut()
            .draw(bx, btn_text_y, branch_glyph(), &icon_opts);
        bx += 6.0 * s;
        bx += sugarloaf.text_mut().draw(
            bx,
            btn_text_y,
            name_fit.as_str(),
            &branch_name_opts,
        );
        bx += 8.0 * s;
        let _ =
            sugarloaf
                .text_mut()
                .draw(bx, btn_text_y, chevron_down_glyph(), &icon_opts);

        cursor_y += header_h;

        // ── Stats row ────────────────────────────────────────────────
        let (loading, error, files, total_add, total_del, current_diff) = {
            let data = match self.data.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            let total_add: u32 = data.files.iter().map(|f| f.additions).sum();
            let total_del: u32 = data.files.iter().map(|f| f.deletions).sum();
            let current_diff = data
                .files
                .get(self.selected)
                .and_then(|f| data.diffs.get(&f.path).cloned());
            (
                data.loading,
                data.error.clone(),
                data.files.clone(),
                total_add,
                total_del,
                current_diff,
            )
        };

        let stats_h = STATS_HEIGHT * s;
        let stats_text_y = cursor_y + (stats_h - STATS_FONT_SIZE * s) / 2.0;
        let stats_clip = [content_x, cursor_y, content_w, stats_h];
        let muted_opts = DrawOpts {
            font_size: STATS_FONT_SIZE * s,
            color: theme.u8(theme.muted),
            clip_rect: Some(stats_clip),
            ..DrawOpts::default()
        };
        let add_opts = DrawOpts {
            font_size: STATS_FONT_SIZE * s,
            color: theme.u8(theme.green),
            bold: true,
            clip_rect: Some(stats_clip),
            ..DrawOpts::default()
        };
        let del_opts = DrawOpts {
            font_size: STATS_FONT_SIZE * s,
            color: theme.u8(theme.red),
            bold: true,
            clip_rect: Some(stats_clip),
            ..DrawOpts::default()
        };
        let files_text = format!(
            "{} {}",
            files.len(),
            if files.len() == 1 { "file" } else { "files" }
        );
        let mut sx = inner_x;
        sx +=
            sugarloaf
                .text_mut()
                .draw(sx, stats_text_y, files_text.as_str(), &muted_opts);
        sx += 10.0 * s;
        let add_text = format!("+{total_add}");
        sx += sugarloaf
            .text_mut()
            .draw(sx, stats_text_y, add_text.as_str(), &add_opts);
        sx += 8.0 * s;
        let del_text = format!("-{total_del}");
        let _ = sugarloaf
            .text_mut()
            .draw(sx, stats_text_y, del_text.as_str(), &del_opts);

        cursor_y += stats_h;

        sugarloaf.rect(
            None,
            content_x,
            cursor_y,
            content_w,
            (1.0 * s).max(1.0),
            theme.f32(theme.border),
            DEPTH,
            ORDER_ACCENT,
        );

        // Empty / error / loading branches.
        let body_top = cursor_y + (1.0 * s).max(1.0) + CARD_GAP_TOP * s;
        let body_h = (content_bottom - frame_stroke - body_top).max(0.0);
        if loading && files.is_empty() {
            let opts = DrawOpts {
                font_size: STATS_FONT_SIZE * s,
                color: theme.u8(theme.muted),
                clip_rect: Some([content_x, body_top, content_w, body_h]),
                ..DrawOpts::default()
            };
            sugarloaf
                .text_mut()
                .draw(inner_x, body_top + 12.0 * s, "Loading…", &opts);
            self.files_card_rect = Rect::ZERO;
            self.files_body_rect = Rect::ZERO;
            self.diff_card_rect = Rect::ZERO;
            self.file_row_rects.clear();
            self.file_checkbox_rects.clear();
            self.folder_row_rects.clear();
            self.selected_cursor_rect = None;
            self.commit_box_rect = Rect::ZERO;
            self.commit_button_rect = Rect::ZERO;
            self.stage_all_rect = Rect::ZERO;
            self.draw_branch_menu(
                sugarloaf,
                s,
                content_x,
                content_bottom,
                frame_stroke,
                theme,
            );
            return;
        }
        if let Some(err) = error.as_ref() {
            let opts = DrawOpts {
                font_size: STATS_FONT_SIZE * s,
                color: theme.u8(theme.red),
                clip_rect: Some([content_x, body_top, content_w, body_h]),
                ..DrawOpts::default()
            };
            sugarloaf
                .text_mut()
                .draw(inner_x, body_top + 12.0 * s, err.as_str(), &opts);
            self.files_card_rect = Rect::ZERO;
            self.files_body_rect = Rect::ZERO;
            self.diff_card_rect = Rect::ZERO;
            self.file_row_rects.clear();
            self.file_checkbox_rects.clear();
            self.folder_row_rects.clear();
            self.selected_cursor_rect = None;
            self.commit_box_rect = Rect::ZERO;
            self.commit_button_rect = Rect::ZERO;
            self.stage_all_rect = Rect::ZERO;
            self.draw_branch_menu(
                sugarloaf,
                s,
                content_x,
                content_bottom,
                frame_stroke,
                theme,
            );
            return;
        }
        if files.is_empty() {
            let opts = DrawOpts {
                font_size: STATS_FONT_SIZE * s,
                color: theme.u8(theme.muted),
                clip_rect: Some([content_x, body_top, content_w, body_h]),
                ..DrawOpts::default()
            };
            sugarloaf
                .text_mut()
                .draw(inner_x, body_top + 12.0 * s, "No changes", &opts);
            self.files_card_rect = Rect::ZERO;
            self.files_body_rect = Rect::ZERO;
            self.diff_card_rect = Rect::ZERO;
            self.file_row_rects.clear();
            self.file_checkbox_rects.clear();
            self.folder_row_rects.clear();
            self.selected_cursor_rect = None;
            self.commit_box_rect = Rect::ZERO;
            self.commit_button_rect = Rect::ZERO;
            self.stage_all_rect = Rect::ZERO;
            self.draw_branch_menu(
                sugarloaf,
                s,
                content_x,
                content_bottom,
                frame_stroke,
                theme,
            );
            return;
        }

        // ── Files card sizing ────────────────────────────────────────
        let card_x = content_x + CARD_PAD_X * s;
        let card_w = (content_w - CARD_PAD_X * 2.0 * s).max(0.0);
        let row_h = FILE_ROW_HEIGHT * s;
        let files_header_h = diff_card::HEADER_HEIGHT * s;
        let max_files_visible = files
            .len()
            .clamp(FILES_CARD_MIN_VISIBLE_ROWS, FILES_CARD_MAX_VISIBLE_ROWS);
        // Files card body: enough rows for `max_files_visible`, plus
        // its header. Diff card gets everything left over.
        let files_body_h = max_files_visible as f32 * row_h
            + (diff_card::BODY_TOP_PAD + diff_card::BODY_BOTTOM_PAD) * s;
        let files_card_h = files_header_h + files_body_h;
        let files_card_y = body_top;
        let diff_card_y = files_card_y + files_card_h + CARD_VGAP * s;
        // Reserve the bottom band for the commit region (branch line +
        // message box + Commit / Stage All). The message box grows with
        // the number of lines in the commit message (Shift+Enter inserts
        // newlines), so the reserved band height is computed from the
        // current message, capped at `COMMIT_INPUT_MAX_LINES`.
        let ca_pad = 8.0 * s;
        let branch_line_h = 16.0 * s;
        let commit_line_h = COMMIT_FONT_SIZE * s * 1.35;
        let commit_lines = self
            .commit_input
            .text()
            .split('\n')
            .count()
            .clamp(1, COMMIT_INPUT_MAX_LINES);
        let commit_box_h = (COMMIT_INPUT_HEIGHT * s)
            .max(commit_lines as f32 * commit_line_h + 2.0 * (6.0 * s));
        let commit_area_h = ca_pad * 2.0
            + branch_line_h
            + 6.0 * s
            + commit_box_h
            + 8.0 * s
            + COMMIT_BUTTON_HEIGHT * s;
        let commit_area_y =
            (content_bottom - frame_stroke - commit_area_h).max(diff_card_y);
        let diff_card_h = (commit_area_y - CARD_VGAP * s - diff_card_y).max(0.0);

        self.files_card_rect = Rect {
            x: card_x,
            y: files_card_y,
            w: card_w,
            h: files_card_h,
        };
        self.diff_card_rect = Rect {
            x: card_x,
            y: diff_card_y,
            w: card_w,
            h: diff_card_h,
        };

        // ── Files card chrome ────────────────────────────────────────
        let card_radius = diff_card::CARD_RADIUS * s;
        let card_stroke = (1.0 * s).max(1.0);
        // Border ring — slightly larger backing in `theme.border`,
        // then header + body fills draw on top, leaving a 1px stroke
        // around the whole card. Same trick `diff_card::render` uses
        // so the two cards read as a matched pair.
        sugarloaf.quad(
            None,
            card_x - card_stroke,
            files_card_y - card_stroke,
            card_w + card_stroke * 2.0,
            files_card_h + card_stroke * 2.0,
            theme.f32(theme.border),
            [
                card_radius + card_stroke,
                card_radius + card_stroke,
                card_radius + card_stroke,
                card_radius + card_stroke,
            ],
            DEPTH,
            ORDER_ROW_BG,
        );
        sugarloaf.quad(
            None,
            card_x,
            files_card_y,
            card_w,
            files_header_h,
            theme.f32(theme.surface),
            [card_radius, card_radius, 0.0, 0.0],
            DEPTH,
            ORDER_ROW_BG + 1,
        );
        sugarloaf.quad(
            None,
            card_x,
            files_card_y + files_header_h,
            card_w,
            files_body_h,
            theme.f32(theme.bg),
            [0.0, 0.0, card_radius, card_radius],
            DEPTH,
            ORDER_ROW_BG + 1,
        );

        let files_header_clip = [card_x, files_card_y, card_w, files_header_h];
        let files_title_opts = DrawOpts {
            font_size: diff_card::HEADER_FONT_SIZE * s,
            color: theme.u8(theme.fg),
            bold: true,
            clip_rect: Some(files_header_clip),
            ..DrawOpts::default()
        };
        let files_subtitle_opts = DrawOpts {
            font_size: diff_card::BADGE_FONT_SIZE * s,
            color: theme.u8(theme.muted),
            bold: true,
            clip_rect: Some(files_header_clip),
            ..DrawOpts::default()
        };
        let files_title_y = files_card_y
            + (files_header_h - diff_card::HEADER_FONT_SIZE * s) / 2.0
            - 1.0 * s;
        let mut hx = card_x + diff_card::HEADER_PAD_X * s;
        hx += sugarloaf
            .text_mut()
            .draw(hx, files_title_y, "Files", &files_title_opts);
        let count_text = format!("  {}", files.len());
        let _ = sugarloaf.text_mut().draw(
            hx,
            files_title_y,
            count_text.as_str(),
            &files_subtitle_opts,
        );

        // ── Files body rows (tree) ───────────────────────────────────
        let files_body_y = files_card_y + files_header_h;
        self.files_body_rect = Rect {
            x: card_x,
            y: files_body_y,
            w: card_w,
            h: files_body_h,
        };
        // Rebuild the flattened tree (folders + file leaves) from the
        // current file list + collapsed-set. Cached on `self` so
        // hit-testing and keyboard navigation see the same order.
        let visual_rows = super::update::build_visual_rows(&files, &self.collapsed_dirs);
        self.visual_rows = visual_rows.clone();
        let total_rows = visual_rows.len();

        let files_body_inner_y = files_body_y + diff_card::BODY_TOP_PAD * s;
        let visible_rows = max_files_visible;
        let max_top = total_rows.saturating_sub(visible_rows);
        let max_scroll = max_top as f32 * row_h;
        if self.file_scroll > max_scroll {
            self.file_scroll = max_scroll;
        }
        let device_scale = sugarloaf.scale_factor();
        let scroll_offset = snap_to_device_px(self.tick_file_scroll(), device_scale);

        self.file_row_rects.clear();
        self.file_checkbox_rects.clear();
        self.folder_row_rects.clear();
        self.selected_cursor_rect = None;

        let row_clip_top = files_body_y;
        let row_clip_bot = files_body_y + files_body_h;
        let indent_px = TREE_INDENT * s;
        let checkbox_focused = self.checkbox_focused;

        let overscan = ((scroll_offset.abs() / row_h).ceil() as usize).saturating_add(1);
        let first_visible = (self.file_scroll / row_h) as usize;
        let start = first_visible.saturating_sub(overscan);
        let end = (first_visible + visible_rows + overscan).min(total_rows);

        // Selected row backing first — resolved to the selected file's
        // position in the tree (folders shift the file's row down).
        let sel_visual = visual_rows.iter().position(|r| {
            matches!(r.kind, VisualRowKind::File { file_index } if file_index == self.selected)
        });
        if let Some(sel_ix) = sel_visual {
            let sel_row_y = files_body_inner_y
                + (sel_ix as f32 * row_h - self.file_scroll)
                + scroll_offset;
            let sel_visible_y = sel_row_y.max(row_clip_top);
            let sel_visible_bot = (sel_row_y + row_h).min(row_clip_bot);
            let sel_visible_h = (sel_visible_bot - sel_visible_y).max(0.0);
            if sel_visible_h > 0.0 {
                sugarloaf.rect(
                    None,
                    card_x,
                    sel_visible_y,
                    card_w,
                    sel_visible_h,
                    theme.f32(theme.hover),
                    DEPTH,
                    ORDER_LINE_BG,
                );
                let stripe_color = if self.focused {
                    theme.f32(theme.accent)
                } else {
                    theme.f32_alpha(theme.accent, 0.45)
                };
                sugarloaf.rect(
                    None,
                    card_x,
                    sel_visible_y,
                    (3.0 * s).max(2.0),
                    sel_visible_h,
                    stripe_color,
                    DEPTH,
                    ORDER_ACCENT,
                );
                // Cursor caret rect — only reported while the selected
                // row's midline sits inside the body so the (unclipped)
                // trail-cursor animation can't phase past the card edges.
                let sel_center = sel_row_y + row_h / 2.0;
                if self.focused
                    && !checkbox_focused
                    && sel_center >= row_clip_top
                    && sel_center <= row_clip_bot
                {
                    let cursor_w = (FILE_FONT_SIZE * s * 0.55).max(2.0);
                    let cursor_h = (row_h - 6.0 * s).max(FILE_FONT_SIZE * s).min(row_h);
                    let cursor_y = (sel_row_y + (row_h - cursor_h) / 2.0)
                        .clamp(row_clip_top, (row_clip_bot - cursor_h).max(row_clip_top));
                    let cursor_x = card_x + (3.0 * s).max(2.0) + 2.0 * s;
                    self.selected_cursor_rect =
                        Some([cursor_x, cursor_y, cursor_w, cursor_h]);
                }
            }
        }

        let checkbox_size = CHECKBOX_SIZE * s;
        let checkbox_x = card_x + card_w - diff_card::HEADER_PAD_X * s - checkbox_size;
        // Left edge of the stats/checkbox zone — text budgets stop here.
        let stats_right_edge = checkbox_x - 8.0 * s;

        for absolute_ix in start..end {
            // `.get()` rather than a raw index: `start..end` is derived
            // from spring-scroll math that can momentarily run a row past
            // the list on an overshoot, and an out-of-bounds index would
            // panic the whole app instead of just skipping a phantom row.
            let Some(vr) = visual_rows.get(absolute_ix) else {
                continue;
            };
            let row_y = files_body_inner_y
                + (absolute_ix as f32 * row_h - self.file_scroll)
                + scroll_offset;
            let row_bot = row_y + row_h;
            if row_bot < row_clip_top || row_y > row_clip_bot {
                continue;
            }
            let visible_y = row_y.max(row_clip_top);
            let visible_h = row_bot.min(row_clip_bot) - visible_y;
            // SCROLL-CLIP FIX: a row whose visible sliver is sub-pixel
            // would snap to a zero-height clip rect — and sugarloaf's
            // text shader *disables* clipping when the clip has zero
            // width/height, letting the file name bleed past the card
            // top/bottom during spring-scroll overshoot. Treat such
            // rows as fully off-screen and skip drawing their text.
            if visible_h * device_scale < 1.0 {
                continue;
            }
            let row_clip = [card_x, visible_y, card_w, visible_h];
            let base_x =
                card_x + diff_card::HEADER_PAD_X * s + vr.depth as f32 * indent_px;
            let text_y = row_y + (row_h - FILE_FONT_SIZE * s) / 2.0;
            let icon_size = FILE_FONT_SIZE * s;
            let icon_y = row_y + (row_h - icon_size) / 2.0;

            match &vr.kind {
                VisualRowKind::Dir { path, collapsed } => {
                    self.folder_row_rects.push((
                        absolute_ix,
                        Rect {
                            x: card_x,
                            y: visible_y,
                            w: card_w,
                            h: visible_h,
                        },
                    ));
                    let chevron = if *collapsed {
                        chevron_right_glyph()
                    } else {
                        chevron_down_glyph()
                    };
                    let folder_glyph = if *collapsed {
                        folder_glyph()
                    } else {
                        folder_open_glyph()
                    };
                    let chevron_opts = DrawOpts {
                        font_size: FILE_FONT_SIZE * s * 0.85,
                        color: theme.u8(theme.muted),
                        clip_rect: Some(row_clip),
                        ..DrawOpts::default()
                    };
                    let folder_opts = DrawOpts {
                        font_size: icon_size,
                        color: theme.u8(theme.folder),
                        clip_rect: Some(row_clip),
                        ..DrawOpts::default()
                    };
                    let dir_name_opts = DrawOpts {
                        font_size: FILE_FONT_SIZE * s,
                        color: theme.u8(theme.fg),
                        clip_rect: Some(row_clip),
                        ..DrawOpts::default()
                    };
                    let mut cx = base_x;
                    let _ = sugarloaf
                        .text_mut()
                        .draw(cx, text_y, chevron, &chevron_opts);
                    cx += indent_px;
                    let _ =
                        sugarloaf
                            .text_mut()
                            .draw(cx, icon_y, folder_glyph, &folder_opts);
                    cx += icon_size + 6.0 * s;
                    let dir_name = path.rsplit('/').next().unwrap_or(path.as_str());
                    let name_budget = (stats_right_edge - cx).max(0.0);
                    let dir_fit =
                        truncate_to_fit(dir_name, name_budget, sugarloaf, &dir_name_opts);
                    let _ = sugarloaf.text_mut().draw(
                        cx,
                        text_y,
                        dir_fit.as_str(),
                        &dir_name_opts,
                    );
                }
                VisualRowKind::File { file_index } => {
                    let file_index = *file_index;
                    // Defensive `.get()`: the visual rows are rebuilt from
                    // `files` so the index should always be valid, but a
                    // raw index would turn any future desync into a panic.
                    let Some(f) = files.get(file_index) else {
                        continue;
                    };
                    self.file_row_rects.push((
                        file_index,
                        Rect {
                            x: card_x,
                            y: visible_y,
                            w: card_w,
                            h: visible_h,
                        },
                    ));

                    // ── Stage checkbox (right edge) ──────────────────
                    let checkbox_y = row_y + (row_h - checkbox_size) / 2.0;
                    let checkbox_visible = checkbox_y >= row_clip_top
                        && checkbox_y + checkbox_size <= row_clip_bot;
                    let is_selected = file_index == self.selected;
                    if checkbox_visible {
                        let cb_radius = 4.0 * s;
                        // Focus ring when Alt+Right parked focus on the
                        // checkbox column and this is the selected row.
                        if checkbox_focused && is_selected {
                            let ring = (1.5 * s).max(1.0);
                            sugarloaf.rounded_rect(
                                None,
                                checkbox_x - ring,
                                checkbox_y - ring,
                                checkbox_size + ring * 2.0,
                                checkbox_size + ring * 2.0,
                                theme.f32(theme.accent),
                                DEPTH,
                                cb_radius + ring,
                                ORDER_SCROLL,
                            );
                        }
                        if f.staged {
                            sugarloaf.rounded_rect(
                                None,
                                checkbox_x,
                                checkbox_y,
                                checkbox_size,
                                checkbox_size,
                                theme.f32(theme.accent),
                                DEPTH,
                                cb_radius,
                                ORDER_SCROLL,
                            );
                            let check_fs = (checkbox_size * 0.72).max(8.0);
                            let check_opts = DrawOpts {
                                font_size: check_fs,
                                color: theme.u8(theme.bg),
                                bold: true,
                                clip_rect: Some([
                                    checkbox_x,
                                    checkbox_y,
                                    checkbox_size,
                                    checkbox_size,
                                ]),
                                ..DrawOpts::default()
                            };
                            let cw =
                                sugarloaf.text_mut().measure(check_glyph(), &check_opts);
                            sugarloaf.text_mut().draw(
                                checkbox_x + (checkbox_size - cw) / 2.0,
                                checkbox_y + (checkbox_size - check_fs) / 2.0,
                                check_glyph(),
                                &check_opts,
                            );
                        } else {
                            sugarloaf.rounded_rect(
                                None,
                                checkbox_x,
                                checkbox_y,
                                checkbox_size,
                                checkbox_size,
                                theme.f32(theme.muted),
                                DEPTH,
                                cb_radius,
                                ORDER_SCROLL,
                            );
                            let inset = (1.5 * s).max(1.0);
                            sugarloaf.rounded_rect(
                                None,
                                checkbox_x + inset,
                                checkbox_y + inset,
                                (checkbox_size - inset * 2.0).max(0.0),
                                (checkbox_size - inset * 2.0).max(0.0),
                                theme.f32(theme.bg),
                                DEPTH,
                                (cb_radius - inset).max(0.0),
                                ORDER_SCROLL,
                            );
                        }
                        self.file_checkbox_rects.push((
                            file_index,
                            Rect {
                                x: checkbox_x,
                                y: checkbox_y,
                                w: checkbox_size,
                                h: checkbox_size,
                            },
                        ));
                    }

                    let label_color = if is_selected {
                        theme.u8(theme.fg)
                    } else {
                        theme.u8(theme.dim)
                    };
                    let marker_opts = DrawOpts {
                        font_size: FILE_FONT_SIZE * s,
                        color: f.status.color(theme),
                        bold: true,
                        clip_rect: Some(row_clip),
                        ..DrawOpts::default()
                    };
                    let name_opts = DrawOpts {
                        font_size: FILE_FONT_SIZE * s,
                        color: label_color,
                        clip_rect: Some(row_clip),
                        ..DrawOpts::default()
                    };
                    let row_add_opts = DrawOpts {
                        font_size: STATS_FONT_SIZE * s,
                        color: theme.u8(theme.green),
                        clip_rect: Some(row_clip),
                        ..DrawOpts::default()
                    };
                    let row_del_opts = DrawOpts {
                        font_size: STATS_FONT_SIZE * s,
                        color: theme.u8(theme.red),
                        clip_rect: Some(row_clip),
                        ..DrawOpts::default()
                    };

                    let mut tx = base_x;
                    tx += sugarloaf.text_mut().draw(
                        tx,
                        text_y,
                        f.status.marker(),
                        &marker_opts,
                    );
                    tx += 8.0 * s;

                    let (filename, _dir) = split_path(&f.path);
                    let add_str = if f.additions > 0 {
                        format!("+{}", f.additions)
                    } else {
                        String::new()
                    };
                    let del_str = if f.deletions > 0 {
                        format!("-{}", f.deletions)
                    } else {
                        String::new()
                    };
                    let add_w = if add_str.is_empty() {
                        0.0
                    } else {
                        sugarloaf
                            .text_mut()
                            .measure(add_str.as_str(), &row_add_opts)
                    };
                    let del_w = if del_str.is_empty() {
                        0.0
                    } else {
                        sugarloaf
                            .text_mut()
                            .measure(del_str.as_str(), &row_del_opts)
                    };
                    let stats_total = add_w
                        + del_w
                        + if !add_str.is_empty() && !del_str.is_empty() {
                            6.0 * s
                        } else {
                            0.0
                        };
                    let name_budget =
                        (stats_right_edge - tx - stats_total - 8.0 * s).max(0.0);
                    let name_fit =
                        truncate_to_fit(filename, name_budget, sugarloaf, &name_opts);
                    let _ = sugarloaf.text_mut().draw(
                        tx,
                        text_y,
                        name_fit.as_str(),
                        &name_opts,
                    );

                    let stats_text_y = row_y + (row_h - STATS_FONT_SIZE * s) / 2.0;
                    let mut rx = stats_right_edge;
                    if !del_str.is_empty() {
                        rx -= del_w;
                        sugarloaf.text_mut().draw(
                            rx,
                            stats_text_y,
                            del_str.as_str(),
                            &row_del_opts,
                        );
                    }
                    if !add_str.is_empty() {
                        if !del_str.is_empty() {
                            rx -= 6.0 * s;
                        }
                        rx -= add_w;
                        sugarloaf.text_mut().draw(
                            rx,
                            stats_text_y,
                            add_str.as_str(),
                            &row_add_opts,
                        );
                    }
                }
            }
        }

        // Files card scrollbar — record the thumb rect so the screen
        // layer's mouse-down handler can grab and drag it.
        self.files_scrollbar_thumb_rect = Rect::ZERO;
        if total_rows > visible_rows {
            let progress = if max_scroll > 0.0 {
                self.file_scroll / max_scroll
            } else {
                0.0
            };
            if let Some((thumb_y, thumb_h)) = scrollbar::compute_thumb(
                visible_rows,
                total_rows,
                files_body_y,
                files_body_h,
                progress,
            ) {
                let thumb_x = card_x + card_w - scrollbar::width() - 2.0 * s;
                scrollbar::draw_track(
                    sugarloaf,
                    thumb_x,
                    files_body_y,
                    files_body_h,
                    0.95,
                    DEPTH,
                    ORDER_SCROLL,
                );
                scrollbar::draw_thumb(
                    sugarloaf,
                    thumb_x,
                    thumb_y,
                    thumb_h,
                    0.95,
                    false,
                    DEPTH,
                    ORDER_SCROLL,
                );
                self.files_scrollbar_thumb_rect = Rect {
                    x: thumb_x,
                    y: thumb_y,
                    w: scrollbar::width(),
                    h: thumb_h,
                };
            }
        }

        // ── Diff card ────────────────────────────────────────────────
        let selected_file = files.get(self.selected);
        if let Some(file) = selected_file {
            let lines = current_diff.unwrap_or_default();
            let lang = crate::syntax::Lang::from_path(&file.path);
            let body_capacity_h = (diff_card_h - diff_card::HEADER_HEIGHT * s).max(0.0);
            // Spring-damped scroll for the diff body. Clamp to the
            // last-line viewport so over-scroll can't flick the diff
            // off the bottom of the card.
            let line_h = diff_card::LINE_HEIGHT * s;
            let visual_row_offsets = diff_card::warm_render_cache(
                &lines,
                diff_card::body_text_width(card_w, s),
                s,
                lang,
            );
            let visual_line_count =
                visual_row_offsets.last().copied().unwrap_or(0).max(1);
            let max_diff_top = visual_line_count
                .saturating_sub(((body_capacity_h / line_h).floor() as usize).max(1))
                as f32
                * line_h;
            if self.diff_scroll > max_diff_top {
                self.diff_scroll = max_diff_top;
            }
            // Spring lag: `tick_diff_scroll` returns the position the
            // spring has yet to absorb. Right after a scroll it equals
            // the just-applied delta and decays back toward 0. To make
            // the body visually start at the old scroll and slide to
            // the new one, subtract the lag from the integer target.
            let diff_scroll_offset =
                snap_to_device_px(self.tick_diff_scroll(), sugarloaf.scale_factor());
            let effective_scroll = (self.diff_scroll - diff_scroll_offset).max(0.0);

            // Focus ring for the Diff section (Alt+Down lands here so
            // ↑/↓ scroll the changes). Drawn just under the card's own
            // border ring (larger accent backing, painted first so the
            // card's border + body cover its interior and only the
            // protruding accent edge shows) — same trick as the branch
            // button / checkbox focus rings.
            if self.focused && self.section == FocusSection::Diff {
                let card_radius = diff_card::CARD_RADIUS * s;
                let card_stroke = (2.0 * s).max(2.0);
                let focus = (1.5 * s).max(1.0);
                let off = card_stroke + focus;
                sugarloaf.rounded_rect(
                    None,
                    card_x - off,
                    diff_card_y - off,
                    card_w + off * 2.0,
                    diff_card_h + off * 2.0,
                    theme.f32(theme.accent),
                    DEPTH,
                    card_radius + off,
                    ORDER_ROW_BG,
                );
            }

            let spec = CardSpec {
                path: file.path.as_str(),
                link_target: None,
                link_hovered: false,
                additions: file.additions,
                deletions: file.deletions,
                lang,
                diff_lines: lines.as_slice(),
                visual_row_offsets: Some(visual_row_offsets.as_slice()),
                body_scroll: effective_scroll,
            };
            let _ = diff_card::render(
                sugarloaf,
                card_x,
                diff_card_y,
                card_w,
                body_capacity_h,
                &spec,
                s,
                theme,
                DEPTH,
                ORDER_ROW_BG,
                diff_card_y,
                diff_card_y + diff_card_h,
            );

            // Diff card scrollbar — same record-rect-for-drag pattern.
            self.diff_scrollbar_thumb_rect = Rect::ZERO;
            if max_diff_top > 0.0 {
                let progress = (self.diff_scroll / max_diff_top).clamp(0.0, 1.0);
                let visible_count = ((body_capacity_h / line_h).floor() as usize).max(1);
                if let Some((thumb_y, thumb_h)) = scrollbar::compute_thumb(
                    visible_count,
                    visual_line_count,
                    diff_card_y + diff_card::HEADER_HEIGHT * s,
                    body_capacity_h,
                    progress,
                ) {
                    let thumb_x = card_x + card_w - scrollbar::width() - 2.0 * s;
                    scrollbar::draw_track(
                        sugarloaf,
                        thumb_x,
                        diff_card_y + diff_card::HEADER_HEIGHT * s,
                        body_capacity_h,
                        0.95,
                        DEPTH,
                        ORDER_SCROLL,
                    );
                    scrollbar::draw_thumb(
                        sugarloaf,
                        thumb_x,
                        thumb_y,
                        thumb_h,
                        0.95,
                        false,
                        DEPTH,
                        ORDER_SCROLL,
                    );
                    self.diff_scrollbar_thumb_rect = Rect {
                        x: thumb_x,
                        y: thumb_y,
                        w: scrollbar::width(),
                        h: thumb_h,
                    };
                }
            }
        } else {
            self.diff_scrollbar_thumb_rect = Rect::ZERO;
        }

        // ── Commit region (bottom band) ──────────────────────────────
        let commit_focused = self.commit_focused;
        let msg = self.commit_input.text().to_string();

        // Top divider.
        sugarloaf.rect(
            None,
            content_x,
            commit_area_y,
            content_w,
            (1.0 * s).max(1.0),
            theme.f32(theme.border),
            DEPTH,
            ORDER_ACCENT,
        );

        let mut cy = commit_area_y + ca_pad;

        // Branch line — "on <branch>" summary under the divider (the
        // switchable selector lives in the header).
        let branch_clip = [content_x, cy, content_w, branch_line_h];
        let branch_icon_opts = DrawOpts {
            font_size: STATS_FONT_SIZE * s,
            color: theme.u8(theme.dim),
            clip_rect: Some(branch_clip),
            ..DrawOpts::default()
        };
        let branch_text_opts = DrawOpts {
            font_size: STATS_FONT_SIZE * s,
            color: theme.u8(theme.muted),
            clip_rect: Some(branch_clip),
            ..DrawOpts::default()
        };
        let branch_text_y = cy + (branch_line_h - STATS_FONT_SIZE * s) / 2.0;
        let mut bx = inner_x;
        bx += sugarloaf.text_mut().draw(
            bx,
            branch_text_y,
            branch_glyph(),
            &branch_icon_opts,
        );
        bx += 6.0 * s;
        let branch_line = if branch_label.is_empty() {
            "detached".to_string()
        } else {
            format!("on {branch_label}")
        };
        let branch_budget = (content_x + content_w - bx - PADDING_X * s).max(0.0);
        let branch_fit =
            truncate_to_fit(&branch_line, branch_budget, sugarloaf, &branch_text_opts);
        let _ = sugarloaf.text_mut().draw(
            bx,
            branch_text_y,
            branch_fit.as_str(),
            &branch_text_opts,
        );
        cy += branch_line_h + 6.0 * s;

        // Commit-message input box (grows with line count).
        let box_x = inner_x;
        let box_w = (content_x + content_w - inner_x - PADDING_X * s).max(0.0);
        let box_h = commit_box_h;
        let box_y = cy;
        let box_radius = 6.0 * s;
        let box_stroke = (1.0 * s).max(1.0);
        let box_border = if commit_focused {
            theme.f32(theme.accent)
        } else {
            theme.f32(theme.border)
        };
        sugarloaf.rounded_rect(
            None,
            box_x - box_stroke,
            box_y - box_stroke,
            box_w + box_stroke * 2.0,
            box_h + box_stroke * 2.0,
            box_border,
            DEPTH,
            box_radius + box_stroke,
            ORDER_ROW_BG,
        );
        sugarloaf.rounded_rect(
            None,
            box_x,
            box_y,
            box_w,
            box_h,
            theme.f32(theme.surface),
            DEPTH,
            box_radius,
            ORDER_ROW_BG + 1,
        );
        self.commit_box_rect = Rect {
            x: box_x,
            y: box_y,
            w: box_w,
            h: box_h,
        };

        let text_pad = 8.0 * s;
        let text_x = box_x + text_pad;
        let box_clip = [box_x, box_y, box_w, box_h];
        let line_top = box_y + 6.0 * s;
        let caret_w = (1.5 * s).max(1.0);
        if msg.is_empty() {
            let ph_opts = DrawOpts {
                font_size: COMMIT_FONT_SIZE * s,
                color: theme.u8(theme.muted),
                clip_rect: Some(box_clip),
                ..DrawOpts::default()
            };
            let ph_y = box_y + (box_h - COMMIT_FONT_SIZE * s) / 2.0;
            let _ = sugarloaf.text_mut().draw(
                text_x,
                ph_y,
                "Commit message… (Shift+Enter for newline)",
                &ph_opts,
            );
            if commit_focused {
                let caret_h = (COMMIT_FONT_SIZE * s + 2.0 * s).min(box_h - 8.0 * s);
                sugarloaf.rect(
                    None,
                    text_x,
                    box_y + (box_h - caret_h) / 2.0,
                    caret_w,
                    caret_h,
                    theme.f32(theme.accent),
                    DEPTH,
                    ORDER_SCROLL,
                );
            }
        } else {
            // Multi-line: one draw per '\n'-separated line so Shift+Enter
            // newlines display stacked instead of running off the right.
            let msg_opts = DrawOpts {
                font_size: COMMIT_FONT_SIZE * s,
                color: theme.u8(theme.fg),
                clip_rect: Some(box_clip),
                ..DrawOpts::default()
            };
            let lines: Vec<&str> = msg.split('\n').collect();
            for (i, line) in lines.iter().enumerate() {
                let ly = line_top + i as f32 * commit_line_h;
                if ly > box_y + box_h {
                    break;
                }
                let _ = sugarloaf.text_mut().draw(text_x, ly, line, &msg_opts);
            }
            if commit_focused {
                // Caret at end of the last line (the commit box types at
                // the tail; no in-box cursor motion yet).
                let last = lines.last().copied().unwrap_or("");
                let last_w = sugarloaf.text_mut().measure(last, &msg_opts);
                let caret_line = lines.len().saturating_sub(1);
                let caret_x = (text_x + last_w).min(box_x + box_w - text_pad).max(text_x);
                let caret_y = line_top + caret_line as f32 * commit_line_h;
                let caret_h = (COMMIT_FONT_SIZE * s + 2.0 * s).min(box_h);
                if caret_y + caret_h <= box_y + box_h + 1.0 * s {
                    sugarloaf.rect(
                        None,
                        caret_x,
                        caret_y,
                        caret_w,
                        caret_h,
                        theme.f32(theme.accent),
                        DEPTH,
                        ORDER_SCROLL,
                    );
                }
            }
        }
        cy += box_h + 8.0 * s;

        // Commit / Stage All buttons.
        let btn_h = COMMIT_BUTTON_HEIGHT * s;
        let btn_y = cy;
        let btn_pad_x = 12.0 * s;
        let btn_gap = 8.0 * s;
        let btn_radius = 6.0 * s;
        let btn_stroke = (1.0 * s).max(1.0);
        let btn_text_y = btn_y + (btn_h - COMMIT_FONT_SIZE * s) / 2.0;

        // Commit — filled accent button.
        let commit_label = "Commit";
        let commit_label_opts = DrawOpts {
            font_size: COMMIT_FONT_SIZE * s,
            color: theme.u8(theme.bg),
            bold: true,
            ..DrawOpts::default()
        };
        let commit_text_w = sugarloaf
            .text_mut()
            .measure(commit_label, &commit_label_opts);
        let commit_btn_w = commit_text_w + btn_pad_x * 2.0;
        let commit_btn_x = inner_x;
        sugarloaf.rounded_rect(
            None,
            commit_btn_x,
            btn_y,
            commit_btn_w,
            btn_h,
            theme.f32(theme.accent),
            DEPTH,
            btn_radius,
            ORDER_ROW_BG + 1,
        );
        let _ = sugarloaf.text_mut().draw(
            commit_btn_x + btn_pad_x,
            btn_text_y,
            commit_label,
            &DrawOpts {
                clip_rect: Some([commit_btn_x, btn_y, commit_btn_w, btn_h]),
                ..commit_label_opts
            },
        );
        self.commit_button_rect = Rect {
            x: commit_btn_x,
            y: btn_y,
            w: commit_btn_w,
            h: btn_h,
        };

        // Stage All / Unstage All — outline button. Reversible toggle:
        // when every file is already staged it reads "Unstage All" and
        // its click unstages; otherwise it reads "Stage All" and stages
        // the unstaged files. Computed from the cloned `files` snapshot
        // so the label matches the same state the click handler sees.
        let all_staged = !files.is_empty() && files.iter().all(|f| f.staged);
        let stage_label = if all_staged {
            "Unstage All"
        } else {
            "Stage All"
        };
        let stage_label_opts = DrawOpts {
            font_size: COMMIT_FONT_SIZE * s,
            color: theme.u8(theme.dim),
            bold: true,
            ..DrawOpts::default()
        };
        let stage_text_w = sugarloaf.text_mut().measure(stage_label, &stage_label_opts);
        let stage_btn_w = stage_text_w + btn_pad_x * 2.0;
        let stage_btn_x = commit_btn_x + commit_btn_w + btn_gap;
        sugarloaf.rounded_rect(
            None,
            stage_btn_x - btn_stroke,
            btn_y - btn_stroke,
            stage_btn_w + btn_stroke * 2.0,
            btn_h + btn_stroke * 2.0,
            theme.f32(theme.border),
            DEPTH,
            btn_radius + btn_stroke,
            ORDER_ROW_BG,
        );
        sugarloaf.rounded_rect(
            None,
            stage_btn_x,
            btn_y,
            stage_btn_w,
            btn_h,
            theme.f32(theme.surface),
            DEPTH,
            btn_radius,
            ORDER_ROW_BG + 1,
        );
        let _ = sugarloaf.text_mut().draw(
            stage_btn_x + btn_pad_x,
            btn_text_y,
            stage_label,
            &DrawOpts {
                clip_rect: Some([stage_btn_x, btn_y, stage_btn_w, btn_h]),
                ..stage_label_opts
            },
        );
        self.stage_all_rect = Rect {
            x: stage_btn_x,
            y: btn_y,
            w: stage_btn_w,
            h: btn_h,
        };

        // Branch dropdown overlays everything else when open.
        self.draw_branch_menu(
            sugarloaf,
            s,
            content_x,
            content_bottom,
            frame_stroke,
            theme,
        );
    }

    /// Draw the branch-selector dropdown (search box + branch rows) when
    /// it's open. Records the menu / filter / row hit-rects. Overlays the
    /// cards at the highest render order in the panel.
    fn draw_branch_menu(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        s: f32,
        content_x: f32,
        content_bottom: f32,
        frame_stroke: f32,
        theme: &IdeTheme,
    ) {
        if !self.branch_menu_open {
            self.branch_menu_rect = Rect::ZERO;
            self.branch_filter_rect = Rect::ZERO;
            self.branch_menu_row_rects.clear();
            return;
        }

        let btn = self.branch_button_rect;
        let filter_text = self.branch_filter.text().to_string();
        let current_branch = self
            .data
            .lock()
            .ok()
            .and_then(|d| d.branch.clone())
            .unwrap_or_default();
        let branches = self.filtered_branches();
        let selected = self
            .branch_menu_selected
            .min(branches.len().saturating_sub(1));

        let gap = 4.0 * s;
        // The branch button hugs the panel's right edge, so the dropdown
        // right-aligns under it and opens leftward — clamped so it never
        // spills past the panel's left content edge.
        let min_left = content_x + 4.0 * s;
        let menu_right = btn.x + btn.w;
        let desired_w = btn.w.max(240.0 * s);
        let menu_w = desired_w.min((menu_right - min_left).max(0.0));
        let menu_x = (menu_right - menu_w).max(min_left);
        let menu_y = btn.y + btn.h + gap;
        let pad = 6.0 * s;
        let search_h = 26.0 * s;
        let row_h = 22.0 * s;
        let shown = branches.len().min(BRANCH_MENU_MAX_ROWS);
        let list_h = shown as f32 * row_h;
        let wanted_h = pad + search_h + 6.0 * s + list_h + pad;
        let menu_h = wanted_h.min((content_bottom - frame_stroke - menu_y).max(0.0));
        let menu_bot = menu_y + menu_h;

        // Border + fill. Drawn in the OVERLAY pass (after normal UI
        // text) so the panel's stats row / "Files" header / file-row
        // text below can't bleed through the popover — a normal-pass
        // rect always loses to normal-pass text no matter its `order`.
        let radius = 8.0 * s;
        let stroke = (1.0 * s).max(1.0);
        sugarloaf.overlay_quad(
            menu_x - stroke,
            menu_y - stroke,
            menu_w + stroke * 2.0,
            menu_h + stroke * 2.0,
            theme.f32(theme.border),
            [
                radius + stroke,
                radius + stroke,
                radius + stroke,
                radius + stroke,
            ],
            DEPTH,
            ORDER_MENU_BG,
        );
        sugarloaf.overlay_quad(
            menu_x,
            menu_y,
            menu_w,
            menu_h,
            theme.f32(theme.surface),
            [radius, radius, radius, radius],
            DEPTH,
            ORDER_MENU_BG,
        );
        self.branch_menu_rect = Rect {
            x: menu_x,
            y: menu_y,
            w: menu_w,
            h: menu_h,
        };

        // Search box.
        let search_x = menu_x + pad;
        let search_w = (menu_w - pad * 2.0).max(0.0);
        let search_y = menu_y + pad;
        let search_radius = 5.0 * s;
        sugarloaf.overlay_rounded_rect(
            search_x - stroke,
            search_y - stroke,
            search_w + stroke * 2.0,
            search_h + stroke * 2.0,
            theme.f32(theme.accent),
            DEPTH,
            search_radius + stroke,
            ORDER_MENU_ROW,
        );
        sugarloaf.overlay_rounded_rect(
            search_x,
            search_y,
            search_w,
            search_h,
            theme.f32(theme.bg),
            DEPTH,
            search_radius,
            ORDER_MENU_ROW,
        );
        self.branch_filter_rect = Rect {
            x: search_x,
            y: search_y,
            w: search_w,
            h: search_h,
        };
        let search_clip = [search_x, search_y, search_w, search_h];
        let search_text_pad = 8.0 * s;
        let search_text_x = search_x + search_text_pad;
        let search_text_y = search_y + (search_h - STATS_FONT_SIZE * s) / 2.0;
        if filter_text.is_empty() {
            let ph = DrawOpts {
                font_size: STATS_FONT_SIZE * s,
                color: theme.u8(theme.muted),
                clip_rect: Some(search_clip),
                ..DrawOpts::default()
            };
            let _ = sugarloaf.overlay_text_mut().draw(
                search_text_x,
                search_text_y,
                "Search branches…",
                &ph,
            );
        } else {
            let so = DrawOpts {
                font_size: STATS_FONT_SIZE * s,
                color: theme.u8(theme.fg),
                clip_rect: Some(search_clip),
                ..DrawOpts::default()
            };
            let _ = sugarloaf.overlay_text_mut().draw(
                search_text_x,
                search_text_y,
                filter_text.as_str(),
                &so,
            );
        }
        // Search caret.
        let caret_x = {
            let so = DrawOpts {
                font_size: STATS_FONT_SIZE * s,
                color: theme.u8(theme.fg),
                ..DrawOpts::default()
            };
            let w = sugarloaf
                .overlay_text_mut()
                .measure(filter_text.as_str(), &so);
            (search_text_x + w).min(search_x + search_w - search_text_pad)
        };
        let caret_h = (search_h - 8.0 * s).max(STATS_FONT_SIZE * s);
        sugarloaf.overlay_rect(
            caret_x,
            search_y + (search_h - caret_h) / 2.0,
            (1.5 * s).max(1.0),
            caret_h,
            theme.f32(theme.accent),
            DEPTH,
            ORDER_MENU_TEXT,
        );

        // Branch rows.
        self.branch_menu_row_rects.clear();
        let rows_top = search_y + search_h + 6.0 * s;
        for (i, b) in branches.iter().take(shown).enumerate() {
            let ry = rows_top + i as f32 * row_h;
            if ry + row_h > menu_bot {
                break;
            }
            let row_clip = [menu_x, ry, menu_w, row_h];
            let is_hl = i == selected;
            let is_current = *b == current_branch;
            if is_hl {
                sugarloaf.overlay_rounded_rect(
                    menu_x + 3.0 * s,
                    ry,
                    (menu_w - 6.0 * s).max(0.0),
                    row_h,
                    theme.f32(theme.hover),
                    DEPTH,
                    4.0 * s,
                    ORDER_MENU_ROW,
                );
            }
            let row_text_y = ry + (row_h - STATS_FONT_SIZE * s) / 2.0;
            let name_color = if is_current {
                theme.u8(theme.accent)
            } else if is_hl {
                theme.u8(theme.fg)
            } else {
                theme.u8(theme.dim)
            };
            let name_opts = DrawOpts {
                font_size: STATS_FONT_SIZE * s,
                color: name_color,
                bold: is_current,
                clip_rect: Some(row_clip),
                ..DrawOpts::default()
            };
            // Leading tick for the current branch, plain indent otherwise.
            let mut rx = menu_x + pad + 4.0 * s;
            if is_current {
                let tick_opts = DrawOpts {
                    font_size: STATS_FONT_SIZE * s,
                    color: theme.u8(theme.accent),
                    bold: true,
                    clip_rect: Some(row_clip),
                    ..DrawOpts::default()
                };
                rx += sugarloaf.overlay_text_mut().draw(
                    rx,
                    row_text_y,
                    check_glyph(),
                    &tick_opts,
                );
                rx += 6.0 * s;
            } else {
                rx += sugarloaf
                    .overlay_text_mut()
                    .measure(check_glyph(), &name_opts)
                    + 6.0 * s;
            }
            let budget = (menu_x + menu_w - pad - rx).max(0.0);
            let fit = truncate_to_fit(b, budget, sugarloaf, &name_opts);
            let _ = sugarloaf.overlay_text_mut().draw(
                rx,
                row_text_y,
                fit.as_str(),
                &name_opts,
            );
            self.branch_menu_row_rects.push((
                b.clone(),
                Rect {
                    x: menu_x,
                    y: ry,
                    w: menu_w,
                    h: row_h,
                },
            ));
        }
        if branches.is_empty() {
            let empty_opts = DrawOpts {
                font_size: STATS_FONT_SIZE * s,
                color: theme.u8(theme.muted),
                clip_rect: Some([menu_x, rows_top, menu_w, row_h]),
                ..DrawOpts::default()
            };
            let _ = sugarloaf.overlay_text_mut().draw(
                menu_x + pad + 4.0 * s,
                rows_top + (row_h - STATS_FONT_SIZE * s) / 2.0,
                "No branches",
                &empty_opts,
            );
        }
    }
}

pub(super) fn hit_scrollbar_thumb(rect: &Rect, mx: f32, my: f32) -> bool {
    if rect.w <= 0.0 || rect.h <= 0.0 {
        return false;
    }
    // Pad the hit area horizontally so the user doesn't need
    // sub-pixel mouse precision to grab the thin scrollbar.
    mx >= rect.x - SCROLLBAR_HIT_PAD
        && mx <= rect.x + rect.w + SCROLLBAR_HIT_PAD
        && my >= rect.y
        && my <= rect.y + rect.h
}

pub(super) fn split_path(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(i) => (&path[i + 1..], &path[..i + 1]),
        None => (path, ""),
    }
}
