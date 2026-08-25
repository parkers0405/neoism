use neoism_agent_core::EventEnvelope;
use serde_json::Value;

use crate::chat_blockers::{permission_blocker, question_blocker, StreamBlocker};
use crate::chat_render::ChatRenderState;
use crate::chat_ui::BottomPrompt;
use crate::{BOLD, DIM, ORANGE, RED, RESET};

#[derive(Default)]
pub(crate) struct ChatEventOutcome {
    pub(crate) done: bool,
    pub(crate) blocker: Option<StreamBlocker>,
    pub(crate) clear_blocker: bool,
}

pub(crate) fn handle_chat_sse_event(
    session_id: &str,
    data_lines: &[String],
    render_state: &mut ChatRenderState,
    mut ui: Option<&mut BottomPrompt>,
) -> anyhow::Result<ChatEventOutcome> {
    if data_lines.is_empty() {
        return Ok(ChatEventOutcome::default());
    }
    let data = data_lines.join("\n");
    let envelope: EventEnvelope<Value> = serde_json::from_str(&data)?;
    let kind = envelope.kind.as_str();
    let properties = &envelope.data;
    if !event_matches_session(properties, session_id) {
        return Ok(ChatEventOutcome::default());
    }
    match kind {
        "message.part.delta" => {
            if properties.get("field").and_then(Value::as_str) == Some("text") {
                if let Some(delta) = properties.get("delta").and_then(Value::as_str) {
                    prepare_stream_output(&mut ui, render_state)?;
                    let part_id = properties
                        .get("partID")
                        .or_else(|| properties.get("partId"))
                        .and_then(Value::as_str);
                    if part_id.is_some_and(|part_id| {
                        render_state.reasoning_parts.contains(part_id)
                    }) {
                        render_state.reasoning_delta(delta)?;
                    } else {
                        render_state.text_delta(delta)?;
                    }
                }
            }
        }
        "message.part.updated" => {
            if let Some(part) = properties.get("part") {
                prepare_stream_output(&mut ui, render_state)?;
                render_state.part_updated(part)?;
            }
        }
        "session.error" => {
            prepare_stream_output(&mut ui, render_state)?;
            render_state.finish()?;
            let message = properties
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("session error");
            println!("{RED}error:{RESET} {message}");
            return Ok(ChatEventOutcome {
                done: true,
                ..Default::default()
            });
        }
        "session.status" => {
            let status = properties
                .get("status")
                .and_then(|status| status.get("type"))
                .and_then(Value::as_str);
            if status == Some("idle") {
                return Ok(ChatEventOutcome {
                    done: true,
                    ..Default::default()
                });
            }
        }
        "permission.asked" => {
            prepare_stream_output(&mut ui, render_state)?;
            render_state.finish()?;
            if let Some(blocker) = permission_blocker(properties) {
                println!(
                    "{ORANGE}{BOLD}Permission required{RESET} {DIM}{} {RESET}",
                    blocker.status()
                );
                return Ok(ChatEventOutcome {
                    blocker: Some(blocker),
                    ..Default::default()
                });
            }
        }
        "permission.replied" | "question.replied" | "question.rejected" => {
            return Ok(ChatEventOutcome {
                clear_blocker: true,
                ..Default::default()
            });
        }
        "question.asked" => {
            prepare_stream_output(&mut ui, render_state)?;
            render_state.finish()?;
            if let Some(blocker) = question_blocker(properties) {
                println!(
                    "{ORANGE}{BOLD}Question required{RESET} {DIM}{} {RESET}",
                    blocker.status()
                );
                return Ok(ChatEventOutcome {
                    blocker: Some(blocker),
                    ..Default::default()
                });
            }
        }
        _ => {}
    }
    Ok(ChatEventOutcome::default())
}

fn prepare_stream_output(
    ui: &mut Option<&mut BottomPrompt>,
    _render_state: &ChatRenderState,
) -> anyhow::Result<()> {
    let Some(ui) = ui.as_deref_mut() else {
        return Ok(());
    };
    ui.clear_overlay_preserving_cursor()
}

fn event_matches_session(properties: &Value, session_id: &str) -> bool {
    properties
        .get("sessionID")
        .or_else(|| properties.get("sessionId"))
        .or_else(|| {
            properties
                .get("info")
                .and_then(|info| info.get("sessionID").or_else(|| info.get("sessionId")))
        })
        .and_then(Value::as_str)
        == Some(session_id)
}
