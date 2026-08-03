use crate::panels::agent_pane::state::NeoismAgentPane;

use super::{
    CHAT_INPUT_MIN_H, HOME_INPUT_MIN_H, INPUT_HELP_STRIP_H, INPUT_LINE_H, MAX_INPUT_LINES,
};

/// Horizontal padding applied to both the chat timeline and the input
/// composer in the Neoism Agent pane. Keep narrow mobile panes edge-to-edge
/// so the shared iOS/web layout does not lose precious text width.
pub fn chat_horizontal_pad(width: f32, s: f32) -> f32 {
    if width < 520.0 * s {
        0.0
    } else {
        12.0 * s
    }
}

pub trait AgentPaneInput {
    fn input(&self) -> &str;
    fn input_help_visible(&self) -> bool;
    fn background_task_details_expanded(&self) -> bool;
    /// Exact rows registered by the most recent render, when they still
    /// describe the current draft. Non-rendering hosts may return `None`.
    fn input_visual_row_count(&self) -> Option<usize> {
        None
    }
}

impl AgentPaneInput for NeoismAgentPane {
    fn input(&self) -> &str {
        self.input()
    }

    fn input_help_visible(&self) -> bool {
        self.input_help_visible()
    }

    fn background_task_details_expanded(&self) -> bool {
        self.background_task_details_expanded()
    }

    fn input_visual_row_count(&self) -> Option<usize> {
        NeoismAgentPane::input_visual_row_count(self)
    }
}

pub fn home_input_rect(pane: &impl AgentPaneInput, rect: [f32; 4], s: f32) -> [f32; 4] {
    let [x, y, w, h] = rect;
    let input_w = home_input_width(rect, s);
    let input_h = pane.input_visual_row_count().map_or_else(
        || {
            input_height_for_width(
                pane.input(),
                input_w,
                s,
                true,
                pane.background_task_details_expanded(),
            )
        },
        |rows| {
            input_height_for_visual_rows(
                rows,
                s,
                true,
                pane.background_task_details_expanded(),
            )
        },
    );
    let input_x = x + (w - input_w) * 0.5;
    // The wordmark anchors a fixed gap above this card, so the pair
    // reads as one group centered slightly above the pane's midline.
    let input_y = y + h * 0.46;
    [input_x, input_y, input_w, input_h]
}

pub fn home_input_width(rect: [f32; 4], s: f32) -> f32 {
    let w = rect[2];
    let side_pad = (24.0 * s).min((w * 0.08).max(0.0));
    let available_w = (w - side_pad * 2.0).max(1.0);
    (820.0 * s).min(available_w)
}

/// Measured-row counterpart used by the actual painter. The public
/// approximation above remains available to hosts that only need a hit-test
/// rect and do not own a Sugarloaf text measurer.
pub fn home_input_rect_for_visual_rows(
    pane: &impl AgentPaneInput,
    rect: [f32; 4],
    s: f32,
    visual_rows: usize,
) -> [f32; 4] {
    let [x, y, w, h] = rect;
    let input_w = home_input_width(rect, s);
    let input_h = input_height_for_visual_rows(
        visual_rows,
        s,
        true,
        pane.background_task_details_expanded(),
    );
    let input_x = x + (w - input_w) * 0.5;
    let input_y = y + h * 0.46;
    [input_x, input_y, input_w, input_h]
}

/// THE shared chat column: the timeline and the composer island use
/// the SAME x/width (insets + wide-pane cap, centered). If the
/// timeline were wider, its content would extend past the floating
/// input bar's borders on both sides and the bar would read as inset
/// into a larger slab instead of floating over the column.
pub fn chat_column(rect: [f32; 4], s: f32) -> (f32, f32) {
    let [x, _y, w, _h] = rect;
    let side_pad = chat_horizontal_pad(w, s) + 8.0 * s;
    let col_w = (w - side_pad * 2.0).max(220.0).min(960.0 * s);
    (x + (w - col_w) * 0.5, col_w)
}

pub fn chat_input_rect(pane: &impl AgentPaneInput, rect: [f32; 4], s: f32) -> [f32; 4] {
    let [_x, y, _w, h] = rect;
    // Floating island: same column as the timeline, lifted off the
    // pane bottom.
    let (input_x, input_w) = chat_column(rect, s);
    // Leave a dedicated band only while the OpenCode-style help strip is
    // visible. `/hints` removes both the paint and this reservation so the
    // timeline/composer reclaim the full row.
    let help_h = if pane.input_help_visible() {
        INPUT_HELP_STRIP_H
    } else {
        0.0
    };
    let bottom_pad = (14.0 + help_h) * s;
    let input_h = pane.input_visual_row_count().map_or_else(
        || {
            input_height_for_width(
                pane.input(),
                input_w,
                s,
                false,
                pane.background_task_details_expanded(),
            )
        },
        |rows| {
            input_height_for_visual_rows(
                rows,
                s,
                false,
                pane.background_task_details_expanded(),
            )
        },
    );
    [input_x, y + h - input_h - bottom_pad, input_w, input_h]
}

pub fn chat_input_rect_for_visual_rows(
    pane: &impl AgentPaneInput,
    rect: [f32; 4],
    s: f32,
    visual_rows: usize,
) -> [f32; 4] {
    let [_x, y, _w, h] = rect;
    let (input_x, input_w) = chat_column(rect, s);
    let help_h = if pane.input_help_visible() {
        INPUT_HELP_STRIP_H
    } else {
        0.0
    };
    let bottom_pad = (14.0 + help_h) * s;
    let input_h = input_height_for_visual_rows(
        visual_rows,
        s,
        false,
        pane.background_task_details_expanded(),
    );
    [input_x, y + h - input_h - bottom_pad, input_w, input_h]
}

pub fn input_height_for_width(
    input: &str,
    input_w: f32,
    s: f32,
    show_status: bool,
    background_details_expanded: bool,
) -> f32 {
    let text_w = (input_w - 48.0 * s).max(32.0 * s);
    let char_w = (16.0 * s * 0.58).max(1.0);
    let cols = (text_w / char_w).floor().max(1.0) as usize;
    let estimated_lines = input
        .split('\n')
        .map(|line| line.chars().count().max(1).div_ceil(cols))
        .sum::<usize>()
        .max(1)
        .min(MAX_INPUT_LINES);
    input_height_for_visual_rows(
        estimated_lines,
        s,
        show_status,
        background_details_expanded,
    )
}

pub fn input_height_for_visual_rows(
    visual_rows: usize,
    s: f32,
    show_status: bool,
    background_details_expanded: bool,
) -> f32 {
    let visual_rows = visual_rows.max(1).min(MAX_INPUT_LINES);
    let min_h = if show_status {
        HOME_INPUT_MIN_H
    } else {
        CHAT_INPUT_MIN_H
    };
    // Base = box paddings + the send band + the skirt chip row below
    // the box. Chat stays slightly tighter than the home splash card.
    let base_h = if show_status { 84.0 } else { 76.0 };
    let status_extra_h = if show_status && background_details_expanded {
        96.0
    } else {
        0.0
    };
    (min_h * s).max((base_h + status_extra_h + visual_rows as f32 * INPUT_LINE_H) * s)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestPane {
        rows: Option<usize>,
    }

    impl AgentPaneInput for TestPane {
        fn input(&self) -> &str {
            "short"
        }

        fn input_help_visible(&self) -> bool {
            false
        }

        fn background_task_details_expanded(&self) -> bool {
            false
        }

        fn input_visual_row_count(&self) -> Option<usize> {
            self.rows
        }
    }

    #[test]
    fn measured_rows_expand_both_composer_variants_at_the_second_row() {
        assert_eq!(input_height_for_visual_rows(1, 1.0, false, false), 98.0);
        assert_eq!(input_height_for_visual_rows(2, 1.0, false, false), 120.0);
        assert_eq!(input_height_for_visual_rows(1, 1.0, true, false), 106.0);
        assert_eq!(input_height_for_visual_rows(2, 1.0, true, false), 128.0);
    }

    #[test]
    fn measured_rows_are_clamped_to_the_visible_composer_limit() {
        assert_eq!(
            input_height_for_visual_rows(MAX_INPUT_LINES + 10, 1.0, false, false),
            input_height_for_visual_rows(MAX_INPUT_LINES, 1.0, false, false)
        );
    }

    #[test]
    fn host_rect_prefers_registered_rows_over_the_character_estimate() {
        let rect = [0.0, 0.0, 800.0, 600.0];
        let estimated = chat_input_rect(&TestPane { rows: None }, rect, 1.0);
        let measured = chat_input_rect(&TestPane { rows: Some(2) }, rect, 1.0);
        assert_eq!(estimated[3], 98.0, "short text estimates one row");
        assert_eq!(measured[3], 120.0, "registered two-row geometry wins");
        assert_eq!(estimated[1] - measured[1], 22.0);
    }
}
