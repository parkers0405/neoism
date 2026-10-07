//! Host-neutral, opt-in HTML artifact projection. No HTML execution lives here.
use super::*;
use crate::panels::agent_pane::view::draw::{
    push_image_overlay_clipped, subtract_image_piece, ImagePiece,
};
use crate::panels::agent_pane::view::OVERLAY_PANEL_ID;

/// All rectangles are `[x, y, width, height]` in shared draw coordinates.
/// `viewport` is the full fixed HTML surface;
/// `visible_rect` is its intersection with the timeline/card clip. `scale`
/// is the shared UI scale, not Sugarloaf's OS pixel scale factor.
/// `key` is stable across source edits; hosts must track HTML revisions separately.
#[derive(Clone, Debug, PartialEq)]
pub struct HtmlArtifactRequest {
    pub key: String,
    pub html: String,
    pub viewport: [f32; 4],
    pub visible_rect: [f32; 4],
    pub scale: f32,
}

/// A host-uploaded Sugarloaf GraphicData image. Width/height are raster pixels;
/// the full image is mapped onto the fixed viewport, independent of its size.
/// The host owns image IDs, replacement, engine isolation, and resource cleanup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HtmlArtifactFrame {
    pub image_id: u32,
    pub width: u32,
    pub height: u32,
}

const VIEWPORT_HEIGHT: f32 = 320.0;

pub(super) fn block_height(scale: f32) -> f32 {
    VIEWPORT_HEIGHT * scale
}

/// Cheap timeline projection: count surrounding text normally, but never count
/// an explicit artifact's HTML payload as visual lines (including while streaming).
/// Track ordinary fences too, so a quoted neoism-html opener stays ordinary code.
pub(crate) fn estimated_body_height(
    text: &str,
    chars_per_line: usize,
    scale: f32,
) -> f32 {
    let mut fence: Option<(md::FenceDelimiter, bool)> = None;
    let mut height = 0.0;
    for line in text.split('\n') {
        if let Some((delimiter, artifact)) = &fence {
            if delimiter.closes(line) {
                if !artifact {
                    height += 20.0 * scale;
                }
                fence = None;
            } else if !artifact {
                height +=
                    (line.chars().count() / chars_per_line + 1) as f32 * 20.0 * scale;
            }
            continue;
        }
        if let Some((delimiter, info)) = md::fence_open(line) {
            let artifact = info.trim() == "neoism-html";
            fence = Some((delimiter, artifact));
            if artifact {
                height += block_height(scale);
                continue;
            }
        }
        height += (line.chars().count() / chars_per_line + 1) as f32 * 20.0 * scale;
    }
    height
}

fn artifact_key(namespace: &str, block_index: usize) -> String {
    format!("markdown:{namespace}:html:{block_index}")
}

/// Called only by the matching-closing-fence path, never the EOF flush.
pub(super) fn completed_fence(
    lang: &str,
    lines: &[String],
) -> Option<AssistantMarkdownBlock> {
    if lang.trim() != "neoism-html" {
        return None;
    }
    let source = join_markdown_lines(lines);
    let copy_target = format!("{COPY_LINK_PREFIX}{}", escape_copy_target(&source));
    Some(AssistantMarkdownBlock::HtmlArtifact {
        source,
        copy_target,
    })
}

fn request_visible_rect(
    viewport: [f32; 4],
    clip: [f32; 4],
    suppressed: bool,
    occlusions: &[[f32; 4]],
) -> Option<[f32; 4]> {
    if suppressed {
        return None;
    }
    let visible = intersect_rect(viewport, clip)?;
    // Conservative host activation: even partial chrome occlusion prevents
    // engine work/input this frame. Painting also subtracts all occlusions.
    if occlusions
        .iter()
        .any(|rect| intersect_rect(visible, *rect).is_some())
    {
        return None;
    }
    Some(visible)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render(
    sugarloaf: &mut Sugarloaf,
    pane: &mut impl AgentMarkdownPane,
    source: &str,
    copy_target: &str,
    namespace: &str,
    block_index: usize,
    rect: [f32; 4],
    theme: &IdeTheme,
    scale: f32,
    suppressed: bool,
    clip: [f32; 4],
    occlusions: &[[f32; 4]],
) {
    let [x, y, w, _] = rect;
    let viewport = [x, y, w, block_height(scale)];
    // Backgrounds follow the same occlusion policy as image/text painting.
    if let Some(visible) = intersect_rect(rect, clip) {
        let mut pieces = vec![ImagePiece {
            rect: visible,
            source_rect: [0.0, 0.0, 1.0, 1.0],
        }];
        for occlusion in occlusions {
            pieces = pieces
                .into_iter()
                .flat_map(|piece| subtract_image_piece(piece, *occlusion))
                .collect();
        }
        for piece in pieces {
            draw_rect_clipped(
                sugarloaf,
                piece.rect,
                theme.f32(theme.panel_bg()),
                ORDER_PANEL,
                clip,
            );
        }
    }
    // Activation/input remain visibility-gated, but clipping or a transient
    // occlusion must not replace an already painted artifact with fallback text.
    let key = artifact_key(namespace, block_index);
    let cached = pane.cached_html_artifact_frame(&key);
    let frame = request_visible_rect(viewport, clip, suppressed, occlusions)
        .and_then(|visible_rect| {
            pane.html_artifact_frame(HtmlArtifactRequest {
                key,
                html: source.to_owned(),
                viewport,
                visible_rect,
                scale,
            })
        })
        .or(cached)
        .filter(|frame| frame.width > 0 && frame.height > 0);
    if let Some(frame) = frame {
        if let Some(visible) = intersect_rect(viewport, clip) {
            let [vx, vy, vw, vh] = visible;
            let uv = [
                (vx - viewport[0]) / viewport[2],
                (vy - viewport[1]) / viewport[3],
                (vx + vw - viewport[0]) / viewport[2],
                (vy + vh - viewport[1]) / viewport[3],
            ];
            push_image_overlay_clipped(
                sugarloaf,
                OVERLAY_PANEL_ID,
                frame.image_id,
                visible,
                uv,
                8,
                sugarloaf.scale_factor(),
                occlusions,
            );
        }
    } else if let Some(body_clip) = intersect_rect(viewport, clip) {
        if let Some(opts) = opts_with_clip(
            DrawOpts {
                font_size: 12.0 * scale,
                color: theme.u8(theme.muted),
                ..DrawOpts::default()
            },
            body_clip,
        ) {
            draw_text_clipped(
                sugarloaf,
                x + 14.0 * scale,
                viewport[1] + 18.0 * scale,
                "Experimental HTML preview unavailable",
                &opts,
                occlusions,
            );
            let copy_label = if pane.code_copy_feedback_progress(copy_target).is_some() {
                "✓ Copied"
            } else {
                "Copy source"
            };
            let copy_w = measure_text_cached(sugarloaf, copy_label, &opts);
            if let Some(hit) = request_visible_rect(
                [
                    x + 14.0 * scale,
                    viewport[1] + 38.0 * scale,
                    copy_w,
                    20.0 * scale,
                ],
                body_clip,
                suppressed,
                occlusions,
            ) {
                pane.register_link_hit_rect(copy_target.to_owned(), hit);
            }
            draw_text_clipped(
                sugarloaf,
                x + 14.0 * scale,
                viewport[1] + 38.0 * scale,
                copy_label,
                &opts,
                occlusions,
            );
        }
    }
}

#[cfg(test)]
mod tests;
