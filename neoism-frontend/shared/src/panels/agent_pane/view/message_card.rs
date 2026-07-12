use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::panels::agent_pane::state::{
    NeoismAgentMessage, NeoismAgentMessageKind, NeoismAgentOutputKind, NeoismAgentPane,
    NeoismAgentTodo,
};

use super::assistant::{
    render_assistant_text_with, render_reasoning_message_with, ASSISTANT_TEXT_PAD_LEFT,
};
use super::code_block::{
    render_code_block, truncate_chars, warm_code_block_render_cache, AgentCodeMessage,
    AgentCodePane,
};
use super::draw::{
    draw_rect_clipped, draw_rounded_rect_clipped, draw_text_clipped, opts_with_clip,
    wrap_text,
};
use super::markdown::{
    layout_assistant_markdown_cached, measure_markdown_blocks, AgentMarkdownPane,
    AssistantMarkdownBlock,
};
use super::tool_message::{
    draw_checkbox, measure_tool_message_height, render_tool_message, AgentToolMessage,
    AgentToolPane, AgentToolTodo, TodoVisualState, ToolDiffSection, TODO_ROW_HEIGHT,
};
use super::user_input::render_user_message;
use super::{ORDER_PANEL, ORDER_TEXT};
use crate::primitives::ide_theme::IdeTheme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentMessageCardKind {
    User,
    Assistant,
    Reasoning,
    Tool,
    System,
    Subtask,
    Compaction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentMessageCardOutputKind {
    Text,
    Code,
    Todos,
}

pub trait AgentMessageCardTodo: AgentToolTodo {}

impl<T> AgentMessageCardTodo for T where T: AgentToolTodo {}

pub trait AgentMessageCardMessage: AgentToolMessage + AgentCodeMessage {
    type CardTodo: AgentMessageCardTodo;

    fn kind(&self) -> AgentMessageCardKind;
    fn output_kind(&self) -> AgentMessageCardOutputKind;
    fn card_todos(&self) -> &[Self::CardTodo];
}

pub trait AgentMessageCardPane<M>:
    AgentToolPane + AgentCodePane + AgentMarkdownPane
where
    M: AgentMessageCardMessage,
{
    fn selected_tool_group_child(&self, group_id: &str) -> Option<&str>;
}

pub trait AgentMessageCardDelegate<P, M>
where
    P: AgentMessageCardPane<M>,
    M: AgentMessageCardMessage,
{
    #[allow(clippy::too_many_arguments)]
    fn render_assistant_text(
        sugarloaf: &mut Sugarloaf,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        body_id: &str,
        text: &str,
        markdown_blocks: Option<&[AssistantMarkdownBlock]>,
        pane: &mut P,
        theme: &IdeTheme,
        s: f32,
        now_seconds: f32,
        mouse: Option<(f32, f32)>,
        viewport_clip: [f32; 4],
        occlusion_rects: &[[f32; 4]],
    ) -> f32 {
        render_assistant_text_with(
            sugarloaf,
            x,
            y,
            w,
            h,
            body_id,
            text,
            markdown_blocks,
            pane,
            theme,
            s,
            now_seconds,
            mouse,
            viewport_clip,
            occlusion_rects,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_reasoning_message(
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
    ) -> f32 {
        render_reasoning_message_with(
            sugarloaf,
            x,
            y,
            w,
            h,
            message,
            markdown_blocks,
            pane,
            theme,
            s,
            now_seconds,
            mouse,
            viewport_clip,
            occlusion_rects,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_user_message(
        sugarloaf: &mut Sugarloaf,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        text: &str,
        theme: &IdeTheme,
        s: f32,
        viewport_clip: [f32; 4],
        occlusion_rects: &[[f32; 4]],
    ) -> f32 {
        render_user_message(
            sugarloaf,
            x,
            y,
            w,
            h,
            text,
            theme,
            s,
            viewport_clip,
            occlusion_rects,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_tool_message(
        sugarloaf: &mut Sugarloaf,
        pane: &mut P,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        message: &M,
        theme: &IdeTheme,
        s: f32,
        viewport_clip: [f32; 4],
        occlusion_rects: &[[f32; 4]],
        prepared_diff_sections: Option<&[ToolDiffSection]>,
    ) -> f32 {
        render_tool_message(
            sugarloaf,
            pane,
            x,
            y,
            w,
            h,
            message,
            theme,
            s,
            viewport_clip,
            occlusion_rects,
            prepared_diff_sections,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_code_block(
        sugarloaf: &mut Sugarloaf,
        pane: &mut P,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        message: &M,
        theme: &IdeTheme,
        s: f32,
        viewport_clip: [f32; 4],
        occlusion_rects: &[[f32; 4]],
    ) {
        render_code_block(
            sugarloaf,
            pane,
            x,
            y,
            w,
            h,
            message,
            theme,
            s,
            viewport_clip,
            occlusion_rects,
        );
    }

    fn measure_markdown_text(
        sugarloaf: &mut Sugarloaf,
        pane: &P,
        text: &str,
        width: f32,
        theme: &IdeTheme,
        s: f32,
    ) -> f32 {
        // Exact height for every message. The wrapped block list is cached
        // (per text/width/scale) and measurement is cached per message, so
        // this stays cheap — and the card height equals what the renderer
        // draws, eliminating the estimate-vs-rendered gaps.
        let blocks =
            layout_assistant_markdown_cached(sugarloaf, pane, text, width, theme, s);
        measure_markdown_blocks(&blocks, width, pane, s)
    }
}

#[macro_export]
macro_rules! neoism_ui_impl_agent_message_card_message {
    ($message:ty, $todo:ty, $kind:ident, $output_kind:ident) => {
        impl $crate::panels::agent_pane::view::message_card::AgentMessageCardMessage
            for $message
        {
            type CardTodo = $todo;

            fn kind(
                &self,
            ) -> $crate::panels::agent_pane::view::message_card::AgentMessageCardKind {
                match self.kind {
                    $kind::User => {
                        $crate::panels::agent_pane::view::message_card::AgentMessageCardKind::User
                    }
                    $kind::Assistant => {
                        $crate::panels::agent_pane::view::message_card::AgentMessageCardKind::Assistant
                    }
                    $kind::Reasoning => {
                        $crate::panels::agent_pane::view::message_card::AgentMessageCardKind::Reasoning
                    }
                    $kind::Tool => {
                        $crate::panels::agent_pane::view::message_card::AgentMessageCardKind::Tool
                    }
                    $kind::System => {
                        $crate::panels::agent_pane::view::message_card::AgentMessageCardKind::System
                    }
                    $kind::Subtask => {
                        $crate::panels::agent_pane::view::message_card::AgentMessageCardKind::Subtask
                    }
                    $kind::Compaction => {
                        $crate::panels::agent_pane::view::message_card::AgentMessageCardKind::Compaction
                    }
                }
            }

            fn output_kind(
                &self,
            ) -> $crate::panels::agent_pane::view::message_card::AgentMessageCardOutputKind {
                match self.output_kind {
                    $output_kind::Text => {
                        $crate::panels::agent_pane::view::message_card::AgentMessageCardOutputKind::Text
                    }
                    $output_kind::Code => {
                        $crate::panels::agent_pane::view::message_card::AgentMessageCardOutputKind::Code
                    }
                    $output_kind::Todos => {
                        $crate::panels::agent_pane::view::message_card::AgentMessageCardOutputKind::Todos
                    }
                }
            }

            fn card_todos(&self) -> &[Self::CardTodo] {
                &self.todos
            }
        }
    };
}

#[macro_export]
macro_rules! neoism_ui_impl_agent_message_card_pane {
    ($pane:ty, $message:ty) => {
        impl
            $crate::panels::agent_pane::view::message_card::AgentMessageCardPane<$message>
            for $pane
        {
            fn selected_tool_group_child(&self, group_id: &str) -> Option<&str> {
                <$pane>::selected_tool_group_child(self, group_id)
            }
        }
    };
}

struct SharedMessageCardDelegate;

impl AgentMessageCardMessage for NeoismAgentMessage {
    type CardTodo = NeoismAgentTodo;

    fn kind(&self) -> AgentMessageCardKind {
        match self.kind {
            NeoismAgentMessageKind::User => AgentMessageCardKind::User,
            NeoismAgentMessageKind::Assistant => AgentMessageCardKind::Assistant,
            NeoismAgentMessageKind::Reasoning => AgentMessageCardKind::Reasoning,
            NeoismAgentMessageKind::Tool => AgentMessageCardKind::Tool,
            NeoismAgentMessageKind::System => AgentMessageCardKind::System,
            NeoismAgentMessageKind::Subtask => AgentMessageCardKind::Subtask,
            NeoismAgentMessageKind::Compaction => AgentMessageCardKind::Compaction,
        }
    }

    fn output_kind(&self) -> AgentMessageCardOutputKind {
        match self.output_kind {
            NeoismAgentOutputKind::Text => AgentMessageCardOutputKind::Text,
            NeoismAgentOutputKind::Code => AgentMessageCardOutputKind::Code,
            NeoismAgentOutputKind::Todos => AgentMessageCardOutputKind::Todos,
        }
    }

    fn card_todos(&self) -> &[Self::CardTodo] {
        &self.todos
    }
}

impl AgentMessageCardPane<NeoismAgentMessage> for NeoismAgentPane {
    fn selected_tool_group_child(&self, group_id: &str) -> Option<&str> {
        NeoismAgentPane::selected_tool_group_child(self, group_id)
    }
}

impl AgentMessageCardDelegate<NeoismAgentPane, NeoismAgentMessage>
    for SharedMessageCardDelegate
{
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_message_card(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    w: f32,
    measured_h: f32,
    pane: &mut NeoismAgentPane,
    message: &NeoismAgentMessage,
    markdown_blocks: Option<&[AssistantMarkdownBlock]>,
    tool_diff_sections: Option<&[ToolDiffSection]>,
    theme: &IdeTheme,
    s: f32,
    now_seconds: f32,
    mouse: Option<(f32, f32)>,
    viewport_clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) -> f32 {
    render_message_card_with::<
        NeoismAgentPane,
        NeoismAgentMessage,
        SharedMessageCardDelegate,
    >(
        sugarloaf,
        x,
        y,
        w,
        measured_h,
        pane,
        message,
        markdown_blocks,
        tool_diff_sections,
        theme,
        s,
        now_seconds,
        mouse,
        viewport_clip,
        occlusion_rects,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn render_message_card_with<P, M, D>(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    w: f32,
    measured_h: f32,
    pane: &mut P,
    message: &M,
    markdown_blocks: Option<&[AssistantMarkdownBlock]>,
    tool_diff_sections: Option<&[ToolDiffSection]>,
    theme: &IdeTheme,
    s: f32,
    now_seconds: f32,
    mouse: Option<(f32, f32)>,
    viewport_clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) -> f32
where
    P: AgentMessageCardPane<M>,
    M: AgentMessageCardMessage,
    D: AgentMessageCardDelegate<P, M>,
{
    let Some(body_opts) = opts_with_clip(
        DrawOpts {
            font_size: 14.0 * s,
            color: theme.u8(theme.fg),
            ..DrawOpts::default()
        },
        viewport_clip,
    ) else {
        return 0.0;
    };
    let h = measured_h.max(0.0);
    if h <= 0.0 {
        return 0.0;
    }

    match message.kind() {
        AgentMessageCardKind::Assistant => {
            return D::render_assistant_text(
                sugarloaf,
                x,
                y,
                w,
                h,
                AgentToolMessage::id(message),
                AgentToolMessage::text(message),
                markdown_blocks,
                pane,
                theme,
                s,
                now_seconds,
                mouse,
                viewport_clip,
                occlusion_rects,
            );
        }
        AgentMessageCardKind::User => {
            return D::render_user_message(
                sugarloaf,
                x,
                y,
                w,
                h,
                AgentToolMessage::text(message),
                theme,
                s,
                viewport_clip,
                occlusion_rects,
            );
        }
        AgentMessageCardKind::Reasoning => {
            return D::render_reasoning_message(
                sugarloaf,
                x,
                y,
                w,
                h,
                message,
                markdown_blocks,
                pane,
                theme,
                s,
                now_seconds,
                mouse,
                viewport_clip,
                occlusion_rects,
            );
        }
        AgentMessageCardKind::Tool => {
            return D::render_tool_message(
                sugarloaf,
                pane,
                x,
                y,
                w,
                h,
                message,
                theme,
                s,
                viewport_clip,
                occlusion_rects,
                tool_diff_sections,
            );
        }
        AgentMessageCardKind::Compaction => {
            return render_compaction_message_with::<P, M, D>(
                sugarloaf,
                x,
                y,
                w,
                h,
                message,
                markdown_blocks,
                pane,
                theme,
                s,
                viewport_clip,
                occlusion_rects,
            );
        }
        _ => {}
    }

    let kind = message.kind();
    let accent = message_accent(kind, theme, message.status());
    let live_task = kind == AgentMessageCardKind::Tool
        && message.status() == "running"
        && (message.tool() == "task"
            || AgentToolMessage::text(message).contains("task_id:")
            || message.detail().contains("task_id:"));
    let live_task_alpha = 0.45 + 0.55 * (now_seconds * 4.0).sin().abs();
    let accent_u8 = if live_task {
        theme.u8_alpha(theme.white, live_task_alpha)
    } else {
        theme.u8(accent)
    };
    let accent_f32 = if live_task {
        theme.f32_alpha(theme.white, live_task_alpha)
    } else {
        theme.f32(accent)
    };
    let surface = match kind {
        AgentMessageCardKind::Reasoning => theme.panel_bg(),
        AgentMessageCardKind::Tool | AgentMessageCardKind::Subtask => theme.surface,
        AgentMessageCardKind::System => theme.surface,
        _ => theme.surface,
    };
    draw_rounded_rect_clipped(
        sugarloaf,
        [x, y, w, h],
        theme.f32(surface),
        14.0 * s,
        ORDER_PANEL,
        viewport_clip,
    );
    let rail_x = x + 13.0 * s;
    if matches!(
        kind,
        AgentMessageCardKind::Tool | AgentMessageCardKind::Subtask
    ) {
        draw_rect_clipped(
            sugarloaf,
            [
                rail_x + 2.0 * s,
                y + 24.0 * s,
                1.0 * s,
                (h - 36.0 * s).max(0.0),
            ],
            theme.f32(theme.border),
            ORDER_TEXT,
            viewport_clip,
        );
        // Tighter status bullet (5px) — same center as the old 7px dot,
        // just a neater bullet against the rail.
        draw_rounded_rect_clipped(
            sugarloaf,
            [rail_x + 1.0 * s, y + 16.0 * s, 5.0 * s, 5.0 * s],
            accent_f32,
            2.5 * s,
            ORDER_TEXT + 1,
            viewport_clip,
        );
    } else {
        draw_rect_clipped(
            sugarloaf,
            [x, y + 12.0 * s, 3.0 * s, (h - 24.0 * s).max(0.0)],
            theme.f32(accent),
            ORDER_TEXT,
            viewport_clip,
        );
    }
    let title = message_title(message);
    let Some(label_opts) = opts_with_clip(
        DrawOpts {
            font_size: 12.0 * s,
            color: accent_u8,
            bold: true,
            ..DrawOpts::default()
        },
        viewport_clip,
    ) else {
        return h;
    };
    draw_text_clipped(
        sugarloaf,
        if matches!(
            kind,
            AgentMessageCardKind::Tool | AgentMessageCardKind::Subtask
        ) {
            x + 30.0 * s
        } else {
            x + 18.0 * s
        },
        y + 12.0 * s,
        &title,
        &label_opts,
        occlusion_rects,
    );
    let mut line_y = y + 34.0 * s;
    let body_x = if matches!(
        kind,
        AgentMessageCardKind::Tool | AgentMessageCardKind::Subtask
    ) {
        x + 30.0 * s
    } else {
        x + 18.0 * s
    };
    let body_w = (w - (body_x - x) - 18.0 * s).max(40.0 * s);

    if message.output_kind() == AgentMessageCardOutputKind::Todos {
        render_todos_with(
            sugarloaf,
            body_x,
            line_y,
            body_w,
            message.card_todos(),
            theme,
            s,
            viewport_clip,
            occlusion_rects,
        );
    } else if message.output_kind() == AgentMessageCardOutputKind::Code {
        D::render_code_block(
            sugarloaf,
            pane,
            body_x,
            line_y,
            body_w,
            h - 44.0 * s,
            message,
            theme,
            s,
            viewport_clip,
            occlusion_rects,
        );
    } else {
        let dim = matches!(kind, AgentMessageCardKind::Reasoning);
        let mut body_opts = body_opts;
        if dim {
            body_opts.color = theme.u8(theme.muted);
        }
        for line in wrap_text(
            sugarloaf,
            AgentToolMessage::text(message),
            body_w.max(80.0 * s),
            &body_opts,
            8,
        ) {
            draw_text_clipped(
                sugarloaf,
                body_x,
                line_y,
                &line,
                &body_opts,
                occlusion_rects,
            );
            line_y += 20.0 * s;
        }
    }
    h
}

pub(super) fn measure_message_height(
    sugarloaf: &mut Sugarloaf,
    pane: &NeoismAgentPane,
    message: &NeoismAgentMessage,
    width: f32,
    theme: &IdeTheme,
    s: f32,
    _tool_expanded: bool,
    tool_expand_progress: f32,
) -> f32 {
    measure_message_height_with::<
        NeoismAgentPane,
        NeoismAgentMessage,
        SharedMessageCardDelegate,
    >(
        sugarloaf,
        pane,
        message,
        width,
        theme,
        s,
        _tool_expanded,
        tool_expand_progress,
    )
}

pub fn measure_message_height_with<P, M, D>(
    sugarloaf: &mut Sugarloaf,
    pane: &P,
    message: &M,
    width: f32,
    theme: &IdeTheme,
    s: f32,
    _tool_expanded: bool,
    tool_expand_progress: f32,
) -> f32
where
    P: AgentMessageCardPane<M>,
    M: AgentMessageCardMessage,
    D: AgentMessageCardDelegate<P, M>,
{
    let body_opts = DrawOpts {
        font_size: 14.0 * s,
        color: theme.u8(theme.fg),
        ..DrawOpts::default()
    };
    match message.kind() {
        AgentMessageCardKind::Assistant => {
            if AgentToolMessage::text(message).trim().is_empty() {
                return 0.0;
            }
            D::measure_markdown_text(
                sugarloaf,
                pane,
                AgentToolMessage::text(message),
                // Mirror the `pad_left` inset in `render_assistant_text_with`
                // (30*s right margin + ASSISTANT_TEXT_PAD_LEFT left pad) so
                // the measured height matches the rendered wrap exactly.
                (width - 30.0 * s - ASSISTANT_TEXT_PAD_LEFT * s).max(80.0 * s),
                theme,
                s,
            )
        }
        AgentMessageCardKind::Reasoning => {
            if AgentToolMessage::text(message).trim().is_empty() {
                return 0.0;
            }
            let markdown_h = D::measure_markdown_text(
                sugarloaf,
                pane,
                AgentToolMessage::text(message),
                (width - 48.0 * s).max(80.0 * s),
                theme,
                s,
            );
            if markdown_h <= 0.0 {
                return 0.0;
            }
            42.0 * s + markdown_h
        }
        AgentMessageCardKind::User => {
            let bubble_w = width.max(160.0 * s);
            let mut user_opts = body_opts;
            user_opts.font_size = 13.5 * s;
            let lines = wrap_text(
                sugarloaf,
                AgentToolMessage::text(message),
                (bubble_w - 34.0 * s).max(80.0 * s),
                &user_opts,
                6,
            );
            24.0 * s + lines.len() as f32 * 19.0 * s
        }
        AgentMessageCardKind::Tool
            if message.output_kind() == AgentMessageCardOutputKind::Todos =>
        {
            42.0 * s + message.card_todos().len().max(1) as f32 * TODO_ROW_HEIGHT * s
        }
        AgentMessageCardKind::Tool
            if message.output_kind() == AgentMessageCardOutputKind::Code =>
        {
            warm_code_block_render_cache(message);
            measure_code_tool_message_height(AgentToolMessage::text(message), s)
        }
        AgentMessageCardKind::Tool => {
            let progress = tool_expand_progress.clamp(0.0, 1.0);
            let selected_group_child = AgentMessageCardPane::selected_tool_group_child(
                pane,
                AgentToolMessage::id(message),
            );
            if let Some(collapsed) = measure_tool_message_height(
                sugarloaf,
                message,
                width,
                s,
                false,
                selected_group_child,
            ) {
                if progress <= 0.001 {
                    return collapsed;
                }
                let expanded = measure_tool_message_height(
                    sugarloaf,
                    message,
                    width,
                    s,
                    true,
                    selected_group_child,
                )
                .unwrap_or(collapsed);
                return collapsed + (expanded - collapsed) * progress;
            }
            let collapsed_lines =
                AgentToolMessage::text(message).lines().count().clamp(1, 4);
            let expanded_lines = if !message.detail().trim().is_empty() {
                message.detail().lines().take(13).count().clamp(1, 12)
            } else {
                collapsed_lines
            };
            let collapsed = 28.0 * s + collapsed_lines as f32 * 20.0 * s;
            let expanded = 28.0 * s + expanded_lines as f32 * 20.0 * s;
            collapsed + (expanded - collapsed) * progress
        }
        AgentMessageCardKind::Compaction => {
            if AgentToolMessage::text(message).trim().is_empty() {
                34.0 * s
            } else {
                let markdown_h = D::measure_markdown_text(
                    sugarloaf,
                    pane,
                    AgentToolMessage::text(message),
                    (width - 30.0 * s - ASSISTANT_TEXT_PAD_LEFT * s).max(80.0 * s),
                    theme,
                    s,
                );
                42.0 * s + markdown_h
            }
        }
        _ => {
            let lines = wrap_text(
                sugarloaf,
                AgentToolMessage::text(message),
                (width - 36.0 * s).max(80.0 * s),
                &body_opts,
                8,
            );
            42.0 * s + lines.len() as f32 * 20.0 * s
        }
    }
}

fn measure_code_tool_message_height(text: &str, s: f32) -> f32 {
    if text.trim().is_empty() {
        0.0
    } else {
        const MAX_CODE_CARD_LINES: usize = 28;
        34.0 * s + text.lines().count().max(1).min(MAX_CODE_CARD_LINES) as f32 * 18.0 * s
    }
}

#[cfg(test)]
mod tests {
    use super::measure_code_tool_message_height;

    #[test]
    fn code_tool_height_caps_large_blocks() {
        let text = (0..64)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(
            measure_code_tool_message_height(&text, 1.0),
            34.0 + 28.0 * 18.0
        );
    }

    #[test]
    fn empty_code_tool_height_stays_zero() {
        assert_eq!(measure_code_tool_message_height("\n  \n", 1.0), 0.0);
    }
}

pub fn message_title(message: &impl AgentMessageCardMessage) -> String {
    if !message.title().is_empty() {
        if message.kind() == AgentMessageCardKind::Tool && !message.status().is_empty() {
            return format!("{}  {}", message.title(), message.status());
        }
        return message.title().to_string();
    }
    match message.kind() {
        AgentMessageCardKind::Reasoning => "Thinking".to_string(),
        AgentMessageCardKind::Tool => "Tool".to_string(),
        AgentMessageCardKind::Subtask => "Task".to_string(),
        AgentMessageCardKind::System => "System".to_string(),
        AgentMessageCardKind::Compaction => "Compaction".to_string(),
        _ => String::new(),
    }
}

pub fn message_accent(kind: AgentMessageCardKind, theme: &IdeTheme, status: &str) -> u32 {
    match kind {
        AgentMessageCardKind::User => theme.blue,
        AgentMessageCardKind::Assistant => theme.fg,
        AgentMessageCardKind::Reasoning => theme.magenta,
        AgentMessageCardKind::Tool => match status {
            "error" => theme.red,
            "completed" => theme.green,
            _ => theme.yellow,
        },
        AgentMessageCardKind::System => theme.yellow,
        AgentMessageCardKind::Subtask => theme.accent,
        AgentMessageCardKind::Compaction => theme.magenta,
    }
}

#[allow(clippy::too_many_arguments)]
fn render_compaction_message_with<P, M, D>(
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
    viewport_clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) -> f32
where
    P: AgentMessageCardPane<M>,
    M: AgentMessageCardMessage,
    D: AgentMessageCardDelegate<P, M>,
{
    let label = if message.status().trim().is_empty() {
        " Compaction ".to_string()
    } else {
        format!(" Compaction · {} ", message.status())
    };
    let Some(label_opts) = opts_with_clip(
        DrawOpts {
            font_size: 12.0 * s,
            color: theme.u8(theme.magenta),
            bold: true,
            ..DrawOpts::default()
        },
        viewport_clip,
    ) else {
        return h;
    };
    let line_y = y + 15.0 * s;
    let label_w = sugarloaf.text_mut().measure(&label, &label_opts).max(1.0);
    let label_x = x + ((w - label_w) * 0.5).max(0.0);
    let gap = 10.0 * s;
    let left_w = (label_x - x - gap).max(0.0);
    let right_x = label_x + label_w + gap;
    let right_w = (x + w - right_x).max(0.0);
    draw_rect_clipped(
        sugarloaf,
        [x, line_y, left_w, 1.0 * s],
        theme.f32(theme.border),
        ORDER_TEXT,
        viewport_clip,
    );
    draw_rect_clipped(
        sugarloaf,
        [right_x, line_y, right_w, 1.0 * s],
        theme.f32(theme.border),
        ORDER_TEXT,
        viewport_clip,
    );
    draw_text_clipped(
        sugarloaf,
        label_x,
        y + 6.0 * s,
        &label,
        &label_opts,
        occlusion_rects,
    );

    if !AgentToolMessage::text(message).trim().is_empty() {
        let body_y = y + 34.0 * s;
        D::render_assistant_text(
            sugarloaf,
            x,
            body_y,
            w,
            (h - 34.0 * s).max(0.0),
            AgentToolMessage::id(message),
            AgentToolMessage::text(message),
            markdown_blocks,
            pane,
            theme,
            s,
            0.0,
            None,
            viewport_clip,
            occlusion_rects,
        );
    }
    h
}

#[allow(clippy::too_many_arguments)]
fn render_todos_with<T>(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    w: f32,
    todos: &[T],
    theme: &IdeTheme,
    s: f32,
    viewport_clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) where
    T: AgentMessageCardTodo,
{
    let Some(opts) = opts_with_clip(
        DrawOpts {
            font_size: 14.0 * s,
            color: theme.u8(theme.fg),
            ..DrawOpts::default()
        },
        viewport_clip,
    ) else {
        return;
    };
    let mut line_y = y;
    if todos.is_empty() {
        draw_text_clipped(
            sugarloaf,
            x,
            line_y,
            "todos updated",
            &opts,
            occlusion_rects,
        );
        return;
    }
    for todo in todos.iter().take(10) {
        let state = TodoVisualState::from_status(todo.status());
        draw_checkbox(
            sugarloaf,
            x + 16.0 * s,
            line_y - 1.0 * s,
            state,
            theme,
            s,
            viewport_clip,
        );
        let mut text_opts = opts;
        text_opts.color = state.text_color(theme);
        text_opts.bold = state.text_bold();
        let text = truncate_chars(todo.content(), 160);
        draw_text_clipped(
            sugarloaf,
            x + 46.0 * s,
            line_y,
            &text,
            &text_opts,
            occlusion_rects,
        );
        line_y += TODO_ROW_HEIGHT * s;
        if x + w <= x {
            break;
        }
    }
}
