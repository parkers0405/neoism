use super::*;

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
            let font_size = 12.0 * s;
            let opts = DrawOpts {
                font_size,
                color: theme.u8(theme.green),
                bold: true,
                clip_rect: Some(viewport_clip),
                ..DrawOpts::default()
            };
            let glyph = "✓";
            let glyph_w = sugarloaf.text_mut().measure(glyph, &opts);
            let cx = x + (size - glyph_w) * 0.5;
            let cy = y + (size - font_size) * 0.5 - 1.0 * s;
            sugarloaf.text_mut().draw(cx, cy, glyph, &opts);
        }
        TodoVisualState::InProgress => {
            // Smaller in-progress bullet (6px). The box's right/bottom
            // borders sit at +size while the left/top sit at the edge, so
            // the bordered box's visual center is +0.5px from the `size`
            // square center — add that so the dot sits dead-center instead
            // of a touch up-and-left.
            let dot = size - 9.0 * s;
            let dot_pos = (size - dot) * 0.5 + 0.5 * s;
            draw_rounded_rect_clipped(
                sugarloaf,
                [x + dot_pos, y + dot_pos, dot, dot],
                theme.f32(theme.yellow),
                dot * 0.5,
                ORDER_TEXT + 1,
                viewport_clip,
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
