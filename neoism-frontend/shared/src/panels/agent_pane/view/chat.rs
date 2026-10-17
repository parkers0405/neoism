use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::panels::agent_pane::conversation_header::{ConversationNavigation, Direction};
use crate::panels::agent_pane::state::NeoismAgentPane;

use super::timeline::{AgentTimelineDelegate, AgentTimelinePane, SharedTimelineDelegate};
use super::user_input::AgentUserInputPane;
use super::{timeline, user_input, AgentCheckoutContext};
use crate::panels::agent_pane::input_controller::InputWrapRow;
use crate::primitives::ide_theme::IdeTheme;

pub trait AgentChatPane: AgentTimelinePane + AgentUserInputPane {
    fn imported_history(
        &self,
    ) -> Option<&crate::panels::agent_pane::api_mapping::ImportedHistory> {
        None
    }
    fn has_conversation(&self) -> bool;
    fn maybe_refresh_side_panel_subagents(&mut self);
    fn is_subagent_session(&self) -> bool;
    fn conversation_navigation(&self) -> ConversationNavigation<'_>;
    fn conversation_header(
        &mut self,
    ) -> &mut crate::panels::agent_pane::conversation_header::ConversationHeader;
}

#[macro_export]
macro_rules! neoism_ui_impl_agent_chat_pane {
    ($pane:ty) => {
        impl $crate::panels::agent_pane::view::chat::AgentChatPane for $pane {
            fn imported_history(
                &self,
            ) -> Option<&$crate::panels::agent_pane::api_mapping::ImportedHistory> {
                <$pane>::imported_history(self)
            }

            fn has_conversation(&self) -> bool {
                <$pane>::has_conversation(self)
            }

            fn maybe_refresh_side_panel_subagents(&mut self) {
                <$pane>::maybe_refresh_side_panel_subagents(self);
            }

            fn is_subagent_session(&self) -> bool {
                <$pane>::is_subagent_session(self)
            }

            fn conversation_navigation(&self) -> $crate::panels::agent_pane::conversation_header::ConversationNavigation<'_> {
                <$pane>::conversation_navigation(self)
            }

            fn conversation_header(
                &mut self,
            ) -> &mut $crate::panels::agent_pane::conversation_header::ConversationHeader
            {
                &mut self.conversation_header
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

    fn conversation_navigation(&self) -> ConversationNavigation<'_> {
        NeoismAgentPane::conversation_navigation(self)
    }

    fn conversation_header(
        &mut self,
    ) -> &mut crate::panels::agent_pane::conversation_header::ConversationHeader {
        &mut self.conversation_header
    }
}

struct HeaderLayout {
    previous: [f32; 4],
    next: Option<[f32; 4]>,
    timeline_top: f32,
}

fn navigation_header_layout(
    rect: [f32; 4],
    scale: f32,
    previous_width: f32,
    next_width: Option<f32>,
) -> Option<HeaderLayout> {
    if !rect.iter().all(|v| v.is_finite())
        || !scale.is_finite()
        || scale <= 0.0
        || rect[2] < 48.0 * scale
        || rect[3] < 28.0 * scale
        || !previous_width.is_finite()
        || previous_width <= 0.0
    {
        return None;
    }
    let previous_w = (previous_width + 4.0 * scale).min(rect[2]);
    let next = next_width
        .filter(|width| width.is_finite() && *width > 0.0)
        .map(|width| width + 4.0 * scale)
        .filter(|width| previous_w + width + 16.0 * scale <= rect[2])
        .map(|width| {
            [
                rect[0] + rect[2] - width,
                rect[1] + 3.0 * scale,
                width,
                22.0 * scale,
            ]
        });
    Some(HeaderLayout {
        previous: [rect[0], rect[1] + 3.0 * scale, previous_w, 22.0 * scale],
        next,
        timeline_top: rect[1] + 28.0 * scale,
    })
}

#[cfg(test)]
mod header_tests {
    use super::navigation_header_layout;

    #[test]
    fn conversation_navigation_header_has_separate_left_and_right_links() {
        for scale in [0.75, 1.0, 1.5, 2.0] {
            let rect = [10.0 * scale, 20.0 * scale, 320.0 * scale, 240.0 * scale];
            let layout =
                navigation_header_layout(rect, scale, 80.0 * scale, Some(40.0 * scale))
                    .unwrap();
            assert_eq!(
                layout.previous,
                [10.0 * scale, 23.0 * scale, 84.0 * scale, 22.0 * scale]
            );
            assert_eq!(
                layout.next,
                Some([286.0 * scale, 23.0 * scale, 44.0 * scale, 22.0 * scale])
            );
            assert_eq!(layout.timeline_top, 48.0 * scale);
            assert!(layout.previous[1] + layout.previous[3] < layout.timeline_top);
            let next = layout.next.unwrap();
            assert!(layout.previous[0] + layout.previous[2] + 16.0 * scale <= next[0]);
            assert_eq!(next[0] + next[2], rect[0] + rect[2]);
        }
    }

    #[test]
    fn conversation_navigation_header_omits_next_instead_of_overlapping() {
        let layout =
            navigation_header_layout([7.0, 10.0, 52.0, 100.0], 1.0, 80.0, Some(40.0))
                .unwrap();
        assert_eq!(layout.previous[2], 52.0);
        assert!(layout.next.is_none());
        assert!(
            navigation_header_layout([0.0, 0.0, 200.0, 100.0], 1.0, 40.0, None)
                .unwrap()
                .next
                .is_none()
        );
    }

    #[test]
    fn conversation_navigation_header_cannot_leave_hits_in_tiny_or_invalid_panes() {
        assert!(
            navigation_header_layout([0.0, 0.0, 47.0, 100.0], 1.0, 40.0, None).is_none()
        );
        assert!(
            navigation_header_layout([0.0, 0.0, 100.0, 27.0], 1.0, 40.0, None).is_none()
        );
        assert!(
            navigation_header_layout([0.0, 0.0, 100.0, 100.0], 0.0, 40.0, None).is_none()
        );
        assert!(
            navigation_header_layout([0.0, 0.0, 100.0, 100.0], 1.0, f32::NAN, None)
                .is_none()
        );
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
    plugins: Option<&neoism_lua::PluginSnapshot>,
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
        plugins,
        None,
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
    plugins: Option<&neoism_lua::PluginSnapshot>,
    prepared_input_rows: Option<&[InputWrapRow]>,
    checkout_context: Option<&AgentCheckoutContext>,
) where
    P: AgentChatPane,
    D: AgentTimelineDelegate<P>,
{
    pane.conversation_header().clear();
    if pane.has_conversation() || pane.is_subagent_session() {
        pane.maybe_refresh_side_panel_subagents();
    }
    // Timeline lives in the SAME column as the composer island (same
    // insets + wide-pane cap, centered) — content wider than the
    // floating input bar would extend past its borders and kill the
    // island illusion.
    let (content_x, content_w) = super::layout::chat_column(rect, s);
    let mut timeline_top = rect[1];
    let navigation = pane.conversation_navigation();
    let previous_label = if navigation.previous_is_main {
        "\u{2190} Back"
    } else {
        "\u{2190} Prev"
    };
    let next_label = "Next \u{2192}";
    let targets = [
        navigation.previous.map(str::to_owned),
        navigation.next.map(str::to_owned),
    ];
    if let Some(previous_target) = targets[0].as_deref() {
        let opts = DrawOpts {
            font_size: 12.0 * s,
            color: theme.u8(theme.muted),
            ..DrawOpts::default()
        };
        let previous_width = sugarloaf.text_mut().measure(previous_label, &opts);
        let next_width = targets[1]
            .as_ref()
            .map(|_| sugarloaf.text_mut().measure(next_label, &opts));
        let column = [content_x, rect[1], content_w, rect[3]];
        if let Some(layout) =
            navigation_header_layout(column, s, previous_width, next_width)
        {
            timeline_top = layout.timeline_top;
            for (direction, button, target, label) in [
                (
                    Direction::Previous,
                    Some(layout.previous),
                    Some(previous_target),
                    previous_label,
                ),
                (
                    Direction::Next,
                    layout.next,
                    targets[1].as_deref(),
                    next_label,
                ),
            ] {
                let (Some(button), Some(target)) = (button, target) else {
                    continue;
                };
                pane.conversation_header().register(
                    direction,
                    target,
                    button,
                    column,
                    occlusion_rects,
                );
                let hovered = mouse.is_some_and(|(x, y)| {
                    pane.conversation_header()
                        .hit_at(x, y)
                        .is_some_and(|hit| hit.direction == direction)
                });
                let mut link_opts = opts.clone();
                if hovered {
                    link_opts.color = theme.u8(theme.fg);
                }
                link_opts.clip_rect = Some(button);
                crate::primitives::draw_text_with_occlusion(
                    sugarloaf,
                    button[0] + 2.0 * s,
                    button[1] + 5.0 * s,
                    label,
                    &link_opts,
                    occlusion_rects,
                );
            }
        }
    }
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
            (content_bottom - timeline_top).max(0.0),
        ],
        theme,
        s,
        now_seconds,
        mouse,
        occlusion_rects,
        plugins,
    );

    if composer_visible {
        // The timeline is clipped exactly at the composer border, so the
        // composer does not need the late quad pass for masking. Keeping it in
        // the normal pass also lets provider logos and attachment thumbnails
        // composite above the island surface.
        user_input::render_input(
            sugarloaf,
            pane,
            input_rect,
            rect[2],
            theme,
            active,
            mouse,
            s,
            false,
            false,
            now_seconds,
            occlusion_rects,
            prepared_input_rows,
            checkout_context,
        );
    }
}
