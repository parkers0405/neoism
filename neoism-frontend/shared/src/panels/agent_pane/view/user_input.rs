use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::panels::agent_pane::state::{
    NeoismAgentPane, NeoismAgentPendingPermission, NeoismAgentPermissionChoice,
    NeoismAgentStreamingState,
};

use super::draw::{
    draw_rect_clipped, draw_rounded_rect_clipped, draw_text_clipped, opts_with_clip,
    wrap_text,
};
use super::wordmark::{format_elapsed, hsl_to_u8_simple};
use super::{
    DEPTH, INPUT_LINE_H, MAX_INPUT_LINES, ORDER_PANEL, ORDER_TEXT,
    STREAMING_STATUS_LINE_H,
};
use crate::panels::file_tree::FRAME_STROKE;
use crate::primitives::ide_theme::IdeTheme;
use crate::render_policy::{
    loader_animation_frame, loader_orbit_position, loader_pastel_color,
};

/// Logical height of the dropdown-chip row (agent ˅ / model ˅ /
/// thinking ˅) rendered below the input box, inside the outer shell.
pub(super) const CHIPS_BAND_H: f32 = 26.0;

/// Radius of the input island's rounded top corners in chat mode, and
/// the width of the corner-notch text occluders. The streaming status
/// row insets its connector past this so it isn't clipped by the notch
/// occluders. `ISLAND_CORNER + a gap` — kept as one source of truth so
/// the occluder (mod.rs) and the connector inset can't drift apart.
pub(super) const ISLAND_CORNER: f32 = 18.0;
pub(super) const STATUS_ROW_CORNER_INSET: f32 = ISLAND_CORNER + 6.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentPermissionChoice {
    Once,
    Always,
    Reject,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentStreamingStatus {
    Idle,
    Thinking,
    Working,
    Generating,
    Compacting,
    WaitingSubagents,
    BackgroundTasks,
}

pub trait AgentPendingPermission: Clone {
    fn parent_session_id(&self) -> Option<&str>;
    fn source_agent(&self) -> Option<&str>;
    fn title(&self) -> &str;
    fn permission(&self) -> &str;
    fn patterns(&self) -> &[String];
    fn selected(&self) -> usize;
    fn responding(&self) -> bool;
}

pub trait AgentUserInputPane {
    type PendingPermission: AgentPendingPermission;

    fn input(&self) -> &str;
    fn cursor_byte(&self) -> usize;
    fn set_cursor_rect(&mut self, rect: Option<[f32; 4]>);
    fn set_input_wrap_ranges(&mut self, ranges: Vec<(usize, usize)>);
    fn clear_usage_chip_rect(&mut self);
    fn register_usage_chip_rect(&mut self, rect: [f32; 4]);
    fn clear_status_chip_rects(&mut self);
    fn register_status_chip_rect(&mut self, index: usize, rect: [f32; 4]);
    fn usage_summary_label(&self) -> Option<String>;
    fn agent_label(&self) -> &str;
    fn model(&self) -> &str;
    fn thinking_label(&self) -> &str;
    fn streaming_label(&self) -> String;
    fn streaming_state(&self) -> AgentStreamingStatus;
    fn has_status_activity(&self) -> bool;
    fn running_background_task_count(&self) -> usize;
    fn background_task_details_expanded(&self) -> bool;
    fn active_background_task_summaries(&self) -> Vec<String>;
    fn register_background_status_rect(&mut self, rect: [f32; 4]);
    fn clear_background_status_rect(&mut self);
    fn streaming_elapsed_seconds(&self) -> Option<f32>;
    fn streaming_state_changed_elapsed(&self) -> Option<f32>;
    fn queued_prompt_count(&self) -> usize;
    fn pending_permission(&self) -> Option<&Self::PendingPermission>;
    fn session_id_str(&self) -> Option<&str>;
    fn register_permission_choice_rect(
        &mut self,
        choice: AgentPermissionChoice,
        rect: [f32; 4],
    );
}

#[macro_export]
macro_rules! neoism_ui_impl_agent_user_input {
    ($pane:ty, $pending:ty, $permission_choice:ident, $streaming_state:ident) => {
        impl $crate::panels::agent_pane::view::user_input::AgentPendingPermission for $pending {
            fn parent_session_id(&self) -> Option<&str> {
                self.parent_session_id.as_deref()
            }

            fn source_agent(&self) -> Option<&str> {
                self.source_agent.as_deref()
            }

            fn title(&self) -> &str {
                &self.title
            }

            fn permission(&self) -> &str {
                &self.permission
            }

            fn patterns(&self) -> &[String] {
                &self.patterns
            }

            fn selected(&self) -> usize {
                self.selected
            }

            fn responding(&self) -> bool {
                self.responding
            }
        }

        impl $crate::panels::agent_pane::view::user_input::AgentUserInputPane for $pane {
            type PendingPermission = $pending;

            fn input(&self) -> &str {
                <$pane>::input(self)
            }

            fn cursor_byte(&self) -> usize {
                <$pane>::cursor_byte(self)
            }

            fn set_cursor_rect(&mut self, rect: Option<[f32; 4]>) {
                <$pane>::set_cursor_rect(self, rect);
            }

            fn set_input_wrap_ranges(&mut self, ranges: Vec<(usize, usize)>) {
                <$pane>::set_input_wrap_ranges(self, ranges);
            }

            fn clear_usage_chip_rect(&mut self) {
                <$pane>::clear_usage_chip_rect(self);
            }

            fn register_usage_chip_rect(&mut self, rect: [f32; 4]) {
                <$pane>::register_usage_chip_rect(self, rect);
            }

            fn clear_status_chip_rects(&mut self) {
                <$pane>::clear_status_chip_rects(self);
            }

            fn register_status_chip_rect(&mut self, index: usize, rect: [f32; 4]) {
                <$pane>::register_status_chip_rect(self, index, rect);
            }

            fn usage_summary_label(&self) -> Option<String> {
                <$pane>::usage_summary_label(self)
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

            fn streaming_label(&self) -> String {
                <$pane>::streaming_label(self)
            }

            fn streaming_state(
                &self,
            ) -> $crate::panels::agent_pane::view::user_input::AgentStreamingStatus {
                match <$pane>::streaming_state(self) {
                    $streaming_state::Idle => {
                        $crate::panels::agent_pane::view::user_input::AgentStreamingStatus::Idle
                    }
                    $streaming_state::Thinking => {
                        $crate::panels::agent_pane::view::user_input::AgentStreamingStatus::Thinking
                    }
                    $streaming_state::Working => {
                        $crate::panels::agent_pane::view::user_input::AgentStreamingStatus::Working
                    }
                    $streaming_state::Generating => {
                        $crate::panels::agent_pane::view::user_input::AgentStreamingStatus::Generating
                    }
                    $streaming_state::Compacting => {
                        $crate::panels::agent_pane::view::user_input::AgentStreamingStatus::Compacting
                    }
                    $streaming_state::WaitingSubagents => {
                        $crate::panels::agent_pane::view::user_input::AgentStreamingStatus::WaitingSubagents
                    }
                    $streaming_state::BackgroundTasks => {
                        $crate::panels::agent_pane::view::user_input::AgentStreamingStatus::BackgroundTasks
                    }
                }
            }

            fn running_background_task_count(&self) -> usize {
                <$pane>::running_background_task_count(self)
            }

            fn has_status_activity(&self) -> bool {
                <$pane>::has_status_activity(self)
            }

            fn background_task_details_expanded(&self) -> bool {
                <$pane>::background_task_details_expanded(self)
            }

            fn active_background_task_summaries(&self) -> Vec<String> {
                <$pane>::active_background_task_summaries(self)
            }

            fn register_background_status_rect(&mut self, rect: [f32; 4]) {
                <$pane>::register_background_status_rect(self, rect);
            }

            fn clear_background_status_rect(&mut self) {
                <$pane>::clear_background_status_rect(self);
            }

            fn streaming_elapsed_seconds(&self) -> Option<f32> {
                <$pane>::streaming_elapsed_seconds(self)
            }

            fn streaming_state_changed_elapsed(&self) -> Option<f32> {
                <$pane>::streaming_state_changed_elapsed(self)
            }

            fn queued_prompt_count(&self) -> usize {
                <$pane>::queued_prompt_count(self)
            }

            fn pending_permission(&self) -> Option<&Self::PendingPermission> {
                <$pane>::pending_permission(self)
            }

            fn session_id_str(&self) -> Option<&str> {
                <$pane>::session_id_str(self)
            }

            fn register_permission_choice_rect(
                &mut self,
                choice: $crate::panels::agent_pane::view::user_input::AgentPermissionChoice,
                rect: [f32; 4],
            ) {
                let choice = match choice {
                    $crate::panels::agent_pane::view::user_input::AgentPermissionChoice::Once => {
                        $permission_choice::Once
                    }
                    $crate::panels::agent_pane::view::user_input::AgentPermissionChoice::Always => {
                        $permission_choice::Always
                    }
                    $crate::panels::agent_pane::view::user_input::AgentPermissionChoice::Reject => {
                        $permission_choice::Reject
                    }
                };
                <$pane>::register_permission_choice_rect(self, choice, rect);
            }
        }
    };
}

impl AgentPendingPermission for NeoismAgentPendingPermission {
    fn parent_session_id(&self) -> Option<&str> {
        self.parent_session_id.as_deref()
    }

    fn source_agent(&self) -> Option<&str> {
        self.source_agent.as_deref()
    }

    fn title(&self) -> &str {
        &self.title
    }

    fn permission(&self) -> &str {
        &self.permission
    }

    fn patterns(&self) -> &[String] {
        &self.patterns
    }

    fn selected(&self) -> usize {
        self.selected
    }

    fn responding(&self) -> bool {
        self.responding
    }
}

impl AgentUserInputPane for NeoismAgentPane {
    type PendingPermission = NeoismAgentPendingPermission;

    fn input(&self) -> &str {
        NeoismAgentPane::input(self)
    }

    fn cursor_byte(&self) -> usize {
        NeoismAgentPane::cursor_byte(self)
    }

    fn set_cursor_rect(&mut self, rect: Option<[f32; 4]>) {
        NeoismAgentPane::set_cursor_rect(self, rect);
    }

    fn set_input_wrap_ranges(&mut self, ranges: Vec<(usize, usize)>) {
        NeoismAgentPane::set_input_wrap_ranges(self, ranges);
    }

    fn clear_usage_chip_rect(&mut self) {
        NeoismAgentPane::clear_usage_chip_rect(self);
    }

    fn register_usage_chip_rect(&mut self, rect: [f32; 4]) {
        NeoismAgentPane::register_usage_chip_rect(self, rect);
    }

    fn clear_status_chip_rects(&mut self) {
        NeoismAgentPane::clear_status_chip_rects(self);
    }

    fn register_status_chip_rect(&mut self, index: usize, rect: [f32; 4]) {
        NeoismAgentPane::register_status_chip_rect(self, index, rect);
    }

    fn usage_summary_label(&self) -> Option<String> {
        NeoismAgentPane::usage_summary_label(self)
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

    fn streaming_label(&self) -> String {
        NeoismAgentPane::streaming_label(self)
    }

    fn streaming_state(&self) -> AgentStreamingStatus {
        match NeoismAgentPane::streaming_state(self) {
            NeoismAgentStreamingState::Idle => AgentStreamingStatus::Idle,
            NeoismAgentStreamingState::Thinking => AgentStreamingStatus::Thinking,
            NeoismAgentStreamingState::Working => AgentStreamingStatus::Working,
            NeoismAgentStreamingState::Generating => AgentStreamingStatus::Generating,
            NeoismAgentStreamingState::Compacting => AgentStreamingStatus::Compacting,
            NeoismAgentStreamingState::WaitingSubagents => {
                AgentStreamingStatus::WaitingSubagents
            }
            NeoismAgentStreamingState::BackgroundTasks => {
                AgentStreamingStatus::BackgroundTasks
            }
        }
    }

    fn has_status_activity(&self) -> bool {
        NeoismAgentPane::has_status_activity(self)
    }

    fn running_background_task_count(&self) -> usize {
        self.running_background_task_count()
    }

    fn background_task_details_expanded(&self) -> bool {
        self.background_task_details_expanded()
    }

    fn active_background_task_summaries(&self) -> Vec<String> {
        self.active_background_task_summaries()
    }

    fn register_background_status_rect(&mut self, rect: [f32; 4]) {
        self.register_background_status_rect(rect);
    }

    fn clear_background_status_rect(&mut self) {
        self.clear_background_status_rect();
    }

    fn streaming_elapsed_seconds(&self) -> Option<f32> {
        NeoismAgentPane::streaming_elapsed_seconds(self)
    }

    fn streaming_state_changed_elapsed(&self) -> Option<f32> {
        NeoismAgentPane::streaming_state_changed_elapsed(self)
    }

    fn queued_prompt_count(&self) -> usize {
        NeoismAgentPane::queued_prompt_count(self)
    }

    fn pending_permission(&self) -> Option<&Self::PendingPermission> {
        NeoismAgentPane::pending_permission(self)
    }

    fn session_id_str(&self) -> Option<&str> {
        NeoismAgentPane::session_id_str(self)
    }

    fn register_permission_choice_rect(
        &mut self,
        choice: AgentPermissionChoice,
        rect: [f32; 4],
    ) {
        let choice = match choice {
            AgentPermissionChoice::Once => NeoismAgentPermissionChoice::Once,
            AgentPermissionChoice::Always => NeoismAgentPermissionChoice::Always,
            AgentPermissionChoice::Reject => NeoismAgentPermissionChoice::Reject,
        };
        NeoismAgentPane::register_permission_choice_rect(self, choice, rect);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render_user_message(
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
    let bubble_w = w.max(160.0 * s);
    let bubble_x = x;
    draw_rounded_rect_clipped(
        sugarloaf,
        [bubble_x, y, bubble_w, h],
        theme.f32(theme.surface),
        14.0 * s,
        ORDER_PANEL,
        viewport_clip,
    );
    draw_rect_clipped(
        sugarloaf,
        [bubble_x, y + 10.0 * s, 3.0 * s, (h - 20.0 * s).max(0.0)],
        theme.f32(theme.blue),
        ORDER_TEXT,
        viewport_clip,
    );
    let Some(opts) = opts_with_clip(
        DrawOpts {
            font_size: 13.5 * s,
            color: theme.u8(theme.fg),
            ..DrawOpts::default()
        },
        viewport_clip,
    ) else {
        return h;
    };
    let mut line_y = y + 12.0 * s;
    for line in wrap_text(
        sugarloaf,
        text,
        (bubble_w - 34.0 * s).max(80.0 * s),
        &opts,
        6,
    ) {
        draw_agent_prompt_text(
            sugarloaf,
            bubble_x + 18.0 * s,
            line_y,
            &line,
            &opts,
            theme,
            occlusion_rects,
        );
        line_y += 19.0 * s;
    }
    h
}

pub fn render_input(
    sugarloaf: &mut Sugarloaf,
    pane: &mut impl AgentUserInputPane,
    rect: [f32; 4],
    theme: &IdeTheme,
    active: bool,
    s: f32,
    show_status: bool,
    now_seconds: f32,
    occlusion_rects: &[[f32; 4]],
) {
    let [x, y, w, h] = rect;
    pane.set_cursor_rect(None);
    pane.clear_usage_chip_rect();
    pane.clear_status_chip_rects();
    // Floating-island composer: the input box's own border IS the
    // island's top and sides — the shell only shows as a "skirt"
    // below the box, wrapping the dropdown chip row (agent ˅ /
    // model ˅ / thinking ˅), so nothing sticks out past the box on
    // top or the sides. No shadow: on near-black themes a halo reads
    // as a smeared bg band around the border, not as depth.
    let corner_radius = if show_status { 18.0 } else { 14.0 } * s;
    let chips_band_h = CHIPS_BAND_H * s;
    let outer_stroke = (1.0 * s).max(1.0);
    let box_x = x;
    let box_y = y;
    let box_w = w;
    let box_h = (h - chips_band_h).max(44.0 * s);
    let box_bottom = box_y + box_h;
    // Skirt: border + hollow fill from the box's midline down to the
    // island's bottom edge — its top half hides behind the opaque box,
    // leaving only the side rails and the bottom line around the chips.
    let skirt_top = box_y + box_h * 0.5;
    sugarloaf.rounded_rect(
        None,
        x,
        skirt_top,
        w,
        (y + h - skirt_top).max(0.0),
        theme.f32_alpha(theme.border, 0.75),
        DEPTH,
        corner_radius,
        ORDER_PANEL,
    );
    sugarloaf.rounded_rect(
        None,
        x + outer_stroke,
        skirt_top,
        (w - 2.0 * outer_stroke).max(0.0),
        (y + h - skirt_top - outer_stroke).max(0.0),
        theme.f32(theme.bg),
        DEPTH,
        (corner_radius - outer_stroke).max(0.0),
        ORDER_PANEL,
    );
    // Bottom band INSIDE the box hosts the square send button; wrapped
    // text never enters it.
    let bottom_reserved = if show_status { 42.0 } else { 38.0 } * s;
    let text_top_pad = if show_status { 15.0 } else { 11.0 } * s;
    let border_w = (FRAME_STROKE * s).max(2.0);
    sugarloaf.rounded_rect(
        None,
        box_x,
        box_y,
        box_w,
        box_h,
        theme.f32(theme.border),
        DEPTH,
        corner_radius,
        ORDER_PANEL,
    );
    sugarloaf.rounded_rect(
        None,
        box_x + border_w,
        box_y + border_w,
        (box_w - 2.0 * border_w).max(0.0),
        (box_h - 2.0 * border_w).max(0.0),
        theme.f32_alpha(theme.surface, 0.96),
        DEPTH,
        (corner_radius - border_w).max(0.0),
        ORDER_PANEL + 1,
    );

    let usage_label = pane.usage_summary_label();
    let usage_opts = DrawOpts {
        font_size: 11.5 * s,
        color: theme.u8(theme.cyan),
        bold: true,
        ..DrawOpts::default()
    };
    let usage_chip_w = usage_label
        .as_ref()
        .map(|label| sugarloaf.text_mut().measure(label, &usage_opts) + 22.0 * s)
        .unwrap_or(0.0);
    // Square send button — bottom-right corner inside the box.
    let send_side = if show_status { 30.0 } else { 26.0 } * s;
    let send_inset = 9.0 * s;
    let send_x = box_x + box_w - send_inset - send_side;
    let send_y = box_bottom - send_inset - send_side;
    // Dropdown chip row in the shell band below the box. Each chip
    // registers a hit rect: clicking one opens its "/" picker.
    render_status_chips(
        sugarloaf,
        pane,
        x + 14.0 * s,
        box_bottom + ((y + h - box_bottom) - 13.5 * s) * 0.5,
        (w - 28.0 * s).max(0.0),
        theme,
        s,
        occlusion_rects,
    );

    let input_text = pane.input().to_string();
    let text: &str = if input_text.is_empty() {
        "Ask anything"
    } else {
        &input_text
    };
    let text_color = if input_text.is_empty() {
        theme.u8(theme.muted)
    } else {
        theme.u8(theme.fg)
    };
    let text_x = box_x + 18.0 * s;
    let text_y = box_y + text_top_pad;
    let text_w = (box_w - 36.0 * s).max(20.0 * s);
    let line_h = INPUT_LINE_H * s;
    let max_visible_lines = (((box_h - bottom_reserved) / line_h).floor().max(1.0)
        as usize)
        .min(MAX_INPUT_LINES);
    let opts = DrawOpts {
        font_size: 16.0 * s,
        color: text_color,
        clip_rect: Some([text_x, box_y + 6.0 * s, text_w, box_h - bottom_reserved]),
        ..DrawOpts::default()
    };
    let wrapped_ranges = wrap_agent_prompt_ranges(sugarloaf, text, text_w, &opts);
    // Register the visual-row byte spans back on the pane so Up/Down
    // arrow movement walks the exact rows drawn below (see
    // `AgentInputBuffer::move_up_with_history_visual`). The placeholder
    // isn't the draft, so an empty input registers no rows.
    pane.set_input_wrap_ranges(if input_text.is_empty() {
        Vec::new()
    } else {
        wrapped_ranges.clone()
    });
    let wrapped_lines: Vec<String> = wrapped_ranges
        .iter()
        .map(|&(start, end)| text[start..end].to_string())
        .collect();
    let visible_line_offset = wrapped_lines.len().saturating_sub(max_visible_lines);
    let visible_lines = &wrapped_lines[visible_line_offset..];
    for (ix, line) in visible_lines.iter().enumerate() {
        draw_agent_prompt_text(
            sugarloaf,
            text_x,
            text_y + ix as f32 * line_h,
            line,
            &opts,
            theme,
            occlusion_rects,
        );
    }

    if active {
        let prefix = if pane.input().is_empty() {
            ""
        } else {
            &pane.input()[..pane.cursor_byte()]
        };
        let mut prefix_lines = wrap_agent_prompt_lines(sugarloaf, prefix, text_w, &opts);
        if prefix_lines.len() > max_visible_lines {
            prefix_lines =
                prefix_lines[prefix_lines.len() - max_visible_lines..].to_vec();
        }
        let last_line = prefix_lines.last().map(String::as_str).unwrap_or("");
        let last_w = sugarloaf.text_mut().measure(last_line, &opts);
        let caret_line = prefix_lines.len().saturating_sub(1);
        let caret_y = text_y + caret_line as f32 * line_h;
        let caret_x = (text_x + last_w).min(box_x + box_w - 28.0 * s).max(text_x);
        let cursor_w = (16.0 * s * 0.6).max(2.0);
        let cursor_h = 20.0 * s;
        let caret_rect = [caret_x, caret_y, cursor_w, cursor_h];
        // The trail-cursor overlay paints this rect ABOVE everything,
        // so when a covering panel (the `/` picker, a modal) occludes
        // the caret position, publishing it would punch the caret
        // through that panel. The input text already segments around
        // these rects; the caret must respect them too.
        let occluded = occlusion_rects
            .iter()
            .any(|rect| crate::primitives::geom::rects_intersect(*rect, caret_rect));
        if !occluded {
            pane.set_cursor_rect(Some(caret_rect));
        }
    }

    // Usage chip sits to the LEFT of the square send button.
    if let Some(label) = usage_label.as_deref() {
        if usage_chip_w > 0.0 {
            let usage_h = 22.0 * s;
            let usage_x = send_x - usage_chip_w - 8.0 * s;
            let usage_y = send_y + (send_side - usage_h) * 0.5;
            if usage_x >= box_x + 16.0 * s {
                sugarloaf.rounded_rect(
                    None,
                    usage_x,
                    usage_y,
                    usage_chip_w,
                    usage_h,
                    theme.f32(theme.surface),
                    DEPTH,
                    usage_h * 0.4,
                    ORDER_TEXT,
                );
                draw_text_clipped(
                    sugarloaf,
                    usage_x + 11.0 * s,
                    usage_y + 5.0 * s,
                    label,
                    &usage_opts,
                    occlusion_rects,
                );
                pane.register_usage_chip_rect([usage_x, usage_y, usage_chip_w, usage_h]);
            }
        }
    }
    // Square send button: filled rounded square, bottom-right. The
    // glyph is a little square — static while idle (the reference
    // look), and while the model responds it becomes the terminal
    // running-block loader (pastel dot orbiting a square path with a
    // fading trail). Enter still submits — the button mirrors state.
    let busy = !matches!(pane.streaming_state(), AgentStreamingStatus::Idle);
    let button_alpha = if busy || !pane.input().trim().is_empty() {
        1.0
    } else {
        0.55
    };
    sugarloaf.rounded_rect(
        None,
        send_x,
        send_y,
        send_side,
        send_side,
        theme.f32_alpha(theme.fg, button_alpha),
        DEPTH,
        8.0 * s,
        ORDER_TEXT,
    );
    if busy {
        // Running state: a dark stop-square with the terminal
        // running-block loader orbiting it (same `loader_*` helpers +
        // cadence as the block header / side-panel spinners). The
        // orbit is sized to stay readable on the light button.
        let center_x = send_x + send_side * 0.5;
        let center_y = send_y + send_side * 0.5;
        let square = send_side * 0.28;
        sugarloaf.rounded_rect(
            None,
            center_x - square * 0.5,
            center_y - square * 0.5,
            square,
            square,
            theme.f32(theme.bg),
            DEPTH,
            (1.5 * s).min(square * 0.3),
            ORDER_TEXT + 1,
        );
        let half = send_side * 0.32;
        let dot = (send_side * 0.15).max(2.5);
        let loader_frame = loader_animation_frame(now_seconds);
        for (trail, alpha) in [1.0f32, 0.58, 0.32, 0.16].into_iter().enumerate() {
            let (dx, dy) =
                loader_orbit_position(loader_frame.phase - trail as f32 * 0.075, half);
            sugarloaf.quad(
                None,
                center_x + dx - dot * 0.5,
                center_y + dy - dot * 0.5,
                dot,
                dot,
                loader_pastel_color(loader_frame.tick, trail, alpha),
                [dot * 0.5; 4],
                DEPTH,
                ORDER_TEXT + 1,
            );
        }
    } else {
        // Idle: send arrow.
        let arrow = "\u{2191}";
        let arrow_opts = DrawOpts {
            font_size: send_side * 0.62,
            color: theme.u8(theme.bg),
            bold: true,
            ..DrawOpts::default()
        };
        let arrow_w = sugarloaf.text_mut().measure(arrow, &arrow_opts);
        draw_text_clipped(
            sugarloaf,
            send_x + (send_side - arrow_w) * 0.5,
            send_y + (send_side - arrow_opts.font_size) * 0.5,
            arrow,
            &arrow_opts,
            occlusion_rects,
        );
    }
}

/// Streaming status row rendered as the last entry of the timeline — it
/// scrolls with the conversation content like any other message line.
#[allow(clippy::too_many_arguments)]
pub fn render_streaming_status_row(
    sugarloaf: &mut Sugarloaf,
    pane: &mut impl AgentUserInputPane,
    rect: [f32; 4],
    theme: &IdeTheme,
    s: f32,
    now_seconds: f32,
    viewport_clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) {
    let [bar_x, bar_y, bar_w, bar_h] = rect;
    // Inset the whole ╰─ connector line past the island's rounded
    // top-corner. This row is the bottom-most timeline element during
    // streaming, so it rests directly on the island's top edge; the
    // corner-notch occluders (see `render_agent_pane_with`) span the
    // island radius from the left edge, and the connector at the
    // former +8s sat inside them — losing its left half. Starting the
    // content past the corner clears the occluder AND aligns the
    // connector under the message-card text above it.
    let bar_x = bar_x + STATUS_ROW_CORNER_INSET * s;
    let bar_w = (bar_w - STATUS_ROW_CORNER_INSET * s).max(0.0);
    if bar_w <= 0.0 || bar_h <= 0.0 {
        pane.clear_background_status_rect();
        return;
    }
    if !pane.has_status_activity() {
        pane.clear_background_status_rect();
        return;
    }
    let label_text = pane.streaming_label();
    let background_count = pane.running_background_task_count();
    if label_text.is_empty() && background_count == 0 {
        pane.clear_background_status_rect();
        return;
    }
    let accent = match pane.streaming_state() {
        AgentStreamingStatus::Thinking => theme.magenta,
        AgentStreamingStatus::Working => theme.yellow,
        AgentStreamingStatus::Generating => theme.accent,
        AgentStreamingStatus::Compacting => theme.green,
        AgentStreamingStatus::WaitingSubagents => theme.yellow,
        AgentStreamingStatus::BackgroundTasks => theme.red,
        AgentStreamingStatus::Idle => theme.muted,
    };
    let state = pane.streaming_state();
    let elapsed = pane.streaming_elapsed_seconds().unwrap_or(0.0);
    let transition = pane.streaming_state_changed_elapsed().unwrap_or(2.0);
    let live_phase = elapsed;
    let queued_count = pane.queued_prompt_count();
    let status_line_h = STREAMING_STATUS_LINE_H * s;
    let primary_y = if queued_count > 0 {
        bar_y
    } else {
        bar_y + (bar_h - status_line_h).max(0.0) * 0.5
    };
    let clip_x = bar_x.max(viewport_clip[0]);
    let clip_y = (bar_y - 10.0 * s).max(viewport_clip[1]);
    let clip_right = (bar_x + bar_w).min(viewport_clip[0] + viewport_clip[2]);
    let clip_bottom = (bar_y + bar_h + 14.0 * s).min(viewport_clip[1] + viewport_clip[3]);
    if clip_right <= clip_x || clip_bottom <= clip_y {
        return;
    }
    let text_clip = [clip_x, clip_y, clip_right - clip_x, clip_bottom - clip_y];

    let connector_phase = now_seconds * 3.1;
    let connector_alpha = 0.55 + (connector_phase.sin() * 0.5 + 0.5) * 0.35;
    let connector_opts = DrawOpts {
        font_size: 14.0 * s,
        color: theme.u8_alpha(accent, connector_alpha),
        bold: true,
        clip_rect: Some(text_clip),
        ..DrawOpts::default()
    };
    let connector_x = bar_x + 8.0 * s;
    let connector_y =
        primary_y + (status_line_h - connector_opts.font_size) * 0.5 - 1.0 * s;
    draw_text_clipped(
        sugarloaf,
        connector_x,
        connector_y,
        "╰─",
        &connector_opts,
        occlusion_rects,
    );

    // Per-letter scramble like the terminal composer's `>>>` chevrons:
    // each character cycles through punctuation under rainbow hues until
    // its lock_threshold passes. Once locked, the word keeps a travelling
    // letter wave plus a faint trailing echo so the status still reads as
    // live motion while the model is working.
    let display_label = if label_text.is_empty() {
        "Background".to_string()
    } else {
        label_text
    };
    const SCRAMBLE_TOTAL: f32 = 0.7;
    let chars: Vec<char> = display_label.chars().collect();
    let lock_per_char = SCRAMBLE_TOTAL / (chars.len().max(1) as f32);
    let frame = (now_seconds * 44.0) as usize;
    const SCRAMBLE: &[u8] = b"|/-\\+!?>?<%#=@*~&^$";

    let word_opts = DrawOpts {
        font_size: 14.0 * s,
        bold: true,
        italic: true,
        clip_rect: Some(text_clip),
        ..DrawOpts::default()
    };
    let word_y = primary_y + (status_line_h - word_opts.font_size) * 0.5 - 1.0 * s;
    let word_motion = live_phase * 3.0;
    let word_drift_x = word_motion.sin() * 1.8 * s;
    let word_drift_y = (word_motion * 0.72).cos() * 0.8 * s;
    let mut cursor_x = bar_x + 34.0 * s + word_drift_x;
    for (ix, target_ch) in chars.iter().enumerate() {
        let lock_threshold = (ix as f32 + 1.0) * lock_per_char;
        let locked = transition >= lock_threshold;
        let mut opts = word_opts;
        let display = if locked {
            *target_ch
        } else {
            let scramble_ix = (frame + ix * 5) % SCRAMBLE.len();
            SCRAMBLE[scramble_ix] as char
        };
        // Rainbow during scramble; locked letters get a slow wave that
        // shifts a few shades around the accent so the word still feels
        // alive after it settles. `Generating` ("Crafting") spreads
        // hues across every letter at once so the whole word reads as
        // a slow-moving rainbow instead of a single accent shade.
        let color = if locked {
            let wave = ((live_phase * 3.4) + ix as f32 * 0.62).sin() * 0.5 + 0.5;
            let pulse = ((live_phase * 6.2) + ix as f32 * 0.9).sin() * 0.5 + 0.5;
            let lightness = 0.52 + wave * 0.16 + pulse * 0.08;
            if matches!(state, AgentStreamingStatus::Generating) {
                let hue = (live_phase * 70.0 + ix as f32 * 42.0).rem_euclid(360.0);
                hsl_to_u8_simple(hue, 0.8, lightness)
            } else {
                let base_hue = match state {
                    AgentStreamingStatus::Thinking => 300.0,
                    AgentStreamingStatus::Working => 52.0,
                    AgentStreamingStatus::Compacting => 158.0,
                    AgentStreamingStatus::WaitingSubagents => 48.0,
                    AgentStreamingStatus::BackgroundTasks => 0.0,
                    _ => 0.0,
                };
                let hue = (base_hue + wave * 18.0 - 9.0).rem_euclid(360.0);
                hsl_to_u8_simple(hue, 0.65, lightness)
            }
        } else {
            // Rainbow scramble — wider hue sweep + brighter saturation.
            let speed = 320.0 + (1.0 - (transition / SCRAMBLE_TOTAL).min(1.0)) * 220.0;
            let hue = (now_seconds * speed + ix as f32 * 48.0).rem_euclid(360.0);
            hsl_to_u8_simple(hue, 1.0, 0.62)
        };
        opts.color = color;
        // Locked letters ride a travelling wave. Scrambling letters get
        // a smaller shake so the word feels active before it resolves.
        let wave_phase = live_phase * 5.6 + ix as f32 * 0.82;
        let (sway_x, lift_y) = if locked {
            (
                wave_phase.cos() * 1.5 * s,
                wave_phase.sin() * 2.4 * s + word_drift_y,
            )
        } else {
            (
                (wave_phase * 1.7).sin() * 0.8 * s,
                (wave_phase * 1.9).cos() * 0.9 * s,
            )
        };
        let mut buf = [0u8; 4];
        let glyph = display.encode_utf8(&mut buf);
        draw_text_clipped(
            sugarloaf,
            cursor_x + sway_x,
            word_y - lift_y,
            glyph,
            &opts,
            occlusion_rects,
        );
        cursor_x += sugarloaf.text_mut().measure(glyph, &opts);
    }

    // Animated `...` after the word — anchored down by the letters' feet
    // and moving like a small ocean swell instead of a simple loader bounce.
    let dot_opts = DrawOpts {
        font_size: 14.0 * s,
        color: theme.u8(theme.muted),
        bold: true,
        clip_rect: Some(text_clip),
        ..DrawOpts::default()
    };
    cursor_x += 7.0 * s;
    let dot_floor_y = word_y + 6.2 * s;
    for ix in 0..3 {
        let phase = live_phase * 4.0 + ix as f32 * 0.95;
        let swell = phase.sin();
        let backwash = (phase * 0.55 + 1.2).cos();
        let lift = (swell * 0.5 + 0.5) * 1.7 * s;
        let drift = backwash * 1.0 * s;
        let alpha = 0.40 + (swell * 0.5 + 0.5) * 0.45;
        let mut opts = dot_opts;
        opts.color = theme.u8_alpha(accent, alpha);
        draw_text_clipped(
            sugarloaf,
            cursor_x + drift,
            dot_floor_y - lift,
            ".",
            &opts,
            occlusion_rects,
        );
        cursor_x += sugarloaf.text_mut().measure(".", &opts) + 2.0 * s;
    }

    cursor_x += 8.0 * s;
    let detail = match state {
        AgentStreamingStatus::Thinking => "thinking",
        AgentStreamingStatus::Working => "tools",
        AgentStreamingStatus::Generating => "reply",
        AgentStreamingStatus::Compacting => "context",
        AgentStreamingStatus::WaitingSubagents => "subagents",
        AgentStreamingStatus::BackgroundTasks => "running",
        AgentStreamingStatus::Idle => "idle",
    };
    let time_label = format!("· {} · {detail}", format_elapsed(elapsed));
    let time_opts = DrawOpts {
        font_size: 13.0 * s,
        color: theme.u8(theme.muted),
        italic: true,
        clip_rect: Some(text_clip),
        ..DrawOpts::default()
    };
    draw_text_clipped(
        sugarloaf,
        cursor_x,
        word_y,
        &time_label,
        &time_opts,
        occlusion_rects,
    );

    if queued_count > 0 {
        let queue_text = if queued_count == 1 {
            "queued message".to_string()
        } else {
            format!("queued messages ({queued_count})")
        };
        let queue_opts = DrawOpts {
            font_size: 13.0 * s,
            color: theme.u8(theme.accent),
            italic: true,
            clip_rect: Some(text_clip),
            ..DrawOpts::default()
        };
        let queue_y =
            primary_y + status_line_h + (status_line_h - queue_opts.font_size) * 0.5
                - 1.0 * s;
        draw_text_clipped(
            sugarloaf,
            bar_x + 34.0 * s,
            queue_y,
            &queue_text,
            &queue_opts,
            occlusion_rects,
        );
    }
    if background_count > 0 {
        let plural = if background_count == 1 {
            "task"
        } else {
            "tasks"
        };
        let bg_text = format!("╰─ {background_count} background {plural} running");
        let bg_opts = DrawOpts {
            font_size: 12.0 * s,
            color: theme.u8(theme.muted),
            italic: true,
            clip_rect: Some(text_clip),
            ..DrawOpts::default()
        };
        let bg_y = primary_y
            + status_line_h
            + if queued_count > 0 { status_line_h } else { 0.0 }
            + (status_line_h - bg_opts.font_size) * 0.5
            - 1.0 * s;
        pane.register_background_status_rect([
            bar_x + 40.0 * s,
            bg_y - 8.0 * s,
            (bar_w - 80.0 * s).max(80.0 * s),
            status_line_h,
        ]);
        draw_text_clipped(
            sugarloaf,
            bar_x + 48.0 * s,
            bg_y,
            &bg_text,
            &bg_opts,
            occlusion_rects,
        );
    } else {
        pane.clear_background_status_rect();
    }
}

pub fn measure_permission_prompt_height(pane: &impl AgentUserInputPane, s: f32) -> f32 {
    let Some(permission) = pane.pending_permission() else {
        return 0.0;
    };
    let pattern_rows = permission.patterns().len().min(2) as f32;
    (122.0 + pattern_rows * 16.0 + 60.0) * s
}

#[allow(clippy::too_many_arguments)]
pub fn render_permission_prompt_row(
    sugarloaf: &mut Sugarloaf,
    pane: &mut impl AgentUserInputPane,
    rect: [f32; 4],
    theme: &IdeTheme,
    s: f32,
    viewport_clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) {
    let Some(permission) = pane.pending_permission().cloned() else {
        return;
    };
    let [x, y, w, h] = rect;
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    draw_rounded_rect_clipped(
        sugarloaf,
        [x, y, w, h],
        theme.f32(theme.surface),
        10.0 * s,
        ORDER_PANEL,
        viewport_clip,
    );
    draw_rect_clipped(
        sugarloaf,
        [x, y + 10.0 * s, 3.0 * s, (h - 20.0 * s).max(0.0)],
        theme.f32(theme.yellow),
        ORDER_TEXT,
        viewport_clip,
    );

    let title_opts = DrawOpts {
        font_size: 12.0 * s,
        color: theme.u8(theme.yellow),
        bold: true,
        clip_rect: Some(viewport_clip),
        ..DrawOpts::default()
    };
    let header = if permission
        .parent_session_id()
        .is_some_and(|parent| Some(parent) == pane.session_id_str())
    {
        if let Some(agent) = permission.source_agent() {
            format!("Subagent @{agent} permission")
        } else {
            "Subagent permission required".to_string()
        }
    } else {
        "Permission required".to_string()
    };
    draw_text_clipped(
        sugarloaf,
        x + 18.0 * s,
        y + 12.0 * s,
        &header,
        &title_opts,
        occlusion_rects,
    );

    let body_opts = DrawOpts {
        font_size: 13.5 * s,
        color: theme.u8(theme.fg),
        bold: true,
        clip_rect: Some(viewport_clip),
        ..DrawOpts::default()
    };
    let body = if permission.title().trim().is_empty() {
        "Allow tool?"
    } else {
        permission.title()
    };
    draw_text_clipped(
        sugarloaf,
        x + 18.0 * s,
        y + 34.0 * s,
        body,
        &body_opts,
        occlusion_rects,
    );

    let meta_opts = DrawOpts {
        font_size: 11.0 * s,
        color: theme.u8(theme.muted),
        clip_rect: Some(viewport_clip),
        ..DrawOpts::default()
    };
    let mut meta_y = y + 55.0 * s;
    let permission_label = if permission.permission().trim().is_empty() {
        "tool"
    } else {
        permission.permission()
    };
    draw_text_clipped(
        sugarloaf,
        x + 18.0 * s,
        meta_y,
        &format!("permission: {permission_label}"),
        &meta_opts,
        occlusion_rects,
    );
    meta_y += 16.0 * s;
    for pattern in permission.patterns().iter().take(2) {
        draw_text_clipped(
            sugarloaf,
            x + 18.0 * s,
            meta_y,
            pattern,
            &meta_opts,
            occlusion_rects,
        );
        meta_y += 16.0 * s;
    }

    // Visual order Always, Yes, No; the second tuple field is the index
    // into NeoismAgentPendingPermission.selected (0=Yes, 1=Always, 2=No),
    // which the keyboard handler in screen/bridges/agent.rs depends on.
    let choices = [
        ("Always", "a", 1usize),
        ("Yes", "enter", 0usize),
        ("No", "n", 2usize),
    ];
    let row_h = 30.0 * s;
    let choice_x = x + 18.0 * s;
    let first_row_y = y + h - 37.0 * s - row_h * (choices.len() as f32 - 1.0);
    for (visual_index, (label, hint, selected_index)) in choices.iter().enumerate() {
        let row_y = first_row_y + row_h * visual_index as f32;
        let selected =
            *selected_index == permission.selected() && !permission.responding();
        let text = if selected {
            format!("> {label}")
        } else {
            format!("  {label}")
        };
        let opts = DrawOpts {
            font_size: 12.0 * s,
            color: if selected {
                theme.u8(theme.black)
            } else if *selected_index == 2 {
                theme.u8(theme.red)
            } else {
                theme.u8(theme.dim)
            },
            bold: selected,
            clip_rect: Some(viewport_clip),
            ..DrawOpts::default()
        };
        let hint_opts = DrawOpts {
            font_size: 10.5 * s,
            color: if selected {
                theme.u8(theme.black)
            } else {
                theme.u8(theme.muted)
            },
            clip_rect: Some(viewport_clip),
            ..DrawOpts::default()
        };
        let text_w = sugarloaf.text_mut().measure(&text, &opts);
        let hint_w = sugarloaf.text_mut().measure(hint, &hint_opts);
        let chip_w = (text_w + hint_w + 24.0 * s).max(58.0 * s);
        let hit_rect = [choice_x - 8.0 * s, row_y - 6.0 * s, chip_w, 25.0 * s];
        let choice = match *selected_index {
            1 => AgentPermissionChoice::Always,
            2 => AgentPermissionChoice::Reject,
            _ => AgentPermissionChoice::Once,
        };
        pane.register_permission_choice_rect(choice, hit_rect);
        if selected {
            draw_rounded_rect_clipped(
                sugarloaf,
                hit_rect,
                theme.f32(theme.yellow),
                5.0 * s,
                ORDER_TEXT,
                viewport_clip,
            );
        }
        draw_text_clipped(sugarloaf, choice_x, row_y, &text, &opts, occlusion_rects);
        draw_text_clipped(
            sugarloaf,
            choice_x + chip_w - hint_w - 12.0 * s,
            row_y + 1.0 * s,
            hint,
            &hint_opts,
            occlusion_rects,
        );
    }

    if permission.responding() {
        let opts = DrawOpts {
            font_size: 12.0 * s,
            color: theme.u8(theme.muted),
            italic: true,
            clip_rect: Some(viewport_clip),
            ..DrawOpts::default()
        };
        draw_text_clipped(
            sugarloaf,
            x + w - 104.0 * s,
            y + 13.0 * s,
            "sending",
            &opts,
            occlusion_rects,
        );
    }
}

pub fn render_status_chips(
    sugarloaf: &mut Sugarloaf,
    pane: &mut impl AgentUserInputPane,
    mut x: f32,
    y: f32,
    max_w: f32,
    theme: &IdeTheme,
    s: f32,
    occlusion_rects: &[[f32; 4]],
) {
    // Dropdown-look chips: label ˅ — each registers a hit rect so a
    // click opens the matching "/" picker (agent / model / thinking).
    // Colors keep the old chip identity (agent = accent, model = blue,
    // thinking = magenta).
    let chips: [(String, u32); 3] = [
        (pane.agent_label().to_string(), theme.accent),
        (pane.model().to_string(), theme.blue),
        (pane.thinking_label().to_string(), theme.magenta),
    ];
    let font_size = 13.5 * s;
    let caret = "\u{f078}";
    let caret_opts = DrawOpts {
        font_size: font_size * 0.66,
        color: theme.u8(theme.muted),
        ..DrawOpts::default()
    };
    let caret_w = sugarloaf.text_mut().measure(caret, &caret_opts);
    let start_x = x;
    for (index, (label, color)) in chips.into_iter().enumerate() {
        if label.is_empty() {
            continue;
        }
        let opts = DrawOpts {
            font_size,
            color: theme.u8(color),
            bold: true,
            ..DrawOpts::default()
        };
        let label_w = sugarloaf.text_mut().measure(&label, &opts);
        let chip_w = label_w + 6.0 * s + caret_w + 18.0 * s;
        if x + chip_w > start_x + max_w {
            break;
        }
        draw_text_clipped(sugarloaf, x, y, &label, &opts, occlusion_rects);
        draw_text_clipped(
            sugarloaf,
            x + label_w + 6.0 * s,
            y + 3.5 * s,
            caret,
            &caret_opts,
            occlusion_rects,
        );
        pane.register_status_chip_rect(
            index,
            [
                x - 4.0 * s,
                y - 5.0 * s,
                chip_w - 8.0 * s,
                font_size + 10.0 * s,
            ],
        );
        x += chip_w;
    }
}

fn draw_agent_prompt_text(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    text: &str,
    opts: &DrawOpts,
    theme: &IdeTheme,
    occlusion_rects: &[[f32; 4]],
) {
    let spans = agent_prompt_link_spans(text);
    if spans.is_empty() {
        draw_text_clipped(sugarloaf, x, y, text, opts, occlusion_rects);
        return;
    }

    let mut link_opts = *opts;
    link_opts.color = theme.u8(theme.blue);

    let mut segment_x = x;
    let mut cursor = 0;
    for (start, end) in spans {
        if start > cursor {
            let segment = &text[cursor..start];
            draw_text_clipped(sugarloaf, segment_x, y, segment, opts, occlusion_rects);
            segment_x += sugarloaf.text_mut().measure(segment, opts);
        }

        let mention = &text[start..end];
        draw_text_clipped(
            sugarloaf,
            segment_x,
            y,
            mention,
            &link_opts,
            occlusion_rects,
        );
        segment_x += sugarloaf.text_mut().measure(mention, &link_opts);
        cursor = end;
    }

    if cursor < text.len() {
        draw_text_clipped(
            sugarloaf,
            segment_x,
            y,
            &text[cursor..],
            opts,
            occlusion_rects,
        );
    }
}

fn wrap_agent_prompt_lines(
    sugarloaf: &mut Sugarloaf,
    text: &str,
    max_w: f32,
    opts: &DrawOpts,
) -> Vec<String> {
    wrap_agent_prompt_ranges(sugarloaf, text, max_w, opts)
        .into_iter()
        .map(|(start, end)| text[start..end].to_string())
        .collect()
}

/// Running wrap state for [`wrap_agent_prompt_ranges`]: the visual row
/// being built covers `text[start..end]` at accumulated width `width`.
#[derive(Default)]
struct PromptWrapRanges {
    lines: Vec<(usize, usize)>,
    start: usize,
    end: usize,
    width: f32,
}

impl PromptWrapRanges {
    fn break_soft(&mut self) {
        self.lines.push((self.start, self.end));
        self.start = self.end;
        self.width = 0.0;
    }
}

/// Byte spans of each visual row the prompt wraps into. This is the
/// wrap CORE — `wrap_agent_prompt_lines` (drawing) slices these same
/// spans, and the pane registers them for Up/Down cursor movement, so
/// layout and navigation can never disagree about row boundaries.
/// Rows exclude their terminating `\n`.
fn wrap_agent_prompt_ranges(
    sugarloaf: &mut Sugarloaf,
    text: &str,
    max_w: f32,
    opts: &DrawOpts,
) -> Vec<(usize, usize)> {
    let token_spans = agent_attachment_token_spans(text);
    let mut wrap = PromptWrapRanges::default();
    let mut cursor = 0;

    for (start, end) in token_spans {
        push_wrapped_prompt_segment(
            sugarloaf, text, cursor, start, max_w, opts, &mut wrap,
        );
        let token = &text[start..end];
        let token_w = sugarloaf.text_mut().measure(token, opts);
        if wrap.end > wrap.start && wrap.width + token_w > max_w {
            wrap.break_soft();
        }
        wrap.end = end;
        wrap.width += token_w;
        cursor = end;
    }

    push_wrapped_prompt_segment(
        sugarloaf,
        text,
        cursor,
        text.len(),
        max_w,
        opts,
        &mut wrap,
    );
    if wrap.end > wrap.start || wrap.lines.is_empty() {
        wrap.lines.push((wrap.start, wrap.end));
    }
    wrap.lines
}

fn push_wrapped_prompt_segment(
    sugarloaf: &mut Sugarloaf,
    text: &str,
    seg_start: usize,
    seg_end: usize,
    max_w: f32,
    opts: &DrawOpts,
    wrap: &mut PromptWrapRanges,
) {
    let mut ix = seg_start;
    for ch in text[seg_start..seg_end].chars() {
        let ch_len = ch.len_utf8();
        if ch == '\n' {
            wrap.lines.push((wrap.start, wrap.end));
            wrap.start = ix + ch_len;
            wrap.end = wrap.start;
            wrap.width = 0.0;
            ix += ch_len;
            continue;
        }
        let mut buf = [0; 4];
        let s = ch.encode_utf8(&mut buf);
        let ch_w = sugarloaf.text_mut().measure(s, opts);
        if wrap.end > wrap.start && wrap.width + ch_w > max_w {
            wrap.lines.push((wrap.start, wrap.end));
            wrap.start = ix;
            wrap.width = 0.0;
        }
        wrap.end = ix + ch_len;
        wrap.width += ch_w;
        ix += ch_len;
    }
}

fn agent_prompt_link_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = agent_file_mention_spans(text);
    spans.extend(agent_attachment_token_spans(text));
    spans.extend(agent_skill_token_spans(text));
    spans.sort_by_key(|(start, end)| (*start, *end));

    let mut merged = Vec::with_capacity(spans.len());
    for span in spans {
        if merged
            .last()
            .is_some_and(|(_, previous_end)| span.0 < *previous_end)
        {
            continue;
        }
        merged.push(span);
    }
    merged
}

fn agent_file_mention_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut chars = text.char_indices().peekable();

    while let Some((start, ch)) = chars.next() {
        if ch != '@' || !is_agent_mention_boundary(text, start) {
            continue;
        }

        let Some((_, next_ch)) = chars.peek().copied() else {
            continue;
        };
        if next_ch.is_whitespace() || next_ch == '@' {
            continue;
        }

        let mut end = text.len();
        while let Some((ix, mention_ch)) = chars.peek().copied() {
            if mention_ch.is_whitespace() {
                end = ix;
                break;
            }
            chars.next();
        }

        spans.push((start, end));
    }

    spans
}

fn is_agent_mention_boundary(text: &str, at_byte: usize) -> bool {
    if at_byte == 0 {
        return true;
    }

    text[..at_byte].chars().next_back().is_some_and(|ch| {
        ch.is_whitespace() || matches!(ch, '(' | '[' | '{' | '"' | '\'' | '`')
    })
}

fn agent_attachment_token_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut cursor = 0;

    while let Some(relative_start) = text[cursor..].find('[') {
        let start = cursor + relative_start;
        let content_start = start + 1;
        let Some(relative_end) = text[content_start..].find(']') else {
            break;
        };
        let end = content_start + relative_end + 1;
        let label = &text[content_start..end - 1];
        if is_agent_token_boundary(text, start) && is_agent_attachment_label(label) {
            spans.push((start, end));
        }
        cursor = end;
    }

    spans
}

fn agent_skill_token_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut chars = text.char_indices().peekable();

    while let Some((start, ch)) = chars.next() {
        if ch != '$' || !is_agent_token_boundary(text, start) {
            continue;
        }
        let Some((_, next_ch)) = chars.peek().copied() else {
            continue;
        };
        if !is_agent_skill_char(next_ch) {
            continue;
        }

        let mut end = text.len();
        while let Some((ix, skill_ch)) = chars.peek().copied() {
            if !is_agent_skill_char(skill_ch) {
                end = ix;
                break;
            }
            chars.next();
        }
        spans.push((start, end));
    }

    spans
}

fn is_agent_token_boundary(text: &str, at_byte: usize) -> bool {
    if at_byte == 0 {
        return true;
    }

    text[..at_byte].chars().next_back().is_some_and(|ch| {
        ch.is_whitespace() || matches!(ch, '(' | '[' | '{' | '"' | '\'' | '`')
    })
}

fn is_agent_skill_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')
}

fn is_agent_attachment_label(label: &str) -> bool {
    let label = label.trim();
    let lower = label.to_ascii_lowercase();
    is_numbered_agent_token(&lower, "image")
        || is_numbered_agent_token(&lower, "pdf")
        || is_file_agent_token(&lower)
        || is_pasted_agent_token(&lower)
}

fn is_numbered_agent_token(label: &str, prefix: &str) -> bool {
    let Some(rest) = label.strip_prefix(prefix) else {
        return false;
    };
    !rest.is_empty() && rest.chars().all(|ch| ch.is_ascii_digit())
}

fn is_file_agent_token(label: &str) -> bool {
    let Some(rest) = label.strip_prefix("file") else {
        return false;
    };
    let digit_count = rest
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .map(char::len_utf8)
        .sum::<usize>();
    if digit_count == 0 {
        return false;
    }
    let suffix = &rest[digit_count..];
    suffix.is_empty() || suffix.starts_with(": ")
}

fn is_pasted_agent_token(label: &str) -> bool {
    let Some(rest) = label.strip_prefix("pasted ") else {
        return false;
    };
    let mut parts = rest.split_whitespace();
    let Some(count) = parts.next() else {
        return false;
    };
    let Some(unit) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    count.chars().any(|ch| ch.is_ascii_digit())
        && count.chars().all(|ch| ch.is_ascii_digit() || ch == ',')
        && matches!(unit, "line" | "lines" | "char" | "chars")
}
