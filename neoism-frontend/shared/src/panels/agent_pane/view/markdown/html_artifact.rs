//! Host-neutral, opt-in HTML artifact projection. No HTML execution lives here.
use super::*;
use crate::panels::agent_pane::view::draw::{
    push_image_overlay_clipped, subtract_image_piece, ImagePiece,
};
use crate::panels::agent_pane::view::OVERLAY_PANEL_ID;

/// All rectangles are `[x, y, width, height]` in shared draw coordinates.
/// `viewport` is the full resolved HTML surface;
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

/// A host-uploaded raster and the accepted snapshot geometry (draw pixels).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HtmlArtifactFrame {
    pub image_id: u32,
    pub width: u32,
    pub height: u32,
    pub source_hash: u64,
    pub viewport: [f32; 2],
    pub scale: f32,
}

/// Learned content height in unscaled CSS pixels; width is shared draw width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HtmlArtifactLayout {
    pub source_hash: u64,
    pub width: f32,
    pub scale: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HtmlArtifactStatus {
    Pending,
    Disabled,
    Failed,
}

/// Stable FNV-1a fingerprint, shared by host and renderer; not cryptographic.
pub fn html_artifact_source_hash(source: &str) -> u64 {
    source
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        })
}

fn same_dimension(a: f32, b: f32) -> bool {
    a.is_finite() && b.is_finite() && a > 0.0 && b > 0.0 && (a - b).abs() < 0.5
}

impl HtmlArtifactLayout {
    pub fn matches(&self, source: &str, width: f32, scale: f32) -> bool {
        self.source_hash == html_artifact_source_hash(source)
            && self.scale == scale
            && same_dimension(self.width, width)
    }
}

impl HtmlArtifactFrame {
    pub fn matches(&self, source: &str, viewport: [f32; 2], scale: f32) -> bool {
        self.width > 0
            && self.height > 0
            && self.source_hash == html_artifact_source_hash(source)
            && self.scale == scale
            && same_dimension(self.viewport[0], viewport[0])
            && same_dimension(self.viewport[1], viewport[1])
    }
}

const INITIAL_HEIGHT: f32 = 160.0;

pub(super) fn block_height(scale: f32) -> f32 {
    INITIAL_HEIGHT * scale
}

pub(super) fn resolved_height(
    pane: &impl AgentMarkdownPane,
    namespace: &str,
    block_index: usize,
    source: &str,
    width: f32,
    scale: f32,
) -> f32 {
    pane.html_artifact_height(&artifact_key(namespace, block_index), source, width, scale)
        .filter(|height| height.is_finite() && *height > 0.0)
        .unwrap_or(INITIAL_HEIGHT)
        .clamp(32.0, 640.0)
        * scale
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

fn image_piece(viewport: [f32; 4], clip: [f32; 4]) -> Option<ImagePiece> {
    if viewport[2] <= 0.0 || viewport[3] <= 0.0 {
        return None;
    }
    let visible = intersect_rect(viewport, clip)?;
    let [x, y, w, h] = visible;
    Some(ImagePiece {
        rect: visible,
        source_rect: [
            (x - viewport[0]) / viewport[2],
            (y - viewport[1]) / viewport[3],
            (x + w - viewport[0]) / viewport[2],
            (y + h - viewport[1]) / viewport[3],
        ],
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
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
    let [x, _, _, _] = rect;
    let viewport = rect;
    // Activation/input remain visibility-gated, but clipping or a transient
    // occlusion must not replace an already painted artifact with fallback text.
    let key = artifact_key(namespace, block_index);
    let cached = pane.cached_html_artifact_frame(&key);
    let frame = request_visible_rect(viewport, clip, suppressed, occlusions)
        .and_then(|visible_rect| {
            pane.html_artifact_frame(HtmlArtifactRequest {
                key: key.clone(),
                html: source.to_owned(),
                viewport,
                visible_rect,
                scale,
            })
        })
        .filter(|frame| frame.matches(source, [viewport[2], viewport[3]], scale))
        .or_else(|| {
            cached
                .filter(|frame| frame.matches(source, [viewport[2], viewport[3]], scale))
        });
    if let Some(frame) = frame {
        if let Some(piece) = image_piece(viewport, clip) {
            let overlay_scale = sugarloaf.scale_factor();
            push_image_overlay_clipped(
                sugarloaf,
                OVERLAY_PANEL_ID,
                frame.image_id,
                piece.rect,
                piece.source_rect,
                8,
                overlay_scale,
                occlusions,
            );
        }
    } else if pane.html_artifact_status(&key) == HtmlArtifactStatus::Pending {
        render_pending(sugarloaf, viewport, theme, scale, clip, occlusions);
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
                "HTML preview unavailable",
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

/// Static loading marks only: no animation, source text or actionable chrome.
pub(super) fn render_pending(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    rect: [f32; 4],
    theme: &IdeTheme,
    scale: f32,
    clip: [f32; 4],
    occlusions: &[[f32; 4]],
) {
    let Some(clip) = intersect_rect(rect, clip) else {
        return;
    };
    for (row, fraction) in [0.55, 0.8, 0.65].into_iter().enumerate() {
        let bar = [
            rect[0] + 14.0 * scale,
            rect[1] + (18.0 + row as f32 * 20.0) * scale,
            ((rect[2] - 28.0 * scale) * fraction).max(0.0),
            6.0 * scale,
        ];
        let Some(visible) = intersect_rect(bar, clip) else {
            continue;
        };
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
                theme.f32_alpha(theme.muted, 0.12),
                ORDER_PANEL,
                clip,
            );
        }
    }
}

#[cfg(test)]
mod tests;
