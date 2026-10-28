use sugarloaf::text::DrawOpts;

use crate::editor::markdown::MarkdownPane;
use crate::editor::neodraw::{render_scene, Camera, Vec2};

use super::types::{
    MermaidDiagram, MermaidDirection, MermaidEdge, MermaidNode, MermaidNodeShape, DEPTH,
    ORDER_BG,
};
use crate::editor::markdown::render::draw::{
    cursor_cell_width, draw_block_chrome, draw_copy_button, draw_if_visible,
    draw_rect_clipped, draw_rounded_rect_clipped, line_height, markdown_font,
    point_in_rect, wrap_lines,
};
use crate::primitives::ide_theme::IdeTheme;
use crate::widgets::mermaid::{
    mermaid_scene, parse_mermaid_diagram as parse_shared_mermaid_diagram,
};

#[allow(dead_code)]
pub(super) fn parse_mermaid_diagram(source: &str) -> Option<MermaidDiagram> {
    let mut direction = MermaidDirection::TopDown;
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut saw_header = false;

    for raw in source.lines() {
        let line = raw
            .split("%%")
            .next()
            .unwrap_or("")
            .trim()
            .trim_end_matches(';')
            .trim();
        if line.is_empty() {
            continue;
        }

        let lower = line.to_ascii_lowercase();
        if lower.starts_with("graph ") || lower.starts_with("flowchart ") {
            saw_header = true;
            if let Some(token) = line.split_whitespace().nth(1) {
                direction = match token.to_ascii_uppercase().as_str() {
                    "LR" | "RL" => MermaidDirection::LeftRight,
                    _ => MermaidDirection::TopDown,
                };
            }
            continue;
        }
        if lower == "end"
            || lower.starts_with("subgraph ")
            || lower.starts_with("classdef ")
        {
            continue;
        }

        if let Some((from, to, label)) = parse_mermaid_edge(line) {
            let from_id = from.id.clone();
            let to_id = to.id.clone();
            add_mermaid_node(&mut nodes, from);
            add_mermaid_node(&mut nodes, to);
            edges.push(MermaidEdge {
                from: from_id,
                to: to_id,
                label,
            });
        } else if let Some(node) = parse_mermaid_node_ref(line) {
            add_mermaid_node(&mut nodes, node);
        }
    }

    (saw_header && !nodes.is_empty()).then_some(MermaidDiagram {
        direction,
        nodes,
        edges,
    })
}

#[allow(dead_code)]
pub(super) fn parse_mermaid_edge(
    line: &str,
) -> Option<(MermaidNode, MermaidNode, Option<String>)> {
    for op in ["-.->", "==>", "-->", "---"] {
        let Some(op_ix) = line.find(op) else {
            continue;
        };
        let mut from_expr = line[..op_ix].trim();
        let mut label = None;
        if let Some((left, maybe_label)) = from_expr.rsplit_once("--") {
            let maybe_label = maybe_label.trim();
            if !maybe_label.is_empty() {
                from_expr = left.trim();
                label = Some(strip_mermaid_label(maybe_label));
            }
        }

        let mut to_expr = line[op_ix + op.len()..].trim();
        if let Some(rest) = to_expr.strip_prefix('|') {
            if let Some(end) = rest.find('|') {
                label = Some(strip_mermaid_label(&rest[..end]));
                to_expr = rest[end + 1..].trim();
            }
        }

        let from = parse_mermaid_node_ref(from_expr)?;
        let to = parse_mermaid_node_ref(to_expr)?;
        return Some((from, to, label.filter(|text| !text.is_empty())));
    }
    None
}

#[allow(dead_code)]
pub(super) fn parse_mermaid_node_ref(expr: &str) -> Option<MermaidNode> {
    let expr = expr.trim().trim_end_matches(';').trim();
    if expr.is_empty() {
        return None;
    }

    for (open, close, shape) in [
        ("[[", "]]", MermaidNodeShape::Rect),
        ("{{", "}}", MermaidNodeShape::Decision),
        ("((", "))", MermaidNodeShape::Round),
        ("[", "]", MermaidNodeShape::Rect),
        ("{", "}", MermaidNodeShape::Decision),
        ("(", ")", MermaidNodeShape::Round),
    ] {
        let Some(open_ix) = expr.find(open) else {
            continue;
        };
        let id = clean_mermaid_id(&expr[..open_ix])?;
        let label_start = open_ix + open.len();
        let label_end = expr[label_start..].find(close)? + label_start;
        let label = strip_mermaid_label(&expr[label_start..label_end]);
        return Some(MermaidNode { id, label, shape });
    }

    let bare = expr.split_whitespace().next().and_then(clean_mermaid_id)?;
    Some(MermaidNode {
        label: bare.clone(),
        id: bare,
        shape: MermaidNodeShape::Rect,
    })
}

#[allow(dead_code)]
pub(super) fn clean_mermaid_id(value: &str) -> Option<String> {
    let value = value.split(":::").next().unwrap_or(value).trim();
    let value = value.trim_matches(|ch: char| !is_mermaid_id_char(ch));
    (!value.is_empty()).then(|| value.to_string())
}

#[allow(dead_code)]
pub(super) fn is_mermaid_id_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':' | '/')
}

#[allow(dead_code)]
pub(super) fn strip_mermaid_label(value: &str) -> String {
    let value = value.trim().trim_matches('/').trim();
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if matches!(
            (bytes[0], bytes[value.len() - 1]),
            (b'"', b'"') | (b'\'', b'\'')
        ) {
            return value[1..value.len() - 1].trim().to_string();
        }
    }
    value.to_string()
}

#[allow(dead_code)]
pub(super) fn add_mermaid_node(nodes: &mut Vec<MermaidNode>, node: MermaidNode) {
    if let Some(existing) = nodes.iter_mut().find(|existing| existing.id == node.id) {
        if existing.label == existing.id && node.label != node.id {
            existing.label = node.label;
            existing.shape = node.shape;
        }
        return;
    }
    nodes.push(node);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_mermaid_block(
    sugarloaf: &mut (impl crate::primitives::paint::Paint + ?Sized),
    pane: &mut MarkdownPane,
    start_line: usize,
    code_end: usize,
    source: &str,
    content_x: f32,
    cursor_y: f32,
    content_w: f32,
    pane_clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
    theme: &IdeTheme,
    mouse: Option<[f32; 2]>,
    text_occlusions: &[[f32; 4]],
    font_scale: f32,
) -> Option<f32> {
    let diagram = parse_shared_mermaid_diagram(source)?;
    let title_opts = DrawOpts {
        font_size: markdown_font(13.0, font_scale),
        color: theme.u8_alpha(theme.muted, 0.95),
        bold: true,
        clip_rect: Some(pane_clip),
        ..DrawOpts::default()
    };
    let scene = mermaid_scene(&diagram, theme, font_scale);
    let bounds = scene.bounds()?;
    let diagram_h = (220.0 * font_scale).max(180.0);
    let block_h = diagram_h + 66.0;
    let block_rect = [content_x - 18.0, cursor_y - 8.0, content_w + 36.0, block_h];
    let handle_rect = [block_rect[0] - 36.0, block_rect[1], 34.0, block_rect[3]];
    let dragging = pane.dragging_line == Some(start_line);
    let node_opts = DrawOpts {
        font_size: markdown_font(14.0, font_scale),
        color: theme.u8(theme.fg),
        bold: true,
        clip_rect: Some(pane_clip),
        ..DrawOpts::default()
    };
    let active = pane.register_block_rect(
        start_line,
        block_rect,
        handle_rect,
        content_x,
        cursor_y + 54.0,
        0,
        cursor_cell_width(&node_opts),
        line_height(&node_opts),
        content_w,
        mouse,
    );
    draw_block_chrome(
        sugarloaf,
        block_rect[0],
        block_rect[1],
        block_rect[2],
        block_rect[3],
        theme,
        pane_clip,
        clip_top,
        clip_bottom,
        active,
        active || dragging,
        dragging,
    );

    if cursor_y + block_h >= clip_top && cursor_y <= clip_bottom {
        draw_rect_clipped(
            sugarloaf,
            pane_clip,
            content_x - 10.0,
            cursor_y + 16.0,
            4.0,
            block_h - 34.0,
            theme.f32_alpha(theme.accent, 0.72),
            DEPTH,
            ORDER_BG + 2,
        );
        let copy_rect = [content_x + content_w - 30.0, cursor_y + 6.0, 24.0, 24.0];
        pane.register_copy_code_rect(copy_rect, start_line, code_end);
        draw_copy_button(sugarloaf, copy_rect, theme, pane_clip, font_scale);

        let summary = format!(
            "Mermaid flowchart · {} nodes · {} edges",
            diagram.nodes.len(),
            diagram.edges.len()
        );
        draw_if_visible(
            sugarloaf,
            content_x,
            cursor_y + 12.0,
            &summary,
            &title_opts,
            clip_top,
            clip_bottom,
            text_occlusions,
        );

        let diagram_rect = [content_x, cursor_y + 48.0, content_w, diagram_h];
        let pad = 12.0 * font_scale;
        let avail_w = (diagram_rect[2] - pad * 2.0).max(1.0);
        let avail_h = (diagram_rect[3] - pad * 2.0).max(1.0);
        let zoom = (avail_w / bounds.width().max(1.0))
            .min(avail_h / bounds.height().max(1.0))
            .min(2.0);
        let center = bounds.center();
        let camera = Camera {
            pan: Vec2::new(
                diagram_rect[0] + diagram_rect[2] * 0.5 - center.x * zoom,
                diagram_rect[1] + diagram_rect[3] * 0.5 - center.y * zoom,
            ),
            zoom,
        };
        render_scene(sugarloaf, &scene, &camera, pane_clip, DEPTH, ORDER_BG + 4);
    }

    Some(cursor_y + block_h + 14.0)
}

#[allow(dead_code)]
pub(super) fn layout_mermaid_nodes(
    diagram: &MermaidDiagram,
    content_x: f32,
    top: f32,
    content_w: f32,
    font_scale: f32,
) -> Vec<(String, [f32; 4])> {
    let node_h = (54.0 * font_scale.min(1.35)).max(46.0);
    match diagram.direction {
        MermaidDirection::TopDown => {
            let node_w = content_w.clamp(156.0, 260.0);
            let x = content_x + (content_w - node_w) * 0.5;
            let gap_y = (42.0 * font_scale.min(1.35)).max(32.0);
            diagram
                .nodes
                .iter()
                .enumerate()
                .map(|(ix, node)| {
                    (
                        node.id.clone(),
                        [x, top + ix as f32 * (node_h + gap_y), node_w, node_h],
                    )
                })
                .collect()
        }
        MermaidDirection::LeftRight => {
            let gap_x = 42.0;
            let gap_y = (40.0 * font_scale.min(1.35)).max(30.0);
            let node_w = (content_w * 0.32).clamp(136.0, 210.0).min(content_w);
            let cols = ((content_w + gap_x) / (node_w + gap_x)).floor().max(1.0) as usize;
            diagram
                .nodes
                .iter()
                .enumerate()
                .map(|(ix, node)| {
                    let col = ix % cols;
                    let row = ix / cols;
                    (
                        node.id.clone(),
                        [
                            content_x + col as f32 * (node_w + gap_x),
                            top + row as f32 * (node_h + gap_y),
                            node_w,
                            node_h,
                        ],
                    )
                })
                .collect()
        }
    }
}

#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
pub(super) fn draw_mermaid_node(
    sugarloaf: &mut (impl crate::primitives::paint::Paint + ?Sized),
    node: &MermaidNode,
    rect: [f32; 4],
    clip: [f32; 4],
    theme: &IdeTheme,
    opts: &DrawOpts,
    clip_top: f32,
    clip_bottom: f32,
    text_occlusions: &[[f32; 4]],
) {
    let fill = match node.shape {
        MermaidNodeShape::Decision => theme.f32_alpha(theme.accent, 0.24),
        MermaidNodeShape::Round => theme.f32_alpha(theme.hover, 0.86),
        MermaidNodeShape::Rect => theme.f32_alpha(theme.surface, 0.96),
    };
    let radius = match node.shape {
        MermaidNodeShape::Rect => 8.0,
        MermaidNodeShape::Round => 22.0,
        MermaidNodeShape::Decision => 10.0,
    };
    draw_rounded_rect_clipped(
        sugarloaf,
        clip,
        rect[0],
        rect[1],
        rect[2],
        rect[3],
        radius,
        fill,
        DEPTH,
        ORDER_BG + 6,
    );
    let border = if node.shape == MermaidNodeShape::Decision {
        theme.accent
    } else {
        theme.border
    };
    draw_mermaid_border(sugarloaf, rect, clip, theme.f32_alpha(border, 0.78));

    let mut lines = wrap_lines(sugarloaf, &node.label, rect[2] - 24.0, opts);
    if lines.len() > 2 {
        lines.truncate(2);
        if let Some(last) = lines.last_mut() {
            last.push_str("...");
        }
    }
    let line_h = line_height(opts);
    let total_h = line_h * lines.len().max(1) as f32;
    let mut text_y = rect[1] + (rect[3] - total_h) * 0.5;
    for line in lines {
        let text_w = sugarloaf.text_mut().measure(&line, opts);
        draw_if_visible(
            sugarloaf,
            rect[0] + (rect[2] - text_w) * 0.5,
            text_y,
            &line,
            opts,
            clip_top,
            clip_bottom,
            text_occlusions,
        );
        text_y += line_h;
    }
}

#[allow(dead_code)]
pub(super) fn draw_mermaid_border(
    sugarloaf: &mut (impl crate::primitives::paint::Paint + ?Sized),
    rect: [f32; 4],
    clip: [f32; 4],
    color: [f32; 4],
) {
    draw_rect_clipped(
        sugarloaf,
        clip,
        rect[0],
        rect[1],
        rect[2],
        1.0,
        color,
        DEPTH,
        ORDER_BG + 7,
    );
    draw_rect_clipped(
        sugarloaf,
        clip,
        rect[0],
        rect[1] + rect[3] - 1.0,
        rect[2],
        1.0,
        color,
        DEPTH,
        ORDER_BG + 7,
    );
    draw_rect_clipped(
        sugarloaf,
        clip,
        rect[0],
        rect[1],
        1.0,
        rect[3],
        color,
        DEPTH,
        ORDER_BG + 7,
    );
    draw_rect_clipped(
        sugarloaf,
        clip,
        rect[0] + rect[2] - 1.0,
        rect[1],
        1.0,
        rect[3],
        color,
        DEPTH,
        ORDER_BG + 7,
    );
}

#[allow(dead_code)]
pub(super) fn draw_mermaid_edge(
    sugarloaf: &mut (impl crate::primitives::paint::Paint + ?Sized),
    edge: &MermaidEdge,
    layout: &[(String, [f32; 4])],
    direction: MermaidDirection,
    clip: [f32; 4],
    theme: &IdeTheme,
    label_opts: &DrawOpts,
) {
    let Some(from) = layout
        .iter()
        .find(|(id, _)| id == &edge.from)
        .map(|(_, rect)| *rect)
    else {
        return;
    };
    let Some(to) = layout
        .iter()
        .find(|(id, _)| id == &edge.to)
        .map(|(_, rect)| *rect)
    else {
        return;
    };
    let color = theme.f32_alpha(theme.accent, 0.58);
    match direction {
        MermaidDirection::TopDown => {
            let start = [from[0] + from[2] * 0.5, from[1] + from[3]];
            let end = [to[0] + to[2] * 0.5, to[1]];
            let mid_y = start[1] + (end[1] - start[1]) * 0.5;
            draw_mermaid_segment(
                sugarloaf, clip, start[0], start[1], start[0], mid_y, color,
            );
            draw_mermaid_segment(sugarloaf, clip, start[0], mid_y, end[0], mid_y, color);
            draw_mermaid_segment(sugarloaf, clip, end[0], mid_y, end[0], end[1], color);
            draw_mermaid_arrowhead(
                sugarloaf,
                clip,
                end[0],
                end[1],
                0.0,
                end[1] - start[1],
                color,
            );
            draw_mermaid_edge_label(
                sugarloaf,
                edge,
                [start[0], mid_y],
                clip,
                label_opts,
                theme,
            );
        }
        MermaidDirection::LeftRight => {
            let start = [from[0] + from[2], from[1] + from[3] * 0.5];
            let end = [to[0], to[1] + to[3] * 0.5];
            let mid_x = start[0] + (end[0] - start[0]) * 0.5;
            draw_mermaid_segment(
                sugarloaf, clip, start[0], start[1], mid_x, start[1], color,
            );
            draw_mermaid_segment(sugarloaf, clip, mid_x, start[1], mid_x, end[1], color);
            draw_mermaid_segment(sugarloaf, clip, mid_x, end[1], end[0], end[1], color);
            draw_mermaid_arrowhead(
                sugarloaf,
                clip,
                end[0],
                end[1],
                end[0] - start[0],
                0.0,
                color,
            );
            draw_mermaid_edge_label(
                sugarloaf,
                edge,
                [mid_x, end[1]],
                clip,
                label_opts,
                theme,
            );
        }
    }
}

#[allow(dead_code)]
pub(super) fn draw_mermaid_segment(
    sugarloaf: &mut (impl crate::primitives::paint::Paint + ?Sized),
    clip: [f32; 4],
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    color: [f32; 4],
) {
    let thickness = 2.0;
    if (x1 - x2).abs() < 0.5 {
        draw_rect_clipped(
            sugarloaf,
            clip,
            x1 - thickness * 0.5,
            y1.min(y2),
            thickness,
            (y1 - y2).abs().max(thickness),
            color,
            DEPTH,
            ORDER_BG + 4,
        );
    } else if (y1 - y2).abs() < 0.5 {
        draw_rect_clipped(
            sugarloaf,
            clip,
            x1.min(x2),
            y1 - thickness * 0.5,
            (x1 - x2).abs().max(thickness),
            thickness,
            color,
            DEPTH,
            ORDER_BG + 4,
        );
    }
}

#[allow(dead_code)]
pub(super) fn draw_mermaid_arrowhead(
    sugarloaf: &mut (impl crate::primitives::paint::Paint + ?Sized),
    clip: [f32; 4],
    x: f32,
    y: f32,
    dx: f32,
    dy: f32,
    color: [f32; 4],
) {
    if !point_in_rect(x, y, clip) {
        return;
    }
    let size = 7.0;
    if dx.abs() > dy.abs() {
        if dx >= 0.0 {
            sugarloaf.triangle(
                x,
                y,
                x - size,
                y - size * 0.65,
                x - size,
                y + size * 0.65,
                DEPTH,
                color,
            );
        } else {
            sugarloaf.triangle(
                x,
                y,
                x + size,
                y - size * 0.65,
                x + size,
                y + size * 0.65,
                DEPTH,
                color,
            );
        }
    } else if dy >= 0.0 {
        sugarloaf.triangle(
            x,
            y,
            x - size * 0.65,
            y - size,
            x + size * 0.65,
            y - size,
            DEPTH,
            color,
        );
    } else {
        sugarloaf.triangle(
            x,
            y,
            x - size * 0.65,
            y + size,
            x + size * 0.65,
            y + size,
            DEPTH,
            color,
        );
    }
}

#[allow(dead_code)]
pub(super) fn draw_mermaid_edge_label(
    sugarloaf: &mut (impl crate::primitives::paint::Paint + ?Sized),
    edge: &MermaidEdge,
    center: [f32; 2],
    clip: [f32; 4],
    opts: &DrawOpts,
    theme: &IdeTheme,
) {
    let Some(label) = edge.label.as_ref() else {
        return;
    };
    let text_w = sugarloaf.text_mut().measure(label, opts);
    let pad = 5.0;
    let x = center[0] - text_w * 0.5 - pad;
    let y = center[1] - opts.font_size * 0.75;
    draw_rounded_rect_clipped(
        sugarloaf,
        clip,
        x,
        y,
        text_w + pad * 2.0,
        opts.font_size + 8.0,
        5.0,
        theme.f32_alpha(theme.bg, 0.88),
        DEPTH,
        ORDER_BG + 5,
    );
    sugarloaf.text_mut().draw(x + pad, y + 3.0, label, opts);
}
