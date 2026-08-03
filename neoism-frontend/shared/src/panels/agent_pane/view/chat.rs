use sugarloaf::Sugarloaf;

use crate::panels::agent_pane::state::NeoismAgentPane;

use super::timeline::{AgentTimelineDelegate, AgentTimelinePane, SharedTimelineDelegate};
use super::user_input::AgentUserInputPane;
use super::{timeline, user_input};
use crate::panels::agent_pane::input_controller::InputWrapRow;
use crate::primitives::ide_theme::IdeTheme;

pub trait AgentChatPane: AgentTimelinePane + AgentUserInputPane {
    fn has_conversation(&self) -> bool;
    fn maybe_refresh_side_panel_subagents(&mut self);
    fn is_subagent_session(&self) -> bool;
}

#[macro_export]
macro_rules! neoism_ui_impl_agent_chat_pane {
    ($pane:ty) => {
        impl $crate::panels::agent_pane::view::chat::AgentChatPane for $pane {
            fn has_conversation(&self) -> bool {
                <$pane>::has_conversation(self)
            }

            fn maybe_refresh_side_panel_subagents(&mut self) {
                <$pane>::maybe_refresh_side_panel_subagents(self);
            }

            fn is_subagent_session(&self) -> bool {
                <$pane>::is_subagent_session(self)
            }
        }
    };
}

impl AgentChatPane for NeoismAgentPane {
    fn has_conversation(&self) -> bool {
        NeoismAgentPane::has_conversation(self)
    }

    fn maybe_refresh_side_panel_subagents(&mut self) {
        NeoismAgentPane::maybe_refresh_side_panel_subagents(self);
    }

    fn is_subagent_session(&self) -> bool {
        NeoismAgentPane::is_subagent_session(self)
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render_chat(
    sugarloaf: &mut Sugarloaf,
    pane: &mut NeoismAgentPane,
    rect: [f32; 4],
    theme: &IdeTheme,
    active: bool,
    now_seconds: f32,
    mouse: Option<(f32, f32)>,
    s: f32,
    input_rect: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) {
    render_chat_with::<NeoismAgentPane, SharedTimelineDelegate>(
        sugarloaf,
        pane,
        rect,
        theme,
        active,
        now_seconds,
        mouse,
        s,
        input_rect,
        occlusion_rects,
        None,
    );
}

#[allow(clippy::too_many_arguments)]
pub fn render_chat_with<P, D>(
    sugarloaf: &mut Sugarloaf,
    pane: &mut P,
    rect: [f32; 4],
    theme: &IdeTheme,
    active: bool,
    now_seconds: f32,
    mouse: Option<(f32, f32)>,
    s: f32,
    input_rect: [f32; 4],
    occlusion_rects: &[[f32; 4]],
    prepared_input_rows: Option<&[InputWrapRow]>,
) where
    P: AgentChatPane,
    D: AgentTimelineDelegate<P>,
{
    if pane.has_conversation() {
        pane.maybe_refresh_side_panel_subagents();
    }
    // Timeline lives in the SAME column as the composer island (same
    // insets + wide-pane cap, centered) — content wider than the
    // floating input bar would extend past its borders and kill the
    // island illusion.
    let (content_x, content_w) = super::layout::chat_column(rect, s);
    // Hug the top of the pane; the pane background is already painted by the
    // caller, so any extra offset shows as unused strip above the timeline.
    let timeline_top = rect[1];
    let composer_visible = !pane.is_subagent_session();
    // Clip the timeline EXACTLY at the island's top border — not at a
    // gap above it. A clip line floating above the card slices glyphs
    // mid-row in open space ("a black line splitting the sentence");
    // cutting at the border itself reads as content sliding under the
    // floating card.
    let content_bottom = if composer_visible {
        // The island's quads cannot mask text (text composites above
        // quads), so stop at the border's first pixel — nothing may
        // draw ON the border itself.
        input_rect[1]
    } else {
        rect[1] + rect[3] - 12.0 * s
    };

    timeline::render_timeline_with::<P, D>(
        sugarloaf,
        pane,
        [
            content_x,
            timeline_top,
            content_w,
            content_bottom - timeline_top,
        ],
        theme,
        s,
        now_seconds,
        mouse,
        occlusion_rects,
    );

    if composer_visible {
        user_input::render_input(
            sugarloaf,
            pane,
            input_rect,
            theme,
            active,
            s,
            false,
            now_seconds,
            occlusion_rects,
            prepared_input_rows,
        );
    }
}
