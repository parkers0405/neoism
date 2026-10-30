use neoism_paint::text::DrawOpts;
use crate::primitives::paint::Paint;

use crate::primitives::ide_theme::IdeTheme;
pub(super) use crate::primitives::{snap_to_device_px, truncate_to_fit};
use crate::widgets::{diff_card, scrollbar};

use super::state::GitDiffPanel;
use super::types::{Rect, VisualRowKind};
use super::{
    branch_glyph, check_glyph, chevron_down_glyph, chevron_right_glyph, close_glyph,
    BRANCH_MENU_MAX_ROWS, CARD_GAP_TOP, CARD_PAD_X, CARD_VGAP, CHECKBOX_SIZE, CLOSE_HIT,
    COMMIT_BUTTON_HEIGHT, COMMIT_FONT_SIZE, COMMIT_INPUT_HEIGHT, COMMIT_INPUT_MAX_LINES,
    DEPTH, FILE_FONT_SIZE, FILE_ROW_HEIGHT, FRAME_RADIUS, FRAME_STROKE, HEADER_FONT_SIZE,
    HEADER_HEIGHT, ORDER_ACCENT, ORDER_FRAME, ORDER_INNER, ORDER_LINE_BG, ORDER_MENU_BG,
    ORDER_MENU_ROW, ORDER_MENU_TEXT, ORDER_ROW_BG, ORDER_SCROLL, PADDING_X,
    SCROLLBAR_HIT_PAD, STATS_FONT_SIZE, STATS_HEIGHT, TREE_INDENT,
};

/// Padded, whole-line viewport for the shared append-only commit input.
/// Always follow its tail, including the empty line after Shift+Enter.
fn commit_text_viewport(box_rect: Rect, scale: f32, line_count: usize) -> (Rect, usize, usize) {
    let pad_x = (8.0 * scale).min(box_rect.w * 0.5);
    let pad_y = (6.0 * scale).min(box_rect.h * 0.5);
    let text_rect = Rect {
        x: box_rect.x + pad_x,
        y: box_rect.y + pad_y,
        w: (box_rect.w - pad_x * 2.0).max(0.0),
        h: (box_rect.h - pad_y * 2.0).max(0.0),
    };
    let visible_lines = (text_rect.h / (COMMIT_FONT_SIZE * scale * 1.35))
        .floor()
        .max(1.0) as usize;
    let first_line = line_count.saturating_sub(visible_lines);
    (text_rect, first_line, visible_lines)
}

fn commit_placeholder(message: &str) -> Option<&'static str> {
    message.is_empty().then_some("Type a commit message...")
}

/// Footer selector and split button share a row, with distinct hit regions.
pub(super) fn footer_control_rects(x: f32, y: f32, width: f32, scale: f32) -> [Rect; 3] {
    let width = width.max(0.0);
    let split_w = (110.0 * scale).min(width * 0.55);
    let arrow_w = (28.0 * scale).min(split_w * 0.3);
    let gap = (8.0 * scale).min((width - split_w).max(0.0));
    let branch_w = (width - split_w - gap).max(0.0);
    let h = COMMIT_BUTTON_HEIGHT * scale;
    [
        Rect {
            x,
            y,
            w: branch_w,
            h,
        },
        Rect {
            x: x + branch_w + gap,
            y,
            w: split_w - arrow_w,
            h,
        },
        Rect {
            x: x + width - arrow_w,
            y,
            w: arrow_w,
            h,
        },
    ]
}

fn pull_request_row_rects(x: f32, y: f32, width: f32, scale: f32) -> (Rect, Rect) {
    let row = Rect {
        x,
        y,
        w: width.max(0.0),
        h: 26.0 * scale,
    };
    let inset = (PADDING_X * scale).min(row.w * 0.5);
    let text = Rect {
        x: x + inset,
        y,
        w: (row.w - inset * 2.0 - 12.0 * scale).max(0.0),
        h: row.h,
    };
    (row, text)
}

fn pull_request_text_opts(
    fonts: &neoism_paint::font::FontLibrary,
    rect: Rect,
    scale: f32,
    color: [u8; 4],
) -> DrawOpts {
    DrawOpts {
        font_id: crate::primitives::prose_font::ui_prose_font_id(fonts),
        font_size: 12.0 * scale,
        color,
        clip_rect: Some(rect.as_array()),
        ..DrawOpts::default()
    }
}

#[cfg(test)]
mod commit_layout_tests {
    use super::*;

    #[test]
    fn pull_request_row_is_inset_centered_and_bounded_at_all_scales() {
        for scale in [0.75, 1.0, 1.5, 2.0] {
            for width in [0.0, 8.0, 28.0, 240.0] {
                let (row, text) =
                    pull_request_row_rects(31.0, 80.0, width * scale, scale);
                assert_eq!(row.h, 26.0 * scale);
                assert!(text.x >= row.x && text.x + text.w <= row.x + row.w);
                assert!(text.w >= 0.0);
                if width == 240.0 {
                    assert_eq!(text.x, row.x + PADDING_X * scale);
                }
                let top = row.y + (row.h - 12.0 * scale) * 0.5;
                assert!(
                    (top - row.y - (row.y + row.h - top - 12.0 * scale)).abs() < 0.001
                );
            }
        }
    }

    #[test]
    fn pull_request_action_uses_regular_proportional_ui_font_not_terminal_primary() {
        use neoism_paint::font::{constants::FONT_PRESS_START_2P, FontLibrary};
        let fonts = FontLibrary {
            inner: Default::default(),
        };
        let primary = fonts.ensure_static_font(FONT_PRESS_START_2P).unwrap();
        let (_, rect) = pull_request_row_rects(0.0, 0.0, 240.0, 1.0);
        let opts = pull_request_text_opts(&fonts, rect, 1.0, [180, 180, 180, 255]);
        assert_eq!(opts.font_id, fonts.font_id_for_family("Geist"));
        assert!(opts.font_id.is_some());
        assert_ne!(opts.font_id, Some(primary));
        assert!(!opts.bold);
        assert_eq!(opts.clip_rect, Some(rect.as_array()));
        let mut text = neoism_paint::text::Text::new(&fonts);
        text.init_cpu();
        assert!(text.measure("WWWW", &opts) > text.measure("iiii", &opts) * 2.0);
    }

    #[test]
    fn commit_text_starts_at_padded_top_left_at_all_scales() {
        for scale in [1.0, 1.5, 2.0] {
            let box_rect = Rect {
                x: 30.0,
                y: 200.0,
                w: 280.0 * scale,
                h: COMMIT_INPUT_HEIGHT * scale,
            };
            for text in ["", "Typing a commit", " ", "one\ntwo"] {
                let (rect, first, _) =
                    commit_text_viewport(box_rect, scale, text.split('\n').count());
                assert_eq!(rect.x, box_rect.x + 8.0 * scale);
                assert_eq!(rect.y, box_rect.y + 6.0 * scale);
                assert_eq!(rect.w, box_rect.w - 16.0 * scale);
                assert_eq!(rect.h, box_rect.h - 12.0 * scale);
                assert_eq!(first, 0);
            }
        }
    }

    #[test]
    fn commit_viewport_follows_tail_without_partial_lower_lines() {
        for scale in [1.0, 1.5, 2.0] {
            let line_h = COMMIT_FONT_SIZE * scale * 1.35;
            let box_rect = Rect {
                x: 0.0,
                y: 100.0,
                w: 280.0 * scale,
                h: COMMIT_INPUT_MAX_LINES as f32 * line_h + 12.0 * scale,
            };
            // A trailing newline must leave the new empty insertion row visible.
            let text = "one\ntwo\nthree\nfour\nfive\nsix\nseven\n";
            let count = text.split('\n').count();
            let (rect, first, visible) = commit_text_viewport(box_rect, scale, count);
            assert!(first > 0);
            assert_eq!(first + visible, count);
            assert!(visible as f32 * line_h <= rect.h + 0.001);
            let caret_bottom = rect.y + (count - first - 1) as f32 * line_h
                + (COMMIT_FONT_SIZE + 2.0) * scale;
            assert!(caret_bottom <= rect.y + rect.h + 0.001);
            assert_eq!(text.split('\n').last(), Some(""));
            // Deleting back to a single line restores the original top origin.
            assert_eq!(commit_text_viewport(box_rect, scale, 1).1, 0);
        }
    }

    #[test]
    fn commit_placeholder_only_appears_for_exactly_empty_buffer() {
        assert_eq!(commit_placeholder(""), Some("Type a commit message..."));
        for message in ["a", "Typing a commit", " ", "\t", "\n", "one\ntwo", "one\n"] {
            assert_eq!(commit_placeholder(message), None, "{message:?}");
        }
    }
}

impl GitDiffPanel {
    pub fn render(
        &mut self,
        sugarloaf: &mut dyn Paint,
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
            self.folder_checkbox_rects.clear();
            self.folder_row_rects.clear();
            self.selected_cursor_rect = None;
            self.commit_box_rect = Rect::ZERO;
            self.commit_button_rect = Rect::ZERO;
            self.push_button_rect = Rect::ZERO;
            self.actions_button_rect = Rect::ZERO;
            self.actions_menu_rect = Rect::ZERO;
            self.actions_menu_row_rects.clear();
            self.divider_rect = Rect::ZERO;
            self.branch_button_rect = Rect::ZERO;
            self.branch_menu_rect = Rect::ZERO;
            self.branch_filter_rect = Rect::ZERO;
            self.branch_menu_row_rects.clear();
            return;
        }

        self.discover_pull_request();
        self.apply_commit_confirmation();
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
        let panel_fill = crate::primitives::surface_background::base_color(
            "git-sidebar",
            theme,
            theme.f32(theme.bg),
        );
        let outer = [panel_x, panel_y, target_w, height];
        let inner = [
            panel_x + frame_stroke,
            panel_y + frame_stroke,
            (target_w - frame_stroke * 2.0).max(0.0),
            (height - frame_stroke).max(0.0),
        ];
        let outer_radii = [frame_radius, frame_radius, 0.0, 0.0];
        let inner_radii = [inner_radius, inner_radius, 0.0, 0.0];
        // Never put an opaque full-panel backing behind a translucent fill.
        if panel_fill[3] >= 1.0 {
            sugarloaf.quad(
                None,
                outer[0],
                outer[1],
                outer[2],
                outer[3],
                theme.f32(theme.surface),
                outer_radii,
                DEPTH,
                ORDER_FRAME,
            );
        } else {
            crate::widgets::frame::draw_background_border(
                sugarloaf,
                outer,
                inner,
                outer_radii,
                inner_radii,
                theme.f32(theme.surface),
                DEPTH,
                ORDER_FRAME,
            );
        }
        sugarloaf.quad(
            None,
            inner[0],
            inner[1],
            inner[2],
            inner[3],
            panel_fill,
            inner_radii,
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
            git_background_opacity(theme.f32(theme.hover)),
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
        sugarloaf
            .text_mut()
            .draw(inner_x, title_y, "Git", &title_opts);

        cursor_y += header_h;

        // ── Stats row ────────────────────────────────────────────────
        let (loading, error, files, total_add, total_del) = {
            let data = match self.data.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            let total_add: u32 = data.files.iter().map(|f| f.additions).sum();
            let total_del: u32 = data.files.iter().map(|f| f.deletions).sum();

            (
                data.loading,
                data.error.clone(),
                data.files.clone(),
                total_add,
                total_del,
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
        if self.pull_request_row_visible() {
            let (pr_rect, pr_text_rect) =
                pull_request_row_rects(content_x, cursor_y, content_w, s);
            let hovered = self.pull_request_hovered();
            let lookup = self.pull_request_lookup_active();
            if hovered || (lookup && self.focused) {
                let inset = (6.0 * s).min(pr_rect.w * 0.5);
                sugarloaf.rounded_rect(
                    None,
                    pr_rect.x + inset,
                    pr_rect.y + 2.0 * s,
                    (pr_rect.w - inset * 2.0).max(0.0),
                    pr_rect.h - 4.0 * s,
                    git_background_opacity(theme.f32(theme.hover)),
                    DEPTH,
                    4.0 * s,
                    ORDER_ROW_BG,
                );
            }
            let pr_opts = pull_request_text_opts(
                sugarloaf.font_library(),
                pr_text_rect,
                s,
                theme.u8(if hovered || lookup {
                    theme.fg
                } else {
                    theme.dim
                }),
            );
            let label = truncate_to_fit(
                &self.pull_request_label(),
                pr_text_rect.w,
                sugarloaf,
                &pr_opts,
            );
            let label_y = pr_rect.y + (pr_rect.h - pr_opts.font_size) * 0.5;
            let label_w =
                sugarloaf
                    .text_mut()
                    .draw(pr_text_rect.x, label_y, &label, &pr_opts);
            if lookup && self.focused && pr_text_rect.w > 0.0 {
                let caret_w = s.max(1.0).min(pr_text_rect.w);
                let caret_x = (pr_text_rect.x + label_w + s)
                    .min(pr_text_rect.x + pr_text_rect.w - caret_w)
                    .max(pr_text_rect.x);
                sugarloaf.rect(
                    None,
                    caret_x,
                    label_y,
                    caret_w,
                    pr_opts.font_size,
                    theme.f32(theme.accent),
                    DEPTH,
                    ORDER_ACCENT,
                );
            } else if hovered && pr_text_rect.w > 0.0 {
                let arrow_opts = DrawOpts {
                    clip_rect: Some(pr_rect.as_array()),
                    ..pr_opts
                };
                sugarloaf.text_mut().draw(
                    pr_text_rect.x + pr_text_rect.w + 4.0 * s,
                    label_y,
                    "›",
                    &arrow_opts,
                );
            }
            if let Ok(mut data) = self.data.lock() {
                data.pr.rect = Some(pr_rect);
            }
            cursor_y += pr_rect.h;
        }

        // Empty / error / loading branches.
        let body_top = cursor_y + (1.0 * s).max(1.0) + CARD_GAP_TOP * s;
        // Keep the action footer available even for a clean worktree or a
        // failed remote operation; the cards below show the empty/error state.

        // ── Files card sizing ────────────────────────────────────────
        let card_x = content_x + CARD_PAD_X * s;
        let card_w = (content_w - CARD_PAD_X * 2.0 * s).max(0.0);
        let row_h = FILE_ROW_HEIGHT * s;
        let files_header_h = diff_card::HEADER_HEIGHT * s;
        // The file list owns all space between the header and footer.
        let files_card_y = body_top;
        let ca_pad = 8.0 * s;
        let branch_line_h = COMMIT_BUTTON_HEIGHT * s;
        let commit_line_h = COMMIT_FONT_SIZE * s * 1.35;
        let commit_lines = self
            .commit_input
            .text()
            .split('\n')
            .count()
            .clamp(1, COMMIT_INPUT_MAX_LINES);
        let commit_box_h =
            (COMMIT_INPUT_HEIGHT * s).max(commit_lines as f32 * commit_line_h + 12.0 * s);
        let commit_area_h = ca_pad * 2.0
            + branch_line_h
            + 8.0 * s
            + commit_box_h
            + 8.0 * s
            + COMMIT_BUTTON_HEIGHT * s;
        let commit_area_y = (content_bottom - frame_stroke - commit_area_h).max(body_top);
        let files_card_h = (commit_area_y - body_top - CARD_VGAP * s).max(0.0);
        let files_header_h = files_header_h.min(files_card_h);
        let files_body_h = (files_card_h - files_header_h).max(0.0);
        self.divider_rect = Rect::ZERO;
        self.divider_dragging = false;
        self.diff_card_rect = Rect::ZERO;
        self.diff_scrollbar_thumb_rect = Rect::ZERO;

        self.files_card_rect = Rect {
            x: card_x,
            y: files_card_y,
            w: card_w,
            h: files_card_h,
        };

        // ── Files card chrome ────────────────────────────────────────
        let card_radius = diff_card::CARD_RADIUS * s;
        let card_stroke = (1.0 * s).max(1.0);
        // Border ring — slightly larger backing in `theme.border`,
        // then header + body fills draw on top, leaving a 1px stroke
        // around the whole card. Same trick `diff_card::render` uses
        // so the two cards read as a matched pair.
        if crate::customization::background_opacity(
            &crate::primitives::surface_background::style("git-sidebar"),
        ) >= 1.0
        {
            sugarloaf.quad(
                None,
                card_x - card_stroke,
                files_card_y - card_stroke,
                card_w + card_stroke * 2.0,
                files_card_h + card_stroke * 2.0,
                theme.f32(theme.border),
                [card_radius + card_stroke; 4],
                DEPTH,
                ORDER_ROW_BG,
            );
        } else {
            git_border_backing(
                sugarloaf,
                card_x - card_stroke,
                files_card_y - card_stroke,
                card_w + card_stroke * 2.0,
                files_card_h + card_stroke * 2.0,
                theme.f32(theme.border),
                DEPTH,
                card_radius + card_stroke,
                ORDER_ROW_BG,
                card_stroke,
            );
        }
        sugarloaf.quad(
            None,
            card_x,
            files_card_y,
            card_w,
            files_header_h,
            git_background_opacity(theme.f32(theme.surface)),
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
            git_background_opacity(theme.f32(theme.bg)),
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
        // Keep rows clear of the rounded bottom corners. This exact viewport
        // drives rendering, hit testing, keyboard reveal and scrollbar bounds.
        let inset = (diff_card::BODY_TOP_PAD * s)
            .max(card_radius)
            .min(files_body_h / 2.0);
        let viewport_y = files_body_y + inset;
        let viewport_h = (files_body_h - inset * 2.0).max(0.0);
        self.files_body_rect = Rect {
            x: card_x,
            y: viewport_y,
            w: card_w,
            h: viewport_h,
        };
        if let Some(message) = error.as_deref().or_else(|| {
            if files.is_empty() {
                Some(if loading { "Loading..." } else { "No changes" })
            } else {
                None
            }
        }) {
            let opts = DrawOpts {
                font_size: STATS_FONT_SIZE * s,
                color: theme.u8(if error.is_some() {
                    theme.red
                } else {
                    theme.muted
                }),
                clip_rect: Some([card_x, files_body_y, card_w, files_body_h]),
                ..DrawOpts::default()
            };
            let text = truncate_to_fit(message, card_w - 20.0 * s, sugarloaf, &opts);
            sugarloaf.text_mut().draw(
                card_x + 10.0 * s,
                files_body_y + 10.0 * s,
                &text,
                &opts,
            );
        }
        // Rebuild the flattened tree (folders + file leaves) from the
        // current file list + collapsed-set. Cached on `self` so
        // hit-testing and keyboard navigation see the same order.
        let visual_rows = super::update::build_visual_rows(&files, &self.collapsed_dirs);
        self.visual_rows = visual_rows.clone();
        let total_rows = visual_rows.len();

        let files_body_inner_y = viewport_y;
        let visible_rows = (viewport_h / row_h).ceil() as usize;
        let max_scroll = self.max_file_scroll();
        self.clamp_file_scroll();
        let device_scale = sugarloaf.scale_factor();
        let spring_offset = self.tick_file_scroll();
        // The spring may retain an old range after a resize or group collapse.
        let rendered_scroll = snap_to_device_px(
            (self.file_scroll - spring_offset).clamp(0.0, max_scroll),
            device_scale,
        )
        .clamp(0.0, max_scroll);
        let scroll_offset = self.file_scroll - rendered_scroll;

        self.file_row_rects.clear();
        self.file_checkbox_rects.clear();
        self.folder_checkbox_rects.clear();
        self.folder_row_rects.clear();
        self.selected_cursor_rect = None;

        let row_clip_top = viewport_y;
        let row_clip_bot = viewport_y + viewport_h;
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
                    git_background_opacity(theme.f32(theme.hover)),
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

            match &vr.kind {
                VisualRowKind::Group {
                    label,
                    path,
                    file_indices,
                    collapsed,
                } => {
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
                    let chevron_opts = DrawOpts {
                        font_size: FILE_FONT_SIZE * s * 0.85,
                        color: theme.u8(theme.muted),
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
                    if path.is_some() {
                        let (glyph, _) =
                            crate::panels::file_tree::icons::folder_icon(!*collapsed);
                        let opts = DrawOpts {
                            color: theme.u8(theme.folder),
                            ..dir_name_opts.clone()
                        };
                        sugarloaf.text_mut().draw(cx, text_y, glyph, &opts);
                        cx += FILE_FONT_SIZE * s + 6.0 * s;
                    }
                    let dir_name =
                        path.as_deref().map(|p| split_path(p).0).unwrap_or(label);
                    let count = format!(" {}", file_indices.len());
                    let count_opts = DrawOpts {
                        color: theme.u8(theme.muted),
                        ..dir_name_opts.clone()
                    };
                    let count_w = sugarloaf.text_mut().measure(&count, &count_opts);
                    let name_budget = (stats_right_edge - cx - count_w).max(0.0);
                    let dir_fit =
                        truncate_to_fit(dir_name, name_budget, sugarloaf, &dir_name_opts);
                    let _ = sugarloaf.text_mut().draw(
                        cx,
                        text_y,
                        dir_fit.as_str(),
                        &dir_name_opts,
                    );
                    let name_w = sugarloaf.text_mut().measure(&dir_fit, &dir_name_opts);
                    sugarloaf
                        .text_mut()
                        .draw(cx + name_w, text_y, &count, &count_opts);
                    let children: Vec<_> = file_indices
                        .iter()
                        .filter_map(|&index| files.get(index))
                        .collect();
                    let all_staged = !children.is_empty()
                        && children.iter().all(|f| {
                            f.staged && f.status != super::types::FileStatus::Mixed
                        });
                    let some_staged = children.iter().any(|f| f.staged);
                    let cb_y = row_y + (row_h - checkbox_size) / 2.0;
                    if cb_y >= row_clip_top && cb_y + checkbox_size <= row_clip_bot {
                        let radius = 4.0 * s;
                        sugarloaf.rounded_rect(
                            None,
                            checkbox_x,
                            cb_y,
                            checkbox_size,
                            checkbox_size,
                            theme.f32(if all_staged {
                                theme.accent
                            } else {
                                theme.muted
                            }),
                            DEPTH,
                            radius,
                            ORDER_SCROLL,
                        );
                        if !all_staged {
                            let inset = (1.5 * s).max(1.0);
                            sugarloaf.rounded_rect(
                                None,
                                checkbox_x + inset,
                                cb_y + inset,
                                checkbox_size - 2.0 * inset,
                                checkbox_size - 2.0 * inset,
                                theme.f32(theme.bg),
                                DEPTH,
                                (radius - inset).max(0.0),
                                ORDER_SCROLL,
                            );
                        }
                        if all_staged || some_staged {
                            let glyph = if all_staged { check_glyph() } else { "-" };
                            let opts = DrawOpts {
                                font_size: checkbox_size * 0.72,
                                color: theme.u8(if all_staged {
                                    theme.bg
                                } else {
                                    theme.fg
                                }),
                                bold: true,
                                clip_rect: Some([
                                    checkbox_x,
                                    cb_y,
                                    checkbox_size,
                                    checkbox_size,
                                ]),
                                ..DrawOpts::default()
                            };
                            let w = sugarloaf.text_mut().measure(glyph, &opts);
                            sugarloaf.text_mut().draw(
                                checkbox_x + (checkbox_size - w) / 2.0,
                                cb_y + (checkbox_size - checkbox_size * 0.72) / 2.0,
                                glyph,
                                &opts,
                            );
                        }
                        self.folder_checkbox_rects.push((
                            absolute_ix,
                            Rect {
                                x: checkbox_x,
                                y: cb_y,
                                w: checkbox_size,
                                h: checkbox_size,
                            },
                        ));
                    }
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

                    let label_color = theme.u8(theme.fg);
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

                    let (filename, _) = split_path(&f.path);
                    let (glyph, color) =
                        crate::panels::file_tree::icons::icon_for_file(filename);
                    let icon_opts = DrawOpts {
                        color,
                        ..name_opts.clone()
                    };
                    let mut tx = base_x;
                    sugarloaf.text_mut().draw(tx, text_y, glyph, &icon_opts);
                    tx += FILE_FONT_SIZE * s + 6.0 * s;
                    let marker_w = sugarloaf
                        .text_mut()
                        .measure(f.status.marker(), &marker_opts);
                    let stats_right_edge = stats_right_edge - marker_w - 8.0 * s;
                    sugarloaf.text_mut().draw(
                        stats_right_edge + 8.0 * s,
                        text_y,
                        f.status.marker(),
                        &marker_opts,
                    );
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
        if max_scroll > 0.0 && viewport_h > 0.0 {
            let progress = rendered_scroll / max_scroll;
            if let Some((thumb_y, thumb_h)) = scrollbar::compute_thumb(
                viewport_h.ceil() as usize,
                (total_rows as f32 * row_h).ceil() as usize,
                viewport_y,
                viewport_h,
                progress,
            ) {
                let thumb_x = card_x + card_w - scrollbar::width() - 2.0 * s;
                scrollbar::draw_track(
                    sugarloaf,
                    thumb_x,
                    viewport_y,
                    viewport_h,
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

        self.draw_footer_controls(
            sugarloaf,
            inner_x,
            cy,
            (content_w - 2.0 * PADDING_X * s).max(0.0),
            theme,
        );
        cy += branch_line_h + 8.0 * s;

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
        git_border_backing(
            sugarloaf,
            box_x - box_stroke,
            box_y - box_stroke,
            box_w + box_stroke * 2.0,
            box_h + box_stroke * 2.0,
            box_border,
            DEPTH,
            box_radius + box_stroke,
            ORDER_ROW_BG,
            box_stroke,
        );
        sugarloaf.rounded_rect(
            None,
            box_x,
            box_y,
            box_w,
            box_h,
            git_background_opacity(theme.f32(theme.surface)),
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

        // Hint, text, and caret share the padded top-left viewport origin.
        let lines: Vec<&str> = msg.split('\n').collect();
        let (text_rect, first_line, visible_lines) =
            commit_text_viewport(self.commit_box_rect, s, lines.len());
        let msg_opts = DrawOpts {
            font_size: COMMIT_FONT_SIZE * s,
            color: theme.u8(theme.fg),
            clip_rect: Some([text_rect.x, text_rect.y, text_rect.w, text_rect.h]),
            ..DrawOpts::default()
        };
        if let Some(hint) = commit_placeholder(&msg) {
            let hint_opts = DrawOpts {
                color: theme.u8(theme.dim),
                ..msg_opts
            };
            sugarloaf.text_mut().draw(text_rect.x, text_rect.y, hint, &hint_opts);
        }
        let caret_w = (1.5 * s).max(1.0).min(text_rect.w);
        let last = lines.last().copied().unwrap_or("");
        let last_w = sugarloaf.text_mut().measure(last, &msg_opts);
        // The input currently appends at the tail. Scroll horizontally as well
        // as vertically so a long final line cannot hide the insertion point.
        let tail_scroll_x = (last_w + caret_w - text_rect.w).max(0.0);
        for (row, line) in lines.iter().skip(first_line).take(visible_lines).enumerate() {
            let is_tail = first_line + row == lines.len() - 1;
            let scroll_x = if is_tail { tail_scroll_x } else { 0.0 };
            sugarloaf.text_mut().draw(
                text_rect.x - scroll_x,
                text_rect.y + row as f32 * commit_line_h,
                line,
                &msg_opts,
            );
        }
        if commit_focused && text_rect.w > 0.0 && text_rect.h > 0.0 {
            let caret_y = text_rect.y
                + (lines.len() - 1 - first_line) as f32 * commit_line_h;
            let caret_h = (COMMIT_FONT_SIZE * s + 2.0 * s)
                .min((text_rect.y + text_rect.h - caret_y).max(0.0));
            sugarloaf.rect(
                None,
                text_rect.x + last_w - tail_scroll_x,
                caret_y,
                caret_w,
                caret_h,
                theme.f32(theme.accent),
                DEPTH,
                ORDER_SCROLL,
            );
        }
        cy += box_h + 8.0 * s;

        // Commit / Stage All buttons.
        let btn_h = COMMIT_BUTTON_HEIGHT * s;
        let btn_y = cy;
        let btn_pad_x = 12.0 * s;
        let btn_radius = 6.0 * s;
        let btn_text_y = btn_y + (btn_h - COMMIT_FONT_SIZE * s) / 2.0;

        // Commit — filled accent button.
        let commit_label = "Commit";
        let commit_enabled = self.git_action_enabled(super::types::GitAction::Commit);
        let commit_label_opts = DrawOpts {
            font_size: COMMIT_FONT_SIZE * s,
            color: theme.u8(if commit_enabled {
                theme.bg
            } else {
                theme.muted
            }),
            bold: true,
            ..DrawOpts::default()
        };
        let commit_text_w = sugarloaf
            .text_mut()
            .measure(commit_label, &commit_label_opts);
        let commit_btn_w = commit_text_w + btn_pad_x * 2.0;
        let commit_btn_x = inner_x + box_w - commit_btn_w;
        self.latest_commit_rect = Rect::ZERO;
        if let Some(commit) = self.latest_commit() {
            let width = (commit_btn_x - inner_x - 12.0 * s).max(0.0);
            self.latest_commit_rect = Rect { x: inner_x, y: btn_y, w: width, h: btn_h };
            let subject = commit.message.lines().next().unwrap_or("");
            let label = truncate_to_fit(subject, width, sugarloaf, &DrawOpts {
                font_size: COMMIT_FONT_SIZE * s, ..DrawOpts::default()
            });
            let _ = sugarloaf.text_mut().draw(inner_x, btn_text_y, &label, &DrawOpts {
                font_size: COMMIT_FONT_SIZE * s, color: theme.u8(theme.dim),
                clip_rect: Some(self.latest_commit_rect.as_array()), ..DrawOpts::default()
            });
        }
        sugarloaf.rounded_rect(
            None,
            commit_btn_x,
            btn_y,
            commit_btn_w,
            btn_h,
            git_background_opacity(theme.f32(if commit_enabled {
                theme.accent
            } else {
                theme.surface
            })),
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

        self.latest_commit_popup_rect = Rect::ZERO;
        self.latest_commit_copy_rect = Rect::ZERO;
        if self.latest_commit_hover {
            if let Some(commit) = self.latest_commit() {
                let width = (330.0 * s).min(content_w - 16.0 * s).max(0.0);
                let height = 94.0 * s;
                let px = inner_x;
                let py = (btn_y - height - 3.0 * s).max(chrome_top);
                self.latest_commit_popup_rect = Rect { x: px, y: py, w: width, h: height + 6.0 * s };
                sugarloaf.rounded_rect(None, px, py, width, height, theme.f32(theme.surface), DEPTH, 6.0 * s, ORDER_MENU_BG + 5);
                let rows = [commit.author.clone(), crate::panels::git_commit_view::format_date(commit.timestamp, 0), commit.message.lines().next().unwrap_or("").to_string(), format!("{}  Copy SHA", commit.sha)];
                for (i, row) in rows.iter().enumerate() {
                    let ry = py + (10.0 + i as f32 * 20.0) * s;
                    let _ = sugarloaf.text_mut().draw(px + 10.0 * s, ry, row, &DrawOpts {
                        font_size: 11.0 * s, color: theme.u8(if i == 3 { theme.accent } else { theme.fg }),
                        clip_rect: Some([px + 8.0 * s, py, (width - 16.0 * s).max(0.0), height]), ..DrawOpts::default()
                    });
                    if i == 3 { self.latest_commit_copy_rect = Rect { x: px, y: ry, w: width, h: 20.0 * s }; }
                }
            }
        }

        self.draw_actions_menu(
            sugarloaf,
            s,
            content_x,
            chrome_top,
            content_bottom,
            theme,
        );

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

    fn draw_footer_controls(
        &mut self,
        sugarloaf: &mut dyn Paint,
        x: f32,
        y: f32,
        width: f32,
        theme: &IdeTheme,
    ) {
        let s = self.scale;
        let [branch, push, actions] = footer_control_rects(x, y, width, s);
        self.branch_button_rect = branch;
        self.push_button_rect = push;
        self.actions_button_rect = actions;
        let enabled = self.push_button_enabled();
        let branch_focused = self.branch_section_focused();
        let label = self.branch().unwrap_or_else(|| "detached".into());
        let split = Rect {
            w: push.w + actions.w,
            ..push
        };
        for (rect, primary) in [(branch, false), (split, true)] {
            let bg = if primary && enabled {
                theme.accent
            } else if !primary && (branch_focused || self.branch_menu_open) {
                theme.hover
            } else {
                theme.surface
            };
            sugarloaf.rounded_rect(
                None,
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                git_background_opacity(theme.f32(bg)),
                DEPTH,
                6.0 * s,
                ORDER_ROW_BG + 1,
            );
        }
        if branch_focused {
            git_border_backing(
                sugarloaf,
                branch.x - s,
                branch.y - s,
                branch.w + 2.0 * s,
                branch.h + 2.0 * s,
                theme.f32(theme.accent),
                DEPTH,
                7.0 * s,
                ORDER_ROW_BG,
                s,
            );
        }
        let branch_opts = DrawOpts {
            font_size: COMMIT_FONT_SIZE * s,
            color: theme.u8(theme.fg),
            clip_rect: Some(branch.as_array()),
            ..DrawOpts::default()
        };
        let ty = y + (branch.h - COMMIT_FONT_SIZE * s) / 2.0;
        let icon_w =
            sugarloaf
                .text_mut()
                .draw(x + 6.0 * s, ty, branch_glyph(), &branch_opts);
        let name_x = x + 12.0 * s + icon_w;
        let fit = truncate_to_fit(
            &label,
            (branch.x + branch.w - name_x - 20.0 * s).max(0.0),
            sugarloaf,
            &branch_opts,
        );
        sugarloaf.text_mut().draw(name_x, ty, &fit, &branch_opts);
        sugarloaf.text_mut().draw(
            branch.x + branch.w - 16.0 * s,
            ty,
            chevron_down_glyph(),
            &branch_opts,
        );
        let push_opts = DrawOpts {
            font_size: COMMIT_FONT_SIZE * s,
            color: theme.u8(if enabled { theme.bg } else { theme.muted }),
            bold: true,
            clip_rect: Some(push.as_array()),
            ..DrawOpts::default()
        };
        let label = self.push_button_label();
        let text_w = sugarloaf.text_mut().measure(label, &push_opts);
        sugarloaf.text_mut().draw(
            push.x + (push.w - text_w) / 2.0,
            ty,
            label,
            &push_opts,
        );
        sugarloaf.rect(
            None,
            actions.x,
            y + 5.0 * s,
            s.max(1.0),
            actions.h - 10.0 * s,
            theme.f32(theme.border),
            DEPTH,
            ORDER_ACCENT,
        );
        sugarloaf.text_mut().draw(
            actions.x + 8.0 * s,
            ty,
            chevron_down_glyph(),
            &DrawOpts {
                color: theme.u8(if enabled { theme.bg } else { theme.fg }),
                clip_rect: Some(actions.as_array()),
                ..push_opts
            },
        );
    }

    fn draw_actions_menu(
        &mut self,
        sugarloaf: &mut dyn Paint,
        s: f32,
        content_x: f32,
        top: f32,
        bottom: f32,
        theme: &IdeTheme,
    ) {
        self.actions_menu_row_rects.clear();
        if !self.actions_menu_is_open() {
            self.actions_menu_rect = Rect::ZERO;
            return;
        }
        self.refresh_actions_menu();
        let pad = 6.0 * s;
        let row_h = 28.0 * s;
        let btn = self.actions_button_rect;
        let left = content_x + pad;
        let viewport_w = (self.panel_rect.x + self.panel_rect.w - left - pad).max(0.0);
        let menu_w = (viewport_w - 16.0).max(0.0).min(270.0 * s);
        let visible = ((btn.y - top - 12.0 * s - pad * 2.0) / row_h)
            .floor()
            .max(1.0) as usize;
        self.actions_menu.content_mut().set_max_visible(visible);
        let menu_h =
            pad * 2.0 + self.actions_menu.content().visible_count() as f32 * row_h;
        self.actions_menu
            .set_anchor(crate::widgets::popover::PopoverAnchor::Rect([
                btn.x - left,
                btn.y - top,
                btn.w,
                btn.h,
            ]));
        let [x, y] = self
            .actions_menu
            .resolve_position((menu_w, menu_h), (viewport_w, bottom - top));
        let menu_x = left + x;
        let menu_y = top + y;
        self.actions_menu_rect = Rect {
            x: menu_x,
            y: menu_y,
            w: menu_w,
            h: menu_h,
        };
        let stroke = (1.0 * s).max(1.0);
        let radius = 8.0 * s;
        sugarloaf.overlay_rounded_rect(
            menu_x - stroke,
            menu_y - stroke,
            menu_w + stroke * 2.0,
            menu_h + stroke * 2.0,
            theme.f32(theme.border),
            DEPTH,
            radius + stroke,
            ORDER_MENU_BG,
        );
        // Opaque popup backing prevents covered file/diff text bleeding through.
        let mut surface = theme.f32(theme.surface);
        surface[3] = 1.0;
        sugarloaf.overlay_rounded_rect(
            menu_x,
            menu_y,
            menu_w,
            menu_h,
            surface,
            DEPTH,
            radius,
            ORDER_MENU_BG,
        );
        let selected = self.actions_menu.content().selected_index();
        for (visible_row, (slot, item)) in
            self.actions_menu.content().visible_items().enumerate()
        {
            let ry = menu_y + pad + visible_row as f32 * row_h;
            let rect = Rect {
                x: menu_x + pad,
                y: ry,
                w: menu_w - pad * 2.0,
                h: row_h,
            };
            if slot == selected && item.enabled {
                sugarloaf.overlay_rounded_rect(
                    rect.x,
                    rect.y,
                    rect.w,
                    rect.h,
                    theme.f32(theme.hover),
                    DEPTH,
                    4.0 * s,
                    ORDER_MENU_ROW,
                );
            }
            // Separate local-change, remote and repository actions without extra chrome.
            if slot == 4 || slot == 7 {
                sugarloaf.overlay_rect(
                    rect.x + 6.0 * s,
                    ry,
                    (rect.w - 12.0 * s).max(0.0),
                    stroke,
                    theme.f32(theme.border),
                    DEPTH,
                    ORDER_MENU_ROW,
                );
            }
            let opts = DrawOpts {
                font_size: COMMIT_FONT_SIZE * s,
                color: theme.u8(if item.enabled { theme.fg } else { theme.muted }),
                clip_rect: Some(rect.as_array()),
                ..DrawOpts::default()
            };
            sugarloaf.overlay_text_mut().draw(
                rect.x + 8.0 * s,
                ry + (row_h - COMMIT_FONT_SIZE * s) / 2.0,
                &item.label,
                &opts,
            );
            self.actions_menu_row_rects.push((slot, rect));
        }
    }

    /// Draw the branch-selector dropdown (search box + branch rows) when
    /// it's open. Records the menu / filter / row hit-rects. Overlays the
    /// cards at the highest render order in the panel.
    fn draw_branch_menu(
        &mut self,
        sugarloaf: &mut dyn Paint,
        s: f32,
        content_x: f32,
        _content_bottom: f32,
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
        // The selector is in the footer, so its menu opens upward.
        let min_left = content_x + 4.0 * s;
        let max_right = self.panel_rect.x + self.panel_rect.w - 4.0 * s;
        let menu_w = (240.0 * s).min((max_right - min_left).max(0.0));
        let menu_x = btn.x.max(min_left).min((max_right - menu_w).max(min_left));
        let pad = 6.0 * s;
        let search_h = 26.0 * s;
        let row_h = 22.0 * s;
        let shown = branches.len().min(BRANCH_MENU_MAX_ROWS);
        let list_h = shown as f32 * row_h;
        let wanted_h = pad + search_h + 6.0 * s + list_h + pad;
        let top = self.panel_rect.y + frame_stroke;
        let menu_h = wanted_h.min((btn.y - gap - top).max(0.0));
        let menu_y = (btn.y - gap - menu_h).max(top);
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
            git_background_opacity(theme.f32(theme.surface)),
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
            git_background_opacity(theme.f32(theme.bg)),
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
        let visible =
            (((menu_bot - rows_top).max(0.0) / row_h).floor() as usize).min(shown);
        let start = selected.saturating_add(1).saturating_sub(visible.max(1));
        for (slot, b) in branches.iter().skip(start).take(visible).enumerate() {
            let i = start + slot;
            let ry = rows_top + slot as f32 * row_h;
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
                    git_background_opacity(theme.f32(theme.hover)),
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

// `git` remains the native theme selector; `git-sidebar` owns panel material.
// Never substitute the root background RGB for a row/button state color.
fn git_background_opacity(color: [f32; 4]) -> [f32; 4] {
    crate::customization::apply_background_opacity(
        &crate::primitives::surface_background::style("git-sidebar"),
        color,
    )
}

// A filled focus/border backing would seal the material underneath its child.
// Keep the historical call exactly when opaque, otherwise paint only its ring.
#[allow(clippy::too_many_arguments)]
fn git_border_backing(
    sugarloaf: &mut dyn Paint,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: [f32; 4],
    depth: f32,
    radius: f32,
    order: u8,
    inset: f32,
) {
    if crate::customization::background_opacity(
        &crate::primitives::surface_background::style("git-sidebar"),
    ) >= 1.0
    {
        sugarloaf.rounded_rect(None, x, y, w, h, color, depth, radius, order);
    } else {
        crate::widgets::frame::draw_background_border(
            sugarloaf,
            [x, y, w, h],
            [
                x + inset,
                y + inset,
                (w - 2.0 * inset).max(0.0),
                (h - 2.0 * inset).max(0.0),
            ],
            [radius; 4],
            [(radius - inset).max(0.0); 4],
            color,
            depth,
            order,
        );
    }
}
