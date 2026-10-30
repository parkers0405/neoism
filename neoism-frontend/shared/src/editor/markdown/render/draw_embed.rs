//! Read-only `.neodraw` embed for markdown.
//!
//! A ```draw fence whose body is a path to a `.neodraw` file renders the
//! referenced drawing inline (fitted into a block), so a sketch authored
//! in the full editor can be transcluded into notes and stays editable
//! at its source. Mirrors the ```mermaid interception in `mod.rs`.

use std::path::Path;

use neoism_paint::text::DrawOpts;

use crate::editor::markdown::MarkdownPane;
use crate::editor::neodraw::{render_scene, Camera, Scene, Vec2};
use crate::primitives::ide_theme::IdeTheme;

use super::draw::{draw_if_visible, draw_rounded_rect_clipped, markdown_font};
use super::types::{DEPTH, ORDER_BG};

/// Fixed embed height in logical pixels.
const BLOCK_H: f32 = 300.0;
const HEADER_H: f32 = 30.0;

#[allow(clippy::too_many_arguments)]
pub(super) fn render_draw_block(
    sugarloaf: &mut (impl crate::primitives::paint::Paint + ?Sized),
    pane: &MarkdownPane,
    source: &str,
    content_x: f32,
    cursor_y: f32,
    content_w: f32,
    pane_clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
    theme: &IdeTheme,
    font_scale: f32,
) -> Option<f32> {
    // The fence body is a path to a `.neodraw` file, resolved relative
    // to the markdown document.
    let rel = source.lines().map(str::trim).find(|l| !l.is_empty())?;
    let base = pane
        .path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let target = base.join(rel);
    let json = sugarloaf.asset_text(&target)?;
    let scene = Scene::from_json(&json).ok()?;

    let block_rect = [content_x - 18.0, cursor_y - 8.0, content_w + 36.0, BLOCK_H];

    // Panel background + accent rail.
    draw_rounded_rect_clipped(
        sugarloaf,
        pane_clip,
        block_rect[0],
        block_rect[1],
        block_rect[2],
        block_rect[3],
        10.0,
        theme.f32_alpha(theme.fg, 0.04),
        DEPTH,
        ORDER_BG,
    );

    // Header label.
    let title_opts = DrawOpts {
        font_size: markdown_font(12.0, font_scale),
        color: theme.u8_alpha(theme.muted, 0.95),
        bold: true,
        clip_rect: Some(pane_clip),
        ..DrawOpts::default()
    };
    let label = format!("Drawing · {rel}");
    draw_if_visible(
        sugarloaf,
        content_x,
        cursor_y + 8.0,
        &label,
        &title_opts,
        clip_top,
        clip_bottom,
        &[],
    );

    // Fit the scene into the canvas area below the header.
    let canvas = [
        content_x,
        cursor_y + HEADER_H,
        content_w,
        BLOCK_H - HEADER_H - 12.0,
    ];
    if (canvas[1] + canvas[3]) >= clip_top && canvas[1] <= clip_bottom {
        if let Some(b) = scene.bounds() {
            let pad = 16.0;
            let avail_w = (canvas[2] - 2.0 * pad).max(1.0);
            let avail_h = (canvas[3] - 2.0 * pad).max(1.0);
            let zoom = (avail_w / b.width().max(1.0))
                .min(avail_h / b.height().max(1.0))
                .min(2.0);
            let world_c = b.center();
            let cx = canvas[0] + canvas[2] * 0.5;
            let cy = canvas[1] + canvas[3] * 0.5;
            let cam = Camera {
                pan: Vec2::new(cx - world_c.x * zoom, cy - world_c.y * zoom),
                zoom,
            };
            render_scene(sugarloaf, &scene, &cam, canvas, DEPTH, ORDER_BG + 2);
        }
    }

    Some(cursor_y + BLOCK_H + 8.0)
}
