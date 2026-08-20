use neoism_agent_core::{Id, Part, PartTime, ToolPart, ToolState};
use serde_json::{json, Value};

use crate::now_millis;

pub(crate) fn append_text_delta(parts: &mut [Part], part_id: &str, delta: &str) {
    for part in parts {
        match part {
            Part::Text(text) if text.id.as_str() == part_id => {
                text.text.push_str(delta);
                return;
            }
            Part::Reasoning(reasoning) if reasoning.id.as_str() == part_id => {
                reasoning.text.push_str(delta);
                return;
            }
            _ => {}
        }
    }
}

pub(crate) fn finish_text_part(
    parts: &mut [Part],
    part_id: &str,
    text: Option<String>,
) -> Option<Part> {
    for part in parts {
        match part {
            Part::Text(text_part) if text_part.id.as_str() == part_id => {
                if let Some(text) = text {
                    text_part.text = text;
                }
                if let Some(time) = &mut text_part.time {
                    time.end = Some(now_millis());
                }
                return Some(Part::Text(text_part.clone()));
            }
            Part::Reasoning(reasoning) if reasoning.id.as_str() == part_id => {
                if let Some(text) = text {
                    reasoning.text = text;
                }
                reasoning.time.end = Some(now_millis());
                return Some(Part::Reasoning(reasoning.clone()));
            }
            _ => {}
        }
    }
    None
}

pub(crate) fn mark_interrupted_tool_parts(parts: &mut [Part]) -> Vec<Part> {
    let mut updated = Vec::new();
    for part in parts {
        let Part::Tool(tool) = part else {
            continue;
        };
        if !matches!(
            tool.state,
            ToolState::Pending { .. } | ToolState::Running { .. }
        ) {
            continue;
        }
        let input = tool_state_input(&tool.state);
        let start = tool_state_start(&tool.state).unwrap_or_else(now_millis);
        tool.state = ToolState::Error {
            input,
            error: "Tool execution aborted".to_string(),
            time: PartTime {
                start,
                end: Some(now_millis()),
            },
        };
        tool.metadata = Some(interrupted_tool_metadata(tool.metadata.take()));
        updated.push(Part::Tool(tool.clone()));
    }
    updated
}

pub(crate) fn finish_open_reasoning_parts(parts: &mut [Part]) -> Vec<Part> {
    let mut updated = Vec::new();
    let open_ids = parts
        .iter()
        .filter_map(|part| match part {
            Part::Reasoning(reasoning) if reasoning.time.end.is_none() => {
                Some(reasoning.id.as_str().to_string())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    for part_id in open_ids {
        if let Some(part) = finish_text_part(parts, &part_id, None) {
            updated.push(part);
        }
    }
    updated
}

fn interrupted_tool_metadata(existing: Option<Value>) -> Value {
    let mut metadata = match existing {
        Some(Value::Object(map)) => Value::Object(map),
        Some(value) => json!({ "previous": value }),
        None => json!({}),
    };
    if let Some(map) = metadata.as_object_mut() {
        map.insert("interrupted".to_string(), json!(true));
    }
    metadata
}

pub(crate) fn upsert_part(parts: &mut Vec<Part>, part: Part) {
    let id = part_id(&part).to_string();
    if let Some(existing) = parts.iter_mut().find(|item| part_id(item) == id) {
        *existing = part;
        return;
    }
    parts.push(part);
}

fn part_id(part: &Part) -> &str {
    match part {
        Part::Text(part) => part.id.as_str(),
        Part::Compaction(part) => part.id.as_str(),
        Part::Agent(part) => part.id.as_str(),
        Part::Subtask(part) => part.id.as_str(),
        Part::Reasoning(part) => part.id.as_str(),
        Part::Tool(part) => part.id.as_str(),
        Part::StepStart(part) => part.id.as_str(),
        Part::StepFinish(part) => part.id.as_str(),
        Part::File(part) => part.id.as_str(),
    }
}

pub(crate) fn append_tool_input_delta(
    parts: &mut [Part],
    part_id: &str,
    delta: &str,
) -> Option<Part> {
    for part in parts {
        if let Part::Tool(tool) = part {
            if tool.id.as_str() == part_id {
                if let ToolState::Pending { raw, .. } = &mut tool.state {
                    raw.push_str(delta);
                }
                return Some(Part::Tool(tool.clone()));
            }
        }
    }
    None
}

pub(crate) fn set_tool_running(
    parts: &mut Vec<Part>,
    part_id: Id,
    session_id: &Id,
    message_id: &Id,
    call_id: String,
    name: String,
    input: Value,
) -> Part {
    let part_id_text = part_id.to_string();
    for part in parts.iter_mut() {
        if let Part::Tool(tool) = part {
            if tool.id.as_str() == part_id_text {
                tool.tool = name;
                tool.call_id = call_id;
                tool.state = ToolState::Running {
                    input,
                    time: PartTime {
                        start: now_millis(),
                        end: None,
                    },
                };
                return Part::Tool(tool.clone());
            }
        }
    }
    let part = Part::Tool(ToolPart {
        id: part_id,
        session_id: session_id.clone(),
        message_id: message_id.clone(),
        tool: name,
        call_id,
        state: ToolState::Running {
            input,
            time: PartTime {
                start: now_millis(),
                end: None,
            },
        },
        metadata: None,
    });
    parts.push(part.clone());
    part
}

pub(crate) fn set_tool_completed(
    parts: &mut [Part],
    part_id: &str,
    output: String,
    title: String,
    metadata: Value,
) -> Option<Part> {
    for part in parts {
        if let Part::Tool(tool) = part {
            if tool.id.as_str() == part_id {
                let input = tool_state_input(&tool.state);
                let start = tool_state_start(&tool.state).unwrap_or_else(now_millis);
                let metadata =
                    stable_tool_metadata(metadata, &tool.tool, &title, &output);
                tool.state = ToolState::Completed {
                    input,
                    output,
                    metadata,
                    title,
                    time: PartTime {
                        start,
                        end: Some(now_millis()),
                    },
                };
                return Some(Part::Tool(tool.clone()));
            }
        }
    }
    None
}

fn stable_tool_metadata(metadata: Value, tool: &str, title: &str, output: &str) -> Value {
    let mut metadata = match metadata {
        Value::Object(object) => object,
        other => {
            let mut object = serde_json::Map::new();
            object.insert("raw".to_string(), other);
            object
        }
    };
    let has_snapshots = metadata.get("snapshots").is_some();
    let kind = if has_snapshots {
        "diff"
    } else if metadata.get("lsp").is_some() {
        "lsp"
    } else if metadata.get("todos").is_some() {
        "todo"
    } else {
        "text"
    };
    metadata.entry("render".to_string()).or_insert_with(|| {
        json!({
            "version": 1,
            "tool": tool,
            "title": title,
            "lineCount": output.lines().count(),
            "byteCount": output.len(),
            "hasSnapshots": has_snapshots,
            "kind": kind,
        })
    });
    Value::Object(metadata)
}

pub(crate) fn set_tool_error(
    parts: &mut [Part],
    part_id: &str,
    error: String,
) -> Option<Part> {
    for part in parts {
        if let Part::Tool(tool) = part {
            if tool.id.as_str() == part_id {
                let input = tool_state_input(&tool.state);
                let start = tool_state_start(&tool.state).unwrap_or_else(now_millis);
                tool.state = ToolState::Error {
                    input,
                    error,
                    time: PartTime {
                        start,
                        end: Some(now_millis()),
                    },
                };
                return Some(Part::Tool(tool.clone()));
            }
        }
    }
    None
}

pub(crate) fn tool_state_input(state: &ToolState) -> Value {
    match state {
        ToolState::Pending { input, .. }
        | ToolState::Running { input, .. }
        | ToolState::Completed { input, .. }
        | ToolState::Error { input, .. } => input.clone(),
    }
}

pub(crate) fn tool_state_start(state: &ToolState) -> Option<u64> {
    match state {
        ToolState::Running { time, .. }
        | ToolState::Completed { time, .. }
        | ToolState::Error { time, .. } => Some(time.start),
        ToolState::Pending { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_agent_core::{IdKind, ReasoningPart};

    #[test]
    fn fatal_stream_cleanup_settles_running_tool_and_open_reasoning() {
        let session_id = Id::ascending(IdKind::Session);
        let message_id = Id::ascending(IdKind::Message);
        let mut parts = vec![
            Part::Reasoning(ReasoningPart {
                id: Id::ascending(IdKind::Part),
                session_id: session_id.clone(),
                message_id: message_id.clone(),
                text: "looking around".to_string(),
                time: PartTime {
                    start: 1,
                    end: None,
                },
                metadata: None,
            }),
            Part::Tool(ToolPart {
                id: Id::ascending(IdKind::Part),
                session_id,
                message_id,
                tool: "read".to_string(),
                call_id: "call-1".to_string(),
                state: ToolState::Running {
                    input: json!({ "filePath": "Memory/feature_workspace_detach.md" }),
                    time: PartTime {
                        start: 2,
                        end: None,
                    },
                },
                metadata: None,
            }),
        ];

        let tools = mark_interrupted_tool_parts(&mut parts);
        let reasoning = finish_open_reasoning_parts(&mut parts);

        assert_eq!(tools.len(), 1);
        assert_eq!(reasoning.len(), 1);
        match &parts[0] {
            Part::Reasoning(part) => assert!(part.time.end.is_some()),
            other => panic!("expected reasoning, got {other:?}"),
        }
        match &parts[1] {
            Part::Tool(part) => match &part.state {
                ToolState::Error { error, time, .. } => {
                    assert_eq!(error, "Tool execution aborted");
                    assert!(time.end.is_some());
                    assert_eq!(part.metadata, Some(json!({ "interrupted": true })));
                }
                other => panic!("expected error tool, got {other:?}"),
            },
            other => panic!("expected tool, got {other:?}"),
        }
    }
}
