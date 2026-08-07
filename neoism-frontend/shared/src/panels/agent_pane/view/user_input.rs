use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::panels::agent_pane::input_controller::{visual_row_index, InputWrapRow};
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
    DEPTH, INPUT_HELP_STRIP_H, INPUT_LINE_H, MAX_INPUT_LINES, ORDER_CARET, ORDER_PANEL,
    ORDER_TEXT, STREAMING_STATUS_LINE_H,
};
use crate::panels::file_tree::FRAME_STROKE;
use crate::primitives::ide_theme::IdeTheme;
use crate::render_policy::opencode_scanner_frame;

/// Logical height of the dropdown-chip row (agent ˅ / model ˅ /
/// thinking ˅) rendered below the input box, inside the outer shell.
pub(super) const CHIPS_BAND_H: f32 = 26.0;

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
    Retrying,
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
    fn input_help_visible(&self) -> bool;
    fn cursor_byte(&self) -> usize;
    fn set_cursor_rect(&mut self, rect: Option<[f32; 4]>);
    fn set_input_wrap_rows(&mut self, rows: Vec<InputWrapRow>);
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
    fn clear_permission_choice_hit_rects(&mut self);
    /// Pending `question`-tool request shown by the prompt picker. Both
    /// panes store the shared `question_policy` struct directly, so no
    /// associated type is needed (unlike permissions, which predate the
    /// shared-state cutover).
    fn pending_question(
        &self,
    ) -> Option<&crate::panels::agent_pane::question_policy::NeoismAgentPendingQuestion>;
    fn clear_question_option_rects(&mut self);
    fn register_question_option_rect(&mut self, index: usize, rect: [f32; 4]);
    /// Card rect of the prompt picker drawn this frame (permission /
    /// question), `None` when no prompt is pending — feeds the same
    /// occlusion path as the "/" picker.
    fn set_prompt_picker_rect(&mut self, rect: Option<[f32; 4]>);
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

            fn input_help_visible(&self) -> bool {
                <$pane>::input_help_visible(self)
            }

            fn cursor_byte(&self) -> usize {
                <$pane>::cursor_byte(self)
            }

            fn set_cursor_rect(&mut self, rect: Option<[f32; 4]>) {
                <$pane>::set_cursor_rect(self, rect);
            }

            fn set_input_wrap_rows(
                &mut self,
                rows: Vec<
                    $crate::panels::agent_pane::input_controller::InputWrapRow,
                >,
            ) {
                <$pane>::set_input_wrap_rows(self, rows);
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
                    $streaming_state::Retrying => {
                        $crate::panels::agent_pane::view::user_input::AgentStreamingStatus::Retrying
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

            fn clear_permission_choice_hit_rects(&mut self) {
                <$pane>::clear_permission_choice_hit_rects(self);
            }

            fn pending_question(
                &self,
            ) -> Option<
                &$crate::panels::agent_pane::question_policy::NeoismAgentPendingQuestion,
            > {
                <$pane>::pending_question(self)
            }

            fn clear_question_option_rects(&mut self) {
                <$pane>::clear_question_option_rects(self);
            }

            fn register_question_option_rect(&mut self, index: usize, rect: [f32; 4]) {
                <$pane>::register_question_option_rect(self, index, rect);
            }

            fn set_prompt_picker_rect(&mut self, rect: Option<[f32; 4]>) {
                <$pane>::set_prompt_picker_rect(self, rect);
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

    fn input_help_visible(&self) -> bool {
        NeoismAgentPane::input_help_visible(self)
    }

    fn cursor_byte(&self) -> usize {
        NeoismAgentPane::cursor_byte(self)
    }

    fn set_cursor_rect(&mut self, rect: Option<[f32; 4]>) {
        NeoismAgentPane::set_cursor_rect(self, rect);
    }

    fn set_input_wrap_rows(&mut self, rows: Vec<InputWrapRow>) {
        NeoismAgentPane::set_input_wrap_rows(self, rows);
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
            NeoismAgentStreamingState::Retrying => AgentStreamingStatus::Retrying,
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

    fn clear_permission_choice_hit_rects(&mut self) {
        NeoismAgentPane::clear_permission_choice_hit_rects(self);
    }

    fn pending_question(
        &self,
    ) -> Option<&crate::panels::agent_pane::question_policy::NeoismAgentPendingQuestion>
    {
        NeoismAgentPane::pending_question(self)
    }

    fn clear_question_option_rects(&mut self) {
        NeoismAgentPane::clear_question_option_rects(self);
    }

    fn register_question_option_rect(&mut self, index: usize, rect: [f32; 4]) {
        NeoismAgentPane::register_question_option_rect(self, index, rect);
    }

    fn set_prompt_picker_rect(&mut self, rect: Option<[f32; 4]>) {
        NeoismAgentPane::set_prompt_picker_rect(self, rect);
    }
}

/// Logical size (pre-scale) of the user-message presence orb — roughly
/// one line height, matching the editor caret / top-chrome orb. The orb
/// now sits INSIDE the message bubble's top-left, with the text inset past
/// it (see `render_user_message`).
const USER_ORB_SIZE: f32 = 18.0;

/// Resolved presence identity for one user message: the deterministic
/// avatar `seed` (fed straight into `AvatarProfile::from_seed`) and the
/// `label` shown in the hover tooltip.
pub struct UserMessageOrbIdentity {
    pub seed: String,
    pub label: String,
}

/// THE choke point that turns a user message's optional `author` name
/// into its presence orb + hover tooltip — the single, deliberately
/// small "author name → orb" seam. A future integration only has to set
/// [`NeoismAgentMessage::author`] (from the shared-session sender, a
/// plugin, wherever) and both the orb seed and the hover label follow;
/// nothing else decides either. Deterministic-from-name is the whole
/// hook, so the author stays a plain name string.
///
/// `local_name` is the local peer's presence display name (the same seed
/// the editor caret / top-chrome orb use). A message with no explicit
/// author is the local user's own message, so it seeds off `local_name`
/// — matching your caret orb — and labels the tooltip "You".
///
/// [`NeoismAgentMessage::author`]:
///   crate::panels::agent_pane::state::NeoismAgentMessage
pub fn user_message_orb_identity(
    author: Option<&str>,
    local_name: Option<&str>,
) -> UserMessageOrbIdentity {
    match author.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => UserMessageOrbIdentity {
            seed: name.to_string(),
            label: name.to_string(),
        },
        None => {
            let seed = local_name
                .map(str::trim)
                .filter(|name| !name.is_empty())
                // No presence name published — a stable generic seed so
                // the orb is still deterministic (and never empty).
                .unwrap_or("you")
                .to_string();
            UserMessageOrbIdentity {
                seed,
                label: "You".to_string(),
            }
        }
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
    // Presence identity resolved by `user_message_orb_identity` — the
    // orb seed (a display name) and the hover-tooltip label.
    orb_seed: &str,
    orb_label: &str,
    // Live cursor position, for the orb hover tooltip. `None` = no hover.
    mouse: Option<(f32, f32)>,
    theme: &IdeTheme,
    s: f32,
    viewport_clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) -> f32 {
    // The grey bubble spans the FULL row now; the orb lives INSIDE it at
    // the top-left (avatar-left chat layout) with the text indented past
    // it — it used to float in a transparent left gutter outside the card.
    let bubble_x = x;
    let bubble_w = w.max(160.0 * s);
    draw_rect_clipped(
        sugarloaf,
        [bubble_x, y, bubble_w, h],
        theme.f32(theme.surface),
        ORDER_PANEL,
        viewport_clip,
    );
    // Sender presence orb INSIDE the bubble, aligned to the first text line.
    // Deterministic in `orb_seed` (a display name), so the same author
    // always wears the same round pixel-plasma orb — the very generator the
    // editor carets / top-chrome face-pile use. MUST feed the dedicated
    // `presence_orb_now_seconds()` clock (a small wrapped value), NOT the
    // render's raw epoch `now_seconds`: at ~1.75e9 an f32 can't resolve a
    // 16ms frame step, so the plasma would animate then stall. The
    // It advances on normal pane invalidations without forcing idle chats to
    // repaint continuously.
    let orb = USER_ORB_SIZE * s;
    let pad_x = 14.0 * s;
    let orb_x = bubble_x + pad_x;
    let orb_y = y + 12.0 * s;
    crate::editor::markdown::render::draw::draw_presence_orb_clipped(
        sugarloaf,
        viewport_clip,
        orb_seed,
        orb_x,
        orb_y,
        orb,
        crate::editor::crdt::presence_orb_now_seconds(),
        DEPTH,
        ORDER_TEXT,
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
    // Text sits to the RIGHT of the orb, inside the same grey bubble.
    let text_x = orb_x + orb + 10.0 * s;
    let text_w = (bubble_x + bubble_w - text_x - pad_x).max(80.0 * s);
    let mut line_y = y + 12.0 * s;
    for line in wrap_text(sugarloaf, text, text_w, &opts, 6) {
        draw_agent_prompt_text(
            sugarloaf,
            text_x,
            line_y,
            &line,
            &opts,
            theme,
            occlusion_rects,
        );
        line_y += 19.0 * s;
    }
    // Hover tooltip: when the cursor is over the orb, name the sender —
    // the same read as the top-chrome presence face-pile. Hit-test the
    // live mouse against the orb rect (drawn this frame, so it can't go
    // stale) and draw a small surface pill below it.
    if let Some((mx, my)) = mouse {
        if mx >= orb_x && mx <= orb_x + orb && my >= orb_y && my <= orb_y + orb {
            draw_user_orb_tooltip(
                sugarloaf,
                orb_x,
                orb_y,
                orb,
                orb_label,
                theme,
                s,
                viewport_clip,
            );
        }
    }
    h
}

/// Small name pill under a hovered user-message orb. Mirrors the
/// top-chrome presence tooltip: a surface rounded-rect + label, clamped
/// inside the timeline's viewport, painted above the message cards.
#[allow(clippy::too_many_arguments)]
fn draw_user_orb_tooltip(
    sugarloaf: &mut Sugarloaf,
    orb_x: f32,
    orb_y: f32,
    orb: f32,
    label: &str,
    theme: &IdeTheme,
    s: f32,
    clip: [f32; 4],
) {
    let font_size = 11.0 * s;
    let opts = DrawOpts {
        font_size,
        color: theme.u8(theme.fg),
        ..DrawOpts::default()
    };
    let pad_x = 7.0 * s;
    let tip_h = 20.0 * s;
    let tip_w = sugarloaf.text_mut().measure(label, &opts) + pad_x * 2.0;
    let margin = 4.0 * s;
    let min_x = clip[0] + margin;
    let max_x = (clip[0] + clip[2] - tip_w - margin).max(min_x);
    let tip_x = (orb_x + orb * 0.5 - tip_w * 0.5).clamp(min_x, max_x);
    let tip_y = orb_y + orb + margin;
    draw_rounded_rect_clipped(
        sugarloaf,
        [tip_x, tip_y, tip_w, tip_h],
        theme.f32(theme.surface),
        5.0 * s,
        ORDER_CARET + 1,
        clip,
    );
    let mut text_opts = opts;
    text_opts.clip_rect = Some([tip_x, tip_y, tip_w, tip_h]);
    sugarloaf.text_mut().draw(
        tip_x + pad_x,
        tip_y + (tip_h - font_size) * 0.5,
        label,
        &text_opts,
    );
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
    prepared_wrap_rows: Option<&[InputWrapRow]>,
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
    let outer_stroke = (FRAME_STROKE * s).max(2.0);
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
        theme.f32(theme.surface),
        DEPTH,
        (corner_radius - border_w).max(0.0),
        ORDER_PANEL + 1,
    );

    let usage_label = pane.usage_summary_label();
    let usage_opts = DrawOpts {
        font_size: 11.5 * s,
        color: theme.u8(theme.readable_accent(theme.cyan)),
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
    let wrapped_rows = prepared_wrap_rows
        .map(|rows| rows.to_vec())
        .unwrap_or_else(|| wrap_agent_prompt_rows(sugarloaf, text, text_w, &opts));
    // Register the visual rows (byte spans + per-boundary x offsets)
    // back on the pane so Up/Down arrow movement walks the exact rows
    // drawn below AND matches their proportional-font caret positions
    // (see `AgentInputBuffer::move_up_with_history_visual`). The
    // placeholder isn't the draft, so an empty input registers no rows.
    pane.set_input_wrap_rows(if input_text.is_empty() {
        Vec::new()
    } else {
        wrapped_rows.clone()
    });

    // The box only shows `max_visible_lines` rows at once. Scroll so the
    // caret's row is always one of them — otherwise pressing Up into a
    // row above the window moves the (invisible) cursor while the text
    // stays put, which is exactly the "cursor going up is screwed up"
    // report. The window is anchored on the caret, not pinned to the
    // bottom, so it follows the cursor in both directions.
    let caret_row = if input_text.is_empty() {
        0
    } else {
        visual_row_index(&wrapped_rows, pane.cursor_byte())
            .unwrap_or(wrapped_rows.len().saturating_sub(1))
    };
    let max_offset = wrapped_rows.len().saturating_sub(max_visible_lines);
    let visible_line_offset = caret_row
        .saturating_sub(max_visible_lines.saturating_sub(1))
        .min(max_offset);
    let visible_rows = &wrapped_rows[visible_line_offset..];
    for (ix, wrapped) in visible_rows.iter().take(max_visible_lines).enumerate() {
        draw_agent_prompt_text(
            sugarloaf,
            text_x,
            text_y + ix as f32 * line_h,
            &text[wrapped.start..wrapped.end],
            &opts,
            theme,
            occlusion_rects,
        );
    }

    if active {
        // Place the caret from the SAME rows the movement logic walks,
        // so the painted caret and the logical cursor never disagree.
        let caret_x_in_row = wrapped_rows
            .get(caret_row)
            .map(|row| {
                let col = pane.input()
                    [row.start..pane.cursor_byte().clamp(row.start, row.end)]
                    .chars()
                    .count();
                row.offsets
                    .get(col)
                    .or_else(|| row.offsets.last())
                    .copied()
                    .unwrap_or(0.0)
            })
            .unwrap_or(0.0);
        let caret_visible_row = caret_row.saturating_sub(visible_line_offset);
        let caret_y = text_y + caret_visible_row as f32 * line_h;
        let caret_x = (text_x + caret_x_in_row)
            .min(box_x + box_w - 28.0 * s)
            .max(text_x);
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

    // Usage chip sits at the FAR LEFT of the send-button line.
    if let Some(label) = usage_label.as_deref() {
        if usage_chip_w > 0.0 {
            let usage_h = 22.0 * s;
            let usage_x = box_x + 16.0 * s;
            let usage_y = send_y + (send_side - usage_h) * 0.5;
            if usage_x + usage_chip_w <= send_x - 8.0 * s {
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
    // Square send button: filled rounded square, bottom-right. While the
    // model responds it becomes a quiet static stop-square; avoid layering
    // another activity animation here because the status row already owns
    // the live response treatment. Enter still submits — the button mirrors
    // state.
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
        // Running state: a static dark stop-square.
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

    if pane.input_help_visible() {
        render_input_help_strip(
            sugarloaf,
            pane,
            [
                x + 8.0 * s,
                y + h + 1.0 * s,
                (w - 16.0 * s).max(0.0),
                (INPUT_HELP_STRIP_H - 7.0) * s,
            ],
            theme,
            s,
            now_seconds,
            occlusion_rects,
        );
    }
}

/// Small OpenCode-style legend below the composer. The left side only
/// advertises interruption while a run is actually active; the right
/// side mirrors the two composer shortcuts that are always available.
fn render_input_help_strip(
    sugarloaf: &mut Sugarloaf,
    pane: &impl AgentUserInputPane,
    rect: [f32; 4],
    theme: &IdeTheme,
    s: f32,
    now_seconds: f32,
    occlusion_rects: &[[f32; 4]],
) {
    let [x, y, w, h] = rect;
    if w <= 0.0 || h <= 0.0 {
        return;
    }

    let clip = [x, y, w, h];
    let key_opts = DrawOpts {
        font_size: 12.5 * s,
        color: theme.u8(theme.fg),
        bold: true,
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let label_opts = DrawOpts {
        font_size: 12.5 * s,
        color: theme.u8(theme.muted),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let baseline_y = y + (h - key_opts.font_size) * 0.5;

    let mut activity_guard = x;
    if !matches!(pane.streaming_state(), AgentStreamingStatus::Idle) {
        let scanner_seconds = pane.streaming_elapsed_seconds().unwrap_or(now_seconds);
        let activity_w = draw_opencode_activity_scanner(
            sugarloaf,
            x,
            baseline_y,
            12.5 * s,
            scanner_seconds,
            clip,
            occlusion_rects,
        );
        let esc_x = x + activity_w + 10.0 * s;
        draw_text_clipped(
            sugarloaf,
            esc_x,
            baseline_y,
            "esc",
            &key_opts,
            occlusion_rects,
        );
        let esc_w = sugarloaf.text_mut().measure("esc", &key_opts);
        draw_text_clipped(
            sugarloaf,
            esc_x + esc_w + 7.0 * s,
            baseline_y,
            "interrupt",
            &label_opts,
            occlusion_rects,
        );
        activity_guard = esc_x
            + esc_w
            + 7.0 * s
            + sugarloaf.text_mut().measure("interrupt", &label_opts)
            + 12.0 * s;
    }

    let command_label = "commands";
    let slash = "/";
    let tab = "tab";
    let agents = "agents";
    let command_label_w = sugarloaf.text_mut().measure(command_label, &label_opts);
    let slash_w = sugarloaf.text_mut().measure(slash, &key_opts);
    let agents_w = sugarloaf.text_mut().measure(agents, &label_opts);
    let tab_w = sugarloaf.text_mut().measure(tab, &key_opts);
    let group_gap = 26.0 * s;
    let inner_gap = 7.0 * s;
    let right_w =
        tab_w + inner_gap + agents_w + group_gap + slash_w + inner_gap + command_label_w;
    let right_x = x + (w - right_w).max(0.0);

    // On narrow panes prefer the actionable slash-command hint. The
    // Tab group is omitted instead of being allowed to overlap the live
    // interruption group on the left.
    let slash_x = right_x + tab_w + inner_gap + agents_w + group_gap;
    if right_x >= activity_guard {
        draw_text_clipped(
            sugarloaf,
            right_x,
            baseline_y,
            tab,
            &key_opts,
            occlusion_rects,
        );
        draw_text_clipped(
            sugarloaf,
            right_x + tab_w + inner_gap,
            baseline_y,
            agents,
            &label_opts,
            occlusion_rects,
        );
    }
    let slash_x = if right_x >= activity_guard {
        slash_x
    } else {
        x + (w - slash_w - inner_gap - command_label_w).max(0.0)
    };
    draw_text_clipped(
        sugarloaf,
        slash_x,
        baseline_y,
        slash,
        &key_opts,
        occlusion_rects,
    );
    draw_text_clipped(
        sugarloaf,
        slash_x + slash_w + inner_gap,
        baseline_y,
        command_label,
        &label_opts,
        occlusion_rects,
    );
}

/// Paint the exact eight-cell block scanner used by OpenCode's TUI.
///
/// OpenCode renders each cell as terminal text, so using the same `■` /
/// `⬝` glyphs here matters: rounded quads or an orbit read differently.
/// Returns the occupied width so callers can place their label one cell
/// after the scanner just like the TUI's `gap={1}`.
#[allow(clippy::too_many_arguments)]
fn draw_opencode_activity_scanner(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    font_size: f32,
    now_seconds: f32,
    clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) -> f32 {
    let base_opts = DrawOpts {
        font_size,
        color: [255, 255, 255, 255],
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let solid_w = sugarloaf.text_mut().measure("■", &base_opts);
    let dot_w = sugarloaf.text_mut().measure("⬝", &base_opts);
    let cell_w = solid_w.max(dot_w);
    let frame = opencode_scanner_frame(now_seconds);

    for (index, cell) in frame.into_iter().enumerate() {
        let brightness = if cell.active { 1.0 } else { cell.brightness };
        let mut opts = base_opts;
        let value = (255.0 * brightness).round() as u8;
        opts.color = [
            value,
            value,
            value,
            (cell.alpha.clamp(0.0, 1.0) * 255.0) as u8,
        ];
        let glyph = if cell.active { "■" } else { "⬝" };
        let mut far_depth_opts = opts;
        far_depth_opts.color = [12, 12, 16, opts.color[3].saturating_mul(2) / 3];
        crate::primitives::draw_text_with_occlusion(
            sugarloaf,
            x + index as f32 * cell_w + 3.0,
            y + 3.0,
            glyph,
            &far_depth_opts,
            occlusion_rects,
        );
        let mut near_depth_opts = opts;
        near_depth_opts.color = [58, 58, 66, opts.color[3].saturating_mul(7) / 8];
        crate::primitives::draw_text_with_occlusion(
            sugarloaf,
            x + index as f32 * cell_w + 1.5,
            y + 1.5,
            glyph,
            &near_depth_opts,
            occlusion_rects,
        );
        crate::primitives::draw_text_with_occlusion(
            sugarloaf,
            x + index as f32 * cell_w,
            y,
            glyph,
            &opts,
            occlusion_rects,
        );
    }
    cell_w * frame.len() as f32
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
        AgentStreamingStatus::Retrying => theme.yellow,
        AgentStreamingStatus::Idle => theme.muted,
    };
    let state = pane.streaming_state();
    let elapsed = pane.streaming_elapsed_seconds().unwrap_or(0.0);
    let transition = pane.streaming_state_changed_elapsed().unwrap_or(2.0);
    let live_phase = elapsed;
    let queued_count = pane.queued_prompt_count();
    let status_line_h = STREAMING_STATUS_LINE_H * s;
    let primary_y = if queued_count > 0 || background_count > 0 {
        bar_y
    } else {
        bar_y + (bar_h - status_line_h).max(0.0) * 0.5
    };
    // Match the input scanner's raw left edge. The animated word is allowed
    // to sway left of that origin, so clipping starts at the viewport rather
    // than at the status row itself.
    let clip_x = viewport_clip[0];
    let clip_y = viewport_clip[1];
    let clip_right = (bar_x + bar_w).min(viewport_clip[0] + viewport_clip[2]);
    let clip_bottom = (bar_y + bar_h + 14.0 * s).min(viewport_clip[1] + viewport_clip[3]);
    if clip_right <= clip_x || clip_bottom <= clip_y {
        return;
    }
    let text_clip = [clip_x, clip_y, clip_right - clip_x, clip_bottom - clip_y];

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

    // Use the same bundled Press Start 2P face as the agent sidebar's
    // headings for every animated state label ("Crafting", "Thinking",
    // "Retrying", and the rest). Keep the old UI face as a graceful
    // fallback on hosts where the bundled font cannot be registered.
    let pixel_font = crate::primitives::pixel_font_id(sugarloaf);
    let word_opts = DrawOpts {
        font_size: if pixel_font.is_some() {
            12.0 * s
        } else {
            14.0 * s
        },
        bold: true,
        italic: false,
        font_id: pixel_font,
        clip_rect: Some(text_clip),
        ..DrawOpts::default()
    };
    let label_lines =
        wrap_streaming_status_label(sugarloaf, &display_label, bar_w, s, &word_opts);
    // The status row sits immediately above the composer. Anchor the final
    // wrapped line here and grow earlier lines upward through the timeline's
    // reserved status rows; growing downward puts line two behind the island.
    let word_y = primary_y + (status_line_h - word_opts.font_size) * 0.5;
    let word_motion = live_phase * 3.0;
    let word_drift_x = word_motion.sin() * 1.8 * s;
    let word_drift_y = (word_motion * 0.72).cos() * 0.8 * s;
    // The word drifts up to 1.8px and each glyph sways another 1.5px left.
    // Reserve exactly that excursion so the animation remains aligned with
    // the scanner at rest but can never disappear under the pane edge.
    let word_x = bar_x + 3.3 * s;
    let mut cursor_x = word_x + word_drift_x;
    let mut trailing_color = theme.u8(accent);
    let mut ix = 0usize;
    for (line_ix, line) in label_lines.iter().enumerate() {
        cursor_x = word_x + word_drift_x;
        let lines_above = label_lines.len().saturating_sub(1 + line_ix);
        let line_y = word_y - lines_above as f32 * status_line_h;
        for target_ch in line.chars() {
            let lock_threshold = (ix as f32 + 1.0) * lock_per_char;
            let locked = transition >= lock_threshold;
            let mut opts = word_opts;
            let display = if locked {
                target_ch
            } else {
                let scramble_ix = (frame + ix * 5) % SCRAMBLE.len();
                SCRAMBLE[scramble_ix] as char
            };
            // Crafting samples the local profile's animated pixel-plasma field
            // even after every letter locks. Other states retain their semantic
            // color family; the transition into them uses the same profile field.
            let color = if locked {
                let wave = ((live_phase * 3.4) + ix as f32 * 0.62).sin() * 0.5 + 0.5;
                let pulse = ((live_phase * 6.2) + ix as f32 * 0.9).sin() * 0.5 + 0.5;
                let lightness = 0.52 + wave * 0.16 + pulse * 0.08;
                if matches!(state, AgentStreamingStatus::Generating) {
                    [255, 255, 255, 255]
                } else {
                    let base_hue = match state {
                        AgentStreamingStatus::Thinking => 300.0,
                        AgentStreamingStatus::Working => 52.0,
                        AgentStreamingStatus::Compacting => 158.0,
                        AgentStreamingStatus::WaitingSubagents => 48.0,
                        AgentStreamingStatus::BackgroundTasks => 0.0,
                        AgentStreamingStatus::Retrying => 30.0,
                        _ => 0.0,
                    };
                    let hue = (base_hue + wave * 18.0 - 9.0).rem_euclid(360.0);
                    hsl_to_u8_simple(hue, 0.65, lightness)
                }
            } else {
                crate::cursor_style::rainbow_color_u8(
                    crate::cursor_style::rainbow_now_seconds() + ix as f32 * 0.16,
                )
            };
            trailing_color = color;
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
            let glyph_x = cursor_x + sway_x;
            let glyph_y = line_y - lift_y;
            let mut far_depth_opts = opts;
            far_depth_opts.color = [10, 10, 14, 210];
            crate::primitives::draw_text_with_occlusion(
                sugarloaf,
                glyph_x + 3.5 * s,
                glyph_y + 3.5 * s,
                glyph,
                &far_depth_opts,
                occlusion_rects,
            );
            let mut near_depth_opts = opts;
            near_depth_opts.color = [62, 62, 72, 240];
            crate::primitives::draw_text_with_occlusion(
                sugarloaf,
                glyph_x + 1.75 * s,
                glyph_y + 1.75 * s,
                glyph,
                &near_depth_opts,
                occlusion_rects,
            );
            let glyph_w = crate::primitives::draw_text_with_occlusion(
                sugarloaf,
                glyph_x,
                glyph_y,
                glyph,
                &opts,
                occlusion_rects,
            );
            cursor_x += glyph_w;
            ix += 1;
        }
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
    // The dots use a different face/size from the animated word. Align their
    // actual normalized baselines instead of carrying a pixel-font-specific
    // vertical nudge.
    let word_baseline = sugarloaf.text_mut().baseline_offset(&word_opts);
    let dot_baseline = sugarloaf.text_mut().baseline_offset(&dot_opts);
    let last_line_y = word_y;
    let dot_floor_y = last_line_y + word_baseline - dot_baseline;
    for ix in 0..3 {
        let phase = live_phase * 4.0 + ix as f32 * 0.95;
        let swell = phase.sin();
        let backwash = (phase * 0.55 + 1.2).cos();
        let lift = (swell * 0.5 + 0.5) * 1.7 * s;
        let drift = backwash * 1.0 * s;
        let alpha = 0.40 + (swell * 0.5 + 0.5) * 0.45;
        let mut opts = dot_opts;
        opts.color = trailing_color;
        opts.color[3] = (alpha * 255.0).round() as u8;
        let dot_w = sugarloaf.text_mut().measure(".", &opts);
        let mut far_depth_opts = opts;
        far_depth_opts.color = [10, 10, 14, opts.color[3].saturating_mul(5) / 6];
        let _ = sugarloaf.text_mut().draw(
            cursor_x + drift + 3.0 * s,
            dot_floor_y - lift + 3.0 * s,
            ".",
            &far_depth_opts,
        );
        let mut near_depth_opts = opts;
        near_depth_opts.color = [62, 62, 72, opts.color[3]];
        let _ = sugarloaf.text_mut().draw(
            cursor_x + drift + 1.5 * s,
            dot_floor_y - lift + 1.5 * s,
            ".",
            &near_depth_opts,
        );
        let _ =
            sugarloaf
                .text_mut()
                .draw(cursor_x + drift, dot_floor_y - lift, ".", &opts);
        cursor_x += dot_w + 2.0 * s;
    }

    cursor_x += 8.0 * s;
    let detail = match state {
        AgentStreamingStatus::Thinking => "thinking",
        AgentStreamingStatus::Working => "tools",
        AgentStreamingStatus::Generating => "reply",
        AgentStreamingStatus::Compacting => "context",
        AgentStreamingStatus::WaitingSubagents => "subagents",
        AgentStreamingStatus::BackgroundTasks => "running",
        AgentStreamingStatus::Retrying => "backoff",
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
    let time_baseline = sugarloaf.text_mut().baseline_offset(&time_opts);
    let time_y = last_line_y + word_baseline - time_baseline;
    draw_text_clipped(
        sugarloaf,
        cursor_x,
        time_y,
        &time_label,
        &time_opts,
        occlusion_rects,
    );

    if queued_count > 0 {
        let queue_label = if queued_count == 1 {
            "queued message".to_string()
        } else {
            format!("queued messages ({queued_count})")
        };
        // Queue state is a child of the active streaming row. Keep the tree
        // branch here even though the root Crafting/Thinking row no longer
        // carries a leading decoration. When another child follows, use a
        // continuing branch and let the final background row close the tree.
        let queue_branch = if background_count > 0 {
            "├─"
        } else {
            "╰─"
        };
        let queue_text = format!("{queue_branch} {queue_label}");
        let queue_opts = DrawOpts {
            font_size: 13.0 * s,
            color: theme.u8(theme.accent),
            italic: true,
            clip_rect: Some(text_clip),
            ..DrawOpts::default()
        };
        let queue_y = primary_y
            + status_line_h * label_lines.len() as f32
            + (status_line_h - queue_opts.font_size) * 0.5
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
            + status_line_h * label_lines.len() as f32
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

/// Number of physical lines occupied by the activity row: the animated
/// primary status plus one line for each optional child branch.
pub(super) const fn streaming_status_line_count(
    primary_count: usize,
    queued_count: usize,
    background_count: usize,
) -> usize {
    let primary_count = if primary_count == 0 { 1 } else { primary_count };
    primary_count + (queued_count > 0) as usize + (background_count > 0) as usize
}

pub(super) fn streaming_status_primary_line_count(
    sugarloaf: &mut Sugarloaf,
    label: &str,
    width: f32,
    s: f32,
) -> usize {
    let opts = DrawOpts {
        font_size: 12.0 * s,
        bold: true,
        font_id: crate::primitives::pixel_font_id(sugarloaf),
        ..DrawOpts::default()
    };
    wrap_streaming_status_label(sugarloaf, label, width, s, &opts)
        .len()
        .max(1)
}

fn wrap_streaming_status_label(
    sugarloaf: &mut Sugarloaf,
    label: &str,
    width: f32,
    s: f32,
    opts: &DrawOpts,
) -> Vec<String> {
    // Reserve room for the animated ellipsis, elapsed time, and detail on
    // every line. A stable right edge reads better than a wide continuation.
    let available = (width - 190.0 * s).max(72.0 * s);
    super::draw::wrap_input_text(sugarloaf, label, available, opts)
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
            color: theme.u8(theme.readable_accent(color)),
            bold: true,
            extrude: true,
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
    link_opts.color = theme.u8(theme.readable_accent(theme.blue));

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

/// Running wrap state for [`wrap_agent_prompt_rows`]: the visual row
/// being built covers `text[start..end]` at accumulated width `width`,
/// with `offsets` holding the x of every character boundary already
/// placed on it.
#[derive(Default)]
struct PromptWrapRanges {
    lines: Vec<InputWrapRow>,
    start: usize,
    end: usize,
    width: f32,
    offsets: Vec<f32>,
}

impl PromptWrapRanges {
    /// Close the row under construction and start a fresh one at
    /// `start`. Every row begins with a boundary at x=0.
    fn push_row(&mut self, start: usize) {
        self.lines.push(InputWrapRow {
            start: self.start,
            end: self.end,
            offsets: std::mem::replace(&mut self.offsets, vec![0.0]),
        });
        self.start = start;
        self.end = start;
        self.width = 0.0;
    }

    fn break_soft(&mut self) {
        let resume = self.end;
        self.push_row(resume);
    }
}

/// Byte spans of each visual row the prompt wraps into, each carrying
/// the x offset of every character boundary on it. This is the wrap
/// CORE — the drawing loop slices these same spans, the caret is placed
/// from these same offsets, and the pane registers them for Up/Down
/// movement, so layout and navigation can never disagree about either
/// row boundaries or column positions. Rows exclude their terminating
/// `\n`.
fn wrap_agent_prompt_rows(
    sugarloaf: &mut Sugarloaf,
    text: &str,
    max_w: f32,
    opts: &DrawOpts,
) -> Vec<InputWrapRow> {
    let token_spans = agent_attachment_token_spans(text);
    let mut wrap = PromptWrapRanges {
        offsets: vec![0.0],
        ..PromptWrapRanges::default()
    };
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
        // A token is measured whole (it draws as one pill), so spread
        // its width evenly over its characters — the caret can still sit
        // inside one, and an even split keeps those positions monotonic.
        let token_chars = token.chars().count().max(1);
        for step in 1..=token_chars {
            let fraction = step as f32 / token_chars as f32;
            wrap.offsets.push(wrap.width + token_w * fraction);
        }
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
    // Once the last glyph exactly fills a row, the insertion point belongs at
    // column zero of the next visual row. Reserve that empty row immediately
    // instead of waiting for the next typed character to make the caret jump.
    let trailing_wrap_boundary = wrap.end > wrap.start && wrap.width >= max_w;
    if trailing_wrap_boundary {
        let end = wrap.end;
        wrap.push_row(end);
    }
    // A trailing hard newline creates a real empty visual row. Without
    // preserving it, Shift+Enter updates `cursor_byte` past the last
    // registered row and the renderer falls back to painting the caret
    // on the first/top row.
    if !trailing_wrap_boundary
        && (wrap.end > wrap.start || wrap.lines.is_empty() || text.ends_with('\n'))
    {
        let end = wrap.end;
        wrap.push_row(end);
    }
    wrap.lines
}

/// Measure the prompt with the exact wrap core used by paint, caret placement,
/// and visual Up/Down movement. Composer layout calls this before choosing its
/// height so a newly-created visual row expands the card in the same frame.
pub(super) fn measure_prompt_visual_rows(
    sugarloaf: &mut Sugarloaf,
    input: &str,
    input_w: f32,
    s: f32,
) -> Vec<InputWrapRow> {
    let text = if input.is_empty() {
        "Ask anything"
    } else {
        input
    };
    let text_w = (input_w - 36.0 * s).max(20.0 * s);
    let opts = DrawOpts {
        font_size: 16.0 * s,
        ..DrawOpts::default()
    };
    wrap_agent_prompt_rows(sugarloaf, text, text_w, &opts)
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
            wrap.push_row(ix + ch_len);
            ix += ch_len;
            continue;
        }
        let mut buf = [0; 4];
        let s = ch.encode_utf8(&mut buf);
        let ch_w = sugarloaf.text_mut().measure(s, opts);
        if wrap.end > wrap.start && wrap.width + ch_w > max_w {
            wrap.push_row(ix);
        }
        wrap.end = ix + ch_len;
        wrap.width += ch_w;
        wrap.offsets.push(wrap.width);
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
