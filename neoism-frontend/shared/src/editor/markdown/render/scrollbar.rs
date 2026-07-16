use sugarloaf::Sugarloaf;

use crate::editor::markdown::MarkdownPane;

use super::types::{
    DEPTH, MARKDOWN_SCROLLBAR_MARGIN, MARKDOWN_SCROLLBAR_MIN_THUMB_HEIGHT,
    MARKDOWN_SCROLLBAR_WIDTH, ORDER_TEXT,
};
use crate::editor::markdown::render::draw::draw_rounded_rect_clipped;
use crate::primitives::ide_theme::IdeTheme;
use crate::primitives::look::scrollbar_style;

pub(super) fn draw_markdown_scrollbar(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    rect: [f32; 4],
    content_height: f32,
    theme: &IdeTheme,
    mouse: Option<[f32; 2]>,
    clip: [f32; 4],
) {
    let [x, y, w, h] = rect;
    // Mash Up Pack scrollbar restyle: width / min-thumb / radius /
    // colors route through the active `ScrollbarStyle`, with this
    // site's rounded themed bar as the per-slot default — an empty
    // style renders today's bar byte-identically.
    let style = scrollbar_style();
    let bar_w = style.width_or(MARKDOWN_SCROLLBAR_WIDTH).max(1.0);
    let min_thumb = style.min_thumb_or(MARKDOWN_SCROLLBAR_MIN_THUMB_HEIGHT);
    if content_height <= h + 1.0 || h <= min_thumb {
        return;
    }
    let track_h = (h - MARKDOWN_SCROLLBAR_MARGIN * 2.0).max(1.0);
    let max_scroll = (content_height - h).max(1.0);
    let thumb_h = (track_h * (h / content_height)).clamp(min_thumb.min(track_h), track_h);
    let progress = (pane.scroll_y / max_scroll).clamp(0.0, 1.0);
    let thumb_y = y + MARKDOWN_SCROLLBAR_MARGIN + (track_h - thumb_h) * progress;
    let track_rect = [
        x + w - bar_w - MARKDOWN_SCROLLBAR_MARGIN,
        y + MARKDOWN_SCROLLBAR_MARGIN,
        bar_w,
        track_h,
    ];
    let thumb_rect = [track_rect[0], thumb_y, bar_w, thumb_h];
    pane.register_scrollbar_rect(track_rect, thumb_rect, h, mouse);

    let hovered = mouse.is_some_and(|[mx, my]| {
        mx >= track_rect[0] - 5.0
            && mx <= track_rect[0] + track_rect[2] + 5.0
            && my >= track_rect[1]
            && my <= track_rect[1] + track_rect[3]
    });
    let radius = style.radius(bar_w, 0.5);
    // This site already draws a track, so its themed color is the
    // `track_or` default; overrides keep the site's hover-driven alpha.
    if let Some(track_color) = style.track_or(Some(
        theme.f32_alpha(theme.border, if hovered { 0.22 } else { 0.12 }),
    )) {
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
            ORDER_TEXT + 2,
        );
    }
    // No discrete drag state at this site — the hover emphasis doubles
    // as the drag look, so `thumb_drag` maps to the hovered color.
    let thumb_color = if hovered {
        style.thumb_drag_or(theme.f32_alpha(theme.fg, 0.62))
    } else {
        style.thumb_or(theme.f32_alpha(theme.fg, 0.44))
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
        ORDER_TEXT + 3,
    );
}
