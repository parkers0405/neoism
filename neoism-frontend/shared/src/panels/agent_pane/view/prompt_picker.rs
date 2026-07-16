//! Agent prompt picker — permission requests and model questions
//! rendered as the same inline-picker card that the "/" menu pops out
//! of the input island, so every "the agent needs you" moment shares
//! one surface: slash commands, permissions, questions.
//!
//! Interaction state stays on the pane (`pending_permission` /
//! `pending_question`); this module only draws and registers row hit
//! rects. Keyboard routing lives with the host (arrows / enter / a / n
//! / esc for permissions; arrows / typing / enter / esc for questions).

use sugarloaf::Sugarloaf;

use crate::panels::agent_pane::permission_policy::VISUAL_SELECTION_ORDER;
use crate::primitives::ide_theme::IdeTheme;
use crate::widgets::inline_picker::{self, InlinePickerRow, InlinePickerView};

use super::user_input::{
    AgentPendingPermission, AgentPermissionChoice, AgentUserInputPane,
};

/// Render the pending prompt (permission first, then question) as an
/// input-anchored picker card. Returns the card rect for occlusion, or
/// `None` when nothing is pending — the caller shows the regular "/"
/// picker instead.
pub fn render_prompt_picker(
    sugarloaf: &mut Sugarloaf,
    pane: &mut impl AgentUserInputPane,
    input_rect: [f32; 4],
    theme: &IdeTheme,
    s: f32,
) -> Option<[f32; 4]> {
    pane.clear_permission_choice_hit_rects();
    pane.clear_question_option_rects();
    if pane.pending_permission().is_some() {
        return render_permission_prompt(sugarloaf, pane, input_rect, theme, s);
    }
    if pane.pending_question().is_some() {
        return render_question_prompt(sugarloaf, pane, input_rect, theme, s);
    }
    None
}

fn render_permission_prompt(
    sugarloaf: &mut Sugarloaf,
    pane: &mut impl AgentUserInputPane,
    input_rect: [f32; 4],
    theme: &IdeTheme,
    s: f32,
) -> Option<[f32; 4]> {
    let (header, body, meta, patterns, selected, responding) = {
        let permission = pane.pending_permission()?;
        let header = if permission
            .parent_session_id()
            .is_some_and(|parent| Some(parent) == pane.session_id_str())
        {
            if let Some(agent) = permission.source_agent() {
                format!("Subagent @{agent}")
            } else {
                "Subagent permission".to_string()
            }
        } else {
            "Permission required".to_string()
        };
        let body = if permission.title().trim().is_empty() {
            "Allow tool?".to_string()
        } else {
            permission.title().to_string()
        };
        let label = if permission.permission().trim().is_empty() {
            "tool"
        } else {
            permission.permission()
        };
        (
            header,
            body,
            format!("permission: {label}"),
            permission.patterns().to_vec(),
            permission.selected(),
            permission.responding(),
        )
    };

    let title = format!("{header} · {body}");
    let pattern_desc = patterns.join("  ·  ");
    let always_desc = if pattern_desc.is_empty() {
        "Don't ask again for this permission".to_string()
    } else {
        pattern_desc
    };
    // Visual order mirrors the old card (and VISUAL_SELECTION_ORDER):
    // Always, Yes, No — with Yes preselected by the state default.
    let rows_data: [(&str, &str, &str, AgentPermissionChoice); 3] = [
        (
            "Always",
            always_desc.as_str(),
            "a",
            AgentPermissionChoice::Always,
        ),
        ("Yes", "Allow once", "enter", AgentPermissionChoice::Once),
        (
            "No",
            "Reject this request",
            "n",
            AgentPermissionChoice::Reject,
        ),
    ];
    let rows = rows_data
        .iter()
        .map(|(label, description, footer, _)| InlinePickerRow {
            title: label,
            description,
            footer,
            is_header: false,
            is_current: false,
            is_pinned: false,
        })
        .collect::<Vec<_>>();
    let visual_selected = VISUAL_SELECTION_ORDER
        .iter()
        .position(|choice| *choice == selected)
        .unwrap_or(1);
    let footer_hint = responding.then_some("sending…");
    let state = inline_picker::render(
        sugarloaf,
        InlinePickerView {
            title: &title,
            query: "",
            selected: visual_selected,
            scroll_offset: 0,
            list_scroll_offset: 0.0,
            cursor_offset: 0.0,
            rows: &rows,
            footer_hint,
            rename: None,
            show_search_caret: false,
            search_placeholder: &meta,
            loading: false,
            loading_elapsed: 0.0,
        },
        input_rect,
        theme,
        s,
    )?;
    if !responding {
        for (visual_ix, (_, _, _, choice)) in rows_data.iter().enumerate() {
            pane.register_permission_choice_rect(
                *choice,
                inline_picker::row_rect(state.rect, visual_ix, s),
            );
        }
    }
    Some(state.rect)
}

fn render_question_prompt(
    sugarloaf: &mut Sugarloaf,
    pane: &mut impl AgentUserInputPane,
    input_rect: [f32; 4],
    theme: &IdeTheme,
    s: f32,
) -> Option<[f32; 4]> {
    let (title, typed, rows_data, selected, responding, has_options) = {
        let question = pane.pending_question()?;
        let item = question.current_item()?;
        let title = if question.questions.len() > 1 {
            format!(
                "({}/{}) {}",
                question.current + 1,
                question.questions.len(),
                item.text
            )
        } else {
            item.text.clone()
        };
        (
            title,
            question.typed.clone(),
            question.visible_rows(),
            question.selected,
            question.responding,
            !item.options.is_empty(),
        )
    };

    let rows = if rows_data.is_empty() {
        // Free-text question with nothing typed yet — show a header hint
        // instead of the widget's "No results" empty state.
        vec![InlinePickerRow {
            title: "Type your answer, then press enter",
            description: "",
            footer: "",
            is_header: true,
            is_current: false,
            is_pinned: false,
        }]
    } else {
        rows_data
            .iter()
            .map(|row| InlinePickerRow {
                title: &row.label,
                description: &row.description,
                footer: if row.is_custom { "enter" } else { "" },
                is_header: false,
                is_current: false,
                is_pinned: false,
            })
            .collect::<Vec<_>>()
    };
    let footer_hint = if responding {
        "sending…"
    } else {
        "enter answer · esc skip"
    };
    let state = inline_picker::render(
        sugarloaf,
        InlinePickerView {
            title: &title,
            query: &typed,
            selected: selected.min(rows_data.len().saturating_sub(1)),
            scroll_offset: 0,
            list_scroll_offset: 0.0,
            cursor_offset: 0.0,
            rows: &rows,
            footer_hint: Some(footer_hint),
            rename: None,
            show_search_caret: true,
            search_placeholder: if has_options {
                "Filter or type your own answer"
            } else {
                "Type your answer"
            },
            loading: false,
            loading_elapsed: 0.0,
        },
        input_rect,
        theme,
        s,
    )?;
    if !responding {
        for index in 0..rows_data.len() {
            pane.register_question_option_rect(
                index,
                inline_picker::row_rect(state.rect, index, s),
            );
        }
    }
    Some(state.rect)
}
