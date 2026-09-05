//! Right-attached side panel for the agent pane.
//!
//! Visual sibling of `editor/file_tree::render` but scoped to the agent
//! pane's own rect — `view::render` carves a right strip off the pane
//! and hands it here. Two modes selected by whether the conversation
//! has started:
//!
//! - Home (`!pane.has_conversation()`): list of previous sessions.
//! - Chat: live session info — agent, model, thinking, streaming
//!   state, queued prompts, pending permission, usage.

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::panels::agent_pane::icon::{self as agent_icon, SIDE_PANEL_ICON_PANEL_ID};
use crate::panels::agent_pane::state::side_panel::{
    BranchActivity, BranchStatus, SessionGoal, FONT_SIZE, FRAME_RADIUS, FRAME_STROKE,
    ROW_HEIGHT, ROW_PADDING_X, SIDE_PANEL_MIN_PANE_WIDTH,
};
use crate::panels::agent_pane::state::side_panel::{
    NeoismAgentSessionEntry, NeoismAgentSidePanel,
};
use crate::panels::agent_pane::state::{
    NeoismAgentMessage, NeoismAgentPane, NeoismAgentTodo,
};
use crate::primitives::ide_theme::IdeTheme;
use crate::primitives::{
    draw_text_with_occlusion, edge_row_radii, snap_to_device_px, truncate_to_fit,
};
use crate::render_policy::{
    loader_animation_frame, loader_orbit_position, loader_pastel_color,
};
use crate::widgets::frame::{draw_frame, FrameConfig, FrameCorners};

use super::draw::{
    draw_status_dot_text, draw_text_clipped, push_image_overlay_clipped, wrap_text,
};
use super::tool_message::{draw_checkbox, TodoVisualState, TODO_ROW_HEIGHT};
use super::{DEPTH, ORDER_PANEL};

pub trait AgentSidePanelTodo {
    fn status(&self) -> &str;
    fn content(&self) -> &str;
}

pub trait AgentSidePanelMessage {
    type Todo: AgentSidePanelTodo + Clone;

    fn is_todos_output(&self) -> bool;
    fn todos(&self) -> &[Self::Todo];
}

pub trait AgentSidePanelPane {
    type Message: AgentSidePanelMessage;

    fn side_panel(&self) -> &NeoismAgentSidePanel;
    fn side_panel_mut(&mut self) -> &mut NeoismAgentSidePanel;
    fn has_conversation(&self) -> bool;
    fn maybe_refresh_side_panel_sessions(&mut self);
    fn maybe_refresh_side_panel_subagents(&mut self);
    fn directory_label(&self) -> String;
    fn agent_label(&self) -> &str;
    fn model(&self) -> &str;
    fn thinking_label(&self) -> &str;
    fn usage_detail_lines(&self) -> Vec<String>;

    /// Latest turn's `(context tokens, context limit)` for the sidebar's
    /// animated context bar.
    fn context_usage(&self) -> Option<(u64, Option<u64>)>;

    fn messages(&self) -> &[Self::Message];
    fn session_id_str(&self) -> Option<&str>;
}

pub trait AgentSidePanelIconHost {
    fn register_agent_icons(sugarloaf: &mut Sugarloaf) -> bool;
}

#[macro_export]
macro_rules! neoism_ui_impl_agent_side_panel {
    (
        todo = $todo:ty,
        message = $message:ty,
        pane = $pane:ty,
        output_kind = $output_kind:ident,
        icons = $icons:ty,
        sugarloaf = $sugarloaf:ty,
        register_icons = $register_icons:path
    ) => {
        impl $crate::panels::agent_pane::view::side_panel::AgentSidePanelIconHost
            for $icons
        {
            fn register_agent_icons(sugarloaf: &mut $sugarloaf) -> bool {
                $register_icons(sugarloaf)
            }
        }

        impl $crate::panels::agent_pane::view::side_panel::AgentSidePanelTodo for $todo {
            fn status(&self) -> &str {
                &self.status
            }

            fn content(&self) -> &str {
                &self.content
            }
        }

        impl $crate::panels::agent_pane::view::side_panel::AgentSidePanelMessage
            for $message
        {
            type Todo = $todo;

            fn is_todos_output(&self) -> bool {
                matches!(self.output_kind, $output_kind::Todos)
            }

            fn todos(&self) -> &[Self::Todo] {
                &self.todos
            }
        }

        impl $crate::panels::agent_pane::view::side_panel::AgentSidePanelPane for $pane {
            type Message = $message;

            fn side_panel(
                &self,
            ) -> &$crate::panels::agent_pane::state::side_panel::NeoismAgentSidePanel
            {
                <$pane>::side_panel(self)
            }

            fn side_panel_mut(
                &mut self,
            ) -> &mut $crate::panels::agent_pane::state::side_panel::NeoismAgentSidePanel
            {
                <$pane>::side_panel_mut(self)
            }

            fn has_conversation(&self) -> bool {
                <$pane>::has_conversation(self)
            }

            fn maybe_refresh_side_panel_sessions(&mut self) {
                <$pane>::maybe_refresh_side_panel_sessions(self);
            }

            fn maybe_refresh_side_panel_subagents(&mut self) {
                <$pane>::maybe_refresh_side_panel_subagents(self);
            }

            fn directory_label(&self) -> String {
                <$pane>::directory_label(self)
            }

            fn agent_label(&self) -> &str {
                <$pane>::agent_label(self)
            }

            fn model(&self) -> &str {
                <$pane>::model(self)
            }

            fn thinking_label(&self) -> &str {
                <$pane>::thinking_label(self)
            }

            fn usage_detail_lines(&self) -> Vec<String> {
                <$pane>::usage_detail_lines(self)
            }

            fn context_usage(&self) -> Option<(u64, Option<u64>)> {
                <$pane>::context_usage(self)
            }

            fn messages(&self) -> &[Self::Message] {
                <$pane>::messages(self)
            }

            fn session_id_str(&self) -> Option<&str> {
                <$pane>::session_id_str(self)
            }
        }
    };
}

pub struct SharedAgentSidePanelIcons;

impl AgentSidePanelIconHost for SharedAgentSidePanelIcons {
    fn register_agent_icons(sugarloaf: &mut Sugarloaf) -> bool {
        agent_icon::register_agent_icons(sugarloaf)
    }
}

impl AgentSidePanelTodo for NeoismAgentTodo {
    fn status(&self) -> &str {
        &self.status
    }

    fn content(&self) -> &str {
        &self.content
    }
}

impl AgentSidePanelMessage for NeoismAgentMessage {
    type Todo = NeoismAgentTodo;

    fn is_todos_output(&self) -> bool {
        matches!(
            self.output_kind,
            crate::panels::agent_pane::state::NeoismAgentOutputKind::Todos
        )
    }

    fn todos(&self) -> &[Self::Todo] {
        &self.todos
    }
}

impl AgentSidePanelPane for NeoismAgentPane {
    type Message = NeoismAgentMessage;

    fn side_panel(&self) -> &NeoismAgentSidePanel {
        NeoismAgentPane::side_panel(self)
    }

    fn side_panel_mut(&mut self) -> &mut NeoismAgentSidePanel {
        NeoismAgentPane::side_panel_mut(self)
    }

    fn has_conversation(&self) -> bool {
        NeoismAgentPane::has_conversation(self)
    }

    fn maybe_refresh_side_panel_sessions(&mut self) {
        NeoismAgentPane::maybe_refresh_side_panel_sessions(self);
    }

    fn maybe_refresh_side_panel_subagents(&mut self) {
        NeoismAgentPane::maybe_refresh_side_panel_subagents(self);
    }

    fn directory_label(&self) -> String {
        NeoismAgentPane::directory_label(self)
    }

    fn agent_label(&self) -> &str {
        NeoismAgentPane::agent_label(self)
    }

    fn model(&self) -> &str {
        NeoismAgentPane::model(self)
    }

    fn thinking_label(&self) -> &str {
        NeoismAgentPane::thinking_label(self)
    }

    fn usage_detail_lines(&self) -> Vec<String> {
        NeoismAgentPane::usage_detail_lines(self)
    }

    fn context_usage(&self) -> Option<(u64, Option<u64>)> {
        let usage = self.latest_usage()?;
        Some((usage.total, usage.context_limit))
    }

    fn messages(&self) -> &[Self::Message] {
        NeoismAgentPane::messages(self)
    }

    fn session_id_str(&self) -> Option<&str> {
        NeoismAgentPane::session_id_str(self)
    }
}

/// Used when the agent pane is wider than [`SIDE_PANEL_MIN_PANE_WIDTH`].
/// Returns the carved-off right strip, or `None` when the pane is too
/// narrow to host the panel. The remaining width is what the chat /
/// home views should lay out against.
pub fn carve_panel_rect<P: AgentSidePanelPane>(
    pane: &P,
    rect: [f32; 4],
    s: f32,
) -> Option<([f32; 4], [f32; 4])> {
    if pane.side_panel().user_hidden() {
        return None;
    }
    let [x, y, w, h] = rect;
    let min_pane = SIDE_PANEL_MIN_PANE_WIDTH * s;
    if w < min_pane {
        return None;
    }
    let panel_w = pane.side_panel().width() * s;
    // Don't allow the panel to consume more than ~40% of the pane —
    // protects the conversation column when the user tugs the window
    // narrow before we add a manual resize handle.
    let panel_w = panel_w.min(w * 0.4);
    let gap = 6.0 * s;
    let main_w = w - panel_w - gap;
    if main_w < 220.0 * s {
        return None;
    }
    let main = [x, y, main_w, h];
    let panel = [x + main[2] + gap, y, panel_w, h];
    Some((main, panel))
}

/// Web/mobile responsive variant. In narrow takeover mode the panel owns the
/// full Agent content rect; the returned zero-width main rect is an explicit
/// signal that timeline/composer paint must be skipped.
pub fn carve_panel_rect_responsive<P: AgentSidePanelPane>(
    pane: &P,
    rect: [f32; 4],
    s: f32,
    narrow_takeover: bool,
) -> Option<([f32; 4], [f32; 4])> {
    if pane.side_panel().user_hidden() {
        return None;
    }
    if narrow_takeover {
        return Some(([rect[0], rect[1], 0.0, rect[3]], rect));
    }
    carve_panel_rect(pane, rect, s)
}

// The side-panel open/close control remains pane-local; the top-bar Agent
// icon is reserved for opening a new Agent tab.

#[allow(clippy::too_many_arguments)]
pub fn render_side_panel<P: AgentSidePanelPane>(
    sugarloaf: &mut Sugarloaf,
    pane: &mut P,
    panel_rect: [f32; 4],
    theme: &IdeTheme,
    s: f32,
    now_seconds: f32,
    mouse: Option<(f32, f32)>,
    occlusion_rects: &[[f32; 4]],
) {
    render_side_panel_with_icons::<P, SharedAgentSidePanelIcons>(
        sugarloaf,
        pane,
        panel_rect,
        theme,
        s,
        now_seconds,
        mouse,
        occlusion_rects,
    );
}

#[allow(clippy::too_many_arguments)]
pub fn render_side_panel_with_icons<P, I>(
    sugarloaf: &mut Sugarloaf,
    pane: &mut P,
    panel_rect: [f32; 4],
    theme: &IdeTheme,
    s: f32,
    now_seconds: f32,
    mouse: Option<(f32, f32)>,
    occlusion_rects: &[[f32; 4]],
) where
    P: AgentSidePanelPane,
    I: AgentSidePanelIconHost,
{
    // Every frame starts with no Usage target. Chat-mode rendering registers
    // it again only when real usage is present; home/narrow views stay clear.
    pane.side_panel_mut().clear_usage_rect();
    pane.side_panel_mut().clear_session_search_rect();
    let [px, py, pw, ph] = panel_rect;
    if pw <= 8.0 || ph <= 8.0 {
        return;
    }
    pane.side_panel_mut().set_last_panel_rect(panel_rect);

    let frame_stroke = (FRAME_STROKE * s).max(2.0);
    let frame_radius = FRAME_RADIUS * s;
    // The bottom strip used to host the open/close toggle icon, but
    // The sidebar uses the full panel height; its open/close controls are
    // handled by the pane and `/sidebar`, not by a reserved footer strip.
    let frame_h = ph;

    draw_frame(
        sugarloaf,
        [px, py, pw, frame_h],
        &FrameConfig {
            outer_color: theme.f32(theme.surface),
            inner_color: theme.f32(theme.bg),
            radius: frame_radius,
            border_thickness: frame_stroke,
            rounded_corners: FrameCorners::Top,
        },
        DEPTH,
        ORDER_PANEL,
        ORDER_PANEL + 1,
    );

    // No footer toggle is painted; clear its legacy hit rectangle.
    pane.side_panel_mut().clear_toggle_button_rect();
    let _ = occlusion_rects;

    let content_x = px + frame_stroke;
    let content_y = py + frame_stroke;
    let content_w = (pw - frame_stroke * 2.0).max(0.0);
    let content_h = (frame_h - frame_stroke).max(0.0);

    // The home/recent-sessions view shows whenever there's no conversation
    // yet OR the user tapped "← Back" to peek at recent chats while keeping
    // the live one open (`show_home_override`). Chat mode (session info) is
    // shown otherwise.
    let conversation_exists = pane.has_conversation();
    let show_home = !conversation_exists || pane.side_panel().show_home_override();
    let mode = if show_home {
        crate::panels::agent_pane::state::side_panel::SidePanelMode::Sessions
    } else {
        crate::panels::agent_pane::state::side_panel::SidePanelMode::Subagents
    };
    pane.side_panel_mut().set_mode(mode);
    let hovered_session = mouse.and_then(|(mx, my)| {
        show_home
            .then(|| pane.side_panel().hit_test_row(mx, my, panel_rect))
            .flatten()
    });
    pane.side_panel_mut()
        .tick_pointer_animations(hovered_session);

    if show_home {
        render_sessions_list(
            sugarloaf,
            pane,
            [content_x, content_y, content_w, content_h],
            theme,
            s,
            now_seconds,
            mouse,
            occlusion_rects,
            frame_radius - frame_stroke,
        );
    } else {
        render_session_info::<I>(
            sugarloaf,
            pane,
            [content_x, content_y, content_w, content_h],
            theme,
            s,
            now_seconds,
            mouse,
            occlusion_rects,
            frame_radius - frame_stroke,
        );
    }
}

pub(crate) mod draw;
pub(crate) mod sections;

use self::draw::render_sessions_list;
use self::sections::render_session_info;

/// Whether a home-mode session row should wear the green "running" dot —
/// its runtime status maps to an actively-working branch state (busy /
/// running / created). Idle, blocked, and finished sessions return false.
pub(crate) fn session_entry_is_running(entry: &NeoismAgentSessionEntry) -> bool {
    entry
        .runtime_status
        .as_deref()
        .and_then(BranchStatus::from_runtime_status)
        .is_some_and(|status| matches!(status, BranchStatus::Active))
}

fn subagent_row_activity(
    pane: &impl AgentSidePanelPane,
    entry: &NeoismAgentSessionEntry,
    is_main_row: bool,
) -> Option<BranchActivity> {
    if is_main_row {
        return None;
    }
    entry
        .runtime_status
        .as_deref()
        .and_then(BranchStatus::from_runtime_status)
        .map(|status| BranchActivity {
            status,
            current_tool: None,
            started_at: None,
            completed_at: None,
            terminal_locked: matches!(
                status,
                BranchStatus::Completed | BranchStatus::Stopped
            ),
        })
        .or_else(|| pane.side_panel().branch_activity(&entry.id).cloned())
}

#[cfg(test)]
mod tests;
