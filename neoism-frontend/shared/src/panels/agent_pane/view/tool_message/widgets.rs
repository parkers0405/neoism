use super::*;
use crate::primitives::draw_icon_centered_with_occlusion;

pub fn draw_checkbox(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    state: TodoVisualState,
    theme: &IdeTheme,
    s: f32,
    viewport_clip: [f32; 4],
) {
    let size = 15.0 * s;
    // Box outline stays in muted/border color regardless of state — the
    // inner check/dot mirrors the terminal chat todo row styling.
    let outline = theme.muted;
    draw_rect_clipped(
        sugarloaf,
        [x, y, size, 1.0 * s],
        theme.f32(outline),
        ORDER_TEXT,
        viewport_clip,
    );
    draw_rect_clipped(
        sugarloaf,
        [x, y + size, size, 1.0 * s],
        theme.f32(outline),
        ORDER_TEXT,
        viewport_clip,
    );
    draw_rect_clipped(
        sugarloaf,
        [x, y, 1.0 * s, size],
        theme.f32(outline),
        ORDER_TEXT,
        viewport_clip,
    );
    draw_rect_clipped(
        sugarloaf,
        [x + size, y, 1.0 * s, size + 1.0 * s],
        theme.f32(outline),
        ORDER_TEXT,
        viewport_clip,
    );
    match state {
        TodoVisualState::Completed => {
            let stroke = 1.0 * s;
            let font_size = 12.0 * s;
            let opts = DrawOpts {
                font_size,
                color: theme.u8(theme.green),
                bold: true,
                clip_rect: Some(viewport_clip),
                ..DrawOpts::default()
            };
            let glyph = "✓";
            let inner_size = (size - stroke).max(0.0);
            draw_icon_centered_with_occlusion(
                sugarloaf,
                x + stroke,
                [x + stroke, y + stroke, inner_size, inner_size],
                glyph,
                &opts,
                &[],
                true,
            );
        }
        TodoVisualState::InProgress => {
            let dot = size - 9.0 * s;
            let dot_pos = (size - dot) * 0.5 + 0.5 * s;
            draw_status_dot_text(
                sugarloaf,
                x + dot_pos,
                y + dot_pos,
                dot,
                theme.u8(theme.yellow),
                Some((theme.u8(theme.yellow), 0.35)),
                viewport_clip,
                &[],
                s,
            );
        }
        TodoVisualState::Pending => {}
    }
}

/// Curved connector glyph drawn to the left of each tool sub-line.
/// Uses the same rounded branch as subagent rows.
pub fn draw_tool_connector(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    _is_last: bool,
    opts: &DrawOpts,
    occlusion_rects: &[[f32; 4]],
) {
    draw_text_clipped(sugarloaf, x, y, "╰─", opts, occlusion_rects);
}

pub fn draw_tool_title(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    title: &str,
    opts: &DrawOpts,
    theme: &IdeTheme,
    occlusion_rects: &[[f32; 4]],
) {
    let Some(open) = title.find('(') else {
        draw_text_clipped(sugarloaf, x, y, title, opts, occlusion_rects);
        return;
    };
    let (name, rest) = title.split_at(open);
    draw_text_clipped(sugarloaf, x, y, name, opts, occlusion_rects);
    let mut rest_opts = *opts;
    rest_opts.bold = false;
    rest_opts.color = theme.u8(theme.fg);
    let name_w = sugarloaf.text_mut().measure(name, opts);
    draw_text_clipped(sugarloaf, x + name_w, y, rest, &rest_opts, occlusion_rects);
}
