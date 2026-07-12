//! Completion popup geometry + tiny string helpers.
//!
//! `draw_completion_popup` is hung off `CommandComposer` so it can read
//! and update the popup-related springs / cached rect. The free
//! functions `completion_selected_index` and `completion_label` are
//! used by the render pass to pick the selected row and strip the `>`
//! prefix the input layer uses to encode selection.

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use super::scrollbar;
use super::state::CommandComposer;
use super::types::{
    COMPLETION_FONT_SIZE, COMPLETION_MAX_VISIBLE_RESULTS, COMPLETION_POP_MS,
    COMPLETION_ROW_HEIGHT, DEPTH, ORDER_STATUS_JOIN,
};
use super::util::{ease_out_back, ease_out_cubic, snap_to_device_px};
use crate::primitives::IdeTheme;

pub(super) fn completion_selected_index(items: &[String]) -> Option<usize> {
    items.iter().position(|item| item.starts_with('>'))
}

pub(super) fn completion_label(item: &str) -> &str {
    item.strip_prefix('>')
        .or_else(|| item.strip_prefix(' '))
        .unwrap_or(item)
}

fn completion_section_label(item: &str) -> Option<&str> {
    item.strip_prefix('§')
}

impl CommandComposer {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_completion_popup(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        items: &[String],
        detail: Option<&str>,
        anchor_x: f32,
        composer_top_y: f32,
        chassis_x: f32,
        chassis_w: f32,
        theme: &IdeTheme,
        scale: f32,
        scale_factor: f32,
        elapsed_ms: f32,
    ) {
        let count = items.len();
        let visible = count.min(COMPLETION_MAX_VISIBLE_RESULTS);
        if visible == 0 {
            self.completion_popup_rect = None;
            return;
        }

        let row_h = COMPLETION_ROW_HEIGHT * scale;
        let detail_h = detail
            .filter(|detail| !detail.is_empty())
            .map(|_| row_h * 0.86)
            .unwrap_or(0.0);
        let pad = 4.0 * scale;
        let input_pad_x = 14.0 * scale;
        let result_font = COMPLETION_FONT_SIZE * scale;
        let radius = 8.0 * scale;
        let selected_ix = completion_selected_index(items).unwrap_or(0).min(count - 1);
        self.sync_completion_motion(count, selected_ix, visible);

        let list_scroll_offset =
            snap_to_device_px(self.tick_completion_scroll(), scale_factor);
        // Snap cursor offset to device px too (was previously raw
        // float). Matches the equivalent fix in finder/render.rs:
        // sub-pixel float positions on the selected-row highlight
        // pushed glyphs onto a different sub-pixel anchor per frame
        // during continuous arrow navigation, reading as smear.
        let cursor_offset =
            snap_to_device_px(self.tick_completion_cursor(), scale_factor);
        let popup_w = (chassis_w - 24.0 * scale).clamp(180.0 * scale, 520.0 * scale);
        let base_popup_h = pad * 2.0 + row_h * visible as f32 + detail_h;
        let t = (elapsed_ms / COMPLETION_POP_MS).clamp(0.0, 1.0);
        let eased = ease_out_back(t).min(1.04);
        let pop_scale = 0.94 + eased * 0.06;
        let pop_offset_y = (1.0 - ease_out_cubic(t)) * 14.0 * scale;
        let popup_w_scaled = popup_w * pop_scale;
        let popup_h = base_popup_h * pop_scale;
        let base_x = anchor_x
            .min(chassis_x + chassis_w - popup_w - 12.0 * scale)
            .max(chassis_x + 12.0 * scale);
        let x = base_x + (popup_w - popup_w_scaled) * 0.5;
        let y =
            (composer_top_y - base_popup_h - 8.0 * scale + pop_offset_y).max(8.0 * scale);
        self.completion_popup_rect = Some([x, y, popup_w_scaled, popup_h]);

        sugarloaf.rounded_rect(
            None,
            x,
            y,
            popup_w_scaled,
            popup_h,
            theme.f32(theme.black),
            DEPTH,
            radius,
            ORDER_STATUS_JOIN + 1,
        );

        let list_x = x + pad;
        let list_y = y + pad;
        let list_w = (popup_w_scaled - pad * 2.0).max(0.0);
        let list_h = visible as f32 * row_h;
        let list_bottom = list_y + list_h;
        let list_clip = [list_x, list_y, list_w, list_h];
        let row_text_x = list_x + input_pad_x;

        let overscan =
            ((list_scroll_offset.abs() / row_h).ceil() as usize).saturating_add(1);
        let start = self.completion_scroll_offset.saturating_sub(overscan);
        let end = (self.completion_scroll_offset + visible + overscan).min(count);
        for absolute_ix in start..end {
            let display_ix =
                absolute_ix as isize - self.completion_scroll_offset as isize;
            let item_y = list_y + row_h * display_ix as f32 + list_scroll_offset;
            if item_y + row_h <= list_y || item_y >= list_bottom {
                continue;
            }
            let section = completion_section_label(&items[absolute_ix]);
            let selected = section.is_none() && absolute_ix == selected_ix;
            if selected {
                let selected_y = item_y + cursor_offset;
                let visible_y = selected_y.max(list_y);
                let visible_h = (selected_y + row_h).min(list_bottom) - visible_y;
                if visible_h > 0.0 {
                    sugarloaf.rounded_rect(
                        None,
                        list_x,
                        visible_y,
                        list_w,
                        visible_h,
                        theme.f32(theme.hover),
                        DEPTH,
                        4.0 * scale,
                        ORDER_STATUS_JOIN + 2,
                    );
                }
            }

            let label = section.unwrap_or_else(|| completion_label(&items[absolute_ix]));
            let row_opts = DrawOpts {
                font_size: if section.is_some() {
                    result_font * 0.78
                } else {
                    result_font
                },
                color: if section.is_some() {
                    theme.u8_alpha(theme.muted, 0.72)
                } else if selected {
                    theme.u8(theme.fg)
                } else {
                    theme.u8(theme.dim)
                },
                bold: section.is_some(),
                clip_rect: Some(list_clip),
                ..DrawOpts::default()
            };
            sugarloaf.text_mut().draw(
                if section.is_some() {
                    list_x + 8.0 * scale
                } else {
                    row_text_x
                },
                item_y + (row_h - result_font) / 2.0,
                label,
                &row_opts,
            );
        }

        if let Some(detail) = detail.filter(|detail| !detail.is_empty()) {
            let detail_y = list_y + list_h + 1.0 * scale;
            let detail_opts = DrawOpts {
                font_size: result_font * 0.82,
                color: theme.u8_alpha(theme.muted, 0.9),
                clip_rect: Some([list_x, detail_y, list_w, detail_h.max(row_h * 0.75)]),
                ..DrawOpts::default()
            };
            sugarloaf.text_mut().draw(
                list_x + 8.0 * scale,
                detail_y + (detail_h - result_font * 0.82) * 0.5,
                detail,
                &detail_opts,
            );
        }

        if count > visible {
            let normalized = self.completion_scroll_offset as f32
                / count.saturating_sub(visible).max(1) as f32;
            if let Some((thumb_y, thumb_h)) =
                scrollbar::compute_thumb(visible, count, list_y, list_h, normalized)
            {
                let opacity = scrollbar::opacity_from_last_scroll(
                    self.completion_last_scroll_time,
                    false,
                );
                let bar_x =
                    list_x + list_w - scrollbar::width() - scrollbar::SCROLLBAR_MARGIN;
                scrollbar::draw_track(
                    sugarloaf,
                    bar_x,
                    list_y,
                    list_h,
                    opacity,
                    DEPTH + 0.05,
                    ORDER_STATUS_JOIN + 3,
                );
                scrollbar::draw_thumb(
                    sugarloaf,
                    bar_x,
                    thumb_y,
                    thumb_h,
                    opacity,
                    false,
                    DEPTH + 0.05,
                    ORDER_STATUS_JOIN + 3,
                );
            }
        }
    }
}
