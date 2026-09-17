use std::collections::BTreeMap;

use crate::editor::neodraw::{
    ArrowHead, Color, Scene, Shape, ShapeId, ShapeKind, Style, Vec2,
};
use crate::primitives::ide_theme::IdeTheme;

const MIN_HEIGHT: f32 = 190.0;
const PAD: f32 = 18.0;
const NODE_W: f32 = 132.0;
const NODE_H: f32 = 42.0;
const GAP_X: f32 = 72.0;
const GAP_Y: f32 = 42.0;
const TEXT_SIZE: f32 = 14.0;
const EDGE_LABEL_SIZE: f32 = 12.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MermaidDirection {
    TopDown,
    LeftRight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MermaidNodeShape {
    Rect,
    Round,
    Circle,
    Diamond,
}

#[derive(Clone, Debug)]
pub struct MermaidNode {
    pub id: String,
    pub label: String,
    pub shape: MermaidNodeShape,
}

#[derive(Clone, Debug)]
pub struct MermaidEdge {
    pub from: String,
    pub to: String,
    pub label: Option<String>,
}

#[derive(Clone, Debug)]
pub struct MermaidDiagram {
    pub direction: MermaidDirection,
    pub nodes: Vec<MermaidNode>,
    pub edges: Vec<MermaidEdge>,
}

#[derive(Clone, Copy, Debug)]
pub struct MermaidLayout {
    pub width: f32,
    pub height: f32,
}

pub fn parse_mermaid_diagram(source: &str) -> Option<MermaidDiagram> {
    let mut direction = MermaidDirection::TopDown;
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut saw_header = false;

    for raw in source.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        if !saw_header {
            let lower = line.to_ascii_lowercase();
            if lower.starts_with("graph") || lower.starts_with("flowchart") {
                saw_header = true;
                let dir = line.split_whitespace().nth(1).unwrap_or("TD");
                direction = match dir.to_ascii_uppercase().as_str() {
                    "LR" | "RL" => MermaidDirection::LeftRight,
                    _ => MermaidDirection::TopDown,
                };
                continue;
            }
            return None;
        }
        if let Some(edge) = parse_edge(line, &mut nodes) {
            edges.push(edge);
        } else if let Some(node) = parse_node(line) {
            upsert_node(&mut nodes, node);
        }
    }

    (saw_header && !nodes.is_empty()).then_some(MermaidDiagram {
        direction,
        nodes,
        edges,
    })
}

pub fn measure_mermaid_diagram(
    diagram: &MermaidDiagram,
    width: f32,
    scale: f32,
) -> MermaidLayout {
    let content = content_size(diagram, scale);
    let pad = PAD * scale;
    MermaidLayout {
        width: width.max(content[0] + pad * 2.0),
        height: (content[1] + pad * 2.0).max(MIN_HEIGHT * scale),
    }
}

pub fn mermaid_scene(diagram: &MermaidDiagram, theme: &IdeTheme, scale: f32) -> Scene {
    let mut scene = Scene::default();
    let positions = node_positions(diagram, scale);
    let edge_style = Style {
        stroke: color_from_u8(theme.u8(theme.muted)),
        fill: None,
        width: 1.8 * scale,
        roughness: 0.55,
        seed: 41,
        opacity: 0.72,
    };
    let node_style = Style {
        stroke: color_from_u8(theme.u8(theme.accent)),
        fill: Some(color_from_u8(theme.u8(theme.surface))),
        width: 1.7 * scale,
        roughness: 0.5,
        seed: 91,
        opacity: 0.9,
    };
    let text_style = Style {
        stroke: color_from_u8(theme.u8(theme.fg)),
        fill: None,
        width: 1.0,
        roughness: 0.0,
        seed: 7,
        opacity: 1.0,
    };

    let mut next_id = 1u64;
    for edge in &diagram.edges {
        let Some(from) = positions.get(&edge.from) else {
            continue;
        };
        let Some(to) = positions.get(&edge.to) else {
            continue;
        };
        let points = edge_points(*from, *to, diagram.direction);
        scene.shapes.push(shape(
            &mut next_id,
            ShapeKind::Arrow {
                points: points.clone(),
                head: ArrowHead::Triangle,
            },
            edge_style.clone(),
        ));
        if let Some(label) = edge.label.as_ref().filter(|label| !label.trim().is_empty())
        {
            let midpoint = polyline_midpoint(&points);
            scene.shapes.push(shape(
                &mut next_id,
                ShapeKind::Text {
                width: None,
                    x: midpoint.x
                        - estimated_text_width(label, EDGE_LABEL_SIZE * scale) * 0.5,
                    y: midpoint.y - EDGE_LABEL_SIZE * scale * 0.75,
                    content: label.clone(),
                    size: EDGE_LABEL_SIZE * scale,
                },
                text_style.clone(),
            ));
        }
    }

    for node in &diagram.nodes {
        let Some(rect) = positions.get(&node.id).copied() else {
            continue;
        };
        match node.shape {
            MermaidNodeShape::Rect
            | MermaidNodeShape::Round
            | MermaidNodeShape::Circle => scene.shapes.push(shape(
                &mut next_id,
                if node.shape == MermaidNodeShape::Circle {
                    ShapeKind::Ellipse {
                        x: rect[0] + (rect[2] - rect[3]) * 0.5,
                        y: rect[1],
                        w: rect[3],
                        h: rect[3],
                    }
                } else {
                    ShapeKind::Rect {
                        x: rect[0],
                        y: rect[1],
                        w: rect[2],
                        h: rect[3],
                        corner: if node.shape == MermaidNodeShape::Round {
                            rect[3] * 0.5
                        } else {
                            10.0 * scale
                        },
                    }
                },
                node_style.clone(),
            )),
            MermaidNodeShape::Diamond => {
                let cx = rect[0] + rect[2] * 0.5;
                let cy = rect[1] + rect[3] * 0.5;
                scene.shapes.push(shape(
                    &mut next_id,
                    ShapeKind::Polygon {
                        points: vec![
                            Vec2::new(cx, rect[1]),
                            Vec2::new(rect[0] + rect[2], cy),
                            Vec2::new(cx, rect[1] + rect[3]),
                            Vec2::new(rect[0], cy),
                        ],
                    },
                    node_style.clone(),
                ));
            }
        }
        scene.shapes.push(shape(
            &mut next_id,
            ShapeKind::Text {
                width: None,
                x: rect[0]
                    + (rect[2] - estimated_text_width(&node.label, TEXT_SIZE * scale))
                        * 0.5,
                y: rect[1] + (rect[3] - TEXT_SIZE * scale * 1.25) * 0.5,
                content: node.label.clone(),
                size: TEXT_SIZE * scale,
            },
            text_style.clone(),
        ));
    }

    scene
}

fn parse_edge(line: &str, nodes: &mut Vec<MermaidNode>) -> Option<MermaidEdge> {
    let normalized = line.replace('→', "-->");
    let line = normalized.as_str();
    let (left, marker, right) = split_edge(line)?;
    let (from_id, from_node) = parse_node_ref(left.trim())?;
    let (label, right_ref) = parse_edge_label(marker, right.trim());
    let (to_id, to_node) = parse_node_ref(right_ref.trim())?;
    upsert_node(nodes, from_node);
    upsert_node(nodes, to_node);
    Some(MermaidEdge {
        from: from_id,
        to: to_id,
        label: label.filter(|label| !label.is_empty()),
    })
}

fn split_edge(line: &str) -> Option<(&str, &str, &str)> {
    if let Some((left, rest)) = line.split_once("--") {
        if let Some((label, right)) = rest.split_once("-->") {
            return Some((left, label, right));
        }
        if let Some((label, right)) = rest.split_once("---") {
            return Some((left, label, right));
        }
    }
    if let Some((left, right)) = line.split_once("-->") {
        return Some((left, "-->", right));
    }
    if let Some((left, right)) = line.split_once("---") {
        return Some((left, "---", right));
    }
    None
}

fn parse_edge_label<'a>(marker: &str, input: &'a str) -> (Option<String>, &'a str) {
    if let Some(rest) = input.strip_prefix('|') {
        if let Some(end) = rest.find('|') {
            return (Some(rest[..end].trim().to_string()), rest[end + 1..].trim());
        }
    }
    if marker != "-->" && marker != "---" {
        let label = marker.trim().trim_matches('-').trim().to_string();
        return ((!label.is_empty()).then_some(label), input);
    }
    (None, input)
}

fn parse_node(line: &str) -> Option<MermaidNode> {
    parse_node_ref(line).map(|(_, node)| node)
}

fn parse_node_ref(input: &str) -> Option<(String, MermaidNode)> {
    let trimmed = input.trim().trim_end_matches(';').trim();
    if trimmed.is_empty() {
        return None;
    }
    let split_at = trimmed
        .find(|ch: char| matches!(ch, '[' | '(' | '{'))
        .unwrap_or(trimmed.len());
    let id = trimmed[..split_at].trim().to_string();
    if id.is_empty() {
        return None;
    }
    let rest = trimmed[split_at..].trim();
    let (label, shape) = if rest.starts_with("{{") && rest.ends_with("}}") {
        (
            rest[2..rest.len() - 2].trim().to_string(),
            MermaidNodeShape::Diamond,
        )
    } else if rest.starts_with("((") && rest.ends_with("))") {
        (
            rest[2..rest.len() - 2].trim().to_string(),
            MermaidNodeShape::Circle,
        )
    } else if rest.starts_with('{') && rest.ends_with('}') {
        (
            rest[1..rest.len() - 1].trim().to_string(),
            MermaidNodeShape::Diamond,
        )
    } else if rest.starts_with("([") && rest.ends_with("])") {
        (
            rest[2..rest.len() - 2].trim().to_string(),
            MermaidNodeShape::Round,
        )
    } else if rest.starts_with('(') && rest.ends_with(')') {
        (
            rest[1..rest.len() - 1].trim().to_string(),
            MermaidNodeShape::Round,
        )
    } else if rest.starts_with('[') && rest.ends_with(']') {
        (
            rest[1..rest.len() - 1].trim().to_string(),
            MermaidNodeShape::Rect,
        )
    } else {
        (id.clone(), MermaidNodeShape::Rect)
    };
    Some((
        id.clone(),
        MermaidNode {
            id,
            label: label.trim_matches('"').to_string(),
            shape,
        },
    ))
}

fn upsert_node(nodes: &mut Vec<MermaidNode>, node: MermaidNode) {
    if let Some(existing) = nodes.iter_mut().find(|existing| existing.id == node.id) {
        if existing.label == existing.id || existing.label.is_empty() {
            *existing = node;
        }
    } else {
        nodes.push(node);
    }
}

fn content_size(diagram: &MermaidDiagram, scale: f32) -> [f32; 2] {
    let count = diagram.nodes.len().max(1) as f32;
    let node_w = NODE_W * scale;
    let node_h = NODE_H * scale;
    match diagram.direction {
        MermaidDirection::TopDown => {
            [node_w, count * node_h + (count - 1.0) * GAP_Y * scale]
        }
        MermaidDirection::LeftRight => {
            [count * node_w + (count - 1.0) * GAP_X * scale, node_h]
        }
    }
}

fn node_positions(diagram: &MermaidDiagram, scale: f32) -> BTreeMap<String, [f32; 4]> {
    let node_w = NODE_W * scale;
    let node_h = NODE_H * scale;
    diagram
        .nodes
        .iter()
        .enumerate()
        .map(|(ix, node)| {
            let rect = match diagram.direction {
                MermaidDirection::TopDown => {
                    [0.0, ix as f32 * (node_h + GAP_Y * scale), node_w, node_h]
                }
                MermaidDirection::LeftRight => {
                    [ix as f32 * (node_w + GAP_X * scale), 0.0, node_w, node_h]
                }
            };
            (node.id.clone(), rect)
        })
        .collect()
}

fn edge_points(from: [f32; 4], to: [f32; 4], direction: MermaidDirection) -> Vec<Vec2> {
    match direction {
        MermaidDirection::TopDown => {
            let start = Vec2::new(from[0] + from[2] * 0.5, from[1] + from[3]);
            let end = Vec2::new(to[0] + to[2] * 0.5, to[1]);
            let mid_y = (start.y + end.y) * 0.5;
            vec![
                start,
                Vec2::new(start.x, mid_y),
                Vec2::new(end.x, mid_y),
                end,
            ]
        }
        MermaidDirection::LeftRight => {
            let start = Vec2::new(from[0] + from[2], from[1] + from[3] * 0.5);
            let end = Vec2::new(to[0], to[1] + to[3] * 0.5);
            let mid_x = (start.x + end.x) * 0.5;
            vec![
                start,
                Vec2::new(mid_x, start.y),
                Vec2::new(mid_x, end.y),
                end,
            ]
        }
    }
}

fn polyline_midpoint(points: &[Vec2]) -> Vec2 {
    if points.is_empty() {
        return Vec2::ZERO;
    }
    points[points.len() / 2]
}

fn shape(next_id: &mut u64, kind: ShapeKind, style: Style) -> Shape {
    let shape = Shape {
        id: ShapeId(*next_id),
        kind,
        style,
    };
    *next_id += 1;
    shape
}

fn color_from_u8(color: [u8; 4]) -> Color {
    Color::rgba(color[0], color[1], color[2], color[3])
}

fn estimated_text_width(text: &str, size: f32) -> f32 {
    text.chars().count() as f32 * size * 0.6
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flowchart_edges_and_nodes() {
        let diagram =
            parse_mermaid_diagram("flowchart LR\nA[Start] -->|go| B{Done}").unwrap();
        assert_eq!(diagram.direction, MermaidDirection::LeftRight);
        assert_eq!(diagram.nodes.len(), 2);
        assert_eq!(diagram.edges.len(), 1);
        assert_eq!(diagram.edges[0].label.as_deref(), Some("go"));
    }

    #[test]
    fn parses_common_mermaid_code_block_syntax() {
        let diagram = parse_mermaid_diagram(
            "graph LR\n  A[Square Rect] -- Link text --> B((Circle))\n  A --> C(Round Rect)\n  B --> D{Rhombus}\n  C --> D",
        )
        .unwrap();

        assert_eq!(diagram.direction, MermaidDirection::LeftRight);
        assert_eq!(diagram.nodes.len(), 4);
        assert_eq!(diagram.edges.len(), 4);
        assert_eq!(diagram.edges[0].label.as_deref(), Some("Link text"));
        assert!(diagram
            .nodes
            .iter()
            .any(|node| node.id == "B" && node.shape == MermaidNodeShape::Circle));
        assert!(diagram
            .nodes
            .iter()
            .any(|node| node.id == "D" && node.shape == MermaidNodeShape::Diamond));
    }
}
