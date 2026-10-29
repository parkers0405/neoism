mod wrap_cache;
pub mod host_footer;
pub mod host_chips;

use std::rc::Rc;

use sugarloaf::text::DrawOpts;
use unicode_segmentation::UnicodeSegmentation;

use crate::panels::agent_pane::input_controller::{visual_row_index, InputWrapRow};
use crate::panels::agent_pane::state::{
    NeoismAgentImage, NeoismAgentPane, NeoismAgentPendingPermission,
    NeoismAgentPermissionChoice, NeoismAgentStreamingState,
};

use super::draw::{
    draw_rect_clipped, draw_rounded_rect_clipped, draw_text_clipped, measure_text_cached,
    opts_with_clip, push_image_overlay_clipped,
};
use super::composer_geometry::{ComposerGeometry, COMPOSER_TRAY_RADIUS};
use super::markdown::AgentMarkdownPane;
use super::wordmark::format_elapsed;
use super::{
    AgentCheckoutContext, DEPTH, INPUT_LINE_H, MAX_INPUT_LINES,
    ORDER_CARET, ORDER_PANEL, ORDER_TEXT, OVERLAY_PANEL_ID, STREAMING_STATUS_LINE_H,
    USER_MESSAGE_MAX_LINES,
};
use crate::panels::file_tree::FRAME_STROKE;
use crate::primitives::ide_theme::IdeTheme;
use crate::primitives::text::draw_icon_centered_with_occlusion;
use crate::render_policy::opencode_scanner_frame;

/// Logical height of the shared agent/model/thinking and submit row.
pub(super) const CHIPS_BAND_H: f32 = 44.0;
const STATUS_CHIP_DESKTOP_FONT: f32 = 14.0;
const STATUS_CHIP_COMPACT_FONT: f32 = 12.0;
const STATUS_CHIP_HIT_H: f32 = 23.5;
const INPUT_COMMAND_HINT: (&str, &str) = ("/", "commands");
const NARROW_CONTROL_HIT_SIDE: f32 = 44.0;
const COMPOSER_CONTROL_RADIUS: f32 = 8.0;
const COMPOSER_STOP_SIDE_RATIO: f32 = 0.28;
const COMPOSER_SEND_FONT_RATIO: f32 = 0.62;
const MODEL_LOGO_SIDE: f32 = 17.0;
const MODEL_LOGO_GAP: f32 = 5.0;

pub(super) fn composer_text_padding(show_status: bool) -> (f32, f32) {
    if show_status {
        (15.0, 21.0)
    } else {
        (11.0, 17.0)
    }
}

fn composer_visible_line_count(text_h: f32, line_h: f32) -> usize {
    // Layout and paint reconstruct the same height through different f32
    // operations. Do not drop an entire row for subpixel roundoff.
    ((text_h / line_h + 1.0e-4).floor().max(1.0) as usize).min(MAX_INPUT_LINES)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposerControlAction {
    Disabled,
    Send,
    Interrupt,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ComposerVisualPolicy {
    narrow: bool,
    visual_side: f32,
    hit_side: f32,
    action: ComposerControlAction,
    show_activity: bool,
    show_interrupt_hint: bool,
    show_command_hint: bool,
}

/// One renderer-owned responsive policy serves desktop, web, and mobile.
/// Widths are converted back to logical/CSS pixels before the breakpoint is
/// applied. Responsive hit geometry must not alter the painted control.
fn composer_visual_policy(
    available_w: f32,
    scale: f32,
    show_status: bool,
    has_draft: bool,
    display_status_active: bool,
    interruptible_run_active: bool,
) -> ComposerVisualPolicy {
    let narrow = super::layout::is_narrow_composer_width(available_w, scale);
    let action = if interruptible_run_active {
        ComposerControlAction::Interrupt
    } else if has_draft {
        ComposerControlAction::Send
    } else {
        ComposerControlAction::Disabled
    };
    // These are the original painted dimensions. Mobile accessibility is
    // provided by `hit_side`, never by enlarging this visual square.
    let visual_side = if show_status { 30.0 } else { 26.0 } * scale;
    ComposerVisualPolicy {
        narrow,
        visual_side,
        hit_side: if narrow {
            (NARROW_CONTROL_HIT_SIDE * scale).max(visual_side)
        } else {
            visual_side
        },
        action,
        show_activity: display_status_active,
        show_interrupt_hint: display_status_active && !narrow,
        show_command_hint: true,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ComposerFooterGeometry {
    visual_rect: [f32; 4],
    hit_rect: [f32; 4],
    chips_rect: [f32; 4],
}

/// Share a row center and reserve the full submit hit target before measuring
/// chips, so narrow touch targets never steal a chip or help-strip click.
fn composer_footer_geometry(
    rect: [f32; 4],
    scale: f32,
    policy: ComposerVisualPolicy,
) -> ComposerFooterGeometry {
    let [x, y, w, h] = rect;
    let inset = 9.0 * scale;
    let center_y = y + h * 0.5;
    let visual_side = policy.visual_side.min((w - inset * 2.0).max(0.0));
    let visual_x = x + w - inset.min(w) - visual_side;
    let visual_y = center_y - visual_side * 0.5;
    let hit_pad = (policy.hit_side - visual_side).max(0.0) * 0.5;
    let hit_x = (visual_x - hit_pad).max(x);
    let hit_right = (visual_x + visual_side + hit_pad).min(x + w);
    let chips_x = x + 14.0 * scale;
    ComposerFooterGeometry {
        visual_rect: [visual_x, visual_y, visual_side, visual_side],
        hit_rect: [
            hit_x,
            center_y - policy.hit_side * 0.5,
            (hit_right - hit_x).max(0.0),
            policy.hit_side,
        ],
        chips_rect: [
            chips_x,
            center_y - STATUS_CHIP_HIT_H * scale * 0.5,
            (hit_x - 8.0 * scale - chips_x).max(0.0),
            STATUS_CHIP_HIT_H * scale,
        ],
    }
}

fn rect_center(rect: [f32; 4]) -> [f32; 2] {
    [rect[0] + rect[2] * 0.5, rect[1] + rect[3] * 0.5]
}

fn centered_square(rect: [f32; 4], side: f32) -> [f32; 4] {
    let center = rect_center(rect);
    [center[0] - side * 0.5, center[1] - side * 0.5, side, side]
}

fn composer_background_color(theme: &IdeTheme) -> [f32; 4] {
    crate::primitives::surface_background::base_color(
        "composer.agent", theme, theme.f32(theme.bg),
    )
}

fn composer_help_panel_color(theme: &IdeTheme) -> [f32; 4] {
    crate::primitives::surface_background::base_color(
        "composer.agent", theme, theme.f32(theme.surface),
    )
}

/// Capitalization is display-only; provider/model IDs and picker values stay
/// unchanged, including existing acronyms and model version suffixes.
pub fn capitalize_chip_label(label: &str) -> String {
    let mut start = true;
    let mut display = String::with_capacity(label.len());
    for ch in label.chars() {
        if start {
            display.extend(ch.to_uppercase());
        } else {
            display.push(ch);
        }
        start = matches!(ch, '/' | '-' | '_') || ch.is_whitespace();
    }
    display
}

/// Narrow chips show only the terminal non-empty model-id segment.
pub fn model_chip_label(model: &str, narrow: bool) -> String {
    let label = if narrow {
        model
            .rsplit('/')
            .find(|segment| !segment.is_empty())
            .unwrap_or("")
    } else {
        model
    };
    capitalize_chip_label(label)
}

/// Scale chip labels down with the shared compositor's available width. This
/// covers mobile/web and narrow desktop panes without platform-specific CSS.
fn status_chip_font_size(max_w: f32, scale: f32) -> f32 {
    let logical_w = max_w / scale.max(0.001);
    let blend = ((logical_w - 420.0) / (760.0 - 420.0)).clamp(0.0, 1.0);
    (STATUS_CHIP_COMPACT_FONT
        + (STATUS_CHIP_DESKTOP_FONT - STATUS_CHIP_COMPACT_FONT) * blend)
        * scale
}

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

pub fn streaming_status_accent(theme: &IdeTheme, state: AgentStreamingStatus) -> u32 {
    let color = match state {
        AgentStreamingStatus::Thinking => theme.magenta,
        AgentStreamingStatus::Working => theme.yellow,
        AgentStreamingStatus::Generating => theme.accent,
        AgentStreamingStatus::Compacting => theme.green,
        AgentStreamingStatus::WaitingSubagents => theme.yellow,
        AgentStreamingStatus::BackgroundTasks => theme.red,
        AgentStreamingStatus::Retrying => theme.yellow,
        AgentStreamingStatus::Idle => theme.muted,
    };
    theme.readable_accent(color)
}

fn agent_chip_accent(theme: &IdeTheme, label: &str) -> u32 {
    if label.eq_ignore_ascii_case("build") {
        theme.red
    } else if label.eq_ignore_ascii_case("plan") {
        theme.yellow
    } else {
        theme.accent
    }
}

/// Native hosts use the same chip ink and per-letter transition as the
/// desktop composer, without reproducing the scramble in their UI layer.
pub fn render_status_chip_label_for_host(
    painter:&mut dyn crate::primitives::paint::Paint,label:&str,kind:&str,font_size:f32,theme:&IdeTheme,
    transition:Option<crate::panels::agent_pane::state::ConfigChipTransition>,agent_elapsed_ms:Option<f32>,
)->f32 {render_status_chip_label_for_host_with_source(painter,label,label,kind,font_size,theme,transition,agent_elapsed_ms)}

pub fn render_status_chip_label_for_host_with_source(
    painter: &mut dyn crate::primitives::paint::Paint,
    label: &str,
    source_label: &str,
    kind: &str,
    font_size: f32,
    theme: &IdeTheme,
    transition: Option<crate::panels::agent_pane::state::ConfigChipTransition>,
    agent_elapsed_ms: Option<f32>,
) -> f32 {
    use crate::panels::agent_pane::state::ConfigChipTransition;
    let color = match kind {
        "agent" => agent_chip_accent(theme, source_label),
        "model" => theme.blue,
        _ => theme.magenta,
    };
    let opts = DrawOpts {
        font_size,
        bold: true,
        color: theme.u8(theme.readable_accent(color)),
        ..DrawOpts::default()
    };
    let width = painter.text_mut().measure(label, &opts);
    match transition {
        Some(ConfigChipTransition::Loading { elapsed_ms }) =>
            super::side_panel::draw::render_loading_scramble_text(painter, 0.0, 0.0, label, &opts, elapsed_ms),
        Some(ConfigChipTransition::Settling { elapsed_ms }) =>
            super::side_panel::draw::render_scramble_text(painter, 0.0, 0.0, label, &opts, elapsed_ms),
        None if kind == "agent" && agent_elapsed_ms.is_some() =>
            super::side_panel::draw::render_scramble_text(painter, 0.0, 0.0, label, &opts, agent_elapsed_ms.unwrap()),
        None => { painter.text_mut().draw(0.0, 0.0, label, &opts); }
    }
    width
}

fn mix_status_color(from: [u8; 4], to: [u8; 4], amount: f32) -> [u8; 4] {
    let amount = amount.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount).round() as u8;
    [
        mix(from[0], to[0]),
        mix(from[1], to[1]),
        mix(from[2], to[2]),
        mix(from[3], to[3]),
    ]
}

/// Animate within the selected theme's palette. The old implementation
/// switched to fixed HSL hues (and pure white for Crafting) after resolving
/// the theme token, so changing themes did not actually change this row.
pub fn animated_streaming_status_color(
    theme: &IdeTheme,
    accent: u32,
    live_phase: f32,
    glyph_index: usize,
    locked: bool,
) -> [u8; 4] {
    let wave_speed = if locked { 3.4 } else { 8.2 };
    let glyph_offset = if locked { 0.62 } else { 0.91 };
    let wave =
        (live_phase * wave_speed + glyph_index as f32 * glyph_offset).sin() * 0.5 + 0.5;
    let pulse = ((live_phase * 6.2) + glyph_index as f32 * 0.9).sin() * 0.5 + 0.5;
    let amount = if locked {
        0.06 + wave * 0.18 + pulse * 0.08
    } else {
        0.12 + wave * 0.34
    };
    mix_status_color(theme.u8(accent), theme.u8(theme.fg), amount)
}

/// Shared paint-only glyph geometry for the desktop and native host adapters.
pub fn streaming_status_glyph_motion(
    time: f32,
    index: usize,
    locked: bool,
    scale: f32,
    word_drift_y: f32,
) -> (f32, f32) {
    let phase = time * 5.6 + index as f32 * 0.82;
    if locked {
        (
            phase.cos() * 1.5 * scale,
            phase.sin() * 2.4 * scale + word_drift_y,
        )
    } else {
        (
            (phase * 1.7).sin() * 0.8 * scale,
            (phase * 1.9).cos() * 0.9 * scale,
        )
    }
}

pub fn streaming_status_display_char(
    target: char,
    time: f32,
    transition: f32,
    index: usize,
    count: usize,
) -> (char, bool) {
    let locked = transition >= (index as f32 + 1.0) * (0.7 / count.max(1) as f32);
    const SCRAMBLE: &[u8] = b"|/-\\+!?>?<%#=@*~&^$";
    let display = if locked {
        target
    } else {
        SCRAMBLE[((time * 44.0) as usize + index * 5) % SCRAMBLE.len()] as char
    };
    (display, locked)
}

fn themed_activity_scanner_color(
    theme: &IdeTheme,
    state: AgentStreamingStatus,
    active: bool,
    brightness: f32,
    alpha: f32,
) -> [u8; 4] {
    let accent = streaming_status_accent(theme, state);
    let bloom = if active {
        0.10 + (brightness - 1.0).max(0.0) * 0.60
    } else {
        0.0
    };
    let mut color = mix_status_color(theme.u8(accent), theme.u8(theme.fg), bloom);
    color[3] = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    color
}

#[cfg(test)]
mod streaming_status_theme_tests {
    use super::*;

    #[test]
    fn streaming_status_uses_the_selected_theme_roles() {
        let theme = IdeTheme::pastel_dark();
        assert_eq!(
            streaming_status_accent(&theme, AgentStreamingStatus::Generating),
            theme.readable_accent(theme.accent)
        );
        assert_eq!(
            streaming_status_accent(&theme, AgentStreamingStatus::Thinking),
            theme.readable_accent(theme.magenta)
        );
        assert_eq!(
            streaming_status_accent(&theme, AgentStreamingStatus::WaitingSubagents),
            theme.readable_accent(theme.yellow)
        );
    }

    #[test]
    fn build_and_plan_chips_use_distinct_theme_roles() {
        let theme = IdeTheme::pastel_dark();
        assert_eq!(agent_chip_accent(&theme, "Build"), theme.red);
        assert_eq!(agent_chip_accent(&theme, "Plan"), theme.yellow);
        assert_ne!(
            agent_chip_accent(&theme, "Build"),
            agent_chip_accent(&theme, "Plan")
        );
    }

    #[test]
    fn animated_status_color_changes_with_the_theme() {
        let mut first = IdeTheme::pastel_dark();
        first.accent = 0x10_20_30;
        first.fg = 0xa0_b0_c0;
        let mut second = first;
        second.accent = 0x90_20_10;
        second.fg = 0xf0_e0_d0;

        let first_color = animated_streaming_status_color(
            &first,
            streaming_status_accent(&first, AgentStreamingStatus::Generating),
            1.25,
            3,
            true,
        );
        let second_color = animated_streaming_status_color(
            &second,
            streaming_status_accent(&second, AgentStreamingStatus::Generating),
            1.25,
            3,
            true,
        );

        assert_ne!(first_color, second_color);
        assert_ne!(first_color, [255, 255, 255, 255]);
    }

    #[test]
    fn help_strip_scanner_uses_the_selected_theme() {
        let mut first = IdeTheme::pastel_dark();
        first.yellow = 0x10_90_30;
        first.fg = 0xd0_f0_d8;
        let mut second = first;
        second.yellow = 0x90_20_10;
        second.fg = 0xf0_d8_d0;

        let first_color = themed_activity_scanner_color(
            &first,
            AgentStreamingStatus::Working,
            true,
            1.15,
            0.9,
        );
        let second_color = themed_activity_scanner_color(
            &second,
            AgentStreamingStatus::Working,
            true,
            1.15,
            0.9,
        );

        assert_ne!(first_color, second_color);
        assert_eq!(first_color[3], 230);
    }
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

    fn conversation_source(
        &self,
    ) -> crate::panels::agent_pane::state::side_panel::ConversationSource;
    fn external_options(
        &self,
    ) -> Option<&crate::panels::agent_pane::state::external_options::ExternalOptions>
    {
        None
    }
    fn external_options_error(&self) -> Option<&str> {
        None
    }
    fn input(&self) -> &str;
    fn no_providers_connected(&self) -> bool;
    fn input_help_visible(&self) -> bool;
    fn input_images(&self) -> Vec<NeoismAgentImage>;
    fn cursor_byte(&self) -> usize;
    fn set_cursor_rect(&mut self, rect: Option<[f32; 4]>);
    fn set_input_wrap_rows(&mut self, rows: Rc<Vec<InputWrapRow>>);
    fn input_viewport_mut(
        &mut self,
    ) -> &mut crate::panels::agent_pane::input_viewport::InputViewport;
    fn composer_interactions(
        &mut self,
    ) -> &mut crate::panels::agent_pane::composer_interactions::ComposerInteractions;
    fn composer_directory(&self) -> Option<String>;
    fn composer_system(
        &self,
    ) -> Option<crate::panels::agent_pane::composer_interactions::ComposerSystemControl>
    {
        None
    }
    fn clear_usage_chip_rect(&mut self);
    fn register_usage_chip_rect(&mut self, rect: [f32; 4]);
    fn clear_composer_control_rect(&mut self);
    fn register_composer_control_rect(&mut self, rect: [f32; 4]);
    fn interruptible_run_active(&self) -> bool;
    fn clear_status_chip_rects(&mut self);
    fn register_status_chip_rect(&mut self, index: usize, rect: [f32; 4]);
    fn usage_summary_label(&self) -> Option<String>;
    fn agent_label(&self) -> &str;
    fn agent_label_changed_elapsed_ms(&self) -> Option<f32>;
    fn config_chip_transition(
        &self,
    ) -> Option<crate::panels::agent_pane::state::ConfigChipTransition>;
    fn status_chip_activation_ms(&self, index: usize) -> Option<f32>;
    fn model(&self) -> &str;
    fn thinking_label(&self) -> &str;
    fn streaming_label(&self) -> String;
    fn streaming_state(&self) -> AgentStreamingStatus;
    fn has_status_activity(&self) -> bool;
    fn running_background_task_count(&self) -> usize;
    fn background_task_details_expanded(&self) -> bool;
    fn active_background_task_summaries(&self) -> Vec<String>;
    fn register_activity_status_rect(&mut self, rect: [f32; 4]);
    fn clear_activity_status_rects(&mut self);
    fn activity_animation_frame(&self) -> Option<crate::primitives::wordmark_animation::AnimationFrame>;
    fn register_background_status_rect(&mut self, rect: [f32; 4]);
    fn clear_background_status_rect(&mut self);
    fn streaming_elapsed_seconds(&self) -> Option<f32>;
    fn streaming_state_changed_elapsed(&self) -> Option<f32>;
    fn queued_prompt_count(&self) -> usize;
    fn queued_prompt_preview(&self) -> Option<&str>;
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

            fn conversation_source(&self) -> $crate::panels::agent_pane::state::side_panel::ConversationSource {
                <$pane>::conversation_source(self)
            }

            fn external_options(&self) -> Option<&$crate::panels::agent_pane::state::external_options::ExternalOptions> {
                <$pane>::external_options(self)
            }

            fn external_options_error(&self) -> Option<&str> {
                <$pane>::external_options_error(self)
            }

            fn input(&self) -> &str {
                <$pane>::input(self)
            }

            fn no_providers_connected(&self) -> bool {
                <$pane>::no_providers_connected(self)
            }

            fn input_help_visible(&self) -> bool {
                <$pane>::input_help_visible(self)
            }

            fn input_images(&self) -> Vec<$crate::panels::agent_pane::state::NeoismAgentImage> {
                <$pane>::input_images(self)
            }

            fn cursor_byte(&self) -> usize {
                <$pane>::cursor_byte(self)
            }

            fn set_cursor_rect(&mut self, rect: Option<[f32; 4]>) {
                <$pane>::set_cursor_rect(self, rect);
            }

            fn set_input_wrap_rows(
                &mut self,
                rows: std::rc::Rc<
                    Vec<$crate::panels::agent_pane::input_controller::InputWrapRow>,
                >,
            ) {
                <$pane>::set_input_wrap_rows(self, rows);
            }

            fn input_viewport_mut(&mut self) -> &mut $crate::panels::agent_pane::input_viewport::InputViewport {
                <$pane>::input_viewport_mut(self)
            }

            fn composer_system(&self) -> Option<$crate::panels::agent_pane::composer_interactions::ComposerSystemControl> {
                <$pane>::composer_system_control(self)
            }

            fn composer_interactions(&mut self) -> &mut $crate::panels::agent_pane::composer_interactions::ComposerInteractions {
                &mut self.composer_interactions
            }

            fn composer_directory(&self) -> Option<String> {
                self.session_directory().map(str::to_string)
            }

            fn clear_usage_chip_rect(&mut self) {
                <$pane>::clear_usage_chip_rect(self);
            }

            fn register_usage_chip_rect(&mut self, rect: [f32; 4]) {
                <$pane>::register_usage_chip_rect(self, rect);
            }

            fn clear_composer_control_rect(&mut self) {
                <$pane>::clear_composer_control_rect(self);
            }

            fn register_composer_control_rect(&mut self, rect: [f32; 4]) {
                <$pane>::register_composer_control_rect(self, rect);
            }

            fn interruptible_run_active(&self) -> bool {
                <$pane>::interruptible_run_active(self)
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

            fn agent_label_changed_elapsed_ms(&self) -> Option<f32> {
                <$pane>::agent_label_changed_elapsed_ms(self)
            }

            fn config_chip_transition(&self) -> Option<$crate::panels::agent_pane::state::ConfigChipTransition> {
                <$pane>::config_chip_transition(self)
            }

            fn status_chip_activation_ms(&self, index: usize) -> Option<f32> {
                <$pane>::status_chip_activation_ms(self, index)
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

            fn register_activity_status_rect(&mut self, rect: [f32; 4]) {
                <$pane>::register_activity_status_rect(self, rect);
            }

            fn clear_activity_status_rects(&mut self) {
                <$pane>::clear_activity_status_rects(self);
            }

            fn activity_animation_frame(&self) -> Option<$crate::primitives::wordmark_animation::AnimationFrame> {
                <$pane>::activity_animation_frame(self)
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

            fn queued_prompt_preview(&self) -> Option<&str> {
                <$pane>::queued_prompt_preview(self)
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

    fn conversation_source(
        &self,
    ) -> crate::panels::agent_pane::state::side_panel::ConversationSource {
        self.new_chat_source()
    }

    fn input(&self) -> &str {
        NeoismAgentPane::input(self)
    }

    fn no_providers_connected(&self) -> bool {
        self.no_providers_connected()
    }

    fn input_help_visible(&self) -> bool {
        NeoismAgentPane::input_help_visible(self)
    }

    fn input_images(&self) -> Vec<NeoismAgentImage> {
        NeoismAgentPane::input_images(self)
    }

    fn cursor_byte(&self) -> usize {
        NeoismAgentPane::cursor_byte(self)
    }

    fn set_cursor_rect(&mut self, rect: Option<[f32; 4]>) {
        NeoismAgentPane::set_cursor_rect(self, rect);
    }

    fn set_input_wrap_rows(&mut self, rows: Rc<Vec<InputWrapRow>>) {
        NeoismAgentPane::set_input_wrap_rows(self, rows);
    }

    fn input_viewport_mut(
        &mut self,
    ) -> &mut crate::panels::agent_pane::input_viewport::InputViewport {
        NeoismAgentPane::input_viewport_mut(self)
    }

    fn composer_interactions(
        &mut self,
    ) -> &mut crate::panels::agent_pane::composer_interactions::ComposerInteractions {
        &mut self.composer_interactions
    }

    fn composer_directory(&self) -> Option<String> {
        self.session_directory().map(str::to_string)
    }

    fn clear_usage_chip_rect(&mut self) {
        NeoismAgentPane::clear_usage_chip_rect(self);
    }

    fn register_usage_chip_rect(&mut self, rect: [f32; 4]) {
        NeoismAgentPane::register_usage_chip_rect(self, rect);
    }

    fn clear_composer_control_rect(&mut self) {
        NeoismAgentPane::clear_composer_control_rect(self);
    }

    fn register_composer_control_rect(&mut self, rect: [f32; 4]) {
        NeoismAgentPane::register_composer_control_rect(self, rect);
    }

    fn interruptible_run_active(&self) -> bool {
        NeoismAgentPane::interruptible_run_active(self)
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

    fn agent_label_changed_elapsed_ms(&self) -> Option<f32> {
        NeoismAgentPane::agent_label_changed_elapsed_ms(self)
    }

    fn config_chip_transition(
        &self,
    ) -> Option<crate::panels::agent_pane::state::ConfigChipTransition> {
        NeoismAgentPane::config_chip_transition(self)
    }

    fn status_chip_activation_ms(&self, index: usize) -> Option<f32> {
        NeoismAgentPane::status_chip_activation_ms(self, index)
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

    fn register_activity_status_rect(&mut self, rect: [f32; 4]) {
        self.register_activity_status_rect(rect);
    }

    fn clear_activity_status_rects(&mut self) {
        self.clear_activity_status_rects();
    }

    fn activity_animation_frame(&self) -> Option<crate::primitives::wordmark_animation::AnimationFrame> {
        self.activity_animation_frame()
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

    fn queued_prompt_preview(&self) -> Option<&str> {
        NeoismAgentPane::queued_prompt_preview(self)
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
/// the editor caret / top-chrome orb use). Only an explicit author matching
/// that name is labelled "You". Missing author metadata cannot prove that
/// this peer sent the message, so it is rendered as an unknown user.
///
/// [`NeoismAgentMessage::author`]:
///   crate::panels::agent_pane::state::NeoismAgentMessage
pub fn user_message_orb_identity(
    author: Option<&str>,
    local_name: Option<&str>,
) -> UserMessageOrbIdentity {
    match author.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name)
            if local_name.map(str::trim).filter(|local| !local.is_empty())
                == Some(name) =>
        {
            UserMessageOrbIdentity {
                seed: name.to_string(),
                label: "You".to_string(),
            }
        }
        Some(name) => UserMessageOrbIdentity {
            seed: name.to_string(),
            label: name.to_string(),
        },
        None => UserMessageOrbIdentity {
            seed: "unknown-user".to_string(),
            label: "Unknown user".to_string(),
        },
    }
}

#[cfg(test)]
mod orb_identity_tests {
    use super::{user_message_line_x, user_message_orb_identity};

    #[test]
    fn explicit_local_author_is_you() {
        let identity = user_message_orb_identity(Some("Parker"), Some("Parker"));
        assert_eq!(identity.seed, "Parker");
        assert_eq!(identity.label, "You");
    }

    #[test]
    fn explicit_remote_author_keeps_its_name() {
        let identity = user_message_orb_identity(Some("Remote peer"), Some("Parker"));
        assert_eq!(identity.seed, "Remote peer");
        assert_eq!(identity.label, "Remote peer");
    }

    #[test]
    fn missing_author_is_not_assumed_local() {
        let identity = user_message_orb_identity(None, Some("Parker"));
        assert_eq!(identity.seed, "unknown-user");
        assert_eq!(identity.label, "Unknown user");
    }

    #[test]
    fn human_message_lines_align_to_the_right_edge_of_the_text_column() {
        assert_eq!(user_message_line_x(20.0, 300.0, 80.0), 240.0);
        assert_eq!(user_message_line_x(20.0, 300.0, 300.0), 20.0);
        assert_eq!(user_message_line_x(20.0, 300.0, 340.0), 20.0);
    }
}

/// The same available column is used to measure and paint the user card.
/// Never grow the bubble or text column past the caller's actual pane width.
pub(super) fn user_message_text_width(width: f32, scale: f32) -> f32 {
    (width.max(0.0) - (28.0 + USER_ORB_SIZE + 10.0) * scale).max(1.0)
}

/// Wrap user prompts at measured glyph widths, including URLs and other tokens
/// without spaces. Keep grapheme clusters intact (emoji sequences/combining
/// marks) and preserve explicit paragraph breaks. `measure` uses the very same
/// font/options as rendering, rather than an estimated character count.
fn wrap_user_message_with(
    text: &str,
    width: f32,
    limit: usize,
    mut measure: impl FnMut(&str) -> f32,
) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let width = width.max(0.0);
    for (paragraph_index, paragraph) in text.split('\n').enumerate() {
        if paragraph_index > 0 {
            lines.push(std::mem::take(&mut current));
            if lines.len() >= limit {
                return lines;
            }
        }
        for word in paragraph.split_whitespace() {
            let candidate = if current.is_empty() {
                word.to_owned()
            } else {
                format!("{current} {word}")
            };
            if measure(&candidate) <= width {
                current = candidate;
                continue;
            }
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                if lines.len() >= limit {
                    return lines;
                }
            }
            if measure(word) <= width {
                current.push_str(word);
                continue;
            }
            // Binary-search a maximal measured prefix; long URLs are common,
            // so avoid measuring every prefix of a potentially huge token.
            let graphemes: Vec<&str> = word.graphemes(true).collect();
            let mut start = 0;
            while start < graphemes.len() {
                let mut lo = start + 1;
                let mut hi = graphemes.len() + 1;
                while lo < hi {
                    let mid = lo + (hi - lo) / 2;
                    if measure(&graphemes[start..mid].concat()) <= width {
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                let end = (lo - 1).max(start + 1);
                current = graphemes[start..end].concat();
                start = end;
                if start < graphemes.len() {
                    lines.push(std::mem::take(&mut current));
                    if lines.len() >= limit {
                        return lines;
                    }
                }
            }
        }
    }
    if lines.len() < limit {
        lines.push(current);
    }
    lines
}

pub(super) fn wrap_user_message(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    text: &str,
    width: f32,
    opts: &DrawOpts,
) -> Vec<String> {
    wrap_cache::bubble(sugarloaf, text, width, opts, |sugarloaf| {
        wrap_user_message_with(text, width, USER_MESSAGE_MAX_LINES, |part| {
            measure_text_cached(sugarloaf, part, opts)
        })
    })
}

#[cfg(test)]
mod user_message_wrap_tests {
    use super::{user_message_text_width, wrap_user_message_with};
    use unicode_segmentation::UnicodeSegmentation;

    fn measure(s: &str) -> f32 {
        s.graphemes(true)
            .map(|g| if g == "界" { 2.0 } else { 1.0 })
            .sum()
    }

    #[test]
    fn url_and_long_token_wrap_at_the_measured_column() {
        let text = "See https://app.read.ai/analytics/meetings/01M3Z0Y8V3T1PFUB1RXYZ now";
        let lines = wrap_user_message_with(text, 12.0, 30, measure);
        assert_eq!(lines.concat().replace(' ', ""), text.replace(' ', ""));
        assert!(lines.len() > 3);
        assert!(lines.iter().all(|line| measure(line) <= 12.0));
    }

    #[test]
    fn utf8_clusters_variable_width_and_hard_breaks() {
        let text = "界界 e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}\n👩‍💻👩‍💻👩‍💻";
        let lines = wrap_user_message_with(text, 4.0, 30, measure);
        assert_eq!(
            lines,
            [
                "界界",
                "e\u{301}e\u{301}e\u{301}e\u{301}",
                "e\u{301}",
                "👩‍💻👩‍💻👩‍💻"
            ]
        );
        assert!(lines.iter().all(|line| measure(line) <= 4.0));
    }

    #[test]
    fn width_and_limit_follow_the_card_at_resize() {
        assert_eq!(user_message_text_width(300.0, 1.0), 244.0);
        assert_eq!(user_message_text_width(100.0, 1.0), 44.0);
        let text = "abcdefghij";
        assert_eq!(
            wrap_user_message_with(text, 4.0, 2, measure),
            ["abcd", "efgh"]
        );
        assert_eq!(wrap_user_message_with(text, 10.0, 2, measure), [text]);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render_user_message<P: AgentMarkdownPane>(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    pane: &mut P,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    text: &str,
    images: &[NeoismAgentImage],
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
    // The grey bubble spans the full row, with the human's text aligned back
    // from the sender orb inside the right edge.
    let bubble_x = x;
    let bubble_w = w.max(0.0);
    draw_rect_clipped(
        sugarloaf,
        [bubble_x, y, bubble_w, h],
        theme.f32(theme.surface),
        ORDER_PANEL,
        viewport_clip,
    );
    // Sender presence orb inside the bubble, aligned to the first text line.
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
    let orb_x = bubble_x + bubble_w - pad_x - orb;
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
            color: theme.u8(super::primary_text_color(theme)),
            ..DrawOpts::default()
        },
        viewport_clip,
    ) else {
        return h;
    };
    let text_x = bubble_x + pad_x;
    let text_w = user_message_text_width(bubble_w, s);
    let mut line_y = y + 12.0 * s;
    if !images.is_empty() {
        let thumb = super::image_preview::MESSAGE_THUMB_SIZE * s;
        let gap = 10.0 * s;
        let shown = images
            .len()
            .min(((text_w + gap) / (thumb + gap)).floor() as usize);
        let strip_w = if shown == 0 {
            0.0
        } else {
            shown as f32 * thumb + (shown - 1) as f32 * gap
        };
        let image_x = text_x + (text_w - strip_w).max(0.0);
        let image_hits = super::image_preview::render_image_strip(
            sugarloaf,
            images,
            image_x,
            line_y,
            strip_w,
            theme,
            s,
            super::image_preview::MESSAGE_THUMB_SIZE,
            viewport_clip,
            occlusion_rects,
        );
        if !pane.suppress_markdown_interactions() {
            for hit in image_hits {
                pane.register_image_preview(hit);
            }
        }
        line_y += 164.0 * s;
    }
    let suppress_interactions = pane.suppress_markdown_interactions();
    for line in wrap_user_message(sugarloaf, text, text_w, &opts) {
        let line_w = measure_text_cached(sugarloaf, &line, &opts).max(12.0);
        let line_x = user_message_line_x(text_x, text_w, line_w);
        if !suppress_interactions {
            // User bubbles belong to the same transcript selection surface as
            // assistant Markdown and tool output. Previously these rows were
            // only painted, so a downward drag commonly appeared to stop as
            // soon as it crossed the next user prompt.
            let line_index = pane.register_selectable_line(
                &line,
                [line_x, line_y - 3.0 * s, line_w, opts.font_size + 8.0 * s],
            );
            if let Some((sel_left, sel_right)) =
                pane.selectable_line_highlight(line_index)
            {
                draw_rounded_rect_clipped(
                    sugarloaf,
                    [
                        sel_left - 2.0,
                        line_y - 3.0 * s,
                        (sel_right - sel_left + 4.0).max(2.0),
                        opts.font_size + 8.0 * s,
                    ],
                    theme.f32_alpha(theme.accent, 0.22),
                    4.0,
                    ORDER_PANEL + 2,
                    viewport_clip,
                );
            }
        }
        draw_agent_prompt_text(
            sugarloaf,
            line_x,
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

fn user_message_line_x(text_x: f32, text_w: f32, line_w: f32) -> f32 {
    text_x + (text_w - line_w).max(0.0)
}

/// Small name pill under a hovered user-message orb. Mirrors the
/// top-chrome presence tooltip: a surface rounded-rect + label, clamped
/// inside the timeline's viewport, painted above the message cards.
#[allow(clippy::too_many_arguments)]
fn draw_user_orb_tooltip(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
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
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    pane: &mut impl AgentUserInputPane,
    rect: [f32; 4],
    responsive_width: f32,
    theme: &IdeTheme,
    active: bool,
    _mouse: Option<(f32, f32)>,
    s: f32,
    show_status: bool,
    home_source: bool,
    now_seconds: f32,
    occlusion_rects: &[[f32; 4]],
    prepared_wrap_rows: Option<&Rc<Vec<InputWrapRow>>>,
    checkout_context: Option<&AgentCheckoutContext>,
) {
    let [x, y, w, h] = rect;
    // Agent cwd is independent of the workspace and terminal cwd.
    let session_checkout = pane.composer_directory().map(|directory| {
        AgentCheckoutContext::new(
            Some(
                crate::panels::agent_pane::attachment_policy::compact_directory_label(
                    &directory,
                ),
            ),
            None,
        )
    });
    let checkout_context = session_checkout.as_ref().or(checkout_context);
    pane.set_cursor_rect(None);
    pane.composer_interactions().clear_composer_rects();
    pane.clear_usage_chip_rect();
    pane.clear_composer_control_rect();
    pane.clear_status_chip_rects();
    pane.composer_interactions()
        .occlusion_rects
        .extend_from_slice(occlusion_rects);
    // One continuous shell surrounds the text and footer, with no divider or
    // separately bordered input box inside it.
    let geometry = ComposerGeometry::new(
        rect,
        s,
        sugarloaf.scale_factor(),
        pane.input_help_visible() || checkout_context.is_some() || pane.composer_system().is_some(),
    );
    let corner_radius = geometry.card_radius;
    let chips_band_h = CHIPS_BAND_H * s;
    let box_x = x;
    let box_y = y;
    let box_w = w;
    let box_h = (h - chips_band_h).max(44.0 * s);
    let box_bottom = box_y + box_h;
    let background = composer_background_color(theme);
    let (text_top_padding, text_bottom_padding) = composer_text_padding(show_status);
    let input_images = pane.input_images();
    let image_rail_h = if input_images.is_empty() {
        0.0
    } else {
        super::INPUT_IMAGE_RAIL_H * s
    };
    let text_top_pad = text_top_padding * s + image_rail_h;
    let border_w = (FRAME_STROKE * 0.5 * s).max(1.0);
    let [frame_x, frame_y, frame_w, frame_h] = geometry.card;
    let inner = [
        frame_x + border_w,
        frame_y + border_w,
        (frame_w - 2.0 * border_w).max(0.0),
        (frame_h - 2.0 * border_w).max(0.0),
    ];
    let inner_radius = (corner_radius - border_w).max(0.0);
    let tray_join = geometry.tray_join_rect(border_w);
    let prior_card_occlusions = tray_join.map(|join| {
        let previous = sugarloaf.set_content_occlusion_rects(Vec::new());
        let mut holes = previous.clone();
        holes.push(join);
        sugarloaf.set_content_occlusion_rects(holes);
        previous
    });
    crate::widgets::frame::draw_background_border(
        sugarloaf,
        geometry.card,
        inner,
        [corner_radius; 4],
        [inner_radius; 4],
        theme.f32_alpha(theme.border, 0.75),
        DEPTH,
        ORDER_PANEL,
    );
    sugarloaf.rounded_rect(
        None,
        inner[0],
        inner[1],
        inner[2],
        inner[3],
        background,
        DEPTH,
        inner_radius,
        ORDER_PANEL + 1,
    );
    crate::primitives::surface_background::render(
        sugarloaf,
        "composer.agent",
        inner,
        inner_radius,
        s,
        DEPTH,
        ORDER_PANEL + 1,
        occlusion_rects,
    );
    if let Some(previous) = prior_card_occlusions {
        sugarloaf.set_content_occlusion_rects(previous);
    }
    if let Some([join_x, join_y, join_w, join_h]) = tray_join {
        // This strip replaces (rather than backs) the card's bottom border and
        // fill. The tray and main card each contribute opacity exactly once.
        sugarloaf.quad(
            None,
            join_x,
            join_y,
            join_w,
            join_h,
            composer_help_panel_color(theme),
            [0.0; 4],
            DEPTH,
            ORDER_PANEL + 1,
        );
    }
    if !input_images.is_empty() {
        let image_hits = super::image_preview::render_image_strip(
            sugarloaf,
            &input_images,
            box_x + 18.0 * s,
            box_y + 12.0 * s,
            (box_w - 36.0 * s).max(0.0),
            theme,
            s,
            super::image_preview::COMPOSER_THUMB_SIZE,
            [box_x, box_y, box_w, box_h],
            occlusion_rects,
        );
        pane.composer_interactions().image_previews.extend(image_hits);
    }

    let mut composer_policy = composer_visual_policy(
        responsive_width,
        s,
        show_status,
        !pane.input().trim().is_empty(),
        !matches!(pane.streaming_state(), AgentStreamingStatus::Idle),
        pane.interruptible_run_active(),
    );
    if pane.conversation_source().provider().is_some() {
        composer_policy.show_command_hint = false;
    }
    let control_geometry = composer_footer_geometry(
        [box_x, box_bottom, box_w, chips_band_h],
        s,
        composer_policy,
    );
    let [send_x, send_y, send_side, _] = control_geometry.visual_rect;
    let [chips_x, chips_y, chips_w, _] = control_geometry.chips_rect;
    render_status_chips(
        sugarloaf,
        pane,
        chips_x,
        chips_y + 5.0 * s,
        chips_w,
        composer_policy.narrow,
        home_source,
        theme,
        s,
        occlusion_rects,
    );

    // Retain one exact draft snapshot per pane. Content comparison deliberately
    // catches same-length edits from every input path without a revision API.
    let previous_text = pane.input_viewport_mut().text_snapshot();
    let input_text: Rc<str> = if previous_text.as_ref() == pane.input() {
        previous_text
    } else {
        Rc::from(pane.input())
    };
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
    let text_h = (box_h - text_top_pad - text_bottom_padding * s).max(line_h);
    let max_visible_lines = composer_visible_line_count(text_h, line_h);
    let opts = DrawOpts {
        font_size: 16.0 * s,
        color: text_color,
        clip_rect: Some([text_x, text_y, text_w, text_h]),
        ..DrawOpts::default()
    };
    let wrapped_rows = prepared_wrap_rows
        .cloned()
        .unwrap_or_else(|| wrap_agent_prompt_rows(sugarloaf, text, text_w, &opts));
    // Register the visual rows (byte spans + per-boundary x offsets)
    // back on the pane so Up/Down arrow movement walks the exact rows
    // drawn below AND matches their proportional-font caret positions
    // (see `AgentInputBuffer::move_up_with_history_visual`). The
    // placeholder isn't the draft, so an empty input registers no rows.
    pane.set_input_wrap_rows(if input_text.is_empty() {
        Rc::default()
    } else {
        wrapped_rows.clone()
    });

    // Manual wheel scrolling survives redraws until the draft/caret changes.
    let caret_row = if input_text.is_empty() {
        0
    } else {
        visual_row_index(&wrapped_rows, pane.cursor_byte())
            .unwrap_or(wrapped_rows.len().saturating_sub(1))
    };
    let cursor_byte = pane.cursor_byte();
    let visible_line_offset = pane.input_viewport_mut().update_shared(
        input_text.clone(),
        cursor_byte,
        caret_row,
        wrapped_rows.len(),
        max_visible_lines,
        [text_x, text_y, text_w, max_visible_lines as f32 * line_h],
        line_h,
    );
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

    if active
        && caret_row >= visible_line_offset
        && caret_row < visible_line_offset + max_visible_lines
    {
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
        let cursor_w = prompt_caret_width(opts.font_size);
        let cursor_h = 20.0 * s;
        // Keep the true insertion x, including interior boundaries of an
        // oversized atomic attachment. Clip the overlay, never shift it left.
        let caret_rect = prompt_caret_rect(
            text_x, caret_y, caret_x_in_row, text_w, cursor_w, cursor_h,
        );
        // The trail-cursor overlay paints this rect ABOVE everything,
        // so when a covering panel (the `/` picker, a modal) occludes
        // the caret position, publishing it would punch the caret
        // through that panel. The input text already segments around
        // these rects; the caret must respect them too.
        let occluded = occlusion_rects
            .iter()
            .any(|rect| crate::primitives::geom::rects_intersect(*rect, caret_rect));
        if !occluded && caret_rect[2] > 0.0 {
            pane.set_cursor_rect(Some(caret_rect));
        }
    }

    // Submit shares the footer row with agent/model/thinking. While the
    // model responds it becomes a quiet static stop-square; avoid layering
    // another activity animation here because the status row already owns
    // the live response treatment. Enter still submits — the button mirrors
    // state.
    let busy = composer_policy.action == ComposerControlAction::Interrupt;
    let button_alpha = if composer_policy.action == ComposerControlAction::Disabled {
        0.55
    } else {
        1.0
    };
    sugarloaf.rounded_rect(
        None,
        send_x,
        send_y,
        send_side,
        send_side,
        crate::customization::apply_background_opacity(
            &crate::primitives::surface_background::style("composer.agent"),
            theme.f32_alpha(theme.fg, button_alpha),
        ),
        DEPTH,
        COMPOSER_CONTROL_RADIUS * s,
        ORDER_TEXT,
    );
    pane.register_composer_control_rect(control_geometry.hit_rect);
    if busy {
        // Running state: a static dark stop-square.
        let square = send_side * COMPOSER_STOP_SIDE_RATIO;
        let [stop_x, stop_y, stop_w, stop_h] =
            centered_square(control_geometry.visual_rect, square);
        sugarloaf.rounded_rect(
            None,
            stop_x,
            stop_y,
            stop_w,
            stop_h,
            theme.f32(theme.bg),
            DEPTH,
            (1.5 * s).min(square * 0.3),
            ORDER_TEXT + 1,
        );
    } else {
        // Idle: send arrow.
        let arrow = "\u{2191}";
        let arrow_opts = DrawOpts {
            font_size: send_side * COMPOSER_SEND_FONT_RATIO,
            color: theme.u8(theme.bg),
            bold: true,
            clip_rect: Some(control_geometry.visual_rect),
            ..DrawOpts::default()
        };
        draw_icon_centered_with_occlusion(
            sugarloaf,
            send_x,
            control_geometry.visual_rect,
            arrow,
            &arrow_opts,
            occlusion_rects,
            true,
        );
    }

    if let Some(tray) = geometry.tray {
        render_input_help_strip(
            sugarloaf,
            pane,
            composer_policy,
            tray,
            theme,
            s,
            now_seconds,
            occlusion_rects,
            checkout_context,
        );
    }
}

/// Small keyboard legend below the composer. The left side only
/// advertises interruption while a run is actually active; the right
/// side advertises the slash-command shortcut that is always available.
fn render_input_help_strip(
    sugarloaf: &mut dyn crate::primitives::paint::Paint, pane: &mut impl AgentUserInputPane,
    policy: ComposerVisualPolicy, rect:[f32;4], theme:&IdeTheme, s:f32,
    now_seconds:f32, occlusion_rects:&[[f32;4]], checkout_context:Option<&AgentCheckoutContext>,
) {
    let mut snapshot=host_footer::FooterSnapshot {
        help_visible:pane.input_help_visible(), no_providers:pane.no_providers_connected(),
        streaming:pane.streaming_state(), system:pane.composer_system(),
        interactions:std::mem::take(pane.composer_interactions()),
    };
    host_footer::render_with_policy(sugarloaf,&mut snapshot,policy,rect,theme,s,now_seconds,occlusion_rects,checkout_context);
    *pane.composer_interactions()=snapshot.interactions;
}

fn visible_footer_checkout_context<'a>(
    policy: ComposerVisualPolicy,
    checkout_context: Option<&'a AgentCheckoutContext>,
) -> Option<&'a AgentCheckoutContext> {
    checkout_context.filter(|_| !policy.show_activity)
}

fn composer_system_layout(
    rect: [f32; 4],
    hint_left: f32,
    measured_width: f32,
    has_checkout: bool,
    scale: f32,
) -> ([f32; 4], [f32; 4]) {
    let available = checkout_context_width(rect, hint_left, scale);
    let budget = if has_checkout {
        available * 0.5
    } else {
        available
    };
    let width = measured_width.max(0.0).min(budget).min(180.0 * scale);
    let advance = if width > 0.0 {
        width + 12.0 * scale
    } else {
        0.0
    }
    .min(rect[2].max(0.0));
    (
        [rect[0], rect[1], width, rect[3]],
        [
            rect[0] + advance,
            rect[1],
            (rect[2] - advance).max(0.0),
            rect[3],
        ],
    )
}

fn checkout_context_width(rect: [f32; 4], hint_left: f32, scale: f32) -> f32 {
    (hint_left - 12.0 * scale - rect[0]).clamp(0.0, rect[2].max(0.0))
}

fn checkout_branch_fits(
    branch_x: f32,
    branch_width: f32,
    checkout_right: f32,
    hint_left: f32,
    scale: f32,
) -> bool {
    let branch_right = branch_x + branch_width;
    branch_right <= checkout_right && branch_right + 12.0 * scale <= hint_left
}

#[allow(clippy::too_many_arguments)]
/// Paint the eight-cell activity scanner with font-independent rectangles.
/// Returns its width so callers can place the label one cell after it.
#[allow(clippy::too_many_arguments)]
fn draw_opencode_activity_scanner(sugarloaf:&mut dyn crate::primitives::paint::Paint,x:f32,y:f32,font_size:f32,now_seconds:f32,theme:&IdeTheme,state:AgentStreamingStatus,clip:[f32;4],occlusions:&[[f32;4]])->f32 {
    host_footer::draw_opencode_activity_scanner(sugarloaf,x,y,font_size,now_seconds,theme,state,clip,occlusions)
}

/// Activity block at the timeline tail, pinned above the composer only while
/// following. The caller supplies both its row geometry and hard viewport clip.
#[allow(clippy::too_many_arguments)]
pub fn render_streaming_status_row(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    pane: &mut impl AgentUserInputPane,
    rect: [f32; 4],
    theme: &IdeTheme,
    s: f32,
    now_seconds: f32,
    viewport_clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) {
    pane.clear_activity_status_rects();
    pane.clear_background_status_rect();
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
    let state = pane.streaming_state();
    let accent = streaming_status_accent(theme, state);
    let elapsed = pane.streaming_elapsed_seconds().unwrap_or(0.0);
    let transition = pane.streaming_state_changed_elapsed().unwrap_or(2.0);
    let live_phase = now_seconds;
    let queued_count = pane.queued_prompt_count();
    let status_line_h = STREAMING_STATUS_LINE_H * s;
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
    let chars: Vec<char> = display_label.chars().collect();

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
    let activity_frame = pixel_font.and_then(|_| pane.activity_animation_frame());
    let label_lines =
        wrap_streaming_status_label(sugarloaf, &display_label, bar_w, s, &word_opts);
    // The status row sits immediately above the composer. Anchor the final
    // wrapped line here and grow earlier lines upward through the timeline's
    // reserved status rows; growing downward puts line two behind the island.
    let primary_y = streaming_status_primary_y(bar_y, label_lines.len(), status_line_h);
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
    let glyph_count = label_lines.iter().map(|line| line.chars().count()).sum::<usize>().max(1);
    let mut ix = 0usize;
    let mut animated_hits = Vec::new();
    for (line_ix, line) in label_lines.iter().enumerate() {
        cursor_x = word_x + word_drift_x;
        let lines_above = label_lines.len().saturating_sub(1 + line_ix);
        let line_y = word_y - lines_above as f32 * status_line_h;
        for target_ch in line.chars() {
            let (display, locked) = streaming_status_display_char(target_ch, live_phase, transition, ix, chars.len());
            let mut opts = word_opts;
            let color =
                animated_streaming_status_color(theme, accent, live_phase, ix, locked);
            trailing_color = color;
            opts.color = color;
            // Locked letters ride a travelling wave. Scrambling letters get
            // a smaller shake so the word feels active before it resolves.
            let (sway_x, lift_y) = streaming_status_glyph_motion(live_phase, ix, locked, s, word_drift_y);
            let mut buf = [0u8; 4];
            let glyph = display.encode_utf8(&mut buf);
            let glyph_x = cursor_x + sway_x;
            let glyph_y = line_y - lift_y;
            let glyph_w = sugarloaf.text_mut().measure(glyph, &opts);
            let draw_idle = |sugarloaf: &mut dyn crate::primitives::paint::Paint| {
                let mut far_depth_opts = opts;
                far_depth_opts.color = theme.u8_alpha(theme.bg, 210.0 / 255.0);
                crate::primitives::draw_text_with_occlusion(
                    sugarloaf, glyph_x + 3.5 * s, glyph_y + 3.5 * s,
                    glyph, &far_depth_opts, occlusion_rects,
                );
                let mut near_depth_opts = opts;
                near_depth_opts.color = theme.u8_alpha(theme.dim, 240.0 / 255.0);
                crate::primitives::draw_text_with_occlusion(
                    sugarloaf, glyph_x + 1.75 * s, glyph_y + 1.75 * s,
                    glyph, &near_depth_opts, occlusion_rects,
                );
                crate::primitives::draw_text_with_occlusion(
                    sugarloaf, glyph_x, glyph_y, glyph, &opts, occlusion_rects,
                );
            };
            let show_idle = activity_frame.as_ref().is_none_or(|frame| frame.show_original());
            if show_idle {
                draw_idle(sugarloaf);
            }
            if let Some(frame) = activity_frame.as_ref() {
                let drawn = frame.draw_glyph(
                    sugarloaf, display, ix, glyph_count,
                    [glyph_x, glyph_y, glyph_w, opts.font_size],
                    color.map(|channel| channel as f32 / 255.0),
                    text_clip, occlusion_rects, DEPTH, ORDER_TEXT, &mut animated_hits,
                );
                for hit in animated_hits.drain(..) {
                    pane.register_activity_status_rect(hit);
                }
                if !drawn && !show_idle {
                    draw_idle(sugarloaf);
                }
            }
            for hit in crate::primitives::geom::visible_rect_fragments(
                [glyph_x, glyph_y - 4.0 * s, glyph_w + 3.5 * s, opts.font_size + 8.0 * s],
                text_clip, occlusion_rects,
            ) {
                pane.register_activity_status_rect(hit);
            }
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
        far_depth_opts.color = theme.u8(theme.bg);
        far_depth_opts.color[3] = opts.color[3].saturating_mul(5) / 6;
        draw_text_clipped(
            sugarloaf,
            cursor_x + drift + 3.0 * s,
            dot_floor_y - lift + 3.0 * s,
            ".",
            &far_depth_opts,
            occlusion_rects,
        );
        let mut near_depth_opts = opts;
        near_depth_opts.color = theme.u8(theme.dim);
        near_depth_opts.color[3] = opts.color[3];
        draw_text_clipped(
            sugarloaf,
            cursor_x + drift + 1.5 * s,
            dot_floor_y - lift + 1.5 * s,
            ".",
            &near_depth_opts,
            occlusion_rects,
        );
        draw_text_clipped(
            sugarloaf,
            cursor_x + drift,
            dot_floor_y - lift,
            ".",
            &opts,
            occlusion_rects,
        );
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
    let time_label = if matches!(state, AgentStreamingStatus::Generating) {
        format!("· {} model", format_elapsed(elapsed))
    } else {
        format!("· {} model · {detail}", format_elapsed(elapsed))
    };
    let time_opts = DrawOpts {
        font_size: 13.0 * s,
        color: theme.u8(theme.muted),
        italic: true,
        clip_rect: Some(text_clip),
        ..DrawOpts::default()
    };
    let time_baseline = sugarloaf.text_mut().baseline_offset(&time_opts);
    let time_y = last_line_y + word_baseline - time_baseline;
    if elapsed > 0.0 {
        draw_text_clipped(
            sugarloaf,
            cursor_x,
            time_y,
            &time_label,
            &time_opts,
            occlusion_rects,
        );
    }

    if queued_count > 0 {
        let queue_label = if pane.queued_prompt_preview() == Some("Compaction queued") {
            if queued_count == 1 {
                "Compaction queued".to_string()
            } else {
                format!("Compaction queued ({queued_count} queued)")
            }
        } else if queued_count == 1 {
            "queued item".to_string()
        } else {
            // The count includes control work even when the head is a prompt.
            format!("queued items ({queued_count})")
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
        let queue_y = bar_y
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
        let bg_y = bar_y
            + status_line_h * label_lines.len() as f32
            + if queued_count > 0 { status_line_h } else { 0.0 }
            + (status_line_h - bg_opts.font_size) * 0.5
            - 1.0 * s;
        let hit_left = (bar_x + 40.0 * s).max(text_clip[0]);
        let hit_top = (bg_y - 8.0 * s).max(text_clip[1]);
        let hit_right = (bar_x + bar_w - 40.0 * s).min(text_clip[0] + text_clip[2]);
        let hit_bottom =
            (bg_y - 8.0 * s + status_line_h).min(text_clip[1] + text_clip[3]);
        if hit_right > hit_left && hit_bottom > hit_top {
            pane.register_background_status_rect([
                hit_left,
                hit_top,
                hit_right - hit_left,
                hit_bottom - hit_top,
            ]);
        }
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

/// Last primary line's row origin. Wrapped predecessors grow upward from
/// here, staying inside the primary block; children start after that block.
pub(super) fn streaming_status_primary_y(
    top: f32,
    primary_lines: usize,
    line_h: f32,
) -> f32 {
    top + primary_lines.saturating_sub(1) as f32 * line_h
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
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    label: &str,
    width: f32,
    s: f32,
) -> usize {
    let pixel_font = crate::primitives::pixel_font_id(sugarloaf);
    let opts = DrawOpts {
        font_size: if pixel_font.is_some() {
            12.0 * s
        } else {
            14.0 * s
        },
        bold: true,
        font_id: pixel_font,
        ..DrawOpts::default()
    };
    let label = if label.is_empty() {
        "Background"
    } else {
        label
    };
    wrap_streaming_status_label(sugarloaf, label, width, s, &opts)
        .len()
        .max(1)
}

fn wrap_streaming_status_label(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
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
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    pane: &mut impl AgentUserInputPane,
    mut x: f32,
    y: f32,
    max_w: f32,
    narrow: bool,
    home_source: bool,
    theme: &IdeTheme,
    s: f32,
    occlusion_rects: &[[f32; 4]],
) {
    if max_w <= 0.0 {
        return;
    }
    let mut remaining_w = max_w;
    let offset = usize::from(home_source);
    if home_source
        && pane.conversation_source()
            != crate::panels::agent_pane::state::side_panel::ConversationSource::Neoism
    {
        let font_size = status_chip_font_size(max_w, s);
        let label = pane.conversation_source().label();
        let opts = DrawOpts {
            font_size,
            color: theme.u8(theme.readable_accent(theme.cyan)),
            bold: true,
            ..DrawOpts::default()
        };
        let label_w = sugarloaf.text_mut().measure(label, &opts);
        let width = (label_w + 8.0 * s).min(max_w);
        let clip_rect = Some([x, y - 5.0 * s, width, STATUS_CHIP_HIT_H * s]);
        draw_text_clipped(
            sugarloaf,
            x,
            y,
            label,
            &DrawOpts { clip_rect, ..opts },
            occlusion_rects,
        );
        x += width + 5.0 * s;
        remaining_w = (max_w - width - 5.0 * s).max(0.0);
    }
    // External providers use the same compact mode/model/thinking footer as
    // Neoism. Provider-specific settings live in the normal slash picker.
    let source = pane.conversation_source();
    if source.provider().is_some() {
        let font_size = status_chip_font_size(max_w, s);
        let Some((provider, order, options)) = pane.external_options().map(|snapshot| {
            (
                snapshot.provider.clone(),
                snapshot.footer_order(),
                snapshot.options.clone(),
            )
        }) else {
            return;
        };
        let caret = "\u{f078}";
        let caret_opts = DrawOpts {
            font_size: font_size * 0.66,
            color: theme.u8(theme.muted),
            ..DrawOpts::default()
        };
        let caret_w = sugarloaf.text_mut().measure(caret, &caret_opts);
        let gap = 4.0 * s;
        let mut thought_width = order.iter()
            .map(|&index| &options[index])
            .find(|option| option.category == "thought_level")
            .map_or(0.0, |option| {
                let opts = DrawOpts { font_size, bold: true, ..DrawOpts::default() };
                sugarloaf.text_mut().measure(&capitalize_chip_label(option.selected_label()), &opts)
                    + caret_w + 22.0 * s + gap
            });
        for option_index in order {
            let option = &options[option_index];
            if option.category == "thought_level" {
                thought_width = 0.0;
            }
            let label = match option.category.as_str() {
                "model" => model_chip_label(option.selected_label(), narrow),
                _ => capitalize_chip_label(option.selected_label()),
            };
            let color = match option.category.as_str() {
                "mode" => theme.yellow,
                "model" => theme.blue,
                "thought_level" => theme.magenta,
                _ => continue,
            };
            let opts = DrawOpts {
                font_size,
                color: theme.u8(theme.readable_accent(color)),
                bold: true,
                ..DrawOpts::default()
            };
            let model_kind = (option.category == "model")
                .then(|| {
                    crate::panels::agent_pane::icon::ProviderLogo::from_model(
                        &option.current_value,
                    )
                    .or_else(|| {
                        crate::panels::agent_pane::icon::ProviderLogo::from_provider_id(
                            &provider,
                        )
                    })
                    .or_else(|| model_kind_for_source(source))
                })
                .flatten();
            let logo_advance = model_logo_advance(model_kind, s);
            let label = if option.category == "model" {
                crate::primitives::text::truncate_to_fit(
                    &label,
                    (remaining_w - thought_width - logo_advance - caret_w - 22.0 * s).max(0.0),
                    sugarloaf,
                    &opts,
                )
            } else {
                label
            };
            let width = logo_advance
                + sugarloaf.text_mut().measure(&label, &opts)
                + caret_w
                + 22.0 * s;
            if width > remaining_w {
                break;
            }
            draw_model_logo(sugarloaf, model_kind, x, y, font_size, theme, s, occlusion_rects);
            draw_text_clipped(
                sugarloaf,
                x + logo_advance,
                y,
                &label,
                &opts,
                occlusion_rects,
            );
            draw_text_clipped(
                sugarloaf,
                x + width - caret_w - 13.0 * s,
                y + 2.0 * s,
                caret,
                &caret_opts,
                occlusion_rects,
            );
            pane.register_status_chip_rect(
                option_index + 1 + offset,
                [x - 3.0 * s, y - 5.0 * s, width, STATUS_CHIP_HIT_H * s],
            );
            x += width + gap;
            remaining_w = (remaining_w - width - gap).max(0.0);
        }
        return;
    }
    // Dropdown-look chips: label ˅ — each registers a hit rect so a
    // click opens the matching "/" picker (agent / model / thinking).
    // Build and Plan have explicit, distinct identities; custom agents retain
    // the normal accent while model and thinking keep their existing roles.
    let agent_label = pane.agent_label().to_string();
    let agent_transition = pane.agent_label_changed_elapsed_ms();
    let config_transition = pane.config_chip_transition();
    let agent_color = agent_chip_accent(theme, &agent_label);
    let chips: [(
        String,
        u32,
        Option<crate::panels::agent_pane::icon::ProviderLogo>,
    ); 3] = [
        (agent_label, agent_color, None),
        (
            model_chip_label(pane.model(), narrow),
            theme.blue,
            crate::panels::agent_pane::icon::ProviderLogo::from_model(pane.model())
                .or_else(|| model_kind_for_source(pane.conversation_source())),
        ),
        (
            capitalize_chip_label(pane.thinking_label()),
            theme.magenta,
            None,
        ),
    ];
    let font_size = status_chip_font_size(max_w, s);
    let caret = "\u{f078}";
    let caret_opts = DrawOpts {
        font_size: font_size * 0.66,
        color: theme.u8(theme.muted),
        ..DrawOpts::default()
    };
    let caret_w = sugarloaf.text_mut().measure(caret, &caret_opts);
    let thought_opts = DrawOpts { font_size, bold: true, ..DrawOpts::default() };
    let thought_width = if chips[2].0.is_empty() {
        0.0
    } else {
        sugarloaf.text_mut().measure(&chips[2].0, &thought_opts) + caret_w + 24.0 * s
    };
    let start_x = x;
    for (index, (label, color, model_kind)) in chips.into_iter().enumerate() {
        if label.is_empty() {
            continue;
        }
        let opts = DrawOpts {
            font_size,
            color: theme.u8(theme.readable_accent(color)),
            bold: true,
            ..DrawOpts::default()
        };
        let logo_advance = model_logo_advance(model_kind, s);
        let label = if index == 1 {
            crate::primitives::text::truncate_to_fit(
                &label,
                (start_x + remaining_w - x - thought_width - logo_advance - caret_w - 24.0 * s).max(0.0),
                sugarloaf,
                &opts,
            )
        } else {
            label
        };
        let label_w = sugarloaf.text_mut().measure(&label, &opts);
        let chip_w = logo_advance + label_w + 6.0 * s + caret_w + 18.0 * s;
        if x + chip_w > start_x + remaining_w {
            break;
        }
        let activation = pane.status_chip_activation_ms(index);
        let lift = activation.map_or(0.0, |ms| {
            let progress = (ms / 280.0).clamp(0.0, 1.0);
            -2.5 * s * (1.0 - progress).powi(2)
        });
        draw_model_logo(
            sugarloaf,
            model_kind,
            x,
            y + lift,
            font_size,
            theme,
            s,
            occlusion_rects,
        );
        let label_x = x + logo_advance;
        match config_transition {
            Some(crate::panels::agent_pane::state::ConfigChipTransition::Loading {
                elapsed_ms,
            }) => {
                super::side_panel::draw::render_loading_scramble_text(
                    sugarloaf,
                    label_x,
                    y + lift,
                    &label,
                    &opts,
                    elapsed_ms,
                );
            }
            Some(crate::panels::agent_pane::state::ConfigChipTransition::Settling {
                elapsed_ms,
            }) => {
                super::side_panel::draw::render_scramble_text(
                    sugarloaf,
                    label_x,
                    y + lift,
                    &label,
                    &opts,
                    elapsed_ms,
                );
            }
            None if index == 0 && agent_transition.is_some() => {
                super::side_panel::draw::render_scramble_text(
                    sugarloaf,
                    label_x,
                    y + lift,
                    &label,
                    &opts,
                    agent_transition.unwrap(),
                );
            }
            None => {
                draw_text_clipped(
                    sugarloaf,
                    label_x,
                    y + lift,
                    &label,
                    &opts,
                    occlusion_rects,
                );
            }
        }
        draw_text_clipped(
            sugarloaf,
            label_x + label_w + 6.0 * s,
            y + 3.5 * s + lift,
            caret,
            &caret_opts,
            occlusion_rects,
        );
        if let Some(ms) = activation {
            let fade = 1.0 - ms / 280.0;
            let accent = theme.u8(theme.readable_accent(color));
            let width = (logo_advance + label_w) * (1.0 - fade.powi(3));
            let line = [x, y + 17.0 * s, width, 1.5 * s];
            if !occlusion_rects
                .iter()
                .any(|rect| super::draw::intersect_rect(line, *rect).is_some())
            {
                draw_rect_clipped(
                    sugarloaf,
                    line,
                    [
                        accent[0] as f32 / 255.0,
                        accent[1] as f32 / 255.0,
                        accent[2] as f32 / 255.0,
                        fade * 0.65,
                    ],
                    ORDER_TEXT,
                    [x, y - 5.0 * s, chip_w, STATUS_CHIP_HIT_H * s],
                );
            }
        }
        pane.register_status_chip_rect(
            index + offset,
            [
                x - 4.0 * s,
                y - 5.0 * s,
                chip_w - 8.0 * s,
                STATUS_CHIP_HIT_H * s,
            ],
        );
        x += chip_w;
    }
}

fn model_kind_for_source(
    source: crate::panels::agent_pane::state::side_panel::ConversationSource,
) -> Option<crate::panels::agent_pane::icon::ProviderLogo> {
    use crate::panels::agent_pane::icon::ProviderLogo;
    use crate::panels::agent_pane::state::side_panel::ConversationSource;
    match source {
        ConversationSource::Neoism => None,
        ConversationSource::OpenCode => Some(ProviderLogo::OpenCode),
        ConversationSource::ClaudeCode => Some(ProviderLogo::Anthropic),
        ConversationSource::Codex => Some(ProviderLogo::OpenAi),
    }
}

fn model_logo_advance(
    kind: Option<crate::panels::agent_pane::icon::ProviderLogo>,
    s: f32,
) -> f32 {
    if kind.is_some() {
        (MODEL_LOGO_SIDE + MODEL_LOGO_GAP) * s
    } else {
        0.0
    }
}

fn status_chip_icon_rect(x: f32, text_y: f32, side: f32, font_size: f32, s: f32) -> [f32; 4] {
    let side = side * s;
    // Text is drawn from its nominal em-box top, independently of the hit target.
    [x, text_y + (font_size - side) * 0.5, side, side]
}

fn draw_model_logo(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    kind: Option<crate::panels::agent_pane::icon::ProviderLogo>,
    x: f32,
    y: f32,
    font_size: f32,
    theme: &IdeTheme,
    s: f32,
    occlusion_rects: &[[f32; 4]],
) {
    let Some(kind) = kind else {
        return;
    };
    let color = kind.composer_color(
        theme.is_dark(), theme.u8(theme.readable_accent(theme.blue)),
    );
    let Some(image_id) =
        crate::panels::agent_pane::icon::register_provider_logo_atlas(sugarloaf, color)
    else {
        return;
    };
    let scale = sugarloaf.scale_factor();
    push_image_overlay_clipped(
        sugarloaf,
        OVERLAY_PANEL_ID,
        image_id,
        status_chip_icon_rect(x, y, MODEL_LOGO_SIDE, font_size, s),
        kind.source_rect(),
        2,
        scale,
        occlusion_rects,
    );
}

#[cfg(test)]
mod composer_visual_policy_tests {
    use super::*;
    use super::super::{composer_geometry::COMPOSER_CORNER_RADIUS, INPUT_HELP_STRIP_H};

    #[test]
    fn system_footer_reserves_directory_space_and_never_crosses_hints() {
        for scale in [0.75, 1.0, 1.5, 2.0] {
            let rect = [10.0 * scale, 20.0 * scale, 400.0 * scale, 24.0 * scale];
            let hint_left = 300.0 * scale;
            let (system, directory) = composer_system_layout(rect, hint_left, 1000.0, true, scale);
            assert!(system[2] <= checkout_context_width(rect, hint_left, scale) * 0.5);
            assert!(system[0] + system[2] < directory[0]);
            assert_eq!(directory[0] + directory[2], rect[0] + rect[2]);
            assert!(system[0] + system[2] < hint_left);
            let (hidden, unchanged) = composer_system_layout(rect, rect[0], 100.0, true, scale);
            assert_eq!(hidden[2], 0.0);
            assert_eq!(unchanged, rect);
        }
    }

    #[test]
    fn checkout_tray_is_narrower_and_touches_the_composer_bottom() {
        for scale in [0.75, 1.0, 1.5, 2.0] {
            for width in [0.0, 32.0, 280.0, 900.0] {
                let card = [30.0, 80.0, width * scale, 98.0 * scale];
                let geometry = ComposerGeometry::new(card, scale, 1.0, true);
                let Some(tray) = geometry.tray else {
                    assert_eq!(width, 0.0);
                    continue;
                };
                assert_eq!(tray[1], geometry.card[1] + geometry.card[3]);
                assert!((tray[3] + geometry.card[3] - card[3] - INPUT_HELP_STRIP_H * scale).abs() < 0.001);
                assert!(tray[0] >= card[0]);
                assert!(tray[0] + tray[2] <= card[0] + card[2]);
                assert!(tray[2] >= 0.0);
                if width >= 280.0 {
                    assert_eq!(tray[0] - card[0], 24.0 * scale);
                    assert_eq!(card[2] - tray[2], 48.0 * scale);
                }
            }
        }
    }

    #[test]
    fn composer_background_follows_every_selected_theme_instead_of_forcing_black() {
        for name in crate::primitives::ide_theme::IdeThemeName::ALL {
            let theme = IdeTheme::by_name(name.as_str());
            assert_eq!(
                composer_background_color(&theme),
                crate::primitives::surface_background::base_color(
                    "composer.agent", &theme, theme.f32(theme.bg),
                ),
                "{name:?}",
            );
        }
    }

    #[test]
    fn composer_and_tray_preserve_theme_tokens_and_configured_opacity() {
        let mut themes: Vec<_> = crate::primitives::ide_theme::IdeThemeName::ALL
            .into_iter().map(|name| IdeTheme::by_name(name.as_str())).collect();
        themes.push(IdeTheme {
            bg: 0xf2_f4_f8,
            surface: 0xff_ff_ff,
            ..IdeTheme::pastel_dark()
        });
        for theme in themes {
            let shell = composer_background_color(&theme);
            let tray = composer_help_panel_color(&theme);
            assert_eq!(shell, crate::primitives::surface_background::base_color(
                "composer.agent", &theme, theme.f32(theme.bg),
            ));
            assert_eq!(tray, crate::primitives::surface_background::base_color(
                "composer.agent", &theme, theme.f32(theme.surface),
            ));
            assert!(tray[..3].iter().zip(&shell[..3]).all(|(a, b)| a > b));
            assert_eq!(tray[3], shell[3]);
        }
    }

    #[test]
    fn help_strip_defines_slash_commands() {
        assert_eq!(INPUT_COMMAND_HINT, ("/", "commands"));
    }

    #[test]
    fn checkout_uses_all_space_before_hints_instead_of_a_percentage_cap() {
        for s in [0.5, 1.0, 2.0, 3.0] {
            let rect = [20.0 * s, 100.0 * s, 1000.0 * s, 20.0 * s];
            let hint_left = 820.0 * s;
            let available = checkout_context_width(rect, hint_left, s);
            assert_eq!(available, 788.0 * s);
            let checkout_right = rect[0] + available;
            // This whole branch extends beyond the old 58% cutoff but fits.
            assert!(checkout_branch_fits(
                620.0 * s,
                188.0 * s,
                checkout_right,
                hint_left,
                s,
            ));
            assert!(!checkout_branch_fits(
                620.0 * s,
                188.5 * s,
                checkout_right,
                hint_left,
                s,
            ));
            assert_eq!(
                checkout_context_width(rect, rect[0] + rect[2], s),
                988.0 * s,
            );
            assert_eq!(checkout_context_width(rect, rect[0] + 5.0 * s, s), 0.0);
        }
    }

    #[test]
    fn checkout_branch_is_hidden_unless_the_whole_group_fits() {
        for s in [0.5, 1.0, 2.0, 3.0] {
            let fits = |start, width, clip_right, hint_left| {
                checkout_branch_fits(
                    start * s,
                    width * s,
                    clip_right * s,
                    hint_left * s,
                    s,
                )
            };
            assert!(fits(200.0, 88.0, 350.0, 300.0));
            assert!(!fits(200.0, 88.5, 350.0, 300.0));
            assert!(fits(200.0, 150.0, 350.0, 400.0));
            assert!(!fits(200.0, 150.5, 350.0, 400.0));
            assert!(!fits(320.0, 88.0, 350.0, 400.0));
            assert!(!fits(200.0, 300.0, 350.0, 400.0));
            // Without hints, the footer edge is the only hint boundary.
            assert!(fits(200.0, 150.0, 350.0, 600.0));
        }
    }

    #[test]
    fn narrow_streaming_hides_only_escape_hint_and_keeps_adjacent_help() {
        let policy = composer_visual_policy(390.0, 1.0, false, false, true, true);
        assert!(policy.narrow);
        assert!(policy.show_activity);
        assert!(!policy.show_interrupt_hint);
        assert!(policy.show_command_hint);
        assert_eq!(policy.action, ComposerControlAction::Interrupt);
        assert_eq!(policy.visual_side, 26.0);
        assert_eq!(policy.hit_side, 44.0);
    }

    #[test]
    fn wide_streaming_retains_escape_hint_and_desktop_size() {
        let policy = composer_visual_policy(900.0, 1.0, false, true, true, true);
        assert!(!policy.narrow);
        assert!(policy.show_activity);
        assert!(policy.show_interrupt_hint);
        assert!(policy.show_command_hint);
        assert_eq!(policy.action, ComposerControlAction::Interrupt);
        assert_eq!(policy.visual_side, 26.0);
        assert_eq!(policy.hit_side, 26.0);
    }

    #[test]
    fn settled_chat_shows_checkout_context_but_running_chat_uses_activity_slot() {
        let context =
            AgentCheckoutContext::new(Some("/workspace".into()), Some("main".into()));
        let settled = composer_visual_policy(900.0, 1.0, false, false, false, false);
        let running = composer_visual_policy(900.0, 1.0, false, false, true, true);

        assert_eq!(
            visible_footer_checkout_context(settled, Some(&context)),
            Some(&context)
        );
        assert_eq!(
            visible_footer_checkout_context(running, Some(&context)),
            None
        );
        assert!(running.show_activity);
        assert!(running.show_interrupt_hint);
    }

    #[test]
    fn hint_breakpoint_compacts_only_phone_widths() {
        let cramped = composer_visual_policy(519.0, 1.0, false, false, true, true);
        let wide = composer_visual_policy(520.0, 1.0, false, false, true, true);
        let scaled_cramped =
            composer_visual_policy(1038.0, 2.0, false, false, true, true);
        let scaled_wide = composer_visual_policy(1040.0, 2.0, false, false, true, true);

        for policy in [cramped, scaled_cramped] {
            assert!(policy.narrow);
            assert!(!policy.show_interrupt_hint);
            assert!(policy.show_activity);
            assert!(policy.show_command_hint);
        }
        for policy in [wide, scaled_wide] {
            assert!(!policy.narrow);
            assert!(policy.show_interrupt_hint);
        }
    }

    #[test]
    fn idle_control_is_send_with_draft_and_disabled_without_one() {
        let send = composer_visual_policy(390.0, 1.0, false, true, false, false);
        let disabled = composer_visual_policy(390.0, 1.0, false, false, false, false);
        assert_eq!(send.action, ComposerControlAction::Send);
        assert_eq!(disabled.action, ComposerControlAction::Disabled);
        assert_eq!(send.visual_side, disabled.visual_side);
        assert_eq!(send.hit_side, disabled.hit_side);
    }

    #[test]
    fn narrow_non_interruptible_status_keeps_scanner_and_command_help() {
        let policy = composer_visual_policy(390.0, 1.0, false, false, true, false);
        assert!(policy.show_activity);
        assert!(!policy.show_interrupt_hint);
        assert!(policy.show_command_hint);
        assert_eq!(policy.action, ComposerControlAction::Disabled);
    }

    #[test]
    fn status_chip_type_scales_down_without_shrinking_hit_height() {
        let compact = status_chip_font_size(360.0, 1.0);
        let desktop = status_chip_font_size(900.0, 1.0);
        assert_eq!(compact, STATUS_CHIP_COMPACT_FONT);
        assert_eq!(desktop, STATUS_CHIP_DESKTOP_FONT);
        assert!(STATUS_CHIP_HIT_H > desktop);
    }

    #[test]
    fn model_logo_stays_centered_on_the_label_at_every_responsive_size() {
        for scale in [0.75, 1.0, 1.25, 1.5, 2.0] {
            for width in [360.0, 420.0, 590.0, 760.0, 900.0] {
                let font_size = status_chip_font_size(width * scale, scale);
                for lift in [0.0, -1.25 * scale, -2.5 * scale] {
                    let text_y = 100.25 + lift;
                    let rect = status_chip_icon_rect(
                        20.0, text_y, MODEL_LOGO_SIDE, font_size, scale,
                    );
                    assert!((rect_center(rect)[1] - (text_y + font_size * 0.5)).abs() < 0.0001);
                    assert_eq!(rect[2], MODEL_LOGO_SIDE * scale);
                    assert_eq!(rect[3], rect[2]);
                    assert_eq!(rect[0], 20.0);
                }
            }
            assert_eq!(model_logo_advance(None, scale), 0.0);
        }
    }

    #[test]
    fn status_chip_type_respects_renderer_scale() {
        assert_eq!(
            status_chip_font_size(720.0, 2.0),
            STATUS_CHIP_COMPACT_FONT * 2.0
        );
    }

    #[test]
    fn chip_capitalization_is_display_only_and_preserves_acronyms() {
        let full = "openrouter/openai/gpt-5.6";
        assert_eq!(model_chip_label(full, true), "Gpt-5.6");
        assert_eq!(model_chip_label(full, false), "Openrouter/Openai/Gpt-5.6");
        assert_eq!(full, "openrouter/openai/gpt-5.6");
        assert_eq!(model_chip_label("provider//family/model/", true), "Model");
        assert_eq!(model_chip_label("/model/", true), "Model");
        assert_eq!(model_chip_label("", true), "");
        assert_eq!(model_chip_label("///", true), "");
        assert_eq!(capitalize_chip_label("high"), "High");
        assert_eq!(capitalize_chip_label("extra high"), "Extra High");
        assert_eq!(capitalize_chip_label("OpenAI/GPT-5.6"), "OpenAI/GPT-5.6");
        assert_eq!(model_chip_label("openai/gpt-6-astra", false), "Openai/Gpt-6-Astra");
        assert_eq!(model_chip_label("openai/gpt-6-astra", true), "Gpt-6-Astra");
        assert_eq!(capitalize_chip_label("extra_high"), "Extra_High");
    }

    #[test]
    fn narrow_control_separates_original_visual_square_from_touch_hit_rect() {
        let idle = composer_visual_policy(390.0, 1.0, false, true, false, false);
        let streaming = composer_visual_policy(390.0, 1.0, false, true, true, true);
        let footer = [10.0, 120.0, 300.0, CHIPS_BAND_H];
        let idle_geometry = composer_footer_geometry(footer, 1.0, idle);
        let streaming_geometry = composer_footer_geometry(footer, 1.0, streaming);

        assert_eq!(idle.action, ComposerControlAction::Send);
        assert_eq!(streaming.action, ComposerControlAction::Interrupt);
        assert_eq!(idle_geometry.visual_rect, [275.0, 129.0, 26.0, 26.0]);
        assert_eq!(idle_geometry.hit_rect, [266.0, 120.0, 44.0, 44.0]);
        assert_eq!(streaming_geometry, idle_geometry);
    }

    #[test]
    fn visual_control_metrics_match_original_chat_and_home_buttons() {
        let chat = composer_visual_policy(390.0, 1.0, false, true, false, false);
        let home = composer_visual_policy(390.0, 1.0, true, true, false, false);
        assert_eq!(chat.visual_side, 26.0);
        assert_eq!(home.visual_side, 30.0);
        assert_eq!(COMPOSER_CONTROL_RADIUS, 8.0);
        assert_eq!(COMPOSER_STOP_SIDE_RATIO, 0.28);
        assert_eq!(COMPOSER_SEND_FONT_RATIO, 0.62);
        assert_eq!(COMPOSER_CORNER_RADIUS, 20.0);
    }

    #[test]
    fn action_glyph_centers_match_chat_home_and_narrow_action_centers() {
        // `draw_icon_centered_with_occlusion` centers real raster ink in these
        // target rects on both the desktop and wasm text backends. Sweep the
        // renderer geometry that feeds it rather than encoding font offsets.
        for (width, scale, show_status) in [
            (900.0, 1.0, false),
            (900.0, 1.0, true),
            (390.0, 1.0, false),
            (780.0, 2.0, false),
            (780.0, 2.0, true),
        ] {
            let policy =
                composer_visual_policy(width, scale, show_status, true, false, false);
            let send = composer_footer_geometry(
                [20.0, 180.0, width, CHIPS_BAND_H * scale], scale, policy,
            );
            let action_center = rect_center(send.visual_rect);
            assert_eq!(rect_center(send.visual_rect), action_center);
            assert_eq!(
                rect_center(centered_square(
                    send.visual_rect,
                    send.visual_rect[2] * COMPOSER_STOP_SIDE_RATIO,
                )),
                action_center
            );
            assert_eq!(rect_center(send.chips_rect)[1], action_center[1]);
        }
    }

    #[test]
    fn footer_chips_and_send_share_a_row_without_overlapping_hit_targets() {
        for scale in [0.75, 1.0, 1.5, 2.0] {
            for width in [280.0, 390.0, 900.0] {
                for home in [false, true] {
                    let policy = composer_visual_policy(width * scale, scale, home, true, false, false);
                    let footer = [20.0, 160.0, width * scale, CHIPS_BAND_H * scale];
                    let geometry = composer_footer_geometry(footer, scale, policy);
                    assert_eq!(rect_center(geometry.chips_rect)[1], rect_center(geometry.visual_rect)[1]);
                    assert!(geometry.chips_rect[0] + geometry.chips_rect[2] <= geometry.hit_rect[0] - 7.99 * scale);
                    assert_eq!(rect_center(geometry.hit_rect), rect_center(geometry.visual_rect));
                    assert!(geometry.hit_rect[1] >= footer[1]);
                    assert!(geometry.hit_rect[1] + geometry.hit_rect[3] <= footer[1] + footer[3]);
                    assert!(geometry.visual_rect[1] >= footer[1]);
                    assert!(geometry.visual_rect[1] + geometry.visual_rect[3] <= footer[1] + footer[3]);
                    if policy.narrow {
                        assert_eq!(geometry.hit_rect[3], 44.0 * scale);
                    }
                }
            }
        }
    }

    #[test]
    fn shift_enter_keeps_caret_on_the_new_row_without_premature_scrolling() {
        use crate::panels::agent_pane::input_controller::AgentInputBuffer;
        use crate::panels::agent_pane::input_viewport::InputViewport;

        for scale in [0.6, 0.75, 0.8, 0.95, 1.0, 1.2, 1.5, 2.0] {
            for home in [false, true] {
                for image_h in [0.0, super::super::INPUT_IMAGE_RAIL_H * scale] {
                    let mut input = AgentInputBuffer::new(
                        "draft".into(), 5, Vec::new(), None, String::new(),
                    );
                    let mut viewport = InputViewport::default();
                    for rows in 1..=MAX_INPUT_LINES + 3 {
                        let wrapped = wrap_agent_prompt_rows_with(&input.input, 100.0, |s| {
                            s.chars().count() as f32
                        });
                        assert_eq!(wrapped.len(), rows);
                        let caret_row = visual_row_index(&wrapped, input.cursor_byte).unwrap();
                        assert_eq!(caret_row, rows - 1);
                        let h = super::super::layout::input_height_for_visual_rows(
                            rows, scale, home, false,
                        ) + image_h;
                        let (top_pad, bottom_pad) = composer_text_padding(home);
                        let text_top_pad = top_pad * scale + image_h;
                        let line_h = INPUT_LINE_H * scale;
                        let text_h = (h - CHIPS_BAND_H * scale - text_top_pad
                            - bottom_pad * scale).max(line_h);
                        let capacity = composer_visible_line_count(text_h, line_h);
                        assert_eq!(capacity, rows.min(MAX_INPUT_LINES));
                        let first_row = viewport.update(
                            &input.input, input.cursor_byte, caret_row, wrapped.len(), capacity,
                            [0.0, text_top_pad, 100.0, capacity as f32 * line_h], line_h,
                        );
                        assert_eq!(first_row, rows.saturating_sub(MAX_INPUT_LINES));
                        assert_eq!(caret_row - first_row, (rows - 1).min(MAX_INPUT_LINES - 1));
                        input.insert_text("\n");
                    }
                }
            }
        }
    }

    #[test]
    fn visible_line_count_does_not_round_genuinely_partial_rows_up() {
        for scale in [0.6, 0.8, 0.95, 1.0, 1.2, 2.0] {
            let line_h = INPUT_LINE_H * scale;
            assert_eq!(composer_visible_line_count(1.5 * line_h, line_h), 1);
            assert_eq!(composer_visible_line_count(1.999 * line_h, line_h), 1);
            assert_eq!(composer_visible_line_count(4.2 * line_h, line_h), 4);
        }
    }

    #[test]
    fn narrow_control_paint_and_hit_width_stay_inside_composer() {
        let policy = composer_visual_policy(32.0, 1.0, false, true, false, false);
        let geometry = composer_footer_geometry([100.0, 120.0, 32.0, CHIPS_BAND_H], 1.0, policy);
        assert_eq!(geometry.chips_rect[2], 0.0);
        for rect in [geometry.visual_rect, geometry.hit_rect] {
            assert!(rect[0] >= 100.0);
            assert!(rect[2] >= 0.0);
            assert!(rect[0] + rect[2] <= 132.0);
        }
    }
}

fn draw_agent_prompt_text(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
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

fn prompt_caret_width(font_size: f32) -> f32 {
    (font_size * 0.6).max(2.0)
}

/// Original prompt block geometry for a host that supplies its own native
/// shaped insertion point. Text wrapping, IME and selection stay in that host.
pub fn prompt_caret_rect_for_host(x:f32,y:f32,remaining_width:f32,font_size:f32,scale:f32)->[f32;4]{
    prompt_caret_rect(x,y,0.0,remaining_width,prompt_caret_width(font_size),20.0*scale)
}

fn prompt_caret_rect(
    text_x: f32,
    y: f32,
    insertion_x: f32,
    text_w: f32,
    caret_w: f32,
    caret_h: f32,
) -> [f32; 4] {
    [text_x + insertion_x, y, caret_w.min((text_w - insertion_x).max(0.0)), caret_h]
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

    // Strict slot fit: equality leaves the insertion point on this row.
    // Full/oversized rows close even with a zero reserve (legacy test core).
    // Re-evaluate this BEFORE placing any next glyph/token so a narrow append
    // cannot flow backwards into a row whose EOF caret already moved down.
    fn is_closed(&self, max_w: f32, caret_slot: f32) -> bool {
        self.end > self.start
            && (self.width >= max_w || self.width + caret_slot > max_w)
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
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    text: &str,
    max_w: f32,
    opts: &DrawOpts,
) -> Rc<Vec<InputWrapRow>> {
    wrap_cache::prompt(sugarloaf, text, max_w, opts, |sugarloaf| {
        // Slot and glyph measurements are both in the scaled DrawOpts units.
        // The cache already keys font_size (and backend scale), so the derived
        // slot cannot change independently of its layout key.
        wrap_agent_prompt_rows_with_slot(text, max_w, prompt_caret_width(opts.font_size), |part| {
            sugarloaf.text_mut().measure(part, opts)
        })
    })
}

#[cfg(test)]
fn wrap_agent_prompt_rows_with(
    text: &str,
    max_w: f32,
    measure: impl FnMut(&str) -> f32,
) -> Vec<InputWrapRow> {
    wrap_agent_prompt_rows_with_slot(text, max_w, 0.0, measure)
}

fn wrap_agent_prompt_rows_with_slot(
    text: &str,
    max_w: f32,
    caret_slot: f32,
    mut measure: impl FnMut(&str) -> f32,
) -> Vec<InputWrapRow> {
    let token_spans = agent_attachment_token_spans(text);
    let mut wrap = PromptWrapRanges {
        offsets: vec![0.0],
        ..PromptWrapRanges::default()
    };
    let mut cursor = 0;

    for (start, end) in token_spans {
        push_wrapped_prompt_segment(text, cursor, start, max_w, caret_slot, &mut measure, &mut wrap);
        let token = &text[start..end];
        let token_w = measure(token);
        if wrap.is_closed(max_w, caret_slot)
            || (wrap.end > wrap.start && wrap.width + token_w > max_w)
        {
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

    push_wrapped_prompt_segment(text, cursor, text.len(), max_w, caret_slot, &mut measure, &mut wrap);
    // Materialize the closed row's EOF successor now, using exactly the same
    // predicate as append placement. A newline instead terminates the current
    // row directly, consuming this pending soft break without a phantom row.
    let trailing_wrap_boundary = wrap.is_closed(max_w, caret_slot);
    if trailing_wrap_boundary {
        let end = wrap.end;
        wrap.push_row(end);
    }
    // A trailing hard newline creates a real empty visual row. Without
    // preserving it, Shift+Enter updates `cursor_byte` past the last
    // registered row and the renderer falls back to painting the caret
    // on the first/top row.
    if trailing_wrap_boundary
        || wrap.end > wrap.start
        || wrap.lines.is_empty()
        || text.ends_with('\n')
    {
        let end = wrap.end;
        wrap.push_row(end);
    }
    wrap.lines
}

/// Measure the prompt with the exact wrap core used by paint, caret placement,
/// and visual Up/Down movement. Composer layout calls this before choosing its
/// height so a newly-created visual row expands the card in the same frame.
pub fn measure_prompt_rows_for_host(paint:&mut (impl crate::primitives::paint::Paint + ?Sized),input:&str,input_w:f32,s:f32)->Vec<InputWrapRow>{
    let text=if input.is_empty(){"Ask anything"}else{input};
    let text_w=(input_w-36.0*s).max(20.0*s);
    let opts=DrawOpts{font_size:16.0*s,..Default::default()};
    wrap_agent_prompt_rows_with_slot(text,text_w,prompt_caret_width(opts.font_size),|part|paint.text_mut().measure(part,&opts))
}

pub(super) fn measure_prompt_visual_rows(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    input: &str,
    input_w: f32,
    s: f32,
) -> Rc<Vec<InputWrapRow>> {
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
    text: &str,
    seg_start: usize,
    seg_end: usize,
    max_w: f32,
    caret_slot: f32,
    measure: &mut impl FnMut(&str) -> f32,
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
        let ch_w = measure(s);
        if wrap.is_closed(max_w, caret_slot)
            || (wrap.end > wrap.start && wrap.width + ch_w > max_w)
        {
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
