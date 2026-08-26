use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use super::draw::{draw_rounded_rect_clipped, draw_text_clipped, opts_with_clip};
use super::markdown::{
    layout_assistant_markdown_cached, render_markdown_blocks, AgentMarkdownPane,
    AssistantMarkdownBlock,
};
use super::message_card::{message_title, AgentMessageCardMessage};
use super::tool_message::AgentToolMessage;
use super::ORDER_PANEL;
use crate::primitives::ide_theme::IdeTheme;

/// Left pad (unscaled px) applied to regular streamed assistant prose so
/// it lines up with the rest of the chat content (reasoning bodies, list
/// items) instead of hugging the card's left edge. The height measurement
/// in `measure_message_height_with` must subtract the SAME amount from the
/// wrap width or card heights drift.
pub(super) const ASSISTANT_TEXT_PAD_LEFT: f32 = 18.0;
/// Extra room below a settled assistant answer for the
/// `Agent · Model · Duration` provenance line.
pub(super) const ASSISTANT_RESPONSE_FOOTER_H: f32 = 28.0;

#[allow(clippy::too_many_arguments)]
pub fn render_assistant_text_with<P: AgentMarkdownPane>(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    body_id: &str,
    text: &str,
    response_footer: &str,
    markdown_blocks: Option<&[AssistantMarkdownBlock]>,
    pane: &mut P,
    theme: &IdeTheme,
    s: f32,
    now_seconds: f32,
    mouse: Option<(f32, f32)>,
    viewport_clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) -> f32 {
    // One render path for every message size. `render_markdown_blocks` culls
    // off-screen blocks itself and advances by exact per-block heights, so a
    // huge message costs the same as a small one without a second
    // (estimate-driven) virtualization layer fighting the card height.
    // Left pad so regular streamed prose lines up with the rest of the
    // chat content (reasoning bodies, list items) instead of hugging the
    // card edge. Matches the 18px content inset used elsewhere. The
    // height measurement (`measure_message_height_with`) wraps at the
    // SAME `(width - 48*s)` so the card height stays exact.
    let pad_left = ASSISTANT_TEXT_PAD_LEFT * s;
    let cached_blocks;
    let blocks = if let Some(blocks) = markdown_blocks {
        blocks
    } else {
        cached_blocks = layout_assistant_markdown_cached(
            sugarloaf,
            pane,
            text,
            (w - 30.0 * s - pad_left).max(80.0 * s),
            theme,
            s,
        );
        cached_blocks.as_slice()
    };
    let footer_h = if response_footer.trim().is_empty() {
        0.0
    } else {
        ASSISTANT_RESPONSE_FOOTER_H * s
    };
    let body_h = (h - footer_h).max(0.0);
    render_markdown_blocks(
        sugarloaf,
        blocks,
        body_id,
        x + pad_left,
        y,
        (w - pad_left).max(80.0 * s),
        body_h,
        pane,
        theme,
        s,
        false,
        theme.fg,
        false,
        now_seconds,
        mouse,
        viewport_clip,
        occlusion_rects,
    );
    if footer_h > 0.0 {
        render_assistant_response_footer(
            sugarloaf,
            x + pad_left,
            y + body_h + 8.0 * s,
            response_footer.trim(),
            theme,
            s,
            viewport_clip,
            occlusion_rects,
        );
    }
    h
}

#[allow(clippy::too_many_arguments)]
fn render_assistant_response_footer(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    footer: &str,
    theme: &IdeTheme,
    s: f32,
    viewport_clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) {
    let (agent, rest) = footer
        .split_once(" · ")
        .map(|(agent, rest)| (agent, Some(rest)))
        .unwrap_or((footer, None));
    let Some(agent_opts) = opts_with_clip(
        DrawOpts {
            font_size: 12.0 * s,
            color: theme.u8(theme.blue),
            ..DrawOpts::default()
        },
        viewport_clip,
    ) else {
        return;
    };
    draw_text_clipped(sugarloaf, x, y, agent, &agent_opts, occlusion_rects);
    let Some(rest) = rest else {
        return;
    };
    let rest_x = x + sugarloaf.text_mut().measure(agent, &agent_opts);
    let Some(rest_opts) = opts_with_clip(
        DrawOpts {
            font_size: 12.0 * s,
            color: theme.u8(theme.muted),
            ..DrawOpts::default()
        },
        viewport_clip,
    ) else {
        return;
    };
    draw_text_clipped(
        sugarloaf,
        rest_x,
        y,
        &format!(" · {rest}"),
        &rest_opts,
        occlusion_rects,
    );
}

pub fn render_reasoning_message_with<P, M>(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    message: &M,
    markdown_blocks: Option<&[AssistantMarkdownBlock]>,
    pane: &mut P,
    theme: &IdeTheme,
    s: f32,
    now_seconds: f32,
    mouse: Option<(f32, f32)>,
    viewport_clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) -> f32
where
    P: AgentMarkdownPane,
    M: AgentMessageCardMessage,
{
    if h <= 0.0 {
        return 0.0;
    }

    draw_rounded_rect_clipped(
        sugarloaf,
        [x, y, w, h],
        theme.f32(theme.panel_bg()),
        14.0 * s,
        ORDER_PANEL,
        viewport_clip,
    );

    let title = message_title(message);
    let Some(label_opts) = opts_with_clip(
        DrawOpts {
            font_size: 12.0 * s,
            // Dim yellow "Thinking" label. On dark themes the 0.6 alpha keeps
            // it recessive against the panel; on light themes (retro_95) the
            // low alpha washes the amber toward the light panel until it reads
            // as near-white, so paint it at full opacity where `theme.yellow`
            // already resolves to a readable dark amber.
            color: if theme.is_dark() {
                theme.u8_alpha(theme.yellow, 0.6)
            } else {
                theme.u8(theme.yellow)
            },
            italic: true,
            ..DrawOpts::default()
        },
        viewport_clip,
    ) else {
        return h;
    };
    draw_text_clipped(
        sugarloaf,
        x + 18.0 * s,
        y + 12.0 * s,
        &title,
        &label_opts,
        occlusion_rects,
    );

    let body_x = x + 18.0 * s;
    let body_y = y + 34.0 * s;
    let body_w = (w - 48.0 * s).max(80.0 * s);
    let body_h = (h - 42.0 * s).max(0.0);
    // Measurement, timeline preparation, and painting must use the exact same
    // parsed block list. Besides honoring Markdown indentation instead of
    // rewriting model output, this prevents an invisible HTML node from being
    // measured one way and rendered another way.
    let cached_blocks;
    let blocks = if let Some(blocks) = markdown_blocks {
        blocks
    } else {
        cached_blocks = layout_assistant_markdown_cached(
            sugarloaf,
            pane,
            AgentToolMessage::text(message),
            body_w,
            theme,
            s,
        );
        cached_blocks.as_slice()
    };
    render_markdown_blocks(
        sugarloaf,
        blocks,
        AgentToolMessage::id(message),
        body_x,
        body_y,
        // `render_markdown_blocks` reserves its standard 30px right inset
        // for rich blocks. Pass that inset back here so the final code/table
        // viewport exactly matches the `body_w` used during layout.
        body_w + 30.0 * s,
        body_h,
        pane,
        theme,
        s,
        true,
        theme.yellow,
        // No leading status dot for the thinking/reasoning block — the
        // yellow marker the chat list items used to get is intentionally
        // suppressed here so the inner monologue reads as plain prose.
        false,
        now_seconds,
        mouse,
        viewport_clip,
        occlusion_rects,
    );
    h
}
