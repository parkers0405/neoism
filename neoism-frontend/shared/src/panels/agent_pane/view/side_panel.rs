//! Shared painter for the workspace Conversations chrome panel and the
//! active Agent chat's optional right-hand detail rail. The catalog is
//! hosted by Chrome/Renderer and never carved from an Agent tab.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::panels::agent_pane::icon::{self as agent_icon, SIDE_PANEL_ICON_PANEL_ID};
use crate::panels::agent_pane::state::side_panel::{
    BranchActivity, BranchStatus, SessionGoal, FONT_SIZE, FRAME_RADIUS, FRAME_STROKE,
    ROW_HEIGHT, ROW_PADDING_X,
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

const TRUNCATION_CACHE_LIMIT: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct TruncationKey {
    text: String,
    available_width_bits: u32,
    font_size_bits: u32,
    bold: bool,
    italic: bool,
    font_id: Option<usize>,
    scale_factor_bits: u32,
}

#[derive(Default)]
struct TruncationCache {
    values: HashMap<TruncationKey, String>,
    order: VecDeque<TruncationKey>,
}

thread_local! {
    static TRUNCATION_CACHE: RefCell<TruncationCache> = RefCell::new(TruncationCache::default());
}

fn truncate_sidebar_text(
    text: &str,
    available_width: f32,
    sugarloaf: &mut Sugarloaf,
    opts: &DrawOpts,
) -> String {
    let key = TruncationKey {
        text: text.to_owned(),
        available_width_bits: available_width.to_bits(),
        font_size_bits: opts.font_size.to_bits(),
        bold: opts.bold,
        italic: opts.italic,
        font_id: opts.font_id,
        scale_factor_bits: sugarloaf.scale_factor().to_bits(),
    };
    if let Some(value) =
        TRUNCATION_CACHE.with(|cache| cache.borrow().values.get(&key).cloned())
    {
        return value;
    }

    let value = truncate_to_fit(text, available_width, sugarloaf, opts);
    TRUNCATION_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.values.insert(key.clone(), value.clone()).is_none() {
            cache.order.push_back(key);
        }
        while cache.order.len() > TRUNCATION_CACHE_LIMIT {
            if let Some(oldest) = cache.order.pop_front() {
                cache.values.remove(&oldest);
            }
        }
    });
    value
}

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

    fn session_rename_buffer(&self) -> Option<String>;
    fn side_panel(&self) -> &NeoismAgentSidePanel;
    fn side_panel_mut(&mut self) -> &mut NeoismAgentSidePanel;
    /// Swap the catalog's navigation state with the independent detail panel.
    /// Called in pairs around right-side rendering; catalog IO remains on the
    /// original pane state and is never redirected to the detail panel.
    fn swap_detail_panel(&mut self);
    fn prepare_detail_panel(&mut self);
    fn has_conversation(&self) -> bool;
    fn maybe_refresh_side_panel_sessions(&mut self);
    fn maybe_refresh_side_panel_subagents(&mut self);
    fn directory_label(&self) -> String;
    fn conversation_source(
        &self,
    ) -> crate::panels::agent_pane::state::side_panel::ConversationSource;
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

            fn session_rename_buffer(&self) -> Option<String> {
                <$pane>::session_rename_buffer(self)
            }

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

            fn swap_detail_panel(&mut self) {
                <$pane>::swap_detail_panel(self);
            }

            fn prepare_detail_panel(&mut self) {
                <$pane>::prepare_detail_panel(self);
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

            fn conversation_source(
                &self,
            ) -> $crate::panels::agent_pane::state::side_panel::ConversationSource {
                <$pane>::conversation_source(self)
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

    fn session_rename_buffer(&self) -> Option<String> {
        NeoismAgentPane::session_rename_buffer(self)
    }

    fn side_panel(&self) -> &NeoismAgentSidePanel {
        NeoismAgentPane::side_panel(self)
    }

    fn side_panel_mut(&mut self) -> &mut NeoismAgentSidePanel {
        NeoismAgentPane::side_panel_mut(self)
    }

    fn swap_detail_panel(&mut self) {
        NeoismAgentPane::swap_detail_panel(self);
    }

    fn prepare_detail_panel(&mut self) {
        NeoismAgentPane::prepare_detail_panel(self);
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

    fn conversation_source(
        &self,
    ) -> crate::panels::agent_pane::state::side_panel::ConversationSource {
        self.new_chat_source()
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

/// Leave room for a readable chat and a usable detail rail after workspace chrome is laid out.
const DETAIL_MIN_CHAT_WIDTH: f32 = 520.0;
const DETAIL_MIN_RAIL_WIDTH: f32 = 200.0;
const DETAIL_RAIL_WIDTH: f32 = 260.0;
const DETAIL_GAP: f32 = 6.0;

pub fn state_detail_min_width(s: f32) -> f32 {
    (DETAIL_MIN_CHAT_WIDTH + DETAIL_MIN_RAIL_WIDTH + DETAIL_GAP) * s
}

pub fn detail_rail_width(available_width: f32, s: f32) -> f32 {
    DETAIL_RAIL_WIDTH
        .min((available_width / s - DETAIL_MIN_CHAT_WIDTH - DETAIL_GAP).max(0.0))
        * s
}

/// Web/mobile responsive variant. In narrow takeover mode the panel owns the
/// full Agent content rect; the returned zero-width main rect is an explicit
/// signal that timeline/composer paint must be skipped.
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
    let _icons_ready = I::register_agent_icons(sugarloaf);
    // The catalog has no Usage target. The independent detail rail registers
    // one only while usage is present.
    pane.side_panel_mut().clear_usage_rect();
    pane.side_panel_mut().clear_session_search_rect();
    pane.side_panel_mut().clear_new_chat_rect();
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

    pane.side_panel_mut()
        .set_mode(crate::panels::agent_pane::state::side_panel::SidePanelMode::Sessions);
    let hovered_session =
        mouse.and_then(|(mx, my)| pane.side_panel().hit_test_row(mx, my, panel_rect));
    let hovered_identity = hovered_session
        .and_then(|index| pane.side_panel().sessions().get(index))
        .filter(|entry| !entry.is_header && !entry.is_excerpt)
        .map(|entry| entry.stable_identity().to_owned());
    pane.side_panel_mut()
        .tick_pointer_animations(hovered_session, hovered_identity.as_deref());

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
}

/// Right-hand detail rail. Only called for an active conversation with enough
/// room after the left catalog and the composer have been laid out.
#[allow(clippy::too_many_arguments)]
pub fn render_detail_panel<P: AgentSidePanelPane, I: AgentSidePanelIconHost>(
    sugarloaf: &mut Sugarloaf,
    pane: &mut P,
    rect: [f32; 4],
    theme: &IdeTheme,
    s: f32,
    now_seconds: f32,
    mouse: Option<(f32, f32)>,
    occlusion_rects: &[[f32; 4]],
) {
    pane.maybe_refresh_side_panel_subagents();
    pane.prepare_detail_panel();
    pane.swap_detail_panel();
    let [x, y, w, h] = rect;
    pane.side_panel_mut()
        .set_mode(crate::panels::agent_pane::state::side_panel::SidePanelMode::Subagents);
    pane.side_panel_mut().set_last_panel_rect(rect);
    pane.side_panel_mut().clear_session_search_rect();
    pane.side_panel_mut().clear_usage_rect();
    let stroke = (FRAME_STROKE * s).max(2.0);
    let radius = FRAME_RADIUS * s;
    draw_frame(
        sugarloaf,
        rect,
        &FrameConfig {
            outer_color: theme.f32(theme.surface),
            inner_color: theme.f32(theme.bg),
            radius,
            border_thickness: stroke,
            rounded_corners: FrameCorners::Top,
        },
        DEPTH,
        ORDER_PANEL,
        ORDER_PANEL + 1,
    );
    render_session_info::<I>(
        sugarloaf,
        pane,
        [
            x + stroke,
            y + stroke,
            (w - 2.0 * stroke).max(0.0),
            (h - stroke).max(0.0),
        ],
        theme,
        s,
        now_seconds,
        mouse,
        occlusion_rects,
        radius - stroke,
    );
    pane.swap_detail_panel();
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
