use super::*;

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_mermaid_block<P: AgentMarkdownPane>(
    sugarloaf: &mut Sugarloaf,
    pane: &mut P,
    lines: &[String],
    diagram: Option<&MermaidDiagram>,
    key: u64,
    copy_target: &str,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    theme: &IdeTheme,
    s: f32,
    suppress_interactions: bool,
    viewport_clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) {
    if !rects_intersect([x, y, w, h], viewport_clip) {
        return;
    }

    draw_rounded_rect_clipped(
        sugarloaf,
        [x, y, w, h],
        theme.f32(theme.panel_bg()),
        10.0 * s,
        ORDER_PANEL,
        viewport_clip,
    );
    let header_h = 34.0 * s;
    let Some(header_opts) = opts_with_clip(
        DrawOpts {
            font_size: 11.5 * s,
            color: theme.u8_alpha(theme.white, 0.68),
            ..DrawOpts::default()
        },
        viewport_clip,
    ) else {
        return;
    };
    draw_text_clipped(
        sugarloaf,
        x + 14.0 * s,
        y + 11.0 * s,
        "mermaid",
        &header_opts,
        occlusion_rects,
    );

    let toggle_target = format!("{MERMAID_TOGGLE_LINK_PREFIX}{key:016x}");
    let raw_mode = pane.mermaid_raw_mode(key) || diagram.is_none();
    let render_label = if raw_mode {
        if !suppress_interactions && pane.link_hovered(&toggle_target) {
            "Render diagram"
        } else {
            "Render"
        }
    } else if !suppress_interactions && pane.link_hovered(&toggle_target) {
        "Raw source"
    } else {
        "Raw"
    };
    let copy_label = if !suppress_interactions && pane.link_hovered(&copy_target) {
        "Copy diagram"
    } else {
        "Copy"
    };
    let raw_w = measure_text_cached(sugarloaf, render_label, &header_opts);
    let copy_w = measure_text_cached(sugarloaf, copy_label, &header_opts);
    let raw_x = (x + w - raw_w - 14.0 * s).max(x + 14.0 * s);
    let copy_x = (raw_x - copy_w - 16.0 * s).max(x + 14.0 * s);
    draw_text_clipped(
        sugarloaf,
        copy_x,
        y + 11.0 * s,
        copy_label,
        &header_opts,
        occlusion_rects,
    );
    draw_text_clipped(
        sugarloaf,
        raw_x,
        y + 11.0 * s,
        render_label,
        &header_opts,
        occlusion_rects,
    );
    if !suppress_interactions {
        pane.register_link_hit_rect(
            copy_target.to_string(),
            [copy_x - 6.0 * s, y, copy_w + 12.0 * s, header_h],
        );
        pane.register_link_hit_rect(
            toggle_target,
            [raw_x - 6.0 * s, y, raw_w + 12.0 * s, header_h],
        );
    }

    draw_rect_clipped(
        sugarloaf,
        [x + 12.0 * s, y + header_h, w - 24.0 * s, 1.0 * s],
        theme.f32_alpha(theme.white, 0.08),
        ORDER_PANEL + 1,
        viewport_clip,
    );
    if raw_mode {
        render_mermaid_raw_body(
            sugarloaf,
            x,
            y,
            w,
            h,
            lines,
            theme,
            s,
            viewport_clip,
            occlusion_rects,
        );
        return;
    }
    let diagram_rect = [
        x + 10.0 * s,
        y + header_h + 8.0 * s,
        w - 20.0 * s,
        h - header_h - 16.0 * s,
    ];
    let Some(diagram_clip) = intersect_rects(diagram_rect, viewport_clip) else {
        return;
    };
    let scene = mermaid_scene(diagram.expect("checked above"), theme, s);
    let Some(bounds) = scene.bounds() else {
        return;
    };
    let pad = 12.0 * s;
    let avail_w = (diagram_rect[2] - pad * 2.0).max(1.0);
    let avail_h = (diagram_rect[3] - pad * 2.0).max(1.0);
    let zoom = (avail_w / bounds.width().max(1.0))
        .min(avail_h / bounds.height().max(1.0))
        .min(2.0);
    let center = bounds.center();
    let screen_cx = diagram_rect[0] + diagram_rect[2] * 0.5;
    let screen_cy = diagram_rect[1] + diagram_rect[3] * 0.5;
    let camera = Camera {
        pan: Vec2::new(screen_cx - center.x * zoom, screen_cy - center.y * zoom),
        zoom,
    };
    render_scene(
        sugarloaf,
        &scene,
        &camera,
        diagram_clip,
        super::super::DEPTH,
        ORDER_PANEL + 2,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_mermaid_raw_body(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    lines: &[String],
    theme: &IdeTheme,
    s: f32,
    viewport_clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) {
    let header_h = 34.0 * s;
    let body_top = y + header_h + 8.0 * s;
    let line_h = 18.0 * s;
    let clip_top = viewport_clip[1];
    let clip_bottom = viewport_clip[1] + viewport_clip[3];
    let line_count = lines.len().max(1);
    let start_ix = ((clip_top - body_top - line_h) / line_h).floor().max(0.0) as usize;
    let end_ix = ((clip_bottom - body_top + line_h) / line_h).ceil().max(0.0) as usize;
    let start_ix = start_ix.min(line_count);
    let end_ix = end_ix.min(line_count);
    let Some(opts) = opts_with_clip(
        DrawOpts {
            font_size: 12.5 * s,
            color: theme.u8(theme.fg),
            bold: true,
            clip_rect: Some([
                x + 18.0 * s,
                body_top,
                (w - 36.0 * s).max(0.0),
                (h - header_h - 12.0 * s).max(0.0),
            ]),
            ..DrawOpts::default()
        },
        viewport_clip,
    ) else {
        return;
    };
    let empty_line = String::new();
    for ix in start_ix..end_ix {
        let line = if lines.is_empty() {
            &empty_line
        } else if let Some(line) = lines.get(ix) {
            line
        } else {
            break;
        };
        draw_text_clipped(
            sugarloaf,
            x + 18.0 * s,
            body_top + ix as f32 * line_h,
            line,
            &opts,
            occlusion_rects,
        );
    }
}

fn intersect_rects(a: [f32; 4], b: [f32; 4]) -> Option<[f32; 4]> {
    let x1 = a[0].max(b[0]);
    let y1 = a[1].max(b[1]);
    let x2 = (a[0] + a[2]).min(b[0] + b[2]);
    let y2 = (a[1] + a[3]).min(b[1] + b[3]);
    (x2 > x1 && y2 > y1).then_some([x1, y1, x2 - x1, y2 - y1])
}

fn rects_intersect(a: [f32; 4], b: [f32; 4]) -> bool {
    let (ax1, ay1, ax2, ay2) = (a[0], a[1], a[0] + a[2], a[1] + a[3]);
    let (bx1, by1, bx2, by2) = (b[0], b[1], b[0] + b[2], b[1] + b[3]);
    ax1 < bx2 && ax2 > bx1 && ay1 < by2 && ay2 > by1
}
