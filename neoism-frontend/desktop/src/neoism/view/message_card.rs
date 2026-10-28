use neoism_ui::panels::agent_pane::view::markdown::AssistantMarkdownBlock;
use neoism_ui::panels::agent_pane::view::message_card::{
    measure_message_height_with, render_message_card_with, AgentMessageCardDelegate,
};
use neoism_ui::panels::agent_pane::view::tool_message::ToolDiffSection;
use neoism_ui::primitives::ide_theme::IdeTheme;

use crate::neoism::agent::{
    NeoismAgentMessage, NeoismAgentMessageKind, NeoismAgentOutputKind, NeoismAgentPane,
    NeoismAgentTodo,
};

neoism_ui::neoism_ui_impl_agent_tool_message!(
    NeoismAgentTodo,
    NeoismAgentMessage,
    NeoismAgentPane,
    NeoismAgentOutputKind
);

struct DesktopMessageCardDelegate;

neoism_ui::neoism_ui_impl_agent_message_card_message!(
    NeoismAgentMessage,
    NeoismAgentTodo,
    NeoismAgentMessageKind,
    NeoismAgentOutputKind
);

neoism_ui::neoism_ui_impl_agent_message_card_pane!(NeoismAgentPane, NeoismAgentMessage);

impl AgentMessageCardDelegate<NeoismAgentPane, NeoismAgentMessage>
    for DesktopMessageCardDelegate
{
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_message_card(
    sugarloaf: &mut dyn neoism_ui::primitives::paint::Paint,
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
        DesktopMessageCardDelegate,
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
pub(super) fn measure_message_height(
    sugarloaf: &mut dyn neoism_ui::primitives::paint::Paint,
    pane: &NeoismAgentPane,
    message: &NeoismAgentMessage,
    width: f32,
    theme: &IdeTheme,
    s: f32,
    tool_expanded: bool,
    tool_expand_progress: f32,
) -> f32 {
    measure_message_height_with::<
        NeoismAgentPane,
        NeoismAgentMessage,
        DesktopMessageCardDelegate,
    >(
        sugarloaf,
        pane,
        message,
        width,
        theme,
        s,
        tool_expanded,
        tool_expand_progress,
    )
}
