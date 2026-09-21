//! Sugarloaf rendering for a [`Scene`].
//!
//! Each shape is drawn in screen space (the [`Camera`] maps world →
//! screen) with a deterministic *rough* pass that gives the
//! Excalidraw-style hand-drawn wobble. The jitter is seeded from each
//! shape's [`Style::seed`](super::Style::seed) so a stroke looks the
//! same on every frame instead of shimmering.
//!
//! This is render-only: it reads a `&Scene` and draws it, so the same
//! entry point serves both the interactive [`DrawPane`](super::DrawPane)
//! editor and the read-only markdown ```draw embed.

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::primitives::ide_theme::IdeTheme;

use std::collections::HashSet;

use super::graph_sim::GraphSim;
use super::pane::{Camera, DrawPane};
use super::scene::{ArrowHead, Scene, ShapeId, ShapeKind, Style, Vec2};

const DEPTH: f32 = 0.0;
const ORDER_CANVAS: u8 = 2;
const ORDER_SCENE: u8 = 4;
const ORDER_OVERLAY: u8 = 200;

/// Draw a whole `DrawPane` into `rect` (logical pixels): canvas
/// background, the scene, any in-progress draft, and the selection
/// overlay. This is the desktop/web entry point — the per-shape
/// [`render_scene`] is reused read-only by the markdown embed.
pub fn render_pane(
    sugarloaf: &mut Sugarloaf,
    pane: &mut DrawPane,
    rect: [f32; 4],
    theme: &IdeTheme,
) {
    // Remember the rect so pointer events can map into world space.
    pane.last_rect = Some(rect);

    // Live note-graph view: step the simulation and draw it instead of a
    // static scene (no toolbar — it's a viewer).
    if pane.graph.is_some() {
        sugarloaf.rect(
            None,
            rect[0],
            rect[1],
            rect[2],
            rect[3],
            theme.f32(theme.bg),
            DEPTH,
            ORDER_CANVAS,
        );
        if pane.graph_needs_center {
            center_graph(pane, rect);
            pane.graph_needs_center = false;
        }
        if let Some(g) = pane.graph.as_mut() {
            g.step();
        }
        let cam = pane.placed_camera(rect);
        let hover = pane.graph_hover;
        if let Some(g) = pane.graph.as_ref() {
            render_graph(sugarloaf, g, &cam, rect, theme, hover);
        }
        // Capture label screen rects so pointer/hover code can hit-test
        // the filename text (matches what render_graph draws).
        let mut rects = Vec::new();
        if cam.zoom > 0.45 {
            if let Some(g) = pane.graph.as_ref() {
                let opts = DrawOpts {
                    font_size: 12.5,
                    ..DrawOpts::default()
                };
                for (i, node) in g.nodes.iter().enumerate() {
                    if node.label.is_empty() {
                        continue;
                    }
                    let p = cam.world_to_screen(node.pos);
                    let r = (node.radius * cam.zoom).max(2.5);
                    let w = sugarloaf.text_mut().measure(&node.label, &opts);
                    rects.push(([p.x + r + 4.0, p.y - 9.0, w, 18.0], i));
                }
            }
        }
        pane.graph_label_rects = rects;
        return;
    }

    // Layout is world-space and must be ready before fitting the camera.
    prepare_text_layouts(sugarloaf, pane);
    if pane.fit_pending {
        pane.fit_to_view(rect);
        pane.fit_pending = false;
    }
    let cam = pane.placed_camera(rect);

    // Canvas background fill.
    sugarloaf.rect(
        None,
        rect[0],
        rect[1],
        rect[2],
        rect[3],
        theme.f32(theme.bg),
        DEPTH,
        ORDER_CANVAS,
    );

    render_scene_dimmed(
        sugarloaf,
        &pane.scene,
        &cam,
        rect,
        DEPTH,
        ORDER_SCENE,
        &pane.erasing,
        Some(&pane.text_layouts),
    );

    // In-progress creation preview.
    if let Some(preview) = pane.draft_preview() {
        draw_shape(
            sugarloaf,
            &preview.kind,
            &preview.style,
            &cam,
            rect,
            DEPTH,
            ORDER_SCENE,
            None,
        );
    }

    render_marquee(sugarloaf, pane, &cam, rect, theme);
    render_selection(sugarloaf, pane, &cam, rect, theme);
    render_text_caret(sugarloaf, pane, &cam, rect, theme);
    render_toolbar(sugarloaf, pane, rect, theme);
}

/// Render a pane as a TRANSPARENT overlay — no canvas background, with the
/// scene drawn at `scene_order` so it can sit *above* a host surface (e.g.
/// a markdown note at ORDER_TEXT). The tool island still draws on top, and
/// the caller owns the camera (lock it to the host's scroll). This is what
/// makes a `DrawPane` multi-use: a standalone file ([`render_pane`]) or an
/// ink layer over something else (here).
pub fn render_pane_overlay(
    sugarloaf: &mut Sugarloaf,
    pane: &mut DrawPane,
    rect: [f32; 4],
    theme: &IdeTheme,
    scene_order: u8,
) {
    pane.last_rect = Some(rect);
    let cam = pane.placed_camera(rect);

    prepare_text_layouts(sugarloaf, pane);

    // No canvas fill — strokes composite over the host surface.
    render_scene_dimmed(
        sugarloaf,
        &pane.scene,
        &cam,
        rect,
        DEPTH,
        scene_order,
        &pane.erasing,
        Some(&pane.text_layouts),
    );
    if let Some(preview) = pane.draft_preview() {
        draw_shape(
            sugarloaf,
            &preview.kind,
            &preview.style,
            &cam,
            rect,
            DEPTH,
            scene_order,
            None,
        );
    }
    render_marquee(sugarloaf, pane, &cam, rect, theme);
    render_selection(sugarloaf, pane, &cam, rect, theme);
    render_text_caret(sugarloaf, pane, &cam, rect, theme);
    render_toolbar(sugarloaf, pane, rect, theme);
}

/// The rubber-band selection rectangle while dragging on empty canvas.
fn render_marquee(
    sugarloaf: &mut Sugarloaf,
    pane: &DrawPane,
    cam: &Camera,
    clip: [f32; 4],
    theme: &IdeTheme,
) {
    let super::input::DrawGesture::Marquee { start, current, .. } = &pane.gesture else {
        return;
    };
    let a = cam.world_to_screen(*start);
    let b = cam.world_to_screen(*current);
    let (x, y) = (a.x.min(b.x), a.y.min(b.y));
    let (w, h) = ((a.x - b.x).abs(), (a.y - b.y).abs());
    if w < 1.0 && h < 1.0 {
        return;
    }
    // Clip the fill rect to the canvas, then a thin border.
    let fx = x.max(clip[0]);
    let fy = y.max(clip[1]);
    let fx2 = (x + w).min(clip[0] + clip[2]);
    let fy2 = (y + h).min(clip[1] + clip[3]);
    if fx2 > fx && fy2 > fy {
        sugarloaf.rect(
            None,
            fx,
            fy,
            fx2 - fx,
            fy2 - fy,
            theme.f32_alpha(theme.accent, 0.12),
            DEPTH,
            ORDER_OVERLAY - 1,
        );
    }
    let accent = theme.f32_alpha(theme.accent, 0.8);
    let edges = [
        (x, y, x + w, y),
        (x + w, y, x + w, y + h),
        (x + w, y + h, x, y + h),
        (x, y + h, x, y),
    ];
    for (p, q, r, s) in edges {
        draw_line_clipped(sugarloaf, clip, p, q, r, s, 1.0, DEPTH, accent);
    }
}

fn measure_text(
    sugarloaf: &mut Sugarloaf,
    content: &str,
    size: f32,
    width: Option<f32>,
) -> super::text_layout::TextLayout {
    let size = super::text_layout::font_size(size);
    let opts = DrawOpts {
        font_size: super::text_layout::TEXT_RASTER_SIZE,
        ..DrawOpts::default()
    };
    let factor = size / super::text_layout::TEXT_RASTER_SIZE;
    super::text_layout::layout_text(content, size, width, |text| {
        sugarloaf.text_mut().measure(text, &opts) * factor
    })
}

fn prepare_text_layouts(sugarloaf: &mut Sugarloaf, pane: &mut DrawPane) {
    pane.text_layouts.clear();
    pane.text_dims.clear();
    for shape in &pane.scene.shapes {
        if let ShapeKind::Text {
            content,
            size,
            width,
            ..
        } = &shape.kind
        {
            let layout = measure_text(sugarloaf, content, *size, *width);
            pane.text_dims.insert(shape.id, layout.bounds);
            pane.text_layouts.insert(shape.id, layout);
        }
    }
}

/// A caret at the end of the text shape currently being edited, so it's
/// obvious which text has focus and where typing lands.
fn render_text_caret(
    sugarloaf: &mut Sugarloaf,
    pane: &DrawPane,
    cam: &Camera,
    clip: [f32; 4],
    theme: &IdeTheme,
) {
    let Some(id) = pane.editing_text else {
        return;
    };
    let Some(shape) = pane.scene.shapes.iter().find(|s| s.id == id) else {
        return;
    };
    let ShapeKind::Text { x, y, .. } = &shape.kind else {
        return;
    };
    // Faint focus box around the text being edited, so it's obvious
    // which text has the caret.
    let b = pane.shape_bounds(shape);
    let bmin = cam.world_to_screen(b.min);
    let bmax = cam.world_to_screen(b.max);
    let pad = 2.0;
    let left = (bmin.x - pad).max(clip[0]);
    let top = (bmin.y - pad).max(clip[1]);
    let right = (bmax.x + pad).min(clip[0] + clip[2]);
    let bottom = (bmax.y + pad).min(clip[1] + clip[3]);
    if right > left && bottom > top {
        sugarloaf.rounded_rect(
            None,
            left,
            top,
            right - left,
            bottom - top,
            theme.f32_alpha(theme.accent, 0.10),
            DEPTH,
            4.0,
            ORDER_SCENE + 1,
        );
    }
    let Some(layout) = pane.text_layouts.get(&id) else {
        return;
    };
    let origin = cam.world_to_screen(Vec2::new(*x, *y));
    let caret_x = origin.x + layout.rows.last().map_or(0.0, |row| row.advance) * cam.zoom;
    let caret_y = origin.y
        + layout.rows.len().saturating_sub(1) as f32 * layout.line_height * cam.zoom;
    let font_size = layout.font_size * cam.zoom;
    let accent = theme.f32(theme.accent);
    draw_line_clipped(
        sugarloaf,
        clip,
        caret_x,
        caret_y,
        caret_x,
        caret_y + font_size,
        2.0,
        DEPTH,
        accent,
    );
}

/// Centre the graph (its centroid) in the pane at a readable zoom.
fn center_graph(pane: &mut DrawPane, rect: [f32; 4]) {
    let Some(g) = pane.graph.as_ref() else { return };
    if g.nodes.is_empty() {
        return;
    }
    let (mut cx, mut cy) = (0.0f32, 0.0f32);
    for node in &g.nodes {
        cx += node.pos.x;
        cy += node.pos.y;
    }
    let n = g.nodes.len() as f32;
    cx /= n;
    cy /= n;
    let zoom = 0.7;
    pane.camera.zoom = zoom;
    pane.camera.pan = Vec2::new(rect[2] * 0.5 - cx * zoom, rect[3] * 0.5 - cy * zoom);
}

/// Draw the live note-graph: edges, node discs, and labels.
fn render_graph(
    sugarloaf: &mut Sugarloaf,
    sim: &GraphSim,
    cam: &Camera,
    clip: [f32; 4],
    theme: &IdeTheme,
    hover: Option<usize>,
) {
    let n = sim.nodes.len();
    let edge_color = theme.f32_alpha(theme.muted, 0.45);
    let hover_edge = theme.f32_alpha(theme.accent, 0.85);
    for &(a, b) in &sim.edges {
        if a >= n || b >= n || a == b {
            continue;
        }
        let pa = cam.world_to_screen(sim.nodes[a].pos);
        let pb = cam.world_to_screen(sim.nodes[b].pos);
        // Highlight edges touching the hovered node.
        let (color, w) = if hover == Some(a) || hover == Some(b) {
            (hover_edge, 2.0)
        } else {
            (edge_color, 1.3)
        };
        draw_line_clipped(sugarloaf, clip, pa.x, pa.y, pb.x, pb.y, w, DEPTH, color);
    }

    let fill = theme.f32(theme.accent);
    let bright = theme.f32(theme.fg);
    let blue = theme.f32(theme.accent);
    let dragging = sim.dragging;
    let label_color = theme.u8(theme.fg);
    let blue_u8 = theme.u8(theme.accent);
    for (i, node) in sim.nodes.iter().enumerate() {
        let p = cam.world_to_screen(node.pos);
        let active = dragging == Some(i) || hover == Some(i);
        // Hovered/dragged nodes grow a touch for feedback.
        let r = (node.radius * cam.zoom).max(2.5) * if active { 1.2 } else { 1.0 };
        if !point_in_rect(p, clip, r + 2.0) {
            continue;
        }
        let color = if active { bright } else { fill };
        draw_disc(sugarloaf, clip, p, r, color);

        if cam.zoom > 0.45 && !node.label.is_empty() {
            let hovered = hover == Some(i);
            let opts = DrawOpts {
                font_size: 12.5,
                color: if hovered { blue_u8 } else { label_color },
                clip_rect: Some(clip),
                ..DrawOpts::default()
            };
            let lx = p.x + r + 4.0;
            let ly = p.y - 7.0;
            sugarloaf.text_mut().draw(lx, ly, &node.label, &opts);
            // Underline the label on hover — reads as a clickable link.
            if hovered {
                let w = sugarloaf.text_mut().measure(&node.label, &opts);
                let uy = ly + 13.0;
                draw_line_clipped(sugarloaf, clip, lx, uy, lx + w, uy, 1.0, DEPTH, blue);
            }
        }
    }
}

/// A filled circle, approximated by a clipped 22-gon.
fn draw_disc(
    sugarloaf: &mut Sugarloaf,
    clip: [f32; 4],
    c: Vec2,
    r: f32,
    color: [f32; 4],
) {
    let sides = 22;
    let pts: Vec<(f32, f32)> = (0..sides)
        .map(|i| {
            let t = i as f32 / sides as f32 * std::f32::consts::TAU;
            (c.x + r * t.cos(), c.y + r * t.sin())
        })
        .collect();
    let clipped = clip_polygon(&pts, clip);
    if clipped.len() >= 3 {
        sugarloaf.polygon(&clipped, DEPTH, color);
    }
}

/// Draw the floating tool/colour/size toolbar.
fn render_toolbar(
    sugarloaf: &mut Sugarloaf,
    pane: &DrawPane,
    rect: [f32; 4],
    theme: &IdeTheme,
) {
    use super::toolbar::{build_toolbar, ToolbarItem};

    let bar = build_toolbar(rect);
    let [bx, by, bw, bh] = bar.bar_rect;
    // Panel background + subtle frame.
    sugarloaf.rounded_rect(
        None,
        bx,
        by,
        bw,
        bh,
        theme.f32_alpha(theme.fg, 0.10),
        DEPTH,
        10.0,
        ORDER_OVERLAY,
    );

    let accent = theme.f32(theme.accent);
    for btn in &bar.buttons {
        let [x, y, w, h] = btn.rect;
        let active = match btn.item {
            ToolbarItem::Tool(t) => pane.tool == t,
            ToolbarItem::Color(c) => pane.style_defaults.stroke == c,
            ToolbarItem::Size(s) => (pane.style_defaults.width - s).abs() < 0.01,
        };
        if active {
            sugarloaf.rounded_rect(
                None,
                x - 1.0,
                y - 1.0,
                w + 2.0,
                h + 2.0,
                accent,
                DEPTH,
                7.0,
                ORDER_OVERLAY + 1,
            );
        }
        match btn.item {
            ToolbarItem::Color(c) => {
                sugarloaf.rounded_rect(
                    None,
                    x + 3.0,
                    y + 3.0,
                    w - 6.0,
                    h - 6.0,
                    c.rgba_f32(),
                    DEPTH,
                    5.0,
                    ORDER_OVERLAY + 2,
                );
            }
            // Tools render their Nerd Font glyph; sizes render S/M/L/XL.
            ToolbarItem::Tool(_) | ToolbarItem::Size(_) => {
                let is_glyph = matches!(btn.item, ToolbarItem::Tool(_));
                let font_size = if is_glyph { 17.0 } else { 12.0 };
                let opts = DrawOpts {
                    font_size,
                    color: theme.u8(if active { theme.bg } else { theme.fg }),
                    clip_rect: Some(rect),
                    ..DrawOpts::default()
                };
                let tw = sugarloaf.text_mut().measure(btn.glyph, &opts);
                // Nerd Font glyphs sit a touch low; nudge up to center.
                let vnudge = if is_glyph { 1.5 } else { 0.0 };
                sugarloaf.text_mut().draw(
                    x + (w - tw) * 0.5,
                    y + (h - font_size) * 0.5 - vnudge,
                    btn.glyph,
                    &opts,
                );
            }
        }
    }
}

/// Selection bounding box + resize handles, in screen space.
fn render_selection(
    sugarloaf: &mut Sugarloaf,
    pane: &DrawPane,
    cam: &Camera,
    clip: [f32; 4],
    theme: &IdeTheme,
) {
    if matches!(pane.gesture, super::input::DrawGesture::Marquee { .. }) {
        for shape in pane
            .scene
            .shapes
            .iter()
            .filter(|shape| pane.selection.contains(&shape.id))
        {
            let bounds = pane.shape_bounds(shape);
            let a = cam.world_to_screen(bounds.min);
            let b = cam.world_to_screen(bounds.max);
            for (x0, y0, x1, y1) in [
                (a.x, a.y, b.x, a.y),
                (b.x, a.y, b.x, b.y),
                (b.x, b.y, a.x, b.y),
                (a.x, b.y, a.x, a.y),
            ] {
                draw_line_clipped(
                    sugarloaf,
                    clip,
                    x0,
                    y0,
                    x1,
                    y1,
                    1.0,
                    DEPTH,
                    theme.f32(theme.accent),
                );
            }
        }
        return;
    }
    let Some(bounds) = pane.selection_bounds() else {
        return;
    };
    let accent = theme.f32(theme.accent);
    let min = cam.world_to_screen(bounds.min);
    let max = cam.world_to_screen(bounds.max);
    // A single clean, thin border — slightly inset off the shape.
    let pad = 2.0;
    let (x0, y0, x1, y1) = (min.x - pad, min.y - pad, max.x + pad, max.y + pad);
    let edges = [
        (x0, y0, x1, y0),
        (x1, y0, x1, y1),
        (x1, y1, x0, y1),
        (x0, y1, x0, y0),
    ];
    for (a, b, c, d) in edges {
        draw_line_clipped(sugarloaf, clip, a, b, c, d, 1.0, DEPTH, accent);
    }
    // Draw the same handles hit-testing uses; text exposes width controls.
    let half = HANDLE_HALF_PX;
    let fill = theme.f32(theme.bg);
    for (_, p) in pane.selection_handle_positions(cam) {
        if !point_in_rect(p, clip, half + 2.0) {
            continue;
        }
        sugarloaf.rect(
            None,
            p.x - half,
            p.y - half,
            half * 2.0,
            half * 2.0,
            accent,
            DEPTH,
            ORDER_OVERLAY,
        );
        let inset = half - 1.2;
        sugarloaf.rect(
            None,
            p.x - inset,
            p.y - inset,
            inset * 2.0,
            inset * 2.0,
            fill,
            DEPTH,
            ORDER_OVERLAY + 1,
        );
    }
}

/// Half the on-screen size of a resize handle square, in logical px.
pub const HANDLE_HALF_PX: f32 = 4.5;

fn point_in_rect(p: Vec2, rect: [f32; 4], pad: f32) -> bool {
    p.x >= rect[0] - pad
        && p.x <= rect[0] + rect[2] + pad
        && p.y >= rect[1] - pad
        && p.y <= rect[1] + rect[3] + pad
}

/// Draw an entire scene. `clip` is the logical-pixel rect the canvas
/// occupies (used to cull offscreen shapes and clip text). `depth` /
/// `order` place the scene within sugarloaf's paint stack.
pub fn render_scene(
    sugarloaf: &mut Sugarloaf,
    scene: &Scene,
    camera: &Camera,
    clip: [f32; 4],
    depth: f32,
    order: u8,
) {
    render_scene_dimmed(
        sugarloaf,
        scene,
        camera,
        clip,
        depth,
        order,
        &HashSet::new(),
        None,
    );
}

/// Like [`render_scene`] but shapes in `dimmed` are drawn translucent —
/// used for the eraser's drag preview before the shapes are removed.
fn render_scene_dimmed(
    sugarloaf: &mut Sugarloaf,
    scene: &Scene,
    camera: &Camera,
    clip: [f32; 4],
    depth: f32,
    order: u8,
    dimmed: &HashSet<ShapeId>,
    layouts: Option<&std::collections::HashMap<ShapeId, super::text_layout::TextLayout>>,
) {
    for shape in &scene.shapes {
        let text_layout = layouts.and_then(|layouts| layouts.get(&shape.id));
        if dimmed.contains(&shape.id) {
            let mut s = shape.style.clone();
            s.opacity *= 0.25;
            draw_shape(
                sugarloaf,
                &shape.kind,
                &s,
                camera,
                clip,
                depth,
                order,
                text_layout,
            );
        } else {
            draw_shape(
                sugarloaf,
                &shape.kind,
                &shape.style,
                camera,
                clip,
                depth,
                order,
                text_layout,
            );
        }
    }
}

fn draw_shape(
    sugarloaf: &mut Sugarloaf,
    kind: &ShapeKind,
    style: &Style,
    camera: &Camera,
    clip: [f32; 4],
    depth: f32,
    order: u8,
    text_layout: Option<&super::text_layout::TextLayout>,
) {
    match kind {
        ShapeKind::Rect { x, y, w, h, corner } => {
            let pts = rect_corners(*x, *y, *w, *h, camera);
            if !bbox_intersects(&pts, clip) {
                return;
            }
            fill_closed(sugarloaf, &pts, style, clip, depth, order);
            // A rounded clean rect reads better than a rough one for
            // small corners; rough strokes for the sketchy look.
            let _ = corner;
            rough_stroke(sugarloaf, &pts, true, style, camera, clip, depth, order + 1);
        }
        ShapeKind::Ellipse { x, y, w, h } => {
            let pts = ellipse_points(*x, *y, *w, *h, camera);
            if !bbox_intersects(&pts, clip) {
                return;
            }
            fill_closed(sugarloaf, &pts, style, clip, depth, order);
            rough_stroke(sugarloaf, &pts, true, style, camera, clip, depth, order + 1);
        }
        ShapeKind::Line { points } => {
            let pts = to_screen(points, camera);
            if pts.len() >= 2 && bbox_intersects(&pts, clip) {
                rough_stroke(
                    sugarloaf,
                    &pts,
                    false,
                    style,
                    camera,
                    clip,
                    depth,
                    order + 1,
                );
            }
        }
        ShapeKind::Polygon { points } => {
            let pts = to_screen(points, camera);
            if pts.len() >= 3 && bbox_intersects(&pts, clip) {
                fill_closed(sugarloaf, &pts, style, clip, depth, order);
                rough_stroke(
                    sugarloaf,
                    &pts,
                    true,
                    style,
                    camera,
                    clip,
                    depth,
                    order + 1,
                );
            }
        }
        ShapeKind::Arrow { points, head } => {
            let pts = to_screen(points, camera);
            if pts.len() >= 2 && bbox_intersects(&pts, clip) {
                rough_stroke(
                    sugarloaf,
                    &pts,
                    false,
                    style,
                    camera,
                    clip,
                    depth,
                    order + 1,
                );
                if matches!(head, ArrowHead::Triangle) {
                    let n = pts.len();
                    draw_arrowhead(
                        sugarloaf,
                        pts[n - 2],
                        pts[n - 1],
                        style,
                        clip,
                        depth,
                        order + 1,
                    );
                }
            }
        }
        ShapeKind::Freehand { points } => {
            let pts = to_screen(points, camera);
            if pts.len() >= 2 && bbox_intersects(&pts, clip) {
                // A captured pen path is already "hand-drawn", so draw
                // it faithfully (single clean polyline) rather than
                // re-wobbling it.
                let width = (style.width * camera.zoom).max(1.0);
                let color = style.stroke.rgba_f32_with_opacity(style.opacity);
                for w in pts.windows(2) {
                    draw_line_clipped(
                        sugarloaf, clip, w[0].x, w[0].y, w[1].x, w[1].y, width, depth,
                        color,
                    );
                }
            }
        }
        ShapeKind::Text {
            x,
            y,
            content,
            size,
            width,
        } => {
            let measured;
            let layout = if let Some(layout) = text_layout {
                layout
            } else {
                measured = measure_text(sugarloaf, content, *size, *width);
                &measured
            };
            let origin = camera.world_to_screen(Vec2::new(*x, *y));
            let scale =
                layout.font_size / super::text_layout::TEXT_RASTER_SIZE * camera.zoom;
            let opts = DrawOpts {
                font_size: super::text_layout::TEXT_RASTER_SIZE,
                color: text_color(style),
                clip_rect: Some(clip),
                ..DrawOpts::default()
            };
            for (index, row) in layout.rows.iter().enumerate() {
                let row_y = origin.y + index as f32 * layout.line_height * camera.zoom;
                if row_y + layout.line_height * camera.zoom < clip[1]
                    || row_y > clip[1] + clip[3]
                    || origin.x + layout.bounds.x * camera.zoom < clip[0]
                    || origin.x > clip[0] + clip[2]
                {
                    continue;
                }
                sugarloaf
                    .text_mut()
                    .draw_scaled(origin.x, row_y, &row.text, &opts, scale);
            }
        }
    }
}

// ---- geometry helpers ------------------------------------------------------

fn to_screen(points: &[Vec2], camera: &Camera) -> Vec<Vec2> {
    points.iter().map(|p| camera.world_to_screen(*p)).collect()
}

fn rect_corners(x: f32, y: f32, w: f32, h: f32, camera: &Camera) -> Vec<Vec2> {
    to_screen(
        &[
            Vec2::new(x, y),
            Vec2::new(x + w, y),
            Vec2::new(x + w, y + h),
            Vec2::new(x, y + h),
        ],
        camera,
    )
}

fn ellipse_points(x: f32, y: f32, w: f32, h: f32, camera: &Camera) -> Vec<Vec2> {
    let cx = x + w * 0.5;
    let cy = y + h * 0.5;
    let rx = w.abs() * 0.5;
    let ry = h.abs() * 0.5;
    // More segments for bigger ellipses; clamped for tiny ones.
    let perimeter = std::f32::consts::PI * (rx + ry);
    let segments = ((perimeter / 10.0) as usize).clamp(16, 96);
    (0..segments)
        .map(|i| {
            let t = i as f32 / segments as f32 * std::f32::consts::TAU;
            camera.world_to_screen(Vec2::new(cx + rx * t.cos(), cy + ry * t.sin()))
        })
        .collect()
}

fn bbox_intersects(pts: &[Vec2], clip: [f32; 4]) -> bool {
    let (mut min_x, mut min_y, mut max_x, mut max_y) =
        (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in pts {
        min_x = min_x.min(p.x);
        min_y = min_y.min(p.y);
        max_x = max_x.max(p.x);
        max_y = max_y.max(p.y);
    }
    let [cx, cy, cw, ch] = clip;
    // Pad so wobble/stroke width near the edge isn't culled.
    let pad = 8.0;
    !(max_x < cx - pad
        || min_x > cx + cw + pad
        || max_y < cy - pad
        || min_y > cy + ch + pad)
}

// ---- rough (hand-drawn) stroke --------------------------------------------

/// Draw a polyline (optionally closed) as a sketchy double stroke. The
/// perpendicular jitter at each interior sample is hashed from the
/// shape's seed, so the wobble is stable across frames.
fn rough_stroke(
    sugarloaf: &mut Sugarloaf,
    pts: &[Vec2],
    closed: bool,
    style: &Style,
    camera: &Camera,
    clip: [f32; 4],
    depth: f32,
    order: u8,
) {
    let _ = order; // sugarloaf.line has no order arg; depth governs stacking.
    let width = (style.width * camera.zoom).max(1.0);
    let color = style.stroke.rgba_f32_with_opacity(style.opacity);
    let roughness = style.roughness.max(0.0);
    let amp = roughness * 1.4 * camera.zoom.clamp(0.5, 3.0);

    // Number of sketchy passes: 2 when rough, 1 when clean.
    let passes = if roughness <= 0.01 { 1 } else { 2 };

    let edge_iter = |i: usize| -> Option<(Vec2, Vec2)> {
        let a = pts.get(i)?;
        let b = if i + 1 < pts.len() {
            pts[i + 1]
        } else if closed {
            pts[0]
        } else {
            return None;
        };
        Some((*a, b))
    };
    let edge_count = if closed {
        pts.len()
    } else {
        pts.len().saturating_sub(1)
    };

    for pass in 0..passes {
        for e in 0..edge_count {
            let Some((a, b)) = edge_iter(e) else { continue };
            let dx = b.x - a.x;
            let dy = b.y - a.y;
            let len = (dx * dx + dy * dy).sqrt().max(0.001);
            let (nx, ny) = (-dy / len, dx / len); // unit perpendicular
            let subdivisions = ((len / 18.0) as usize).clamp(1, 24);

            let salt = (e as u32).wrapping_mul(2_654_435_761)
                ^ (pass as u32).wrapping_mul(0x9E37_79B1);
            let mut prev = jittered_endpoint(a, nx, ny, style.seed ^ salt, amp, pass);
            for s in 1..=subdivisions {
                let t = s as f32 / subdivisions as f32;
                let on = Vec2::new(a.x + dx * t, a.y + dy * t);
                let next = if s == subdivisions {
                    jittered_endpoint(b, nx, ny, style.seed ^ salt ^ 0xABCD, amp, pass)
                } else {
                    let j = hash_signed(
                        style.seed ^ salt ^ (s as u32).wrapping_mul(0x1000_193),
                    );
                    Vec2::new(on.x + nx * j * amp, on.y + ny * j * amp)
                };
                draw_line_clipped(
                    sugarloaf, clip, prev.x, prev.y, next.x, next.y, width, depth, color,
                );
                prev = next;
            }
        }
    }
}

/// Endpoints get a small, mostly-tangential nudge on the second pass so
/// the two strokes don't perfectly overlap (the open-corner look).
fn jittered_endpoint(
    p: Vec2,
    nx: f32,
    ny: f32,
    seed: u32,
    amp: f32,
    pass: usize,
) -> Vec2 {
    if pass == 0 {
        return p;
    }
    let j = hash_signed(seed) * amp * 0.6;
    Vec2::new(p.x + nx * j, p.y + ny * j)
}

/// Filled interior for a closed shape (drawn under the stroke).
fn fill_closed(
    sugarloaf: &mut Sugarloaf,
    pts: &[Vec2],
    style: &Style,
    clip: [f32; 4],
    depth: f32,
    _order: u8,
) {
    let Some(fill) = style.fill else { return };
    if pts.len() < 3 {
        return;
    }
    let color = fill.rgba_f32_with_opacity(style.opacity);
    let tuples: Vec<(f32, f32)> = pts.iter().map(|p| (p.x, p.y)).collect();
    let clipped = clip_polygon(&tuples, clip);
    if clipped.len() >= 3 {
        sugarloaf.polygon(&clipped, depth, color);
    }
}

fn draw_arrowhead(
    sugarloaf: &mut Sugarloaf,
    from: Vec2,
    tip: Vec2,
    style: &Style,
    clip: [f32; 4],
    depth: f32,
    _order: u8,
) {
    let dx = tip.x - from.x;
    let dy = tip.y - from.y;
    let len = (dx * dx + dy * dy).sqrt().max(0.001);
    let (ux, uy) = (dx / len, dy / len);
    let size = (8.0 + style.width * 1.5) * 1.0;
    // Two barbs rotated ±30° back from the tip.
    let barb = |angle: f32| -> Vec2 {
        let (s, c) = angle.sin_cos();
        let rx = ux * c - uy * s;
        let ry = ux * s + uy * c;
        Vec2::new(tip.x - rx * size, tip.y - ry * size)
    };
    let a = barb(0.52);
    let b = barb(-0.52);
    let color = style.stroke.rgba_f32_with_opacity(style.opacity);
    let tri = [(tip.x, tip.y), (a.x, a.y), (b.x, b.y)];
    let clipped = clip_polygon(&tri, clip);
    if clipped.len() >= 3 {
        sugarloaf.polygon(&clipped, depth, color);
    }
}

// ---- pixel clipping --------------------------------------------------------
//
// sugarloaf's immediate line/polygon/triangle primitives have no clip
// arg, so geometry is clipped to the pane rect here (text uses the GPU
// `clip_rect`). This makes shapes show *partial* at the edge instead of
// bleeding through the sidebar.

/// Draw a line clipped to `clip` (Liang–Barsky). Nothing drawn if fully out.
#[allow(clippy::too_many_arguments)]
fn draw_line_clipped(
    sugarloaf: &mut Sugarloaf,
    clip: [f32; 4],
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    width: f32,
    depth: f32,
    color: [f32; 4],
) {
    if let Some((ax, ay, bx, by)) = clip_line(clip, x1, y1, x2, y2) {
        sugarloaf.line(ax, ay, bx, by, width, depth, color);
    }
}

/// Liang–Barsky segment clip. Returns the visible sub-segment.
fn clip_line(
    clip: [f32; 4],
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
) -> Option<(f32, f32, f32, f32)> {
    let [cx, cy, cw, ch] = clip;
    let (dx, dy) = (x2 - x1, y2 - y1);
    let p = [-dx, dx, -dy, dy];
    let q = [x1 - cx, cx + cw - x1, y1 - cy, cy + ch - y1];
    let mut t0 = 0.0f32;
    let mut t1 = 1.0f32;
    for i in 0..4 {
        if p[i].abs() < 1e-6 {
            if q[i] < 0.0 {
                return None; // parallel and outside this boundary
            }
        } else {
            let r = q[i] / p[i];
            if p[i] < 0.0 {
                if r > t1 {
                    return None;
                }
                if r > t0 {
                    t0 = r;
                }
            } else {
                if r < t0 {
                    return None;
                }
                if r < t1 {
                    t1 = r;
                }
            }
        }
    }
    Some((x1 + t0 * dx, y1 + t0 * dy, x1 + t1 * dx, y1 + t1 * dy))
}

/// Sutherland–Hodgman clip of a polygon to the rect.
fn clip_polygon(points: &[(f32, f32)], clip: [f32; 4]) -> Vec<(f32, f32)> {
    let [cx, cy, cw, ch] = clip;
    let mut poly = points.to_vec();
    poly = clip_axis(&poly, true, cx, true); // left:   x >= cx
    poly = clip_axis(&poly, true, cx + cw, false); // right:  x <= cx+cw
    poly = clip_axis(&poly, false, cy, true); // top:    y >= cy
    poly = clip_axis(&poly, false, cy + ch, false); // bottom: y <= cy+ch
    poly
}

/// Clip a polygon against one axis-aligned half-plane. `vertical` =
/// boundary is a vertical line `x == bound`; `keep_greater` keeps the
/// side >= bound.
fn clip_axis(
    poly: &[(f32, f32)],
    vertical: bool,
    bound: f32,
    keep_greater: bool,
) -> Vec<(f32, f32)> {
    if poly.is_empty() {
        return Vec::new();
    }
    let coord = |p: &(f32, f32)| if vertical { p.0 } else { p.1 };
    let inside = |p: &(f32, f32)| {
        if keep_greater {
            coord(p) >= bound
        } else {
            coord(p) <= bound
        }
    };
    let mut out = Vec::with_capacity(poly.len() + 4);
    let n = poly.len();
    for i in 0..n {
        let cur = poly[i];
        let prev = poly[(i + n - 1) % n];
        let cur_in = inside(&cur);
        let prev_in = inside(&prev);
        if cur_in != prev_in {
            // Edge crosses the boundary — add the intersection.
            let (a, b) = (coord(&prev), coord(&cur));
            let t = (bound - a) / (b - a);
            out.push((prev.0 + t * (cur.0 - prev.0), prev.1 + t * (cur.1 - prev.1)));
        }
        if cur_in {
            out.push(cur);
        }
    }
    out
}

fn text_color(style: &Style) -> [u8; 4] {
    let c = style.stroke;
    let a = (c.a as f32 * style.opacity.clamp(0.0, 1.0)).round() as u8;
    [c.r, c.g, c.b, a]
}

// ---- deterministic noise ---------------------------------------------------

/// Hash a seed to `[0, 1)`.
fn hash01(seed: u32) -> f32 {
    let mut x = seed.wrapping_mul(0x9E37_79B1).wrapping_add(0x7F4A_7C15);
    x ^= x >> 16;
    x = x.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 13;
    x = x.wrapping_mul(0xC2B2_AE35);
    x ^= x >> 16;
    (x as f32) / (u32::MAX as f32)
}

/// Hash a seed to `[-1, 1)`.
fn hash_signed(seed: u32) -> f32 {
    hash01(seed) * 2.0 - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_deterministic_and_bounded() {
        assert_eq!(hash01(42), hash01(42));
        for s in 0..1000u32 {
            let v = hash01(s.wrapping_mul(7919));
            assert!((0.0..1.0).contains(&v), "out of range: {v}");
            let sgn = hash_signed(s);
            assert!((-1.0..1.0).contains(&sgn));
        }
    }

    #[test]
    fn ellipse_segment_count_scales_and_clamps() {
        let cam = Camera::default();
        let tiny = ellipse_points(0.0, 0.0, 4.0, 4.0, &cam);
        assert_eq!(tiny.len(), 16, "tiny ellipse clamps to min segments");
        let big = ellipse_points(0.0, 0.0, 800.0, 800.0, &cam);
        assert_eq!(big.len(), 96, "huge ellipse clamps to max segments");
    }

    #[test]
    fn clip_line_trims_to_rect() {
        let clip = [0.0, 0.0, 100.0, 100.0];
        // Crosses the right edge — trimmed at x=100.
        let (ax, ay, bx, by) = clip_line(clip, 50.0, 50.0, 200.0, 50.0).unwrap();
        assert_eq!((ax, ay), (50.0, 50.0));
        assert!((bx - 100.0).abs() < 0.01 && (by - 50.0).abs() < 0.01);
        // Fully outside → None.
        assert!(clip_line(clip, 200.0, 200.0, 300.0, 300.0).is_none());
        // Fully inside → unchanged.
        let inside = clip_line(clip, 10.0, 10.0, 90.0, 90.0).unwrap();
        assert_eq!(inside, (10.0, 10.0, 90.0, 90.0));
    }

    #[test]
    fn clip_polygon_clips_to_rect() {
        let clip = [0.0, 0.0, 100.0, 100.0];
        // Triangle poking out the right side gets clipped to <= x=100.
        let tri = [(50.0, 50.0), (150.0, 50.0), (50.0, 150.0)];
        let out = clip_polygon(&tri, clip);
        assert!(out.len() >= 3);
        assert!(out.iter().all(|p| p.0 <= 100.01 && p.1 <= 100.01));
        // Fully-outside polygon clips away to nothing.
        let far = [(200.0, 200.0), (250.0, 200.0), (200.0, 250.0)];
        assert!(clip_polygon(&far, clip).len() < 3);
    }

    #[test]
    fn bbox_cull_rejects_offscreen() {
        let clip = [0.0, 0.0, 100.0, 100.0];
        let on = vec![Vec2::new(10.0, 10.0), Vec2::new(20.0, 20.0)];
        let off = vec![Vec2::new(500.0, 500.0), Vec2::new(600.0, 600.0)];
        assert!(bbox_intersects(&on, clip));
        assert!(!bbox_intersects(&off, clip));
    }
}
