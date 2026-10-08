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
    draw_checkbox_painted(
        sugarloaf,
        x,
        y,
        state,
        &super::motion::PaintTheme {
            theme,
            opacity: 1.0,
        },
        s,
        viewport_clip,
    );
}

pub(super) fn draw_checkbox_painted(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    state: TodoVisualState,
    theme: &super::motion::PaintTheme<'_>,
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

fn tool_symbol(tool: &str) -> &'static str {
    match tool.to_ascii_lowercase().as_str() {
        "bash" | "shell" | "terminal" | "exec" => "\u{f120}",
        "read" | "read_file" | "readfile" | "view" | "cat" => "\u{2192}",
        "grep" | "glob" | "search" | "find" | "rg" | "tool_group" => "\u{f002}",
        "edit" | "write" | "apply_patch" | "applypatch" | "patch" | "multiedit" => {
            "\u{f044}"
        }
        "webfetch" | "websearch" => "\u{f0ac}",
        "task" => "\u{f0e8}",
        "todowrite" | "todoread" => "\u{f0ae}",
        _ => "\u{f013}",
    }
}

pub(super) fn draw_tool_symbol(
    sugarloaf: &mut Sugarloaf,
    rect: [f32; 4],
    tool: &str,
    opts: &DrawOpts,
    occlusion_rects: &[[f32; 4]],
) {
    draw_icon_centered_with_occlusion(
        sugarloaf,
        rect[0],
        rect,
        tool_symbol(tool),
        opts,
        occlusion_rects,
        true,
    );
}

/// One curved connector anchors the entire expanded details area.
pub fn draw_tool_connector(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    opts: &DrawOpts,
    occlusion_rects: &[[f32; 4]],
) {
    draw_text_clipped(sugarloaf, x, y, "╰─", opts, occlusion_rects);
}

#[cfg(test)]
mod tests {
    use super::tool_symbol;

    #[test]
    fn tools_use_distinct_symbols_and_mcp_tools_use_a_gear() {
        assert_eq!(tool_symbol("Read"), "\u{2192}");
        assert_eq!(tool_symbol("Bash"), "\u{f120}");
        assert_eq!(tool_symbol("grep"), "\u{f002}");
        assert_eq!(tool_symbol("apply_patch"), "\u{f044}");
        assert_eq!(tool_symbol("fff_find_files"), "\u{f013}");
        assert_eq!(tool_symbol(""), "\u{f013}");
    }
}
